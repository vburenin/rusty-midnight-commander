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
            // Draw at least at 30 FPS-equivalent idle to react to resize
            if last_draw.elapsed() > Duration::from_millis(33) {
                renderer.draw(app)?;
                last_draw = Instant::now();
            }

            if event::poll(Duration::from_millis(50))? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        Self::handle_key(app, key)?;
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

    fn handle_key(app: &mut App, key: KeyEvent) -> Result<()> {
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
                Action::Mkdir => {
                    let cwd = app.active_panel().cwd.clone();
                    app.ui_mode = UiMode::PromptInput {
                        title: "Make directory".into(),
                        value: String::new(),
                        on_submit: Box::new(move |app, name| {
                            if name.is_empty() {
                                return Ok(());
                            }
                            let path = cwd.join(name);
                            app.vfs.mkdir(&path)?;
                            Ok(())
                        }),
                    };
                }
                Action::Delete => {
                    if let Some(ent) = app.active_panel().current_entry().cloned() {
                        let path = ent.path.clone();
                        app.ui_mode = UiMode::DialogConfirm {
                            title: "Delete".into(),
                            message: format!("Delete \"{}\"?", ent.name),
                            on_ok: Box::new(move |app| {
                                app.vfs.remove(&path, true)?;
                                Ok(())
                            }),
                        };
                    }
                }
                Action::Copy => {
                    if let Some(ent) = app.active_panel().current_entry().cloned() {
                        let dst_dir = app.inactive_panel_mut().cwd.clone();
                        let default_to = dst_dir.join(&ent.name);
                        app.ui_mode = UiMode::PromptInput {
                            title: "Copy to".into(),
                            value: default_to.display().to_string(),
                            on_submit: Box::new(move |app, to| {
                                app.vfs.copy(&ent.path, std::path::Path::new(&to))?;
                                Ok(())
                            }),
                        };
                    }
                }
                Action::Move => {
                    if let Some(ent) = app.active_panel().current_entry().cloned() {
                        let dst_dir = app.inactive_panel_mut().cwd.clone();
                        let default_to = dst_dir.join(&ent.name);
                        app.ui_mode = UiMode::PromptInput {
                            title: "Rename/move to".into(),
                            value: default_to.display().to_string(),
                            on_submit: Box::new(move |app, to| {
                                app.vfs.move_path(&ent.path, std::path::Path::new(&to))?;
                                Ok(())
                            }),
                        };
                    }
                }
                _ => app.handle_action(action)?,
            }
        }
        Ok(())
    }
}
