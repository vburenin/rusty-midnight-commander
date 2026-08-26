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
        // Dialog handling first
        match &mut app.ui_mode {
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
            UiMode::UserMenu {
                title: _,
                entries,
                selected_index,
            } => {
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
                            let cmd = rmc_core::user_menu::expand_macros(app, &ent.command);
                            let _ = rmc_core::user_menu::run_menu_command(app, &cmd);
                            app.ui_mode = UiMode::Normal;
                            app.reload_panels()?;
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
                                let cmd = rmc_core::user_menu::expand_macros(app, &ent.command);
                                let _ = rmc_core::user_menu::run_menu_command(app, &cmd);
                                app.ui_mode = UiMode::Normal;
                                app.reload_panels()?;
                            }
                        }
                    }
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
                    &["Find file", "Compare dirs"],
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

        if let Some(action) = app.keymap.resolve(&key) {
            match action {
                Action::PageUp => app.page_up_by(page_rows),
                Action::PageDown => app.page_down_by(page_rows),
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
