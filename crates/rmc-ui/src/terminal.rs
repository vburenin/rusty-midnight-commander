use crate::help::{global_index, HelpItem};
use crate::render::Renderer;
use crate::skin::load_default_palette;
use anyhow::Result;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use rmc_core::actions::Action;
use rmc_core::app::{App, UiMode};
use rmc_core::find::{
    search_files_streaming, CancelHandle, FindDialogFocus as FF, FindDialogState,
};
use rmc_core::hotlist::HotlistDialogFocus as HDF;
use std::io::stdout;
use std::time::{Duration, Instant};

pub struct TerminalApp;

// Keep the current viewer's effective bytes alive across renders and filtering.
// Stores the original display path and the current ViewData (may reference a temp file).
pub(crate) struct ViewerState {
    pub display_path: std::path::PathBuf,
    pub view: rmc_view::ViewData,
}

use once_cell::sync::Lazy;
use std::sync::Mutex;
pub(crate) static VIEWER_STATE: Lazy<Mutex<Option<ViewerState>>> = Lazy::new(|| Mutex::new(None));

pub(crate) fn viewer_clear_state() {
    if let Ok(mut g) = VIEWER_STATE.lock() {
        *g = None;
    }
}

pub(crate) fn viewer_ensure_view_for(display_path: &std::path::Path) -> std::path::PathBuf {
    let mut g = VIEWER_STATE.lock().expect("viewer state mutex poisoned");
    let need_new = g
        .as_ref()
        .map(|s| s.display_path != display_path)
        .unwrap_or(true);
    if need_new {
        let view = rmc_view::ViewData::open_view(display_path)
            .unwrap_or_else(|_| rmc_view::ViewData::from_path(display_path.to_path_buf()));
        let p = view.path().to_path_buf();
        *g = Some(ViewerState {
            display_path: display_path.to_path_buf(),
            view,
        });
        return p;
    }
    g.as_ref()
        .map(|s| s.view.path().to_path_buf())
        .unwrap_or_else(|| display_path.to_path_buf())
}

pub(crate) fn viewer_apply_filter_to_current(cmd: &str) -> anyhow::Result<()> {
    let mut g = VIEWER_STATE.lock().expect("viewer state mutex poisoned");
    if let Some(state) = g.as_mut() {
        let filter = rmc_view::ExternalFilter::new("sh")
            .with_args(["-c".to_string(), cmd.to_string()])
            .with_input(rmc_view::FilterInput::Stdin);
        let new_view = rmc_view::ViewData::from_filter(state.view.path(), &filter)?;
        state.view = new_view;
        Ok(())
    } else {
        anyhow::bail!("No active viewer");
    }
}

impl TerminalApp {
    fn apply_sort_dialog(
        app: &mut App,
        side: rmc_core::actions::PaneSide,
        by: rmc_core::panel::SortBy,
        reverse: bool,
        dirs_first: bool,
    ) -> Result<()> {
        let p = if matches!(side, rmc_core::actions::PaneSide::Left) {
            &mut app.left
        } else {
            &mut app.right
        };
        p.sort_by = by;
        p.sort_dir = if reverse {
            rmc_core::sorting::SortDir::Desc
        } else {
            rmc_core::sorting::SortDir::Asc
        };
        p.dirs_first = dirs_first;
        p.apply_sort();
        Ok(())
    }
    pub fn run(app: &mut App) -> Result<()> {
        let mut out = stdout();
        enable_raw_mode()?;
        execute!(out, EnterAlternateScreen, EnableMouseCapture)?;
        let palette = load_default_palette();
        let mut renderer = Renderer::new(palette);
        let mut last_draw = Instant::now();
        // pending_ctrl_x lives on App; no local flag here

        loop {
            // Compute content rows for page/scroll visibility
            let (cols, rows) = crossterm::terminal::size()?;
            let panel_top = 1u16;
            let gauge_row = rows.saturating_sub(4);
            let content_bottom = gauge_row.saturating_sub(1);
            let panel_h = content_bottom - panel_top;
            let content_rows = panel_h.saturating_sub(4) as usize;
            // Compute per-panel visible capacity (rows or 2*rows for Brief two-column)
            let mid = cols / 2;
            let left_w = mid;
            let right_w = cols - mid;
            let left_two_cols =
                matches!(app.left.listing, rmc_core::panel::ListingFormat::Brief) && left_w >= 30;
            let right_two_cols =
                matches!(app.right.listing, rmc_core::panel::ListingFormat::Brief) && right_w >= 30;
            let left_capacity = content_rows * if left_two_cols { 2 } else { 1 };
            let right_capacity = content_rows * if right_two_cols { 2 } else { 1 };
            // Ensure cursor visibility based on last-known height
            {
                let left = &mut app.left;
                left.ensure_visible(left_capacity);
                let right = &mut app.right;
                right.ensure_visible(right_capacity);
            }
            // Draw at least at 30 FPS-equivalent idle to react to resize
            // Also check for background find results to update UI even without keypresses
            if let UiMode::FindDialog(state) = &mut app.ui_mode {
                if state.running {
                    if let Some(rx) = &state.results_rx {
                        loop {
                            match rx.try_recv() {
                                Ok(p) => {
                                    state.results.paths.push(p);
                                    if state.selected_index >= state.results.paths.len() {
                                        state.selected_index =
                                            state.results.paths.len().saturating_sub(1);
                                    }
                                }
                                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                                    state.running = false;
                                    state.cancel = None;
                                    state.results_rx = None;
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            if last_draw.elapsed() > Duration::from_millis(33) {
                renderer.draw(app)?;
                last_draw = Instant::now();
            }

            if event::poll(Duration::from_millis(50))? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        // Effective page size depends on active panel format/width
                        let active_capacity = match app.active {
                            rmc_core::actions::PaneSide::Left => left_capacity,
                            rmc_core::actions::PaneSide::Right => right_capacity,
                        };
                        Self::handle_key(app, key, active_capacity)?;
                    }
                    Event::Resize(_, _) => {
                        // redraw next loop
                    }
                    _ => {}
                }
            }

            if app.quit {
                break;
            }
        }

        disable_raw_mode()?;
        execute!(out, LeaveAlternateScreen, DisableMouseCapture)?;
        Ok(())
    }

    fn handle_key(app: &mut App, key: KeyEvent, page_rows: usize) -> Result<()> {
        let active_cwd = app.active_panel().cwd.clone();
        // Subshell full-screen mode: C-o toggles back; support PgUp/PgDn/Up/Down scrolling.
        if app.subshell.show_output_screen {
            match key.code {
                KeyCode::Char('o')
                    if key
                        .modifiers
                        .contains(crossterm::event::KeyModifiers::CONTROL) =>
                {
                    app.handle_action(Action::ToggleSubshell)?;
                }
                KeyCode::PageUp => {
                    let (_c, r) = crossterm::terminal::size()?;
                    app.subshell.scroll_page_up(r as usize);
                }
                KeyCode::PageDown => {
                    let (_c, r) = crossterm::terminal::size()?;
                    app.subshell.scroll_page_down(r as usize);
                }
                KeyCode::Up => app.subshell.scroll_page_up(1),
                KeyCode::Down => app.subshell.scroll_page_down(1),
                _ => {}
            }
            return Ok(());
        }
        // Dialog handling first
        match &mut app.ui_mode {
            UiMode::Editor {
                buf,
                show_menu: _,
                status_msg,
                search_input,
                save_as_input,
                pending_quit: _,
                confirm_exit,
            } => {
                // Confirm-exit overlay (F10/q on dirty)
                if let Some(c) = confirm_exit {
                    use rmc_core::app::YncFocus as F;
                    match key.code {
                        KeyCode::Esc | KeyCode::F(10) => {
                            // Close overlay only
                            *confirm_exit = None;
                        }
                        KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
                            c.focus = match c.focus {
                                F::Yes => F::No,
                                F::No => F::Cancel,
                                F::Cancel => F::Yes,
                            };
                        }
                        KeyCode::Enter => {
                            match c.focus {
                                F::Yes => {
                                    // Save and exit to panels
                                    if let Some(path) = &buf.path {
                                        let mut w = app
                                            .vfs
                                            .write_file(path)
                                            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
                                        use std::io::Write;
                                        let _ = w.write_all(&buf.to_bytes());
                                    }
                                    app.ui_mode = UiMode::Normal;
                                }
                                F::No => {
                                    app.ui_mode = UiMode::Normal;
                                }
                                F::Cancel => {
                                    *confirm_exit = None;
                                }
                            }
                        }
                        _ => {}
                    }
                    return Ok(());
                }
                // Inline "Find:" overlay
                if let Some(q) = search_input {
                    match key.code {
                        KeyCode::Esc | KeyCode::F(10) => {
                            // Close overlay only
                            *search_input = None;
                        }
                        KeyCode::Enter => {
                            let needle = q.clone();
                            if !needle.is_empty() {
                                let _ = buf.search_forward_opts(needle.as_bytes(), false, true);
                                *status_msg = if buf.last_search.is_empty() {
                                    Some("Not found".into())
                                } else {
                                    Some("Found".into())
                                };
                            }
                            *search_input = None;
                        }
                        KeyCode::Backspace => {
                            q.pop();
                        }
                        KeyCode::Char(c) if key.modifiers.is_empty() => {
                            q.push(c);
                        }
                        _ => {}
                    }
                    return Ok(());
                }
                // Inline "Save as:" overlay used here for Replace-with text as well
                if let Some(val) = save_as_input {
                    // Disambiguate by status message hint (very lightweight)
                    let replacing = status_msg
                        .as_deref()
                        .is_some_and(|s| s.starts_with("Replace with"));
                    if replacing {
                        match key.code {
                            KeyCode::Esc | KeyCode::F(10) => {
                                *save_as_input = None;
                                *status_msg = None;
                            }
                            KeyCode::Enter => {
                                let repl = val.clone();
                                if !buf.last_search.is_empty() {
                                    let find = buf.last_search.clone();
                                    let ci = buf.last_search_case_insensitive;
                                    let _ = buf.replace_next(&find, repl.as_bytes(), ci, true);
                                    *status_msg = Some("Replaced".into());
                                }
                                *save_as_input = None;
                            }
                            KeyCode::Char('a')
                                if key.modifiers.contains(crossterm::event::KeyModifiers::ALT) =>
                            {
                                let repl = val.clone();
                                if !buf.last_search.is_empty() {
                                    let find = buf.last_search.clone();
                                    let ci = buf.last_search_case_insensitive;
                                    let n = buf.replace_all(&find, repl.as_bytes(), ci);
                                    *status_msg = Some(format!("Replaced all: {n}"));
                                }
                                *save_as_input = None;
                            }
                            KeyCode::Backspace => {
                                val.pop();
                            }
                            KeyCode::Char(c) if key.modifiers.is_empty() => {
                                val.push(c);
                            }
                            _ => {}
                        }
                        return Ok(());
                    } else {
                        // Save-as path input
                        match key.code {
                            KeyCode::Esc | KeyCode::F(10) => {
                                *save_as_input = None;
                            }
                            KeyCode::Enter => {
                                let p = std::path::PathBuf::from(val.clone());
                                let mut w = app
                                    .vfs
                                    .write_file(&p)
                                    .map_err(|e| anyhow::anyhow!(e.to_string()))?;
                                use std::io::Write;
                                let _ = w.write_all(&buf.to_bytes());
                                buf.path = Some(p);
                                buf.dirty = false;
                                *save_as_input = None;
                                *status_msg = Some("Saved".into());
                            }
                            KeyCode::Backspace => {
                                val.pop();
                            }
                            KeyCode::Char(c) if key.modifiers.is_empty() => {
                                val.push(c);
                            }
                            _ => {}
                        }
                        return Ok(());
                    }
                }
                // Base editor keys
                match key.code {
                    // MC: F7 search dialog
                    KeyCode::F(7) => {
                        *search_input = Some(String::new());
                    }
                    // Shift-F7 (F19) or 'n': next match with wrap
                    KeyCode::F(19) | KeyCode::Char('n') => {
                        let _ = buf.search_next_opts(true);
                    }
                    // F4 Replace: prompt find if empty, otherwise prompt replacement
                    KeyCode::F(4) => {
                        if buf.last_search.is_empty() {
                            *search_input = Some(String::new());
                            *status_msg = Some("Find (Enter to confirm)".into());
                        } else {
                            *save_as_input = Some(String::new());
                            *status_msg = Some("Replace with (Enter=Replace, Alt-a=All)".into());
                        }
                    }
                    // Pipe selection (or whole buffer) through external command (GNU mcedit behavior)
                    KeyCode::Char('|') => {
                        let buf_snapshot = buf.clone();
                        app.ui_mode = UiMode::InputDialog {
                            title: "Pipe command".into(),
                            prompt: "Enter shell command:".into(),
                            value: String::new(),
                            focus_ok: false,
                            on_submit: Box::new(move |app, input| {
                                let cmd = input.trim();
                                // Always return to editor; empty command is a no-op.
                                if cmd.is_empty() {
                                    app.ui_mode = UiMode::Editor {
                                        buf: buf_snapshot,
                                        show_menu: false,
                                        status_msg: None,
                                        search_input: None,
                                        save_as_input: None,
                                        pending_quit: false,
                                        confirm_exit: None,
                                    };
                                    return Ok(());
                                }
                                // Apply pipe to a working copy of the buffer; on success update editor, on error show dialog and restore.
                                let mut new_buf = buf_snapshot.clone();
                                if let Err(e) = new_buf.pipe_selection(cmd) {
                                    let restore_buf = buf_snapshot;
                                    app.ui_mode = UiMode::DialogConfirm {
                                        title: "Error".into(),
                                        message: format!("{e}"),
                                        on_ok: Box::new(move |app| {
                                            app.ui_mode = UiMode::Editor {
                                                buf: restore_buf,
                                                show_menu: false,
                                                status_msg: None,
                                                search_input: None,
                                                save_as_input: None,
                                                pending_quit: false,
                                                confirm_exit: None,
                                            };
                                            Ok(())
                                        }),
                                    };
                                } else {
                                    app.ui_mode = UiMode::Editor {
                                        buf: new_buf,
                                        show_menu: false,
                                        status_msg: None,
                                        search_input: None,
                                        save_as_input: None,
                                        pending_quit: false,
                                        confirm_exit: None,
                                    };
                                }
                                Ok(())
                            }),
                        };
                    }
                    // Block ops
                    KeyCode::F(3) => {
                        if buf.selection_bounds().is_none() {
                            buf.mark_start();
                            *status_msg = Some("Mark start".into());
                        } else if buf.selection_bounds().is_some() && buf.last_search.is_empty() {
                            // No-op hint; allow second press to clear
                            buf.mark_end();
                            *status_msg = Some("Mark end".into());
                        } else {
                            buf.clear_selection();
                            *status_msg = Some("Unmark".into());
                        }
                    }
                    KeyCode::F(5) => {
                        if buf.copy_block_here() {
                            *status_msg = Some("Copied block".into());
                        }
                    }
                    KeyCode::F(6) => {
                        if buf.move_block_here() {
                            *status_msg = Some("Moved block".into());
                        }
                    }
                    KeyCode::F(8) => {
                        if buf.delete_selection() {
                            *status_msg = Some("Deleted block".into());
                        }
                    }
                    // Save / Quit
                    KeyCode::F(2) => {
                        if let Some(path) = &buf.path {
                            let mut w = app
                                .vfs
                                .write_file(path)
                                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
                            use std::io::Write;
                            let _ = w.write_all(&buf.to_bytes());
                            buf.dirty = false;
                            *status_msg = Some("Saved".into());
                        } else {
                            *save_as_input = Some(String::new());
                        }
                    }
                    KeyCode::F(10) | KeyCode::Char('q') => {
                        if buf.dirty {
                            *confirm_exit = Some(rmc_core::app::YncDialog {
                                title: "Save modified file?".into(),
                                message:
                                    "The file has unsaved changes. Save before leaving the editor?"
                                        .into(),
                                focus: rmc_core::app::YncFocus::Yes,
                            });
                        } else {
                            app.ui_mode = UiMode::Normal;
                        }
                    }
                    // Basic cursor/editing
                    KeyCode::Left => buf.move_left(),
                    KeyCode::Right => buf.move_right(),
                    KeyCode::Up => buf.move_up(),
                    KeyCode::Down => buf.move_down(),
                    KeyCode::Backspace => buf.backspace(),
                    KeyCode::Delete => buf.delete(),
                    KeyCode::Enter => buf.insert_newline(),
                    KeyCode::Insert => buf.toggle_overwrite(),
                    KeyCode::Char(c) if key.modifiers.is_empty() && !c.is_control() => {
                        buf.insert_char(c)
                    }
                    _ => {}
                }
                return Ok(());
            }
            UiMode::Help { state, prev } => {
                // Navigation within help
                match key.code {
                    KeyCode::Esc | KeyCode::F(10) => {
                        let prior = std::mem::replace(&mut **prev, UiMode::Normal);
                        app.ui_mode = prior;
                    }
                    KeyCode::F(2) => {
                        // Index (Contents)
                        state.topic = global_index().contents_name();
                        state.cursor = 0;
                        state.scroll_top = 0;
                    }
                    KeyCode::F(3) => {
                        // Prev (history back)
                        if let Some(prev_topic) = state.history.pop() {
                            state.topic = prev_topic;
                            state.cursor = 0;
                            state.scroll_top = 0;
                        }
                    }
                    KeyCode::F(4) => {
                        // Next (follow selected link)
                        if let Some(node) = global_index().get(&state.topic) {
                            let mut cur = 0usize;
                            for it in &node.items {
                                if let HelpItem::Link { target, .. } = it {
                                    if cur == state.cursor {
                                        state.history.push(state.topic.clone());
                                        state.topic = target.clone();
                                        state.cursor = 0;
                                        state.scroll_top = 0;
                                        break;
                                    }
                                    cur += 1;
                                }
                            }
                        }
                    }
                    KeyCode::Tab | KeyCode::Down | KeyCode::Right => {
                        if let Some(node) = global_index().get(&state.topic) {
                            let links = node
                                .items
                                .iter()
                                .filter(|it| matches!(it, HelpItem::Link { .. }))
                                .count();
                            if links > 0 {
                                state.cursor = (state.cursor + 1) % links;
                            }
                        }
                    }
                    KeyCode::Up | KeyCode::Left => {
                        if let Some(node) = global_index().get(&state.topic) {
                            let links = node
                                .items
                                .iter()
                                .filter(|it| matches!(it, HelpItem::Link { .. }))
                                .count();
                            if links > 0 {
                                let prev_idx = state.cursor as isize - 1;
                                state.cursor = if prev_idx < 0 {
                                    links.saturating_sub(1)
                                } else {
                                    prev_idx as usize
                                };
                            }
                        }
                    }
                    KeyCode::PageDown => {
                        state.scroll_top = state.scroll_top.saturating_add(page_rows);
                    }
                    KeyCode::PageUp => {
                        state.scroll_top = state.scroll_top.saturating_sub(page_rows);
                    }
                    KeyCode::Home => state.scroll_top = 0,
                    KeyCode::End => {
                        state.scroll_top = state.scroll_top.saturating_add(10 * page_rows)
                    }
                    KeyCode::Enter | KeyCode::Char('\n') => {
                        if let Some(node) = global_index().get(&state.topic) {
                            let mut cur = 0usize;
                            for it in &node.items {
                                if let HelpItem::Link { target, .. } = it {
                                    if cur == state.cursor {
                                        state.history.push(state.topic.clone());
                                        state.topic = target.clone();
                                        state.cursor = 0;
                                        state.scroll_top = 0;
                                        break;
                                    }
                                    cur += 1;
                                }
                            }
                        }
                    }
                    KeyCode::Backspace | KeyCode::Char('l') => {
                        if let Some(prev_topic) = state.history.pop() {
                            state.topic = prev_topic;
                            state.cursor = 0;
                            state.scroll_top = 0;
                        }
                    }
                    KeyCode::F(1) => {
                        state.topic = global_index().contents_name();
                        state.cursor = 0;
                        state.scroll_top = 0;
                    }
                    _ => {}
                }
                return Ok(());
            }
            UiMode::ShellInput => {
                // Command line editing and execution
                match key.code {
                    KeyCode::Esc => {
                        app.ui_mode = UiMode::Normal;
                    }
                    KeyCode::Enter if key.modifiers.is_empty() => {
                        if app.subshell.cmdline.trim().is_empty() {
                            // Empty command: use panel Enter behavior
                            app.ui_mode = UiMode::Normal;
                            app.handle_action(Action::Enter)?;
                        } else {
                            let _outcome = app.subshell.execute_current(&active_cwd)?;
                            // Always rescan panels after a command (local VFS only here).
                            app.reload_panels()?;
                            app.subshell.clear_cmdline();
                            app.ui_mode = UiMode::Normal;
                        }
                    }
                    KeyCode::Backspace => {
                        app.subshell.cmdline.pop();
                        app.subshell.clear_history_nav();
                    }
                    KeyCode::Up => {
                        if let Some(s) = app.subshell.history_prev() {
                            app.subshell.cmdline = s;
                        }
                    }
                    KeyCode::Down => {
                        if let Some(s) = app.subshell.history_next() {
                            app.subshell.cmdline = s;
                        }
                    }
                    KeyCode::Char('p')
                        if key.modifiers.contains(crossterm::event::KeyModifiers::ALT) =>
                    {
                        if let Some(s) = app.subshell.history_prev() {
                            app.subshell.cmdline = s;
                        }
                    }
                    KeyCode::Char('n')
                        if key.modifiers.contains(crossterm::event::KeyModifiers::ALT) =>
                    {
                        if let Some(s) = app.subshell.history_next() {
                            app.subshell.cmdline = s;
                        }
                    }
                    KeyCode::Enter
                        if key.modifiers.contains(crossterm::event::KeyModifiers::ALT) =>
                    {
                        // Alt-Enter: copy current filename into command line
                        if let Some(ent) = app.active_panel().current_entry() {
                            let name = ent.name.clone();
                            app.subshell.append_filename(&name);
                        }
                    }
                    KeyCode::Char(c) if key.modifiers.is_empty() => {
                        app.subshell.cmdline.push(c);
                        app.subshell.clear_history_nav();
                    }
                    _ => {}
                }
                return Ok(());
            }
            UiMode::HotlistDialog(state) => {
                // Estimate list rows based on current terminal size
                let (_cols, rows) = crossterm::terminal::size()?;
                let list_rows = rows.saturating_sub(4).clamp(12, 20).saturating_sub(4) as usize;
                match key.code {
                    KeyCode::Esc | KeyCode::F(10) => {
                        app.ui_mode = UiMode::Normal;
                    }
                    KeyCode::Tab => {
                        state.focus = match state.focus {
                            HDF::List => HDF::ButtonGoto,
                            HDF::ButtonGoto => HDF::ButtonAdd,
                            HDF::ButtonAdd => HDF::ButtonRemove,
                            HDF::ButtonRemove => HDF::ButtonCancel,
                            HDF::ButtonCancel => HDF::List,
                        };
                    }
                    KeyCode::Up if matches!(state.focus, HDF::List) => {
                        if state.selected_index > 0 {
                            state.selected_index -= 1;
                        }
                        if state.selected_index < state.scroll_top {
                            state.scroll_top = state.selected_index;
                        }
                    }
                    KeyCode::Down if matches!(state.focus, HDF::List) => {
                        if state.selected_index + 1 < state.entries.len() {
                            state.selected_index += 1;
                        }
                        if state.selected_index >= state.scroll_top + list_rows {
                            state.scroll_top = state
                                .selected_index
                                .saturating_sub(list_rows.saturating_sub(1));
                        }
                    }
                    KeyCode::Home if matches!(state.focus, HDF::List) => {
                        state.selected_index = 0;
                        state.scroll_top = 0;
                    }
                    KeyCode::End if matches!(state.focus, HDF::List) => {
                        if !state.entries.is_empty() {
                            state.selected_index = state.entries.len() - 1;
                            state.scroll_top = state
                                .selected_index
                                .saturating_sub(list_rows.saturating_sub(1));
                        }
                    }
                    KeyCode::Enter => match state.focus {
                        HDF::List | HDF::ButtonGoto => {
                            if let Some(entry) = state.entries.get(state.selected_index).cloned() {
                                let _ = app.change_dir(&entry.path);
                                app.ui_mode = UiMode::Normal;
                            }
                        }
                        HDF::ButtonAdd => {
                            let cwd = app.active_panel().cwd.clone();
                            let suggested = cwd
                                .file_name()
                                .and_then(|s| s.to_str())
                                .unwrap_or("")
                                .to_string();
                            app.ui_mode = UiMode::PromptInput {
                                title: "Add to hotlist: Label".into(),
                                value: suggested,
                                on_submit: Box::new(move |app, label| {
                                    if !label.trim().is_empty() {
                                        app.hotlist
                                            .add_or_replace(label.trim().to_string(), cwd)?;
                                        app.hotlist.save_to_default_path()?;
                                    }
                                    // Reopen dialog with updated entries
                                    let st = rmc_core::hotlist::HotlistDialogState::new(
                                        app.hotlist.entries.clone(),
                                    );
                                    app.ui_mode = UiMode::HotlistDialog(st);
                                    Ok(())
                                }),
                            };
                        }
                        HDF::ButtonRemove => {
                            if state.selected_index < state.entries.len() {
                                app.hotlist.remove_at(state.selected_index);
                                app.hotlist.save_to_default_path()?;
                                // refresh dialog state
                                state.entries = app.hotlist.entries.clone();
                                if state.selected_index >= state.entries.len()
                                    && !state.entries.is_empty()
                                {
                                    state.selected_index = state.entries.len() - 1;
                                }
                            }
                        }
                        HDF::ButtonCancel => {
                            app.ui_mode = UiMode::Normal;
                        }
                    },
                    _ => {}
                }
                return Ok(());
            }
            UiMode::PromptInput {
                title: _,
                value,
                on_submit,
            } => {
                match key.code {
                    KeyCode::Esc => {
                        app.ui_mode = UiMode::Normal;
                    }
                    KeyCode::Enter => {
                        let cb = std::mem::replace(on_submit, Box::new(|_, _| Ok(())));
                        let val = value.clone();
                        app.ui_mode = UiMode::Normal;
                        cb(app, val)?;
                        app.reload_panels()?;
                    }
                    KeyCode::Backspace => {
                        value.pop();
                    }
                    KeyCode::Char(c) if key.modifiers.is_empty() => {
                        value.push(c);
                    }
                    KeyCode::Char(_) => {}
                    _ => {}
                }
                return Ok(());
            }
            UiMode::InputDialog {
                title: _,
                prompt: _,
                value,
                on_submit,
                focus_ok,
            } => {
                match key.code {
                    KeyCode::Esc | KeyCode::F(10) => {
                        app.ui_mode = UiMode::Normal;
                    }
                    KeyCode::Tab => {
                        *focus_ok = !*focus_ok;
                    }
                    KeyCode::Enter => {
                        if *focus_ok {
                            let cb = std::mem::replace(on_submit, Box::new(|_, _| Ok(())));
                            let val = value.clone();
                            app.ui_mode = UiMode::Normal;
                            cb(app, val)?;
                            app.reload_panels()?;
                        } else {
                            // stay, user can toggle to OK
                        }
                    }
                    KeyCode::Backspace => {
                        value.pop();
                    }
                    KeyCode::Char(c) if key.modifiers.is_empty() => {
                        value.push(c);
                    }
                    _ => {}
                }
                return Ok(());
            }
            UiMode::DialogConfirm {
                title: _,
                message: _,
                on_ok,
            } => {
                match key.code {
                    KeyCode::Esc => {
                        app.ui_mode = UiMode::Normal;
                    }
                    KeyCode::Enter => {
                        let cb = std::mem::replace(on_ok, Box::new(|_| Ok(())));
                        app.ui_mode = UiMode::Normal;
                        cb(app)?;
                        app.reload_panels()?;
                    }
                    _ => {}
                }
                return Ok(());
            }
            UiMode::FindDialog(state) => {
                // Update from background search if completed
                if state.running {
                    if let Some(rx) = &state.results_rx {
                        loop {
                            match rx.try_recv() {
                                Ok(p) => state.results.paths.push(p),
                                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                                    state.running = false;
                                    state.cancel = None;
                                    state.results_rx = None;
                                    break;
                                }
                            }
                        }
                    }
                }
                match key.code {
                    KeyCode::Esc => {
                        app.ui_mode = UiMode::Normal;
                    }
                    KeyCode::Tab => {
                        state.focus = match state.focus {
                            FF::StartDir => FF::NamePattern,
                            FF::NamePattern => FF::Content,
                            FF::Content => FF::CaseSensitive,
                            FF::CaseSensitive => FF::ButtonStart,
                            FF::ButtonStart => FF::ButtonStop,
                            FF::ButtonStop => FF::ButtonChdir,
                            FF::ButtonChdir => FF::ButtonAgain,
                            FF::ButtonAgain => FF::ButtonPanelize,
                            FF::ButtonPanelize => FF::ButtonQuit,
                            FF::ButtonQuit => FF::StartDir,
                        };
                    }
                    KeyCode::Up => {
                        if state.selected_index > 0 {
                            state.selected_index -= 1;
                        }
                        // ensure visible
                        let (_c, r) = crossterm::terminal::size()?;
                        let h = r.saturating_sub(4).clamp(16, 22);
                        let list_rows = (h - 12) as usize;
                        if state.selected_index < state.scroll_top {
                            state.scroll_top = state.selected_index;
                        } else if state.selected_index >= state.scroll_top + list_rows {
                            state.scroll_top = state
                                .selected_index
                                .saturating_sub(list_rows.saturating_sub(1));
                        }
                    }
                    KeyCode::Down => {
                        if state.selected_index + 1 < state.results.paths.len() {
                            state.selected_index += 1;
                        }
                        let (_c, r) = crossterm::terminal::size()?;
                        let h = r.saturating_sub(4).clamp(16, 22);
                        let list_rows = (h - 12) as usize;
                        if state.selected_index < state.scroll_top {
                            state.scroll_top = state.selected_index;
                        } else if state.selected_index >= state.scroll_top + list_rows {
                            state.scroll_top = state
                                .selected_index
                                .saturating_sub(list_rows.saturating_sub(1));
                        }
                    }
                    KeyCode::Home => {
                        state.selected_index = 0;
                        state.scroll_top = 0;
                    }
                    KeyCode::End => {
                        if !state.results.paths.is_empty() {
                            state.selected_index = state.results.paths.len() - 1;
                            let (_c, r) = crossterm::terminal::size()?;
                            let h = r.saturating_sub(4).clamp(16, 22);
                            let list_rows = (h - 12) as usize;
                            state.scroll_top = state
                                .selected_index
                                .saturating_sub(list_rows.saturating_sub(1));
                        }
                    }
                    KeyCode::Backspace => match state.focus {
                        FF::StartDir => {
                            state.start_dir_edit.pop();
                        }
                        FF::NamePattern => match &mut state.params.name_pattern {
                            rmc_core::find::NamePattern::Glob(s) => {
                                s.pop();
                            }
                        },
                        FF::Content => {
                            if let Some(s) = &mut state.params.content_substring {
                                s.pop();
                                if s.is_empty() {
                                    state.params.content_substring = None;
                                }
                            }
                        }
                        _ => {}
                    },
                    KeyCode::Char(c) => {
                        if key.modifiers.is_empty() {
                            match state.focus {
                                FF::StartDir => {
                                    if !c.is_control() {
                                        state.start_dir_edit.push(c);
                                    }
                                }
                                FF::NamePattern => match &mut state.params.name_pattern {
                                    rmc_core::find::NamePattern::Glob(s) => {
                                        s.push(c);
                                    }
                                },
                                FF::Content => {
                                    if c == '\n' {
                                        // ignore
                                    } else if let Some(ref mut s) = state.params.content_substring {
                                        s.push(c);
                                    } else {
                                        state.params.content_substring = Some(c.to_string());
                                    }
                                }
                                FF::CaseSensitive if c == ' ' => {
                                    state.params.case_sensitive = !state.params.case_sensitive;
                                }
                                _ => {}
                            }
                        } else if key
                            .modifiers
                            .contains(crossterm::event::KeyModifiers::CONTROL)
                            && c == 'c'
                        {
                            // Allow Ctrl-C to stop search when focused anywhere
                            if let Some(ch) = &state.cancel {
                                ch.cancel();
                            }
                        }
                    }
                    KeyCode::Enter => {
                        match state.focus {
                            FF::ButtonStart | FF::ButtonAgain => {
                                if !state.running {
                                    // Prepare params (apply Start at edit)
                                    let start_str = state.start_dir_edit.trim().to_string();
                                    let start_dir = if start_str.is_empty() {
                                        active_cwd.clone()
                                    } else {
                                        std::path::PathBuf::from(start_str)
                                    };
                                    state.params.start_dir = start_dir;
                                    // Reset results and selection
                                    state.results.paths.clear();
                                    state.selected_index = 0;
                                    state.scroll_top = 0;
                                    // Kick off background search (streaming)
                                    let params = state.params.clone();
                                    let cancel = CancelHandle::new();
                                    let flag = cancel.flag();
                                    let (tx, rx) = std::sync::mpsc::channel();
                                    state.cancel = Some(cancel);
                                    state.results_rx = Some(rx);
                                    state.running = true;
                                    std::thread::spawn(move || {
                                        search_files_streaming(&params, &flag, |p| {
                                            let _ = tx.send(p);
                                        });
                                        // dropping tx signals completion
                                    });
                                }
                            }
                            FF::ButtonStop => {
                                if let Some(ch) = &state.cancel {
                                    ch.cancel();
                                }
                            }
                            FF::ButtonChdir => {
                                if let Some(p) =
                                    state.results.paths.get(state.selected_index).cloned()
                                {
                                    if let Ok(md) = app.vfs.stat(&p) {
                                        let dest = if md.is_dir {
                                            p
                                        } else {
                                            p.parent()
                                                .map(|x| x.to_path_buf())
                                                .unwrap_or(active_cwd.clone())
                                        };
                                        let _ = app.change_dir(&dest);
                                        app.ui_mode = UiMode::Normal;
                                    }
                                }
                            }
                            FF::ButtonPanelize => {
                                if !state.running && !state.results.paths.is_empty() {
                                    let paths = state.results.paths.clone();
                                    let base = state.params.start_dir.clone();
                                    app.panelize_paths(&paths, Some(&base))?;
                                    app.ui_mode = UiMode::Normal;
                                }
                            }
                            FF::ButtonQuit => {
                                app.ui_mode = UiMode::Normal;
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
                return Ok(());
            }
            UiMode::SortDialog {
                side,
                focus_index,
                by,
                reverse,
                dirs_first,
            } => {
                // Focus order: 0..3 radios; 4 Reverse; 5 Dirs-first; 6 OK; 7 Cancel
                let mut apply: Option<(
                    rmc_core::actions::PaneSide,
                    rmc_core::panel::SortBy,
                    bool,
                    bool,
                )> = None;
                let mut close_dialog = false;
                match key.code {
                    KeyCode::Esc | KeyCode::F(10) => {
                        close_dialog = true;
                    }
                    KeyCode::Tab => {
                        *focus_index = (*focus_index + 1) % 8;
                    }
                    KeyCode::BackTab => {
                        *focus_index = (*focus_index + 8 - 1) % 8;
                    }
                    KeyCode::Up => {
                        if *focus_index > 0 {
                            *focus_index -= 1;
                        }
                    }
                    KeyCode::Down => {
                        *focus_index = (*focus_index + 1).min(7);
                    }
                    KeyCode::Char(' ') => {
                        if *focus_index <= 3 {
                            *by = match *focus_index {
                                0 => rmc_core::panel::SortBy::Name,
                                1 => rmc_core::panel::SortBy::Ext,
                                2 => rmc_core::panel::SortBy::Time,
                                3 => rmc_core::panel::SortBy::Size,
                                _ => *by,
                            };
                        } else if *focus_index == 4 {
                            *reverse = !*reverse;
                        } else if *focus_index == 5 {
                            *dirs_first = !*dirs_first;
                        } else if *focus_index == 6 {
                            // OK via space
                            apply = Some((*side, *by, *reverse, *dirs_first));
                            close_dialog = true;
                        } else if *focus_index == 7 {
                            // Cancel via space
                            close_dialog = true;
                        }
                    }
                    KeyCode::Enter => {
                        match *focus_index {
                            0..=3 => {
                                // select radio
                                *by = match *focus_index {
                                    0 => rmc_core::panel::SortBy::Name,
                                    1 => rmc_core::panel::SortBy::Ext,
                                    2 => rmc_core::panel::SortBy::Time,
                                    3 => rmc_core::panel::SortBy::Size,
                                    _ => *by,
                                };
                            }
                            4 => *reverse = !*reverse,
                            5 => *dirs_first = !*dirs_first,
                            6 => {
                                apply = Some((*side, *by, *reverse, *dirs_first));
                                close_dialog = true;
                            }
                            7 => {
                                close_dialog = true;
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
                let _ = side;
                let _ = focus_index;
                let _ = by;
                let _ = reverse;
                let _ = dirs_first;
                if let Some((s, b, r, d)) = apply {
                    Self::apply_sort_dialog(app, s, b, r, d)?;
                }
                if close_dialog {
                    app.ui_mode = UiMode::Normal;
                }
                return Ok(());
            }
            UiMode::MkdirDialog { value, focus_ok } => {
                match key.code {
                    KeyCode::Esc => app.ui_mode = UiMode::Normal,
                    KeyCode::Tab => *focus_ok = !*focus_ok,
                    KeyCode::Enter => {
                        if *focus_ok {
                            if !value.is_empty() {
                                let dir = active_cwd.join(value.clone());
                                app.vfs.mkdir(&dir)?;
                            }
                            app.reload_panels()?;
                        }
                        app.ui_mode = UiMode::Normal;
                    }
                    KeyCode::Backspace => {
                        if !*focus_ok {
                            value.pop();
                        }
                    }
                    KeyCode::Char(c) if !*focus_ok && key.modifiers.is_empty() => {
                        value.push(c);
                    }
                    KeyCode::Char(_) => {}
                    _ => {}
                }
                return Ok(());
            }
            UiMode::UserMenu {
                title: _,
                entries,
                selected_index,
            } => {
                let mut run_command_text: Option<String> = None;
                match key.code {
                    KeyCode::Esc | KeyCode::F(10) => {
                        app.ui_mode = UiMode::Normal;
                    }
                    KeyCode::Up => {
                        if *selected_index > 0 {
                            *selected_index -= 1;
                        }
                    }
                    KeyCode::Down => {
                        if *selected_index + 1 < entries.len() {
                            *selected_index += 1;
                        }
                    }
                    KeyCode::Enter => {
                        if let Some(ent) = entries.get(*selected_index) {
                            run_command_text = Some(ent.command.clone());
                            app.ui_mode = UiMode::Normal;
                        }
                    }
                    KeyCode::Char(c) => {
                        // Hotkey invoke
                        let lc = c.to_ascii_lowercase();
                        if let Some((idx, _)) = entries
                            .iter()
                            .enumerate()
                            .find(|(_, e)| e.hotkey.map(|k| k.to_ascii_lowercase()) == Some(lc))
                        {
                            *selected_index = idx;
                            if let Some(ent) = entries.get(idx) {
                                run_command_text = Some(ent.command.clone());
                                app.ui_mode = UiMode::Normal;
                            }
                        }
                    }
                    _ => {}
                }
                // After releasing the mutable borrow on ui_mode, run the command if requested
                if let Some(txt) = run_command_text {
                    let cmd = rmc_core::user_menu::expand_macros(app, &txt);
                    let _ = rmc_core::user_menu::run_menu_command(app, &cmd);
                    app.reload_panels()?;
                }
                return Ok(());
            }
            UiMode::DeleteDialog {
                name: _,
                path,
                focus_ok,
            } => {
                match key.code {
                    KeyCode::Esc => app.ui_mode = UiMode::Normal,
                    KeyCode::Tab => *focus_ok = !*focus_ok,
                    KeyCode::Enter => {
                        if *focus_ok {
                            app.vfs.remove(path, true)?;
                            app.reload_panels()?;
                        }
                        app.ui_mode = UiMode::Normal;
                    }
                    _ => {}
                }
                return Ok(());
            }
            UiMode::CopyDialog {
                title,
                src_name: _,
                src_path,
                mask,
                to,
                using_shell_patterns,
                follow_links,
                preserve_attrs,
                dive_into_subdir,
                stable_symlinks,
                focus,
            } => {
                use rmc_core::app::CopyDialogFocus as F;
                match key.code {
                    KeyCode::Esc => app.ui_mode = UiMode::Normal,
                    KeyCode::Tab => {
                        *focus = match *focus {
                            F::Mask => F::To,
                            F::To => F::Checkbox1,
                            F::Checkbox1 => F::Checkbox2,
                            F::Checkbox2 => F::Checkbox3,
                            F::Checkbox3 => F::Checkbox4,
                            F::Checkbox4 => F::Checkbox5,
                            F::Checkbox5 => F::Ok,
                            F::Ok => F::Background,
                            F::Background => F::Cancel,
                            F::Cancel => F::Mask,
                        };
                    }
                    KeyCode::Backspace => match *focus {
                        F::Mask => {
                            mask.pop();
                        }
                        F::To => {
                            to.pop();
                        }
                        _ => {}
                    },
                    KeyCode::Char(c) => {
                        if key.modifiers.is_empty() {
                            if c == ' ' {
                                match *focus {
                                    F::Checkbox1 => *using_shell_patterns = !*using_shell_patterns,
                                    F::Checkbox2 => *follow_links = !*follow_links,
                                    F::Checkbox3 => *preserve_attrs = !*preserve_attrs,
                                    F::Checkbox4 => *dive_into_subdir = !*dive_into_subdir,
                                    F::Checkbox5 => *stable_symlinks = !*stable_symlinks,
                                    _ => {}
                                }
                            } else {
                                match *focus {
                                    F::Mask => mask.push(c),
                                    F::To => to.push(c),
                                    _ => {}
                                }
                            }
                        }
                    }
                    KeyCode::Enter => {
                        use std::path::Path;
                        match *focus {
                            F::Checkbox1 => *using_shell_patterns = !*using_shell_patterns,
                            F::Checkbox2 => *follow_links = !*follow_links,
                            F::Checkbox3 => *preserve_attrs = !*preserve_attrs,
                            F::Checkbox4 => *dive_into_subdir = !*dive_into_subdir,
                            F::Checkbox5 => *stable_symlinks = !*stable_symlinks,
                            F::Ok => {
                                // If destination exists, open overwrite dialog; else perform action
                                let dst = Path::new(&*to).to_path_buf();
                                let exists = app.vfs.stat(&dst).is_ok();
                                if exists {
                                    let op = if title == "Copy" {
                                        rmc_core::app::CopyMoveOp::Copy
                                    } else {
                                        rmc_core::app::CopyMoveOp::Move
                                    };
                                    app.ui_mode = UiMode::OverwriteDialog {
                                        op,
                                        src_path: src_path.clone(),
                                        dst_path: dst,
                                        focus: rmc_core::app::OverwriteFocus::Yes,
                                    };
                                } else {
                                    if title == "Copy" {
                                        app.vfs.copy(src_path, Path::new(&*to))?;
                                    } else {
                                        app.vfs.move_path(src_path, Path::new(&*to))?;
                                    }
                                    app.ui_mode = UiMode::Normal;
                                    app.reload_panels()?;
                                }
                            }
                            F::Background | F::Cancel => {
                                app.ui_mode = UiMode::Normal;
                            }
                            _ => {}
                        }
                    }

                    _ => {}
                }
                return Ok(());
            }
            UiMode::OverwriteDialog {
                op,
                src_path,
                dst_path,
                focus,
            } => {
                use rmc_core::app::OverwriteFocus as OF;
                match key.code {
                    KeyCode::Esc | KeyCode::F(10) => {
                        app.ui_mode = UiMode::Normal;
                    }
                    KeyCode::Tab => {
                        *focus = match *focus {
                            OF::Yes => OF::No,
                            OF::No => OF::All,
                            OF::All => OF::Older,
                            OF::Older => OF::None,
                            OF::None => OF::Smaller,
                            OF::Smaller => OF::SizeDiffers,
                            OF::SizeDiffers => OF::Append,
                            OF::Append => OF::Yes,
                        };
                    }
                    KeyCode::BackTab => {
                        *focus = match *focus {
                            OF::Yes => OF::Append,
                            OF::No => OF::Yes,
                            OF::All => OF::No,
                            OF::Older => OF::All,
                            OF::None => OF::Older,
                            OF::Smaller => OF::None,
                            OF::SizeDiffers => OF::Smaller,
                            OF::Append => OF::SizeDiffers,
                        };
                    }
                    KeyCode::Enter => {
                        // Apply selection
                        let act_yes = match *focus {
                            OF::Yes | OF::All => true,
                            OF::No | OF::None => false,
                            OF::Older => {
                                if let (Ok(s), Ok(d)) =
                                    (app.vfs.stat(src_path), app.vfs.stat(dst_path))
                                {
                                    s.modified > d.modified
                                } else {
                                    false
                                }
                            }
                            OF::Smaller => {
                                if let (Ok(s), Ok(d)) =
                                    (app.vfs.stat(src_path), app.vfs.stat(dst_path))
                                {
                                    s.size < d.size
                                } else {
                                    false
                                }
                            }
                            OF::SizeDiffers => {
                                if let (Ok(s), Ok(d)) =
                                    (app.vfs.stat(src_path), app.vfs.stat(dst_path))
                                {
                                    s.size != d.size
                                } else {
                                    false
                                }
                            }
                            OF::Append => false,
                        };
                        if *focus == OF::Append {
                            use std::fs::OpenOptions;
                            use std::io::{Read, Write};
                            let res = (|| -> anyhow::Result<()> {
                                let mut rdr = app.vfs.read_file(src_path)?;
                                let mut f = OpenOptions::new()
                                    .create(true)
                                    .append(true)
                                    .open(dst_path)?;
                                let mut buf = Vec::new();
                                rdr.read_to_end(&mut buf)?;
                                f.write_all(&buf)?;
                                Ok(())
                            })();
                            match res {
                                Ok(()) => {
                                    app.ui_mode = UiMode::Normal;
                                    app.reload_panels()?;
                                }
                                Err(err) => {
                                    app.ui_mode = UiMode::DialogConfirm {
                                        title: "Error".into(),
                                        message: format!("{err}"),
                                        on_ok: Box::new(|_| Ok(())),
                                    };
                                }
                            }
                        } else if act_yes {
                            let _ = app.vfs.remove(dst_path, false);
                            let res = match *op {
                                rmc_core::app::CopyMoveOp::Copy => app.vfs.copy(src_path, dst_path),
                                rmc_core::app::CopyMoveOp::Move => {
                                    app.vfs.move_path(src_path, dst_path)
                                }
                            };
                            match res {
                                Ok(()) => {
                                    app.ui_mode = UiMode::Normal;
                                    app.reload_panels()?;
                                }
                                Err(err) => {
                                    app.ui_mode = UiMode::DialogConfirm {
                                        title: "Error".into(),
                                        message: format!("{err}"),
                                        on_ok: Box::new(|_| Ok(())),
                                    };
                                }
                            }
                        } else {
                            app.ui_mode = UiMode::Normal;
                        }
                    }
                    _ => {}
                }
                return Ok(());
            }
            UiMode::ChmodDialog {
                name: _,
                mode,
                ur,
                uw,
                ux,
                gr,
                gw,
                gx,
                or_,
                ow,
                ox,
                suid,
                sgid,
                sticky,
                recursive,
                focus_index,
            } => {
                // 0..8: rwx (u,g,o), 9..11: suid/sgid/sticky, 12: recursive, 13: OK, 14: Cancel
                let total_fields = 15usize;
                match key.code {
                    KeyCode::Esc | KeyCode::F(10) => app.ui_mode = UiMode::Normal,
                    KeyCode::Tab => {
                        *focus_index = (*focus_index + 1) % total_fields;
                    }
                    KeyCode::BackTab => {
                        *focus_index = (*focus_index + total_fields - 1) % total_fields;
                    }
                    KeyCode::Char(' ') => {
                        match *focus_index {
                            0 => *ur = !*ur,
                            1 => *uw = !*uw,
                            2 => *ux = !*ux,
                            3 => *gr = !*gr,
                            4 => *gw = !*gw,
                            5 => *gx = !*gx,
                            6 => *or_ = !*or_,
                            7 => *ow = !*ow,
                            8 => *ox = !*ox,
                            9 => *suid = !*suid,
                            10 => *sgid = !*sgid,
                            11 => *sticky = !*sticky,
                            12 => *recursive = !*recursive,
                            _ => {}
                        }
                        // Recompute mode from flags
                        let mut m = 0u32;
                        if *ur {
                            m |= 0o400
                        }
                        if *uw {
                            m |= 0o200
                        }
                        if *ux {
                            m |= 0o100
                        }
                        if *gr {
                            m |= 0o040
                        }
                        if *gw {
                            m |= 0o020
                        }
                        if *gx {
                            m |= 0o010
                        }
                        if *or_ {
                            m |= 0o004
                        }
                        if *ow {
                            m |= 0o002
                        }
                        if *ox {
                            m |= 0o001
                        }
                        if *suid {
                            m |= 0o4000
                        }
                        if *sgid {
                            m |= 0o2000
                        }
                        if *sticky {
                            m |= 0o1000
                        }
                        *mode = m;
                    }
                    KeyCode::Enter => {
                        // Cancel selected
                        if *focus_index == 14 {
                            app.ui_mode = UiMode::Normal;
                            return Ok(());
                        }
                        // Only OK applies
                        if *focus_index != 13 {
                            return Ok(());
                        }
                        let mode_val = *mode;
                        let recursive_val = *recursive;
                        app.ui_mode = UiMode::Normal;
                        // Collect paths: selected entries or current
                        let paths: Vec<std::path::PathBuf> = {
                            let p = app.active_panel();
                            let mut out = Vec::new();
                            if p.selection.is_empty() {
                                if let Some(ent) = p.current_entry() {
                                    if ent.name != ".." {
                                        out.push(ent.path.clone());
                                    }
                                }
                            } else {
                                for idx in p.selection.iter() {
                                    if let Some(ent) = p.entries.get(idx) {
                                        if ent.name != ".." {
                                            out.push(ent.path.clone());
                                        }
                                    }
                                }
                            }
                            out
                        };
                        // Try apply; on error, show error dialog
                        let mut first_err: Option<anyhow::Error> = None;
                        for p in paths {
                            if let Err(e) = app.vfs.chmod(&p, mode_val, recursive_val) {
                                first_err = Some(anyhow::Error::new(e));
                                break;
                            }
                        }
                        if let Some(err) = first_err {
                            app.ui_mode = UiMode::DialogConfirm {
                                title: "Error".into(),
                                message: format!("{err}"),
                                on_ok: Box::new(|_| Ok(())),
                            };
                        } else {
                            app.reload_panels()?;
                        }
                    }
                    _ => {}
                }
                return Ok(());
            }
            UiMode::ChownDialog {
                owner,
                group,
                recursive,
                focus_index,
            } => {
                match key.code {
                    KeyCode::Esc | KeyCode::F(10) => app.ui_mode = UiMode::Normal,
                    KeyCode::Tab => {
                        *focus_index = (*focus_index + 1) % 5;
                    }
                    KeyCode::BackTab => {
                        *focus_index = (*focus_index + 5 - 1) % 5;
                    }
                    KeyCode::Char(c) if *focus_index == 0 && key.modifiers.is_empty() => {
                        owner.push(c);
                    }
                    KeyCode::Backspace if *focus_index == 0 => {
                        owner.pop();
                    }
                    KeyCode::Char(c) if *focus_index == 1 && key.modifiers.is_empty() => {
                        group.push(c);
                    }
                    KeyCode::Backspace if *focus_index == 1 => {
                        group.pop();
                    }
                    KeyCode::Char(' ') if *focus_index == 2 => {
                        *recursive = !*recursive;
                    }
                    KeyCode::Enter => {
                        // Cancel
                        if *focus_index == 4 {
                            app.ui_mode = UiMode::Normal;
                            return Ok(());
                        }
                        // Only OK applies
                        if *focus_index != 3 {
                            return Ok(());
                        }
                        // Capture values and close dialog to avoid borrow conflicts
                        let owner_val = owner.clone();
                        let group_val = group.clone();
                        let recursive_val = *recursive;
                        app.ui_mode = UiMode::Normal;
                        // Apply to selected/current
                        let paths: Vec<std::path::PathBuf> = {
                            let p = app.active_panel();
                            let mut out = Vec::new();
                            if p.selection.is_empty() {
                                if let Some(ent) = p.current_entry() {
                                    if ent.name != ".." {
                                        out.push(ent.path.clone());
                                    }
                                }
                            } else {
                                for idx in p.selection.iter() {
                                    if let Some(ent) = p.entries.get(idx) {
                                        if ent.name != ".." {
                                            out.push(ent.path.clone());
                                        }
                                    }
                                }
                            }
                            out
                        };
                        let owner_opt = if owner_val.trim().is_empty() {
                            None
                        } else {
                            Some(owner_val.trim().to_string())
                        };
                        let group_opt = if group_val.trim().is_empty() {
                            None
                        } else {
                            Some(group_val.trim().to_string())
                        };
                        let mut first_err: Option<anyhow::Error> = None;
                        for p in paths {
                            if let Err(e) = app.vfs.chown(
                                &p,
                                owner_opt.as_deref(),
                                group_opt.as_deref(),
                                recursive_val,
                            ) {
                                first_err = Some(anyhow::Error::new(e));
                                break;
                            }
                        }
                        if let Some(err) = first_err {
                            app.ui_mode = UiMode::DialogConfirm {
                                title: "Error".into(),
                                message: format!("{err}"),
                                on_ok: Box::new(|_| Ok(())),
                            };
                        } else {
                            app.reload_panels()?;
                        }
                    }
                    _ => {}
                }
                return Ok(());
            }
            UiMode::Menu {
                top_index,
                selected_index,
            } => {
                let menus: [&[&str]; 5] = [
                    &[
                        "Copy",
                        "Move",
                        "Mkdir",
                        "Delete",
                        "FTP link",
                        "SFTP link",
                        "Sort order...",
                        "Tree",
                        "Filter",
                    ],
                    &[
                        "View",
                        "Edit",
                        "Copy",
                        "Move",
                        "Mkdir",
                        "Delete",
                        "Chmod",
                        "Chown",
                        "Hard link",
                        "SymLink",
                        "Relative symlink",
                        "Quit",
                    ],
                    &[
                        "User menu",
                        "Find file",
                        "Directory hotlist",
                        "Compare dirs",
                    ],
                    &["Layout", "Panels", "Confirmations"],
                    &[
                        "Copy",
                        "Move",
                        "Mkdir",
                        "Delete",
                        "FTP link",
                        "SFTP link",
                        "Sort order...",
                        "Tree",
                        "Filter",
                    ],
                ];
                match key.code {
                    KeyCode::Esc | KeyCode::F(9) | KeyCode::F(10) => {
                        app.ui_mode = UiMode::Normal;
                    }
                    KeyCode::Left => {
                        if *top_index > 0 {
                            *top_index -= 1;
                        }
                        *selected_index = 0;
                    }
                    KeyCode::Right => {
                        if *top_index < 4 {
                            *top_index += 1;
                        }
                        *selected_index = 0;
                    }
                    KeyCode::Up => {
                        if *selected_index > 0 {
                            *selected_index -= 1;
                        }
                    }
                    KeyCode::Down => {
                        let max = menus[*top_index].len().saturating_sub(1);
                        if *selected_index < max {
                            *selected_index += 1;
                        }
                    }
                    KeyCode::Enter => {
                        let item = menus[*top_index][*selected_index];
                        match item {
                            "Filter" => {
                                // Open simple input dialog to set filename filter for the chosen side.
                                let set_left = *top_index == 0;
                                app.ui_mode = UiMode::InputDialog {
                                    title: "Filter".into(),
                                    prompt: "Enter glob (e.g. *.c):".into(),
                                    value: "*".into(),
                                    focus_ok: false,
                                    on_submit: Box::new(move |app, input| {
                                        let pat = input.trim().to_string();
                                        let panel = if set_left {
                                            &mut app.left
                                        } else {
                                            &mut app.right
                                        };
                                        if pat.is_empty() || pat == "*" {
                                            panel.filter_glob = None;
                                        } else {
                                            panel.filter_glob = Some(pat);
                                        }
                                        Ok(())
                                    }),
                                };
                            }
                            "FTP link" | "SFTP link" => {
                                let scheme = if item == "FTP link" { "ftp" } else { "sftp" };
                                // Prompt user for host or URL; accept "user@host" or full URL.
                                app.ui_mode = UiMode::InputDialog {
                                    title: format!("{} link", scheme.to_uppercase()),
                                    prompt: format!(
                                        "Enter {} host/user or URL:",
                                        scheme.to_uppercase()
                                    ),
                                    value: String::new(),
                                    focus_ok: false,
                                    on_submit: Box::new(move |app, input| {
                                        let trimmed = input.trim();
                                        if trimmed.is_empty() {
                                            return Ok(());
                                        }
                                        let url_str = if trimmed.starts_with("ftp://")
                                            || trimmed.starts_with("sftp://")
                                        {
                                            trimmed.to_string()
                                        } else {
                                            // Build scheme://<input> (ensure one leading slash for empty path)
                                            format!("{scheme}://{trimmed}")
                                        };
                                        let path = std::path::PathBuf::from(url_str);
                                        match app.change_dir(&path) {
                                            Ok(()) => Ok(()),
                                            Err(err) => {
                                                app.ui_mode = UiMode::DialogConfirm {
                                                    title: "Error".into(),
                                                    message: format!("{err}"),
                                                    on_ok: Box::new(|_| Ok(())),
                                                };
                                                Ok(())
                                            }
                                        }
                                    }),
                                };
                            }
                            "Sort order..." => {
                                let side = match *top_index {
                                    0 => rmc_core::actions::PaneSide::Left,
                                    4 => rmc_core::actions::PaneSide::Right,
                                    _ => rmc_core::actions::PaneSide::Left,
                                };
                                // Prefill from the chosen panel
                                let (by, reverse, dirs_first) = {
                                    let p = if matches!(side, rmc_core::actions::PaneSide::Left) {
                                        &app.left
                                    } else {
                                        &app.right
                                    };
                                    (
                                        p.sort_by,
                                        matches!(p.sort_dir, rmc_core::sorting::SortDir::Desc),
                                        p.dirs_first,
                                    )
                                };
                                app.ui_mode = UiMode::SortDialog {
                                    side,
                                    focus_index: 0,
                                    by,
                                    reverse,
                                    dirs_first,
                                };
                            }
                            "Tree" => {
                                // Set Tree mode for the chosen side and make it active.
                                let set_left = *top_index == 0;
                                let panel = if set_left {
                                    &mut app.left
                                } else {
                                    &mut app.right
                                };
                                panel.mode = rmc_core::panel::PanelMode::Tree;
                                // Build a simple flattened tree starting at the panel's cwd
                                let start = panel.cwd.clone();
                                let max_entries = 2048usize;
                                let mut entries: Vec<rmc_core::panel::TreeEntry> = Vec::new();
                                build_tree_flat(&*app.vfs, &start, 0, max_entries, &mut entries);
                                panel.tree = Some(rmc_core::panel::TreeState {
                                    entries,
                                    cursor: 0,
                                    scroll_top: 0,
                                });
                                app.active = if set_left {
                                    rmc_core::actions::PaneSide::Left
                                } else {
                                    rmc_core::actions::PaneSide::Right
                                };
                                app.ui_mode = UiMode::Normal;
                            }
                            "Copy" => {
                                return Self::handle_key(
                                    app,
                                    KeyEvent::new(KeyCode::F(5), key.modifiers),
                                    page_rows,
                                );
                            }
                            "Move" => {
                                return Self::handle_key(
                                    app,
                                    KeyEvent::new(KeyCode::F(6), key.modifiers),
                                    page_rows,
                                );
                            }
                            "Mkdir" => {
                                return Self::handle_key(
                                    app,
                                    KeyEvent::new(KeyCode::F(7), key.modifiers),
                                    page_rows,
                                );
                            }
                            "Delete" => {
                                return Self::handle_key(
                                    app,
                                    KeyEvent::new(KeyCode::F(8), key.modifiers),
                                    page_rows,
                                );
                            }
                            "Chmod" => {
                                // Simulate C-x c chord
                                app.pending_ctrl_x = true;
                                return Self::handle_key(
                                    app,
                                    KeyEvent::new(KeyCode::Char('c'), key.modifiers),
                                    page_rows,
                                );
                            }
                            "Chown" => {
                                app.pending_ctrl_x = true;
                                return Self::handle_key(
                                    app,
                                    KeyEvent::new(KeyCode::Char('o'), key.modifiers),
                                    page_rows,
                                );
                            }
                            "Hard link" => {
                                app.pending_ctrl_x = true;
                                return Self::handle_key(
                                    app,
                                    KeyEvent::new(KeyCode::Char('l'), key.modifiers),
                                    page_rows,
                                );
                            }
                            "SymLink" => {
                                app.pending_ctrl_x = true;
                                return Self::handle_key(
                                    app,
                                    KeyEvent::new(KeyCode::Char('s'), key.modifiers),
                                    page_rows,
                                );
                            }
                            "Relative symlink" => {
                                app.pending_ctrl_x = true;
                                return Self::handle_key(
                                    app,
                                    KeyEvent::new(KeyCode::Char('v'), key.modifiers),
                                    page_rows,
                                );
                            }
                            "User menu" => {
                                return Self::handle_key(
                                    app,
                                    KeyEvent::new(KeyCode::F(2), key.modifiers),
                                    page_rows,
                                );
                            }
                            "Directory hotlist" => {
                                let st = rmc_core::hotlist::HotlistDialogState::new(
                                    app.hotlist.entries.clone(),
                                );
                                app.ui_mode = UiMode::HotlistDialog(st);
                            }
                            "Find file" => {
                                let start = app.active_panel().cwd.clone();
                                app.ui_mode = UiMode::FindDialog(FindDialogState::new(start));
                            }
                            "Quit" => {
                                app.handle_action(Action::Quit)?;
                            }
                            _ => {
                                app.ui_mode = UiMode::Normal;
                            }
                        }
                    }
                    _ => {}
                }
                return Ok(());
            }
            UiMode::Diff(state) => {
                // Confirm-exit overlay
                if let Some(confirm) = &mut state.confirm_exit {
                    match key.code {
                        KeyCode::Esc => {
                            state.confirm_exit = None;
                        }
                        KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
                            use rmc_core::app::YncFocus as F;
                            confirm.focus = match (key.code, confirm.focus) {
                                (KeyCode::Left, F::No) => F::Yes,
                                (KeyCode::Left, F::Cancel) => F::No,
                                (KeyCode::Right, F::Yes) => F::No,
                                (KeyCode::Right, F::No) => F::Cancel,
                                (KeyCode::Right, F::Cancel) => F::Cancel,
                                (_, f) => match f {
                                    F::Yes => F::No,
                                    F::No => F::Cancel,
                                    F::Cancel => F::Yes,
                                },
                            };
                        }
                        KeyCode::Enter => {
                            use rmc_core::app::YncFocus as F;
                            match confirm.focus {
                                F::Yes => {
                                    // Save modified then exit
                                    if state.left_modified {
                                        let mut w = app
                                            .vfs
                                            .write_file(&state.left_path)
                                            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
                                        use std::io::Write;
                                        let s = rmc_diff::join_lines(&state.left_lines);
                                        let _ = w.write_all(s.as_bytes());
                                    }
                                    if state.right_modified {
                                        let mut w = app
                                            .vfs
                                            .write_file(&state.right_path)
                                            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
                                        use std::io::Write;
                                        let s = rmc_diff::join_lines(&state.right_lines);
                                        let _ = w.write_all(s.as_bytes());
                                    }
                                    app.ui_mode = UiMode::Normal;
                                }
                                F::No => {
                                    // Discard changes and exit
                                    app.ui_mode = UiMode::Normal;
                                }
                                F::Cancel => {
                                    state.confirm_exit = None;
                                }
                            }
                        }
                        _ => {}
                    }
                    return Ok(());
                }
                // Handle inline search prompt
                if let Some(prompt) = &mut state.search_prompt {
                    match key.code {
                        KeyCode::Esc => {
                            state.search_prompt = None;
                        }
                        KeyCode::Enter => {
                            let q = prompt.clone();
                            state.search = if q.is_empty() { None } else { Some(q) };
                            state.search_prompt = None;
                            // Jump to first matching hunk from current
                            if let Some(ref qq) = state.search {
                                if let Some(idx) = Self::search_next_hunk_with(
                                    &state.hunks,
                                    &state.left_lines,
                                    &state.right_lines,
                                    state.current_hunk,
                                    qq,
                                ) {
                                    state.current_hunk = idx;
                                    Self::ensure_hunk_visible(state);
                                }
                            }
                        }
                        KeyCode::Backspace => {
                            prompt.pop();
                        }
                        KeyCode::Char(c) if key.modifiers.is_empty() => {
                            prompt.push(c);
                        }
                        _ => {}
                    }
                    return Ok(());
                }
                // Handle goto prompt
                if let Some(prompt) = &mut state.goto_prompt {
                    match key.code {
                        KeyCode::Esc => state.goto_prompt = None,
                        KeyCode::Enter => {
                            let val = prompt.clone();
                            if let Ok(n) = val.trim().parse::<usize>() {
                                let line = n.saturating_sub(1);
                                state.left_scroll = line;
                                state.right_scroll = line;
                            }
                            state.goto_prompt = None;
                        }
                        KeyCode::Backspace => {
                            prompt.pop();
                        }
                        KeyCode::Char(c) if key.modifiers.is_empty() => prompt.push(c),
                        _ => {}
                    }
                    return Ok(());
                }
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc | KeyCode::F(10) => {
                        if state.left_modified || state.right_modified {
                            state.confirm_exit = Some(rmc_core::app::YncDialog {
                                title: "Save modified files?".into(),
                                message: "You have unsaved merges. Save before quitting?".into(),
                                focus: rmc_core::app::YncFocus::Yes,
                            });
                        } else {
                            app.ui_mode = UiMode::Normal;
                        }
                    }
                    KeyCode::Enter | KeyCode::Char(' ') => {
                        if !state.hunks.is_empty() {
                            if let Some(next) =
                                Self::next_diff_hunk_index(&state.hunks, state.current_hunk)
                            {
                                state.current_hunk = next;
                            }
                            Self::ensure_hunk_visible(state);
                        }
                    }
                    KeyCode::Backspace | KeyCode::Char('p') => {
                        if !state.hunks.is_empty() {
                            if let Some(prev) =
                                Self::prev_diff_hunk_index(&state.hunks, state.current_hunk)
                            {
                                state.current_hunk = prev;
                            }
                            Self::ensure_hunk_visible(state);
                        }
                    }
                    KeyCode::Char('g') => {
                        state.goto_prompt = Some(String::new());
                    }
                    KeyCode::Char('f') => state.panel_ratio = 0.8,
                    KeyCode::Char('=') => state.panel_ratio = 0.5,
                    KeyCode::Char('>') => state.panel_ratio = (state.panel_ratio + 0.05).min(0.8),
                    KeyCode::Char('<') => state.panel_ratio = (state.panel_ratio - 0.05).max(0.2),
                    KeyCode::Char('l') => state.show_line_numbers = !state.show_line_numbers,
                    KeyCode::Char('s') => state.show_hunk_status = !state.show_hunk_status,
                    KeyCode::Char('2') => state.tab_width = 2,
                    KeyCode::Char('3') => state.tab_width = 3,
                    KeyCode::Char('4') => state.tab_width = 4,
                    KeyCode::Char('8') => state.tab_width = 8,
                    KeyCode::Char('/') | KeyCode::F(7) => state.search_prompt = Some(String::new()),
                    KeyCode::Char('n') | KeyCode::F(17) => {
                        if let Some(ref qq) = state.search {
                            if let Some(idx) = Self::search_next_hunk_with(
                                &state.hunks,
                                &state.left_lines,
                                &state.right_lines,
                                state.current_hunk,
                                qq,
                            ) {
                                state.current_hunk = idx;
                                Self::ensure_hunk_visible(state);
                            }
                        } else if !state.hunks.is_empty() {
                            if let Some(next) =
                                Self::next_diff_hunk_index(&state.hunks, state.current_hunk)
                            {
                                state.current_hunk = next;
                                Self::ensure_hunk_visible(state);
                            }
                        }
                    }
                    KeyCode::F(2) => {
                        // Save modified files
                        if state.left_modified {
                            let mut w = app
                                .vfs
                                .write_file(&state.left_path)
                                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
                            use std::io::Write;
                            let s = rmc_diff::join_lines(&state.left_lines);
                            let _ = w.write_all(s.as_bytes());
                            state.left_modified = false;
                        }
                        if state.right_modified {
                            let mut w = app
                                .vfs
                                .write_file(&state.right_path)
                                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
                            use std::io::Write;
                            let s = rmc_diff::join_lines(&state.right_lines);
                            let _ = w.write_all(s.as_bytes());
                            state.right_modified = false;
                        }
                        // Recompute diff
                        let la = rmc_diff::join_lines(&state.left_lines);
                        let ra = rmc_diff::join_lines(&state.right_lines);
                        state.hunks = rmc_diff::compute_diff(&la, &ra).hunks;
                        state.current_hunk =
                            state.current_hunk.min(state.hunks.len().saturating_sub(1));
                        Self::ensure_hunk_visible(state);
                    }
                    KeyCode::F(4) => {
                        // Disabled for now to avoid losing diff session; leave PARITY unchecked
                    }
                    KeyCode::F(5) => {
                        if !state.hunks.is_empty() {
                            let idx = state.current_hunk.min(state.hunks.len() - 1);
                            if state.merge_target_right {
                                let _ = rmc_diff::apply_hunk_merge(
                                    &mut state.left_lines,
                                    &mut state.right_lines,
                                    &state.hunks,
                                    idx,
                                    rmc_diff::MergeTarget::Right,
                                );
                                state.right_modified = true;
                            } else {
                                let _ = rmc_diff::apply_hunk_merge(
                                    &mut state.left_lines,
                                    &mut state.right_lines,
                                    &state.hunks,
                                    idx,
                                    rmc_diff::MergeTarget::Left,
                                );
                                state.left_modified = true;
                            }
                            // Re-diff
                            let la = rmc_diff::join_lines(&state.left_lines);
                            let ra = rmc_diff::join_lines(&state.right_lines);
                            state.hunks = rmc_diff::compute_diff(&la, &ra).hunks;
                            state.current_hunk =
                                state.current_hunk.min(state.hunks.len().saturating_sub(1));
                            Self::ensure_hunk_visible(state);
                        }
                    }
                    KeyCode::Up => {
                        state.left_scroll = state.left_scroll.saturating_sub(1);
                        state.right_scroll = state.right_scroll.saturating_sub(1);
                    }
                    KeyCode::Down => {
                        state.left_scroll = state.left_scroll.saturating_add(1);
                        state.right_scroll = state.right_scroll.saturating_add(1);
                    }
                    KeyCode::PageUp => {
                        let step = page_rows.max(1);
                        state.left_scroll = state.left_scroll.saturating_sub(step);
                        state.right_scroll = state.right_scroll.saturating_sub(step);
                    }
                    KeyCode::PageDown => {
                        let step = page_rows.max(1);
                        state.left_scroll = state.left_scroll.saturating_add(step);
                        state.right_scroll = state.right_scroll.saturating_add(step);
                    }
                    _ => {
                        // Ctrl-U: swap sides; Ctrl-R: refresh
                        if let KeyCode::Char('u') = key.code {
                            if key
                                .modifiers
                                .contains(crossterm::event::KeyModifiers::CONTROL)
                            {
                                std::mem::swap(&mut state.left_path, &mut state.right_path);
                                std::mem::swap(&mut state.left_lines, &mut state.right_lines);
                                state.merge_target_right = !state.merge_target_right;
                                let la = rmc_diff::join_lines(&state.left_lines);
                                let ra = rmc_diff::join_lines(&state.right_lines);
                                state.hunks = rmc_diff::compute_diff(&la, &ra).hunks;
                                state.current_hunk = 0;
                                state.left_scroll = 0;
                                state.right_scroll = 0;
                            }
                        } else if let KeyCode::Char('r') = key.code {
                            if key
                                .modifiers
                                .contains(crossterm::event::KeyModifiers::CONTROL)
                            {
                                let mut ltxt = String::new();
                                let mut rtxt = String::new();
                                if let Ok(mut r) = app.vfs.read_file(&state.left_path) {
                                    use std::io::Read;
                                    let _ = r.read_to_string(&mut ltxt);
                                }
                                if let Ok(mut r) = app.vfs.read_file(&state.right_path) {
                                    use std::io::Read;
                                    let _ = r.read_to_string(&mut rtxt);
                                }
                                state.left_lines = rmc_diff::split_lines(&ltxt);
                                state.right_lines = rmc_diff::split_lines(&rtxt);
                                state.hunks = rmc_diff::compute_diff(&ltxt, &rtxt).hunks;
                                state.current_hunk = 0;
                                state.left_modified = false;
                                state.right_modified = false;
                                state.left_scroll = 0;
                                state.right_scroll = 0;
                            }
                        }
                    }
                }
                return Ok(());
            }
            UiMode::Viewer { .. } => {
                // If viewer has an active goto or search prompt, handle overlay first
                if let UiMode::Viewer {
                    path,
                    offset,
                    hex,
                    goto_prompt: Some(prompt),
                    ..
                } = &mut app.ui_mode
                {
                    match key.code {
                        KeyCode::Esc | KeyCode::F(10) => {
                            *prompt = String::new();
                            if let UiMode::Viewer { goto_prompt, .. } = &mut app.ui_mode {
                                *goto_prompt = None;
                            }
                        }
                        KeyCode::Enter => {
                            let q = prompt.trim().to_string();
                            // Parse goto input:
                            // - Prefix "@": decimal byte offset
                            // - Prefix "0x": hex byte offset
                            // - Prefix ":" or "l " => line number (1-based)
                            // - Otherwise: if hex mode => offset; else => line number
                            let lower = q.to_ascii_lowercase();
                            let cpath = crate::terminal::viewer_ensure_view_for(path);
                            let res = if let Some(rest) = lower.strip_prefix('@') {
                                rest.parse::<u64>()
                                    .ok()
                                    .and_then(|v| rmc_view::clamp_offset(&cpath, v).ok())
                            } else if let Some(rest) = lower.strip_prefix("0x") {
                                u64::from_str_radix(rest, 16)
                                    .ok()
                                    .and_then(|v| rmc_view::clamp_offset(&cpath, v).ok())
                            } else if lower.starts_with(':') || lower.starts_with("l ") {
                                let num_str = if let Some(rest) = lower.strip_prefix(':') {
                                    rest
                                } else {
                                    // must start with "l " here
                                    &lower[2..]
                                };
                                num_str
                                    .trim()
                                    .parse::<u64>()
                                    .ok()
                                    .and_then(|ln| rmc_view::goto_line(&cpath, ln).ok())
                            } else if *hex {
                                // hex mode default: treat as offset (hex if contains 0x else decimal)
                                if let Some(rest) = lower.strip_prefix("0x") {
                                    u64::from_str_radix(rest, 16)
                                        .ok()
                                        .and_then(|v| rmc_view::clamp_offset(&cpath, v).ok())
                                } else {
                                    lower
                                        .parse::<u64>()
                                        .ok()
                                        .and_then(|v| rmc_view::clamp_offset(&cpath, v).ok())
                                }
                            } else {
                                // text mode default: treat as line number
                                lower
                                    .parse::<u64>()
                                    .ok()
                                    .and_then(|ln| rmc_view::goto_line(&cpath, ln).ok())
                            };
                            if let Some(new_off) = res {
                                *offset = new_off;
                            }
                            if let UiMode::Viewer { goto_prompt, .. } = &mut app.ui_mode {
                                *goto_prompt = None;
                            }
                        }
                        KeyCode::Backspace => {
                            prompt.pop();
                        }
                        KeyCode::Char(c) if key.modifiers.is_empty() => {
                            prompt.push(c);
                        }
                        _ => {}
                    }
                    return Ok(());
                }
                // Then handle search overlay
                if let UiMode::Viewer {
                    path,
                    offset,
                    search,
                    search_prompt: Some(prompt),
                    ..
                } = &mut app.ui_mode
                {
                    match key.code {
                        KeyCode::Esc | KeyCode::F(10) => {
                            *prompt = String::new();
                            if let UiMode::Viewer { search_prompt, .. } = &mut app.ui_mode {
                                *search_prompt = None;
                            }
                        }
                        KeyCode::Enter => {
                            let q = prompt.clone();
                            let cpath = crate::terminal::viewer_ensure_view_for(path);
                            if let Some(pos) = rmc_view::search_forward(&cpath, *offset, &q)? {
                                *offset = pos;
                            }
                            *search = if q.is_empty() { None } else { Some(q) };
                            if let UiMode::Viewer { search_prompt, .. } = &mut app.ui_mode {
                                *search_prompt = None;
                            }
                        }
                        KeyCode::Backspace => {
                            prompt.pop();
                        }
                        KeyCode::Char(c) if key.modifiers.is_empty() => {
                            prompt.push(c);
                        }
                        _ => {}
                    }
                    return Ok(());
                }
                match key.code {
                    KeyCode::Char('q') | KeyCode::F(3) | KeyCode::F(10) => {
                        app.handle_action(Action::ViewerQuit)?
                    }
                    KeyCode::Char('h') | KeyCode::Char('x') | KeyCode::F(4) => {
                        app.handle_action(Action::ViewerToggleHex)?
                    }
                    KeyCode::F(5) | KeyCode::Char('g') => {
                        if let UiMode::Viewer { goto_prompt, .. } = &mut app.ui_mode {
                            *goto_prompt = Some(String::new());
                        }
                    }
                    // MC viewer: open "Filter command" dialog
                    KeyCode::Char('|') => {
                        // Use existing generic InputDialog; on submit apply filter to current viewed bytes.
                        app.ui_mode = UiMode::InputDialog {
                            title: "Filter command".into(),
                            prompt: "Apply command to viewed bytes:".into(),
                            value: String::new(),
                            focus_ok: false,
                            on_submit: Box::new(|app, input| {
                                let cmd = input.trim();
                                if cmd.is_empty() {
                                    return Ok(());
                                }
                                if let Err(e) = crate::terminal::viewer_apply_filter_to_current(cmd)
                                {
                                    app.ui_mode = UiMode::DialogConfirm {
                                        title: "Error".into(),
                                        message: format!("{e}"),
                                        on_ok: Box::new(|_| Ok(())),
                                    };
                                } else if let UiMode::Viewer { offset, search, .. } =
                                    &mut app.ui_mode
                                {
                                    *offset = 0;
                                    *search = None;
                                }
                                Ok(())
                            }),
                        };
                    }
                    KeyCode::F(2) => {
                        // Save — no-op stub for now
                    }
                    KeyCode::Char('w') => {
                        if let UiMode::Viewer { hex, wrap, .. } = &mut app.ui_mode {
                            if !*hex {
                                *wrap = !*wrap;
                            }
                        }
                    }
                    KeyCode::Char('l') => {
                        if let UiMode::Viewer {
                            show_line_numbers, ..
                        } = &mut app.ui_mode
                        {
                            *show_line_numbers = !*show_line_numbers;
                        }
                    }
                    KeyCode::Char('r') => {
                        if let UiMode::Viewer { show_cr, .. } = &mut app.ui_mode {
                            *show_cr = !*show_cr;
                        }
                    }
                    KeyCode::Up => {
                        if let UiMode::Viewer {
                            path, hex, offset, ..
                        } = &mut app.ui_mode
                        {
                            if *hex {
                                *offset = offset.saturating_sub(16);
                            } else {
                                let cpath = crate::terminal::viewer_ensure_view_for(path);
                                *offset = rmc_view::nav_line_up(&cpath, *offset)?;
                            }
                        }
                    }
                    KeyCode::Down => {
                        if let UiMode::Viewer {
                            path, hex, offset, ..
                        } = &mut app.ui_mode
                        {
                            if *hex {
                                *offset = offset.saturating_add(16);
                            } else {
                                let cpath = crate::terminal::viewer_ensure_view_for(path);
                                *offset = rmc_view::nav_line_down(&cpath, *offset)?;
                            }
                        }
                    }
                    KeyCode::PageDown => {
                        if let UiMode::Viewer {
                            path,
                            hex,
                            wrap,
                            offset,
                            ..
                        } = &mut app.ui_mode
                        {
                            let (cols, rows) = crossterm::terminal::size()?;
                            // content area inside frame is rows-3, cols-2
                            let content_rows = rows.saturating_sub(3);
                            if *hex {
                                let step = 16u64 * (content_rows as u64);
                                *offset = offset.saturating_add(step);
                            } else {
                                let cpath = crate::terminal::viewer_ensure_view_for(path);
                                *offset = rmc_view::nav_page_down(
                                    &cpath,
                                    *offset,
                                    cols.saturating_sub(2),
                                    content_rows,
                                    *wrap,
                                )?;
                            }
                        }
                    }
                    KeyCode::PageUp => {
                        if let UiMode::Viewer {
                            path,
                            hex,
                            wrap,
                            offset,
                            ..
                        } = &mut app.ui_mode
                        {
                            let (cols, rows) = crossterm::terminal::size()?;
                            let content_rows = rows.saturating_sub(3);
                            if *hex {
                                let step = 16u64 * (content_rows as u64);
                                *offset = offset.saturating_sub(step);
                            } else {
                                let cpath = crate::terminal::viewer_ensure_view_for(path);
                                *offset = rmc_view::nav_page_up(
                                    &cpath,
                                    *offset,
                                    cols.saturating_sub(2),
                                    content_rows,
                                    *wrap,
                                )?;
                            }
                        }
                    }
                    KeyCode::Home => {
                        if let UiMode::Viewer { offset, .. } = &mut app.ui_mode {
                            *offset = rmc_view::nav_home();
                        }
                    }
                    KeyCode::End => {
                        if let UiMode::Viewer {
                            path,
                            hex,
                            wrap,
                            offset,
                            ..
                        } = &mut app.ui_mode
                        {
                            let (cols, rows) = crossterm::terminal::size()?;
                            let content_rows = rows.saturating_sub(3);
                            if *hex {
                                let cpath = crate::terminal::viewer_ensure_view_for(path);
                                let len = rmc_view::file_len(&cpath)?;
                                let page = 16u64 * (content_rows as u64);
                                *offset = len.saturating_sub(page);
                            } else {
                                let cpath = crate::terminal::viewer_ensure_view_for(path);
                                *offset = rmc_view::nav_end(
                                    &cpath,
                                    cols.saturating_sub(2),
                                    content_rows,
                                    *wrap,
                                )?;
                            }
                        }
                    }
                    KeyCode::Char('/') | KeyCode::F(7) => {
                        if let UiMode::Viewer { search_prompt, .. } = &mut app.ui_mode {
                            *search_prompt = Some(String::new());
                        }
                    }
                    KeyCode::Char('n') => {
                        if let UiMode::Viewer {
                            path,
                            offset,
                            search,
                            ..
                        } = &mut app.ui_mode
                        {
                            if let Some(q) = search.clone() {
                                let cpath = crate::terminal::viewer_ensure_view_for(path);
                                if let Some(pos) =
                                    rmc_view::search_forward(&cpath, offset.saturating_add(1), &q)?
                                {
                                    *offset = pos;
                                }
                            }
                        }
                    }
                    KeyCode::Char('N') => {
                        if let UiMode::Viewer {
                            path,
                            offset,
                            search,
                            ..
                        } = &mut app.ui_mode
                        {
                            if let Some(q) = search.clone() {
                                let cpath = crate::terminal::viewer_ensure_view_for(path);
                                if let Some(pos) = rmc_view::search_backward(&cpath, *offset, &q)? {
                                    *offset = pos;
                                }
                            }
                        }
                    }
                    _ => {}
                }
                return Ok(());
            }
            _ => {}
        }

        // (C-x handling centralized below with app.pending_ctrl_x)

        // Global Alt-Enter: append filename to command line and enter ShellInput if necessary
        if matches!(key.code, KeyCode::Enter)
            && key.modifiers.contains(crossterm::event::KeyModifiers::ALT)
        {
            if let Some(ent) = app.active_panel().current_entry() {
                let name = ent.name.clone();
                app.subshell.append_filename(&name);
                app.ui_mode = UiMode::ShellInput;
            }
            return Ok(());
        }
        // If UI is Normal and Esc is pressed, exit QuickView/Info/Tree modes on panels back to Listing.
        if matches!(app.ui_mode, UiMode::Normal) && matches!(key.code, KeyCode::Esc) {
            let mut changed = false;
            for side in [
                rmc_core::actions::PaneSide::Left,
                rmc_core::actions::PaneSide::Right,
            ] {
                let p = if matches!(side, rmc_core::actions::PaneSide::Left) {
                    &mut app.left
                } else {
                    &mut app.right
                };
                if !matches!(p.mode, rmc_core::panel::PanelMode::Listing) {
                    p.mode = rmc_core::panel::PanelMode::Listing;
                    changed = true;
                }
            }
            if changed {
                return Ok(());
            }
        }
        // Key chord handling for C-x prefix (emulate MC prefixes)
        if app.pending_ctrl_x {
            app.pending_ctrl_x = false;
            if key.modifiers.is_empty() {
                if let KeyCode::Char('h') = key.code {
                    // Add current dir to hotlist with label prompt
                    let cwd = active_cwd.clone();
                    app.ui_mode = UiMode::PromptInput {
                        title: "Add to hotlist: Label".into(),
                        value: cwd
                            .file_name()
                            .and_then(|s| s.to_str())
                            .unwrap_or("")
                            .to_string(),
                        on_submit: Box::new(move |app, label| {
                            if !label.trim().is_empty() {
                                app.hotlist.add_or_replace(label.trim().to_string(), cwd)?;
                                app.hotlist.save_to_default_path()?;
                            }
                            Ok(())
                        }),
                    };
                    return Ok(());
                } else if let KeyCode::Char(c) = key.code {
                    match c {
                        'q' => {
                            // Toggle Quick view on the inactive panel
                            let p = app.inactive_panel_mut();
                            p.mode = if matches!(p.mode, rmc_core::panel::PanelMode::QuickView) {
                                rmc_core::panel::PanelMode::Listing
                            } else {
                                rmc_core::panel::PanelMode::QuickView
                            };
                            return Ok(());
                        }
                        'i' => {
                            // Toggle Info on the inactive panel
                            let p = app.inactive_panel_mut();
                            p.mode = if matches!(p.mode, rmc_core::panel::PanelMode::Info) {
                                rmc_core::panel::PanelMode::Listing
                            } else {
                                rmc_core::panel::PanelMode::Info
                            };
                            return Ok(());
                        }
                        'c' => {
                            if let Some(ent) = app.active_panel().current_entry().cloned() {
                                let m = ent.permissions & 0o7777;
                                app.ui_mode = UiMode::ChmodDialog {
                                    name: ent.name,
                                    mode: m,
                                    ur: (m & 0o400) != 0,
                                    uw: (m & 0o200) != 0,
                                    ux: (m & 0o100) != 0,
                                    gr: (m & 0o040) != 0,
                                    gw: (m & 0o020) != 0,
                                    gx: (m & 0o010) != 0,
                                    or_: (m & 0o004) != 0,
                                    ow: (m & 0o002) != 0,
                                    ox: (m & 0o001) != 0,
                                    suid: (m & 0o4000) != 0,
                                    sgid: (m & 0o2000) != 0,
                                    sticky: (m & 0o1000) != 0,
                                    recursive: false,
                                    focus_index: 0,
                                };
                            }
                            return Ok(());
                        }
                        'o' => {
                            if let Some(ent) = app.active_panel().current_entry().cloned() {
                                app.ui_mode = UiMode::ChownDialog {
                                    owner: ent.owner.unwrap_or_default(),
                                    group: ent.group.unwrap_or_default(),
                                    recursive: false,
                                    focus_index: 0,
                                };
                            } else {
                                app.ui_mode = UiMode::ChownDialog {
                                    owner: String::new(),
                                    group: String::new(),
                                    recursive: false,
                                    focus_index: 0,
                                };
                            }
                            return Ok(());
                        }
                        'l' | 's' | 'v' => {
                            if let Some(ent) = app.active_panel().current_entry().cloned() {
                                let dst_dir = app.inactive_panel_mut().cwd.clone();
                                let default_to = dst_dir.join(&ent.name).display().to_string();
                                let is_hard = c == 'l';
                                let is_symlink_abs = c == 's';
                                let (dlg_title, prompt) = if is_hard {
                                    (
                                        "Link".to_string(),
                                        "Enter the name of the hard link to:".to_string(),
                                    )
                                } else if is_symlink_abs {
                                    (
                                        "Symbolic link".to_string(),
                                        "Enter name of the symlink:".to_string(),
                                    )
                                } else {
                                    (
                                        "Relative symlink".to_string(),
                                        "Enter name of the symlink:".to_string(),
                                    )
                                };
                                app.ui_mode = UiMode::InputDialog {
                                    title: dlg_title,
                                    prompt,
                                    value: default_to.clone(),
                                    focus_ok: true,
                                    on_submit: Box::new(move |app, val| {
                                        let src = ent.path.clone();
                                        let dst = std::path::PathBuf::from(val);
                                        if is_hard {
                                            app.vfs.link_hard(&src, &dst)?;
                                        } else if is_symlink_abs {
                                            let abs_target = if src.is_absolute() {
                                                src.clone()
                                            } else {
                                                active_cwd.join(&src)
                                            };
                                            app.vfs.symlink(&abs_target, &dst)?;
                                        } else {
                                            let base = dst
                                                .parent()
                                                .unwrap_or_else(|| std::path::Path::new("."));
                                            let abs_src = if src.is_absolute() {
                                                src.clone()
                                            } else {
                                                active_cwd.join(&src)
                                            };
                                            let abs_base = if base.is_absolute() {
                                                base.to_path_buf()
                                            } else {
                                                active_cwd.join(base)
                                            };
                                            fn relpath(
                                                from: &std::path::Path,
                                                to: &std::path::Path,
                                            ) -> std::path::PathBuf
                                            {
                                                let from = from.components().collect::<Vec<_>>();
                                                let to = to.components().collect::<Vec<_>>();
                                                let mut i = 0usize;
                                                while i < from.len()
                                                    && i < to.len()
                                                    && from[i] == to[i]
                                                {
                                                    i += 1;
                                                }
                                                let mut out = std::path::PathBuf::new();
                                                for _ in i..from.len() {
                                                    out.push("..");
                                                }
                                                for comp in &to[i..] {
                                                    out.push(comp.as_os_str());
                                                }
                                                out
                                            }
                                            let rel = relpath(&abs_base, &abs_src);
                                            app.vfs.symlink(&rel, &dst)?;
                                        }
                                        Ok(())
                                    }),
                                };
                            }
                            return Ok(());
                        }
                        _ => {}
                    }
                }
            }
            // Unrecognized chord after C-x: fall through to normal handling
        } else if key
            .modifiers
            .contains(crossterm::event::KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('x'))
        {
            app.pending_ctrl_x = true;
            return Ok(());
        }
        // Panel quick search handling (only in UiMode::Normal), placed after C-x handling.
        if matches!(app.ui_mode, UiMode::Normal) {
            // Next-match helper with wrap-around
            let find_next =
                |app_ref: &App, pattern: &str, start_after: Option<usize>| -> Option<usize> {
                    if pattern.is_empty() {
                        return None;
                    }
                    let entries = &app_ref.active_panel().entries;
                    if entries.is_empty() {
                        return None;
                    }
                    let mut start = start_after
                        .map(|i| i.saturating_add(1))
                        .unwrap_or_else(|| app_ref.active_panel().cursor);
                    if start >= entries.len() {
                        start = 0;
                    }
                    let total = entries.len();
                    for pass in 0..2 {
                        let (begin, end) = if pass == 0 {
                            (start, total)
                        } else {
                            (0, start.min(total))
                        };
                        for (i, e) in entries
                            .iter()
                            .enumerate()
                            .skip(begin)
                            .take(end.saturating_sub(begin))
                        {
                            if e.name == ".." {
                                continue;
                            }
                            if rmc_core::matchutil::name_matches(pattern, &e.name) {
                                return Some(i);
                            }
                        }
                    }
                    None
                };
            if app.quick_search.is_some() {
                let mut qs = app.quick_search.take().unwrap();
                match key.code {
                    KeyCode::Esc | KeyCode::Enter => {
                        app.quick_search = None;
                        return Ok(());
                    }
                    KeyCode::Backspace => {
                        qs.pattern.pop();
                        if let Some(idx) = find_next(app, &qs.pattern, Some(usize::MAX)) {
                            app.active_panel_mut().cursor = idx;
                            app.active_panel_mut().ensure_visible(page_rows);
                        }
                        app.quick_search = Some(qs);
                        return Ok(());
                    }
                    KeyCode::Char(c) if key.modifiers.is_empty() => {
                        if !c.is_control() {
                            qs.pattern.push(c);
                            if let Some(idx) = find_next(app, &qs.pattern, Some(usize::MAX)) {
                                app.active_panel_mut().cursor = idx;
                                app.active_panel_mut().ensure_visible(page_rows);
                            }
                        }
                        app.quick_search = Some(qs);
                        return Ok(());
                    }
                    _ => {
                        // C-s / Alt-s repeats next match
                        if (matches!(key.code, KeyCode::Char('s'))
                            && (key
                                .modifiers
                                .contains(crossterm::event::KeyModifiers::CONTROL)
                                || key.modifiers.contains(crossterm::event::KeyModifiers::ALT)))
                        {
                            let cur = app.active_panel().cursor;
                            if let Some(idx) = find_next(app, &qs.pattern, Some(cur)) {
                                app.active_panel_mut().cursor = idx;
                                app.active_panel_mut().ensure_visible(page_rows);
                            }
                            app.quick_search = Some(qs);
                            return Ok(());
                        }
                        // Not a search key: exit search and fall through (do not swallow C-x etc.)
                        app.quick_search = None;
                    }
                }
            } else if matches!(key.code, KeyCode::Char('s'))
                && (key
                    .modifiers
                    .contains(crossterm::event::KeyModifiers::CONTROL)
                    || key.modifiers.contains(crossterm::event::KeyModifiers::ALT))
            {
                app.quick_search = Some(rmc_core::quicksearch::QuickSearchState::new());
                return Ok(());
            }
        }
        if let Some(action) = app.keymap.resolve(&key) {
            // Intercept navigation and Enter for Tree mode on the ACTIVE panel.
            if matches!(app.ui_mode, UiMode::Normal) {
                let is_tree_active = {
                    let ap = if matches!(app.active, rmc_core::actions::PaneSide::Left) {
                        &app.left
                    } else {
                        &app.right
                    };
                    matches!(ap.mode, rmc_core::panel::PanelMode::Tree)
                };
                if is_tree_active {
                    match action {
                        Action::MoveUp => {
                            let p = if matches!(app.active, rmc_core::actions::PaneSide::Left) {
                                &mut app.left
                            } else {
                                &mut app.right
                            };
                            if let Some(tree) = &mut p.tree {
                                if tree.cursor > 0 {
                                    tree.cursor -= 1;
                                    if tree.cursor < tree.scroll_top {
                                        tree.scroll_top = tree.cursor;
                                    }
                                }
                            }
                            return Ok(());
                        }
                        Action::MoveDown => {
                            let p = if matches!(app.active, rmc_core::actions::PaneSide::Left) {
                                &mut app.left
                            } else {
                                &mut app.right
                            };
                            if let Some(tree) = &mut p.tree {
                                if tree.cursor + 1 < tree.entries.len() {
                                    tree.cursor += 1;
                                    let (_c, r) = crossterm::terminal::size()?;
                                    let panel_top = 1u16;
                                    let gauge_row = r.saturating_sub(4);
                                    let content_bottom = gauge_row.saturating_sub(1);
                                    let panel_h = content_bottom - panel_top;
                                    let content_rows = panel_h.saturating_sub(4) as usize;
                                    if tree.cursor >= tree.scroll_top + content_rows {
                                        tree.scroll_top = tree
                                            .cursor
                                            .saturating_sub(content_rows.saturating_sub(1));
                                    }
                                }
                            }
                            return Ok(());
                        }
                        Action::Home => {
                            let p = if matches!(app.active, rmc_core::actions::PaneSide::Left) {
                                &mut app.left
                            } else {
                                &mut app.right
                            };
                            if let Some(tree) = &mut p.tree {
                                tree.cursor = 0;
                                tree.scroll_top = 0;
                            }
                            return Ok(());
                        }
                        Action::End => {
                            let p = if matches!(app.active, rmc_core::actions::PaneSide::Left) {
                                &mut app.left
                            } else {
                                &mut app.right
                            };
                            if let Some(tree) = &mut p.tree {
                                if !tree.entries.is_empty() {
                                    tree.cursor = tree.entries.len() - 1;
                                    let (_c, r) = crossterm::terminal::size()?;
                                    let panel_top = 1u16;
                                    let gauge_row = r.saturating_sub(4);
                                    let content_bottom = gauge_row.saturating_sub(1);
                                    let panel_h = content_bottom - panel_top;
                                    let content_rows = panel_h.saturating_sub(4) as usize;
                                    tree.scroll_top =
                                        tree.cursor.saturating_sub(content_rows.saturating_sub(1));
                                }
                            }
                            return Ok(());
                        }
                        Action::PageUp => {
                            let p = if matches!(app.active, rmc_core::actions::PaneSide::Left) {
                                &mut app.left
                            } else {
                                &mut app.right
                            };
                            if let Some(tree) = &mut p.tree {
                                let prev = tree.cursor;
                                tree.cursor = tree.cursor.saturating_sub(page_rows);
                                if tree.cursor < tree.scroll_top {
                                    tree.scroll_top = tree.cursor;
                                }
                                if prev != tree.cursor {
                                    // adjusted above
                                }
                                return Ok(());
                            }
                        }
                        Action::PageDown => {
                            let p = if matches!(app.active, rmc_core::actions::PaneSide::Left) {
                                &mut app.left
                            } else {
                                &mut app.right
                            };
                            if let Some(tree) = &mut p.tree {
                                let max = tree.entries.len().saturating_sub(1);
                                let prev = tree.cursor;
                                tree.cursor = (tree.cursor + page_rows).min(max);
                                if tree.cursor >= tree.scroll_top + page_rows {
                                    tree.scroll_top =
                                        tree.cursor.saturating_sub(page_rows.saturating_sub(1));
                                }
                                if prev != tree.cursor {
                                    // adjusted
                                }
                                return Ok(());
                            }
                        }
                        Action::Enter => {
                            // Enter in Tree changes directory of the other panel (keep focus)
                            let (tree_side_left, dest_opt) = {
                                let is_left =
                                    matches!(app.active, rmc_core::actions::PaneSide::Left);
                                let p = if is_left { &app.left } else { &app.right };
                                let path = p
                                    .tree
                                    .as_ref()
                                    .and_then(|t| t.entries.get(t.cursor))
                                    .map(|e| e.path.clone());
                                (is_left, path)
                            };
                            if let Some(dest) = dest_opt {
                                let original_active = app.active;
                                app.active = if tree_side_left {
                                    rmc_core::actions::PaneSide::Right
                                } else {
                                    rmc_core::actions::PaneSide::Left
                                };
                                let _ = app.change_dir(&dest);
                                app.active = original_active;
                            }
                            return Ok(());
                        }
                        _ => {}
                    }
                }
            }
            match action {
                Action::FunctionKey(4) => {
                    // Open editor on selected file (panels Normal mode)
                    if matches!(app.ui_mode, UiMode::Normal) {
                        if let Some(ent) = app.active_panel().current_entry().cloned() {
                            if !ent.is_dir {
                                // Read file bytes via VFS
                                let mut data = Vec::new();
                                if let Ok(mut r) = app.vfs.read_file(&ent.path) {
                                    use std::io::Read;
                                    let _ = r.read_to_end(&mut data);
                                }
                                let buf = rmc_edit::EditorBuffer::from_bytes(
                                    &data,
                                    Some(ent.path.clone()),
                                );
                                app.ui_mode = UiMode::Editor {
                                    buf,
                                    show_menu: false,
                                    status_msg: None,
                                    search_input: None,
                                    save_as_input: None,
                                    pending_quit: false,
                                    confirm_exit: None,
                                };
                            }
                        }
                    }
                }
                Action::PageUp => app.page_up_by(page_rows),
                Action::PageDown => app.page_down_by(page_rows),
                Action::ToggleSubshell => {
                    app.handle_action(Action::ToggleSubshell)?;
                }
                Action::OpenHotlist => {
                    let st =
                        rmc_core::hotlist::HotlistDialogState::new(app.hotlist.entries.clone());
                    app.ui_mode = UiMode::HotlistDialog(st);
                }
                Action::Mkdir => {
                    app.ui_mode = UiMode::MkdirDialog {
                        value: String::new(),
                        focus_ok: false,
                    };
                }
                Action::Delete => {
                    if let Some(ent) = app.active_panel().current_entry().cloned() {
                        let path = ent.path.clone();
                        app.ui_mode = UiMode::DeleteDialog {
                            name: ent.name,
                            path,
                            focus_ok: true,
                        };
                    }
                }
                Action::Chmod => {
                    if let Some(ent) = app.active_panel().current_entry().cloned() {
                        let m = ent.permissions & 0o7777;
                        app.ui_mode = UiMode::ChmodDialog {
                            name: ent.name,
                            mode: m,
                            ur: (m & 0o400) != 0,
                            uw: (m & 0o200) != 0,
                            ux: (m & 0o100) != 0,
                            gr: (m & 0o040) != 0,
                            gw: (m & 0o020) != 0,
                            gx: (m & 0o010) != 0,
                            or_: (m & 0o004) != 0,
                            ow: (m & 0o002) != 0,
                            ox: (m & 0o001) != 0,
                            suid: (m & 0o4000) != 0,
                            sgid: (m & 0o2000) != 0,
                            sticky: (m & 0o1000) != 0,
                            recursive: false,
                            focus_index: 0,
                        };
                    }
                }
                Action::Chown => {
                    if let Some(ent) = app.active_panel().current_entry().cloned() {
                        app.ui_mode = UiMode::ChownDialog {
                            owner: ent.owner.unwrap_or_default(),
                            group: ent.group.unwrap_or_default(),
                            recursive: false,
                            focus_index: 0,
                        };
                    } else {
                        app.ui_mode = UiMode::ChownDialog {
                            owner: String::new(),
                            group: String::new(),
                            recursive: false,
                            focus_index: 0,
                        };
                    }
                }
                Action::LinkHard | Action::SymlinkAbs | Action::SymlinkRel => {
                    if let Some(ent) = app.active_panel().current_entry().cloned() {
                        let dst_dir = app.inactive_panel_mut().cwd.clone();
                        let default_to = dst_dir.join(&ent.name).display().to_string();
                        let is_hard = matches!(action, Action::LinkHard);
                        let is_symlink_abs = matches!(action, Action::SymlinkAbs);
                        let (dlg_title, prompt) = if is_hard {
                            (
                                "Link".to_string(),
                                "Enter the name of the hard link to:".to_string(),
                            )
                        } else if is_symlink_abs {
                            (
                                "Symbolic link".to_string(),
                                "Enter name of the symlink:".to_string(),
                            )
                        } else {
                            (
                                "Relative symlink".to_string(),
                                "Enter name of the symlink:".to_string(),
                            )
                        };
                        app.ui_mode = UiMode::InputDialog {
                            title: dlg_title,
                            prompt,
                            value: default_to.clone(),
                            focus_ok: true,
                            on_submit: Box::new(move |app, val| {
                                let src = ent.path.clone();
                                let dst = std::path::PathBuf::from(val);
                                if is_hard {
                                    app.vfs.link_hard(&src, &dst)?;
                                } else if is_symlink_abs {
                                    let abs_target = if src.is_absolute() {
                                        src.clone()
                                    } else {
                                        active_cwd.join(&src)
                                    };
                                    app.vfs.symlink(&abs_target, &dst)?;
                                } else {
                                    let base =
                                        dst.parent().unwrap_or_else(|| std::path::Path::new("."));
                                    let abs_src = if src.is_absolute() {
                                        src.clone()
                                    } else {
                                        active_cwd.join(&src)
                                    };
                                    let abs_base = if base.is_absolute() {
                                        base.to_path_buf()
                                    } else {
                                        active_cwd.join(base)
                                    };
                                    let rel = {
                                        fn relpath(
                                            from: &std::path::Path,
                                            to: &std::path::Path,
                                        ) -> std::path::PathBuf
                                        {
                                            let from = from.components().collect::<Vec<_>>();
                                            let to = to.components().collect::<Vec<_>>();
                                            let mut i = 0usize;
                                            while i < from.len() && i < to.len() && from[i] == to[i]
                                            {
                                                i += 1;
                                            }
                                            let mut out = std::path::PathBuf::new();
                                            for _ in i..from.len() {
                                                out.push("..");
                                            }
                                            for comp in &to[i..] {
                                                out.push(comp.as_os_str());
                                            }
                                            out
                                        }
                                        relpath(&abs_base, &abs_src)
                                    };
                                    app.vfs.symlink(&rel, &dst)?;
                                }
                                Ok(())
                            }),
                        };
                    }
                }
                Action::Copy => {
                    if let Some(ent) = app.active_panel().current_entry().cloned() {
                        let dst_dir = app.inactive_panel_mut().cwd.clone();
                        let default_to = dst_dir.join(&ent.name).display().to_string();
                        app.ui_mode = UiMode::CopyDialog {
                            title: "Copy".into(),
                            src_name: ent.name.clone(),
                            src_path: ent.path.clone(),
                            mask: "*".into(),
                            to: default_to,
                            using_shell_patterns: true,
                            follow_links: false,
                            preserve_attrs: true,
                            dive_into_subdir: false,
                            stable_symlinks: false,
                            focus: rmc_core::app::CopyDialogFocus::To,
                        };
                    }
                }
                Action::Move => {
                    if let Some(ent) = app.active_panel().current_entry().cloned() {
                        let dst_dir = app.inactive_panel_mut().cwd.clone();
                        let default_to = dst_dir.join(&ent.name).display().to_string();
                        app.ui_mode = UiMode::CopyDialog {
                            title: "Move".into(),
                            src_name: ent.name.clone(),
                            src_path: ent.path.clone(),
                            mask: "*".into(),
                            to: default_to,
                            using_shell_patterns: true,
                            follow_links: false,
                            preserve_attrs: true,
                            dive_into_subdir: false,
                            stable_symlinks: false,
                            focus: rmc_core::app::CopyDialogFocus::To,
                        };
                    }
                }
                _ => app.handle_action(action)?,
            }
        } else {
            // Only if not a mapped action, treat plain char as command-line typing.
            if let KeyCode::Char(c) = key.code {
                if key.modifiers.is_empty() {
                    app.subshell.cmdline.push(c);
                    app.subshell.clear_history_nav();
                    app.ui_mode = UiMode::ShellInput;
                    return Ok(());
                }
            }
        }
        // Dedicated handling for F2 User Menu action
        if let Some(Action::ShowUserMenu) = app.keymap.resolve(&key) {
            // Load menu and open dialog
            let cwd = app.active_panel().cwd.clone();
            match rmc_core::user_menu::load_menu(&cwd) {
                Ok(menu) => {
                    app.ui_mode = UiMode::UserMenu {
                        title: menu.title,
                        entries: menu.entries,
                        selected_index: 0,
                    };
                }
                Err(_) => {
                    // No menu found — ignore
                }
            }
            return Ok(());
        }
        Ok(())
    }
}

/// Build a flattened directory tree starting at `start`, up to `max_entries`, depth-first.
fn build_tree_flat(
    vfs: &dyn rmc_fs::Vfs,
    start: &std::path::Path,
    depth: usize,
    max_entries: usize,
    out: &mut Vec<rmc_core::panel::TreeEntry>,
) {
    if out.len() >= max_entries {
        return;
    }
    let mut dirs: Vec<(std::path::PathBuf, usize)> = Vec::new();
    if let Ok(entries) = vfs.list_dir(start, false) {
        for e in entries {
            if e.meta.is_dir {
                dirs.push((e.path, depth + 1));
            }
        }
    }
    dirs.sort_by(|a, b| a.0.cmp(&b.0));
    for (p, d) in dirs {
        if out.len() >= max_entries {
            break;
        }
        out.push(rmc_core::panel::TreeEntry {
            path: p.clone(),
            depth: d,
        });
        build_tree_flat(vfs, &p, d, max_entries, out);
        if out.len() >= max_entries {
            break;
        }
    }
}
impl TerminalApp {
    fn ensure_hunk_visible(state: &mut rmc_core::app::DiffState) {
        if state.hunks.is_empty() {
            return;
        }
        let cur = state.current_hunk.min(state.hunks.len().saturating_sub(1));
        if let Some(h) = state.hunks.get(cur) {
            state.left_scroll = h.left_start.saturating_sub(1);
            state.right_scroll = h.right_start.saturating_sub(1);
        }
    }
}

impl TerminalApp {
    fn is_non_equal(h: &rmc_diff::Hunk) -> bool {
        !matches!(h.kind, rmc_diff::HunkKind::Equal)
    }
    fn next_diff_hunk_index(hunks: &[rmc_diff::Hunk], current: usize) -> Option<usize> {
        let mut i = current.saturating_add(1);
        while i < hunks.len() {
            if Self::is_non_equal(&hunks[i]) {
                return Some(i);
            }
            i += 1;
        }
        None
    }
    fn prev_diff_hunk_index(hunks: &[rmc_diff::Hunk], current: usize) -> Option<usize> {
        if current == 0 {
            return None;
        }
        let mut i = current.saturating_sub(1);
        loop {
            if Self::is_non_equal(&hunks[i]) {
                return Some(i);
            }
            if i == 0 {
                break;
            }
            i -= 1;
        }
        None
    }
    fn hunk_contains_query(h: &rmc_diff::Hunk, left: &[String], right: &[String], q: &str) -> bool {
        let ql = q;
        for li in h.left_start..h.left_start.saturating_add(h.left_len) {
            if let Some(s) = left.get(li) {
                if s.contains(ql) {
                    return true;
                }
            }
        }
        for ri in h.right_start..h.right_start.saturating_add(h.right_len) {
            if let Some(s) = right.get(ri) {
                if s.contains(ql) {
                    return true;
                }
            }
        }
        false
    }
    fn search_next_hunk_with(
        hunks: &[rmc_diff::Hunk],
        left: &[String],
        right: &[String],
        current: usize,
        q: &str,
    ) -> Option<usize> {
        let mut i = current.saturating_add(1);
        while i < hunks.len() {
            let h = &hunks[i];
            if Self::is_non_equal(h) && Self::hunk_contains_query(h, left, right, q) {
                return Some(i);
            }
            i += 1;
        }
        // Wrap around
        i = 0;
        while i <= current && i < hunks.len() {
            let h = &hunks[i];
            if Self::is_non_equal(h) && Self::hunk_contains_query(h, left, right, q) {
                return Some(i);
            }
            i += 1;
        }
        None
    }
}
