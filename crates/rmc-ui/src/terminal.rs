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
            let (cols, rows) = crossterm::terminal::size()?;
            let panel_top = 1u16;
            let gauge_row = rows.saturating_sub(4);
            let content_bottom = gauge_row.saturating_sub(1);
            let panel_h = content_bottom - panel_top;
            let content_rows = panel_h.saturating_sub(4) as usize;
            // Ensure cursor visibility based on last-known height
            {
                match &mut app.ui_mode {
                    UiMode::Editor { buf, .. } => {
                        // Editor viewport: full width, rows between menu and status
                        let view_h = rows.saturating_sub(3) as usize; // rows - (menu + status + fbar)
                        let view_w = cols as usize;
                        buf.adjust_viewport(view_w, view_h);
                    }
                    _ => {
                        let left = &mut app.left;
                        left.ensure_visible(content_rows);
                        let right = &mut app.right;
                        right.ensure_visible(content_rows);
                    }
                }
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
                    _ => {}
                }
                return Ok(());
            }
            UiMode::Editor { buf, show_menu, status_msg, search_input, save_as_input, pending_quit, confirm_exit } => {
                use std::io::Write;
                match key.code {
                    // Confirm dialog handling (when open)
                    KeyCode::Left | KeyCode::Right | KeyCode::Tab | KeyCode::Enter | KeyCode::Esc if confirm_exit.is_some() => {
                        if let Some(c) = confirm_exit {
                            match key.code {
                                KeyCode::Left => {
                                    c.focus = match c.focus {
                                        rmc_core::app::YncFocus::Yes => rmc_core::app::YncFocus::Cancel,
                                        rmc_core::app::YncFocus::No => rmc_core::app::YncFocus::Yes,
                                        rmc_core::app::YncFocus::Cancel => rmc_core::app::YncFocus::No,
                                    };
                                }
                                KeyCode::Right | KeyCode::Tab => {
                                    c.focus = match c.focus {
                                        rmc_core::app::YncFocus::Yes => rmc_core::app::YncFocus::No,
                                        rmc_core::app::YncFocus::No => rmc_core::app::YncFocus::Cancel,
                                        rmc_core::app::YncFocus::Cancel => rmc_core::app::YncFocus::Yes,
                                    };
                                }
                                KeyCode::Esc => {
                                    *confirm_exit = None;
                                }
                                KeyCode::Enter => {
                                    match c.focus {
                                        rmc_core::app::YncFocus::Yes => {
                                            if let Some(path) = &buf.path {
                                                let mut w = app.vfs.write_file(path)?;
                                                let data = buf.to_bytes();
                                                w.write_all(&data)?; w.flush()?;
                                                app.ui_mode = UiMode::Normal;
                                            } else {
                                                *save_as_input = Some(String::new());
                                                *pending_quit = true;
                                                *confirm_exit = None;
                                                *status_msg = Some("Enter filename and press Enter".into());
                                            }
                                        }
                                        rmc_core::app::YncFocus::No => {
                                            app.ui_mode = UiMode::Normal;
                                        }
                                        rmc_core::app::YncFocus::Cancel => {
                                            *confirm_exit = None;
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    KeyCode::F(9) => { *show_menu = !*show_menu; }
                    KeyCode::F(2) => {
                        if let Some(path) = &buf.path {
                            let mut w = app.vfs.write_file(path)?;
                            let data = buf.to_bytes();
                            w.write_all(&data)?; w.flush()?;
                            *status_msg = Some("Saved".into()); buf.dirty = false;
                        } else {
                            *save_as_input = Some(String::new());
                            *status_msg = Some("Enter filename and press Enter".into());
                        }
                    }
                    KeyCode::F(10) => {
                        if buf.dirty {
                            *confirm_exit = Some(rmc_core::app::YncDialog {
                                title: "Confirm".into(),
                                message: "File was modified. Save with exit?".into(),
                                focus: rmc_core::app::YncFocus::Yes,
                            });
                        } else {
                            app.ui_mode = UiMode::Normal;
                        }
                    }
                    KeyCode::Esc => {
                        if *save_as_input == Some(String::new()) || search_input.is_some() {
                            *save_as_input = None; *search_input = None;
                        } else if buf.dirty {
                            *confirm_exit = Some(rmc_core::app::YncDialog {
                                title: "Confirm".into(),
                                message: "File was modified. Save with exit?".into(),
                                focus: rmc_core::app::YncFocus::Yes,
                            });
                        } else { app.ui_mode = UiMode::Normal; }
                    }
                    KeyCode::F(7) => { *search_input = Some(String::new()); *status_msg = Some("Find: type text and press Enter".into()); }
                    KeyCode::Enter => {
                        if let Some(q) = search_input.take() {
                            if let Some((_r, _c)) = buf.search_forward(q.as_bytes()) { *status_msg = Some("Found".into()); } else { *status_msg = Some("Not found".into()); }
                        } else if let Some(name) = save_as_input.take() {
                            let mut path = active_cwd.join(name);
                            if path.is_dir() { path = path.join("untitled.txt"); }
                            let mut w = app.vfs.write_file(&path)?; let data = buf.to_bytes();
                            w.write_all(&data)?; w.flush()?;
                            buf.path = Some(path); buf.dirty = false; *status_msg = Some("Saved".into());
                            if *pending_quit { app.ui_mode = UiMode::Normal; }
                        } else {
                            buf.insert_newline(); *pending_quit = false;
                        }
                    }
                    KeyCode::Backspace => {
                        if let Some(s) = search_input { s.pop(); }
                        else if let Some(s) = save_as_input { s.pop(); }
                        else { buf.backspace(); *pending_quit = false; }
                    }
                    KeyCode::Delete => { buf.delete(); *pending_quit = false; }
                    KeyCode::Insert => { buf.toggle_overwrite(); }
                    KeyCode::Left => { buf.move_left(); *pending_quit = false; }
                    KeyCode::Right => { buf.move_right(); *pending_quit = false; }
                    KeyCode::Up => { buf.move_up(); *pending_quit = false; }
                    KeyCode::Down => { buf.move_down(); *pending_quit = false; }
                    KeyCode::Char('z') if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => { let _ = buf.undo(); *pending_quit = false; }
                    KeyCode::Char(c) => {
                        if key.modifiers.is_empty() {
                            if let Some(s) = search_input { s.push(c); }
                            else if let Some(s) = save_as_input { s.push(c); }
                            else { buf.insert_char(c); *pending_quit = false; }
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
                Action::FunctionKey(4) => {
                    // Open editor on current file (or new empty)
                    let buf = if let Some(ent) = app.active_panel().current_entry().cloned() {
                        if !ent.is_dir && ent.name != ".." {
                            // Read file bytes
                            let mut reader = app.vfs.read_file(&ent.path)?;
                            let mut bytes = Vec::new();
                            use std::io::Read;
                            reader.read_to_end(&mut bytes)?;
                            rmc_edit::EditorBuffer::from_bytes(&bytes, Some(ent.path))
                        } else {
                            rmc_edit::EditorBuffer::new_empty()
                        }
                    } else {
                        rmc_edit::EditorBuffer::new_empty()
                    };
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
                _ => app.handle_action(action)?,
            }
        }
        Ok(())
    }
}
