use crate::mc_colors::McPalette;
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
        let mut renderer = Renderer::new(McPalette::default());
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
            UiMode::CopyDialog { title, src_name: _, src_path, mask: _, to, using_shell_patterns: _, follow_links: _, preserve_attrs: _, dive_into_subdir: _, stable_symlinks: _, focus } => {
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
                            F::Checkbox4 => F::Ok,
                            F::Ok => F::Background,
                            F::Background => F::Cancel,
                            F::Cancel => F::Mask,
                        };
                    }
                    KeyCode::Backspace => {
                        if matches!(*focus, F::Mask | F::To) {
                            to.pop();
                        }
                    }
                    KeyCode::Char(c) => {
                        if key.modifiers.is_empty() && matches!(*focus, F::Mask | F::To) {
                            to.push(c);
                        }
                    }
                    KeyCode::Enter => {
                        match *focus {
                            F::Ok => {
                                if title == "Copy" {
                                    app.vfs.copy(src_path, std::path::Path::new(&to))?;
                                } else {
                                    app.vfs.move_path(src_path, std::path::Path::new(&to))?;
                                }
                                app.ui_mode = UiMode::Normal;
                                app.reload_panels()?;
                            }
                            F::Background => {
                                app.ui_mode = UiMode::Normal;
                            }
                            F::Cancel => {
                                app.ui_mode = UiMode::Normal;
                            }
                            _ => {}
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
            _ => {}
        }

        if let Some(action) = app.keymap.resolve(&key) {
            match action {
                Action::PageUp => app.page_up_by(page_rows),
                Action::PageDown => app.page_down_by(page_rows),
                Action::Mkdir => {
                    app.ui_mode = UiMode::MkdirDialog { value: String::new(), focus_ok: true };
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
