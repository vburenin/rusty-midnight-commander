use crate::skin::load_default_palette;
use crate::render::Renderer;
use anyhow::Result;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::execute;
use rmc_core::actions::Action;
use rmc_core::app::{App, UiMode};
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
            UiMode::PromptInput { title: _, value, on_submit } => {
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
                    KeyCode::Char(c) => {
                        if key.modifiers.is_empty() {
                            value.push(c);
                        }
                    }
                    _ => {}
                }
                return Ok(());
            }
            UiMode::DialogConfirm { title: _, message: _, on_ok } => {
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
                    KeyCode::Char(c) => {
                        if !*focus_ok && key.modifiers.is_empty() {
                            value.push(c);
                        }
                    }
                    _ => {}
                }
                return Ok(());
            }
            UiMode::DeleteDialog { name: _, path, focus_ok } => {
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
            UiMode::CopyDialog { title, src_name: _, src_path, mask, to, using_shell_patterns, follow_links, preserve_attrs, dive_into_subdir, stable_symlinks, focus } => {
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
                    KeyCode::Backspace => {
                        match *focus {
                            F::Mask => { mask.pop(); }
                            F::To => { to.pop(); }
                            _ => {}
                        }
                    }
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
            UiMode::Menu { top_index, selected_index } => {
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
                        if *top_index > 0 { *top_index -= 1; }
                        *selected_index = 0;
                    }
                    KeyCode::Right => {
                        if *top_index < 4 { *top_index += 1; }
                        *selected_index = 0;
                    }
                    KeyCode::Up => {
                        if *selected_index > 0 { *selected_index -= 1; }
                    }
                    KeyCode::Down => {
                        let max = menus[*top_index].len().saturating_sub(1);
                        if *selected_index < max { *selected_index += 1; }
                    }
                    KeyCode::Enter => {
                        let item = menus[*top_index][*selected_index];
                        match item {
                            "Copy" => { return Self::handle_key(app, KeyEvent::new(KeyCode::F(5), key.modifiers), page_rows); }
                            "Move" => { return Self::handle_key(app, KeyEvent::new(KeyCode::F(6), key.modifiers), page_rows); }
                            "Mkdir" => { return Self::handle_key(app, KeyEvent::new(KeyCode::F(7), key.modifiers), page_rows); }
                            "Delete" => { return Self::handle_key(app, KeyEvent::new(KeyCode::F(8), key.modifiers), page_rows); }
                            "Quit" => { app.handle_action(Action::Quit)?; }
                            _ => { app.ui_mode = UiMode::Normal; }
                        }
                    }
                    _ => {}
                }
                return Ok(());
            }
            UiMode::Viewer { .. } => {
                match key.code {
                    KeyCode::Char('q') => app.handle_action(Action::ViewerQuit)?,
                    KeyCode::Char('h') | KeyCode::Char('x') => app.handle_action(Action::ViewerToggleHex)?,
                    KeyCode::Char('w') => {
                        if let UiMode::Viewer { hex, wrap, .. } = &mut app.ui_mode {
                            if !*hex {
                                *wrap = !*wrap;
                            }
                        }
                    }
                    KeyCode::Up => {
                        if let UiMode::Viewer { path, hex, offset, .. } = &mut app.ui_mode {
                            if *hex {
                                *offset = offset.saturating_sub(16);
                            } else {
                                *offset = rmc_view::nav_line_up(path, *offset)?;
                            }
                        }
                    }
                    KeyCode::Down => {
                        if let UiMode::Viewer { path, hex, offset, .. } = &mut app.ui_mode {
                            if *hex {
                                *offset = offset.saturating_add(16);
                            } else {
                                *offset = rmc_view::nav_line_down(path, *offset)?;
                            }
                        }
                    }
                    KeyCode::PageDown => {
                        if let UiMode::Viewer { path, hex, wrap, offset, .. } = &mut app.ui_mode {
                            let (cols, rows) = crossterm::terminal::size()?;
                            // content area inside frame is rows-2, cols-2
                            let content_rows = rows.saturating_sub(2);
                            if *hex {
                                let step = 16u64 * (content_rows as u64);
                                *offset = offset.saturating_add(step);
                            } else {
                                *offset = rmc_view::nav_page_down(path, *offset, cols.saturating_sub(2), content_rows, *wrap)?;
                            }
                        }
                    }
                    KeyCode::PageUp => {
                        if let UiMode::Viewer { path, hex, wrap, offset, .. } = &mut app.ui_mode {
                            let (cols, rows) = crossterm::terminal::size()?;
                            let content_rows = rows.saturating_sub(2);
                            if *hex {
                                let step = 16u64 * (content_rows as u64);
                                *offset = offset.saturating_sub(step);
                            } else {
                                *offset = rmc_view::nav_page_up(path, *offset, cols.saturating_sub(2), content_rows, *wrap)?;
                            }
                        }
                    }
                    KeyCode::Home => {
                        if let UiMode::Viewer { offset, .. } = &mut app.ui_mode {
                            *offset = rmc_view::nav_home();
                        }
                    }
                    KeyCode::End => {
                        if let UiMode::Viewer { path, hex, wrap, offset, .. } = &mut app.ui_mode {
                            let (cols, rows) = crossterm::terminal::size()?;
                            let content_rows = rows.saturating_sub(2);
                            if *hex {
                                let len = rmc_view::file_len(path)?;
                                let page = 16u64 * (content_rows as u64);
                                *offset = len.saturating_sub(page);
                            } else {
                                *offset = rmc_view::nav_end(path, cols.saturating_sub(2), content_rows, *wrap)?;
                            }
                        }
                    }
                    KeyCode::Char('/') | KeyCode::F(7) => {
                        // Prompt for search term
                        app.ui_mode = UiMode::PromptInput {
                            title: "Search".into(),
                            value: String::new(),
                            on_submit: Box::new(|app, q| {
                                if let UiMode::Viewer { path, offset, search, .. } = &mut app.ui_mode {
                                    *search = if q.is_empty() { None } else { Some(q.clone()) };
                                    if let Some(pos) = rmc_view::search_forward(path, *offset, &q)? {
                                        *offset = pos;
                                    }
                                }
                                Ok(())
                            }),
                        };
                    }
                    KeyCode::Char('n') => {
                        if let UiMode::Viewer { path, offset, search, .. } = &mut app.ui_mode {
                            if let Some(q) = search.clone() {
                                if let Some(pos) = rmc_view::search_forward(path, offset.saturating_add(1), &q)? {
                                    *offset = pos;
                                }
                            }
                        }
                    }
                    KeyCode::Char('N') => {
                        if let UiMode::Viewer { path, offset, search, .. } = &mut app.ui_mode {
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
                    app.ui_mode = UiMode::MkdirDialog { value: String::new(), focus_ok: false };
                }
                Action::Delete => {
                    if let Some(ent) = app.active_panel().current_entry().cloned() {
                        let path = ent.path.clone();
                        app.ui_mode = UiMode::DeleteDialog { name: ent.name, path, focus_ok: true };
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
        Ok(())
    }
}
