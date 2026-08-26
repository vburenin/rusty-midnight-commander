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
use std::io::stdout;
use std::time::{Duration, Instant};

pub struct TerminalApp;

impl TerminalApp {
    pub fn run(app: &mut App) -> Result<()> {
        let mut out = stdout();
        enable_raw_mode()?;
        execute!(out, EnterAlternateScreen, EnableMouseCapture)?;
        let palette = load_default_palette();
        let mut renderer = Renderer::new(palette);
        let mut last_draw = Instant::now();

        loop {
            // Compute content rows for page/scroll visibility
            let (_cols, rows) = crossterm::terminal::size()?;
            let panel_top = 1u16;
            let gauge_row = rows.saturating_sub(4);
            let content_bottom = gauge_row.saturating_sub(1);
            let panel_h = content_bottom - panel_top;
            let content_rows = panel_h.saturating_sub(4) as usize;
            // Ensure cursor visibility based on last-known height
            {
                let left = &mut app.left;
                left.ensure_visible(content_rows);
                let right = &mut app.right;
                right.ensure_visible(content_rows);
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
                        Self::handle_key(app, key, content_rows)?;
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
            UiMode::DialogConfirm {
                title: _,
                message: _,
                on_ok,
            } => {
                match key.code {
                    KeyCode::Esc => app.ui_mode = UiMode::Normal,
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
                    KeyCode::Esc => app.ui_mode = UiMode::Normal,
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
                                if title == "Copy" {
                                    app.vfs.copy(src_path, Path::new(&*to))?;
                                } else {
                                    app.vfs.move_path(src_path, Path::new(&*to))?;
                                }
                                app.ui_mode = UiMode::Normal;
                                app.reload_panels()?;
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
            UiMode::Menu {
                top_index,
                selected_index,
            } => {
                let menus: [&[&str]; 5] = [
                    &["Copy", "Move", "Mkdir", "Delete"],
                    &["View", "Edit", "Copy", "Move", "Mkdir", "Delete", "Quit"],
                    &["Find file", "Compare files", "Compare dirs"],
                    &["Layout", "Panels", "Confirmations"],
                    &["Copy", "Move", "Mkdir", "Delete"],
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
                            "Find file" => {
                                let start = app.active_panel().cwd.clone();
                                app.ui_mode = UiMode::FindDialog(FindDialogState::new(start));
                            }
                            "Compare files" => {
                                // Implement Compare files (mcdiff-like)
                                if let Some(a_ent) = app.active_panel().current_entry().cloned() {
                                    if a_ent.is_dir {
                                        app.ui_mode = UiMode::DialogConfirm {
                                            title: "Compare files".into(),
                                            message: "Select a file (not a directory) to compare."
                                                .into(),
                                            on_ok: Box::new(|_| Ok(())),
                                        };
                                    } else {
                                        let other_entries =
                                            app.inactive_panel_mut().entries.clone();
                                        let mut b_path = None;
                                        for e in other_entries.iter() {
                                            if e.name == a_ent.name && !e.is_dir {
                                                b_path = Some(e.path.clone());
                                                break;
                                            }
                                        }
                                        if b_path.is_none() {
                                            if let Some(b_ent) =
                                                app.inactive_panel_mut().current_entry().cloned()
                                            {
                                                if !b_ent.is_dir {
                                                    b_path = Some(b_ent.path);
                                                }
                                            }
                                        }
                                        if let Some(b) = b_path {
                                            // Load file contents
                                            let mut ltxt = String::new();
                                            let mut rtxt = String::new();
                                            {
                                                let mut r = app
                                                    .vfs
                                                    .read_file(&a_ent.path)
                                                    .map_err(|e| anyhow::anyhow!(e.to_string()))?;
                                                use std::io::Read;
                                                let _ = r.read_to_string(&mut ltxt);
                                            }
                                            {
                                                let mut r = app
                                                    .vfs
                                                    .read_file(&b)
                                                    .map_err(|e| anyhow::anyhow!(e.to_string()))?;
                                                use std::io::Read;
                                                let _ = r.read_to_string(&mut rtxt);
                                            }
                                            let left_lines = rmc_diff::split_lines(&ltxt);
                                            let right_lines = rmc_diff::split_lines(&rtxt);
                                            let dr = rmc_diff::compute_diff(&ltxt, &rtxt);
                                            let state = rmc_core::app::DiffState {
                                                left_path: a_ent.path.clone(),
                                                right_path: b.clone(),
                                                left_lines,
                                                right_lines,
                                                hunks: dr.hunks,
                                                current_hunk: 0,
                                                left_modified: false,
                                                right_modified: false,
                                                show_line_numbers: false,
                                                show_hunk_status: true,
                                                search: None,
                                                search_prompt: None,
                                                left_scroll: 0,
                                                right_scroll: 0,
                                                panel_ratio: 0.5,
                                                tab_width: 4,
                                                merge_target_right: true,
                                            };
                                            app.ui_mode = UiMode::Diff(state);
                                        } else {
                                            app.ui_mode = UiMode::DialogConfirm {
                                                title: "Compare files".into(),
                                                message:
                                                    "Could not determine the other file to compare."
                                                        .into(),
                                                on_ok: Box::new(|_| Ok(())),
                                            };
                                        }
                                    }
                                }
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
                    KeyCode::Char('q') | KeyCode::Esc | KeyCode::F(10) => {
                        app.ui_mode = UiMode::Normal;
                    }
                    KeyCode::Enter | KeyCode::Char('n') | KeyCode::Char(' ') => {
                        if !state.hunks.is_empty() {
                            state.current_hunk =
                                (state.current_hunk + 1).min(state.hunks.len().saturating_sub(1));
                            Self::ensure_hunk_visible(state);
                        }
                    }
                    KeyCode::Backspace | KeyCode::Char('p') => {
                        if !state.hunks.is_empty() {
                            state.current_hunk = state.current_hunk.saturating_sub(1);
                            Self::ensure_hunk_visible(state);
                        }
                    }
                    KeyCode::Char('g') => {
                        let on_submit = Box::new(|app: &mut App, val: String| -> anyhow::Result<()> {
                            if let UiMode::Diff(s) = &mut app.ui_mode {
                                if let Ok(n) = val.trim().parse::<usize>() {
                                    let line = n.saturating_sub(1);
                                    s.left_scroll = line;
                                    s.right_scroll = line;
                                }
                            }
                            Ok(())
                        });
                        app.ui_mode = UiMode::PromptInput {
                            title: "Goto line".into(),
                            value: String::new(),
                            on_submit,
                        };
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
                        // Open left file in internal editor
                        if let Ok(mut r) = app.vfs.read_file(&state.left_path) {
                            let mut buf = Vec::new();
                            use std::io::Read;
                            let _ = r.read_to_end(&mut buf);
                            let eb = rmc_edit::EditorBuffer::from_bytes(
                                &buf,
                                Some(state.left_path.clone()),
                            );
                            app.ui_mode = UiMode::Editor {
                                buf: eb,
                                show_menu: false,
                                status_msg: None,
                                search_input: None,
                                save_as_input: None,
                                pending_quit: false,
                                confirm_exit: None,
                            };
                        }
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
                // If viewer has an active search prompt, handle it first
                if let UiMode::Viewer {
                    path,
                    offset,
                    search,
                    search_prompt: Some(prompt),
                    ..
                } = &mut app.ui_mode
                {
                    match key.code {
                        KeyCode::Esc => {
                            *prompt = String::new();
                            if let UiMode::Viewer { search_prompt, .. } = &mut app.ui_mode {
                                *search_prompt = None;
                            }
                        }
                        KeyCode::Enter => {
                            let q = prompt.clone();
                            if let Some(pos) = rmc_view::search_forward(path, *offset, &q)? {
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
                    KeyCode::Up => {
                        if let UiMode::Viewer {
                            path, hex, offset, ..
                        } = &mut app.ui_mode
                        {
                            if *hex {
                                *offset = offset.saturating_sub(16);
                            } else {
                                *offset = rmc_view::nav_line_up(path, *offset)?;
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
                                *offset = rmc_view::nav_line_down(path, *offset)?;
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
                                *offset = rmc_view::nav_page_down(
                                    path,
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
                                *offset = rmc_view::nav_page_up(
                                    path,
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
                                let len = rmc_view::file_len(path)?;
                                let page = 16u64 * (content_rows as u64);
                                *offset = len.saturating_sub(page);
                            } else {
                                *offset = rmc_view::nav_end(
                                    path,
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
                                if let Some(pos) =
                                    rmc_view::search_forward(path, offset.saturating_add(1), &q)?
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
                                if let Some(pos) = rmc_view::search_backward(path, *offset, &q)? {
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
        if let Some(action) = app.keymap.resolve(&key) {
            match action {
                Action::PageUp => app.page_up_by(page_rows),
                Action::PageDown => app.page_down_by(page_rows),
                Action::ToggleSubshell => {
                    app.handle_action(Action::ToggleSubshell)?;
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
        Ok(())
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
