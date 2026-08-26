use crate::actions::{Action, PaneSide, SortBy as SortByAction};
use crate::config::KeyMap;
use crate::panel::{FileEntry, PanelState, SortBy};
use anyhow::Result;
use rmc_fs::{DirEntry, Vfs};
use std::path::{Path, PathBuf};

type UiOkCb = Box<dyn FnOnce(&mut App) -> Result<()> + Send>;
type UiPromptCb = Box<dyn FnOnce(&mut App, String) -> Result<()> + Send>;

pub enum UiMode {
    Normal,
    Viewer { path: PathBuf, hex: bool },
    CopyDialog {
        title: String, // "Copy" or "Move"
        src_name: String,
        src_path: PathBuf,
        mask: String,
        to: String,
        using_shell_patterns: bool,
        follow_links: bool,
        preserve_attrs: bool,
        dive_into_subdir: bool,
        stable_symlinks: bool,
        focus: CopyDialogFocus,
    },
    MkdirDialog {
        value: String,
        focus_ok: bool,
    },
    DeleteDialog {
        name: String,
        path: PathBuf,
        focus_ok: bool,
    },
    DialogConfirm {
        title: String,
        message: String,
        on_ok: UiOkCb,
    },
    PromptInput {
        title: String,
        value: String,
        on_submit: UiPromptCb,
    },
    MenuFocused,
    Help,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CopyDialogFocus {
    Mask,
    To,
    Checkbox1,
    Checkbox2,
    Checkbox3,
    Checkbox4,
    Ok,
    Background,
    Cancel,
}
pub struct App {
    pub vfs: Box<dyn Vfs>,
    pub keymap: KeyMap,
    pub left: PanelState,
    pub right: PanelState,
    pub active: PaneSide,
    pub show_hidden: bool,
    pub quit: bool,
    pub ui_mode: UiMode,
}

impl App {
    pub fn new(vfs: Box<dyn Vfs>, keymap: KeyMap) -> Result<Self> {
        let cwd = vfs.cwd()?;
        let right_cwd = vfs.cwd()?;
        let mut app = Self {
            vfs,
            keymap,
            left: PanelState::new(&cwd),
            right: PanelState::new(&right_cwd),
            active: PaneSide::Left,
            show_hidden: false,
            quit: false,
            ui_mode: UiMode::Normal,
        };
        app.reload_panels()?;
        Ok(app)
    }

    pub fn active_panel_mut(&mut self) -> &mut PanelState {
        match self.active {
            PaneSide::Left => &mut self.left,
            PaneSide::Right => &mut self.right,
        }
    }
    pub fn inactive_panel_mut(&mut self) -> &mut PanelState {
        match self.active {
            PaneSide::Left => &mut self.right,
            PaneSide::Right => &mut self.left,
        }
    }
    pub fn active_panel(&self) -> &PanelState {
        match self.active {
            PaneSide::Left => &self.left,
            PaneSide::Right => &self.right,
        }
    }

    pub fn reload_panels(&mut self) -> Result<()> {
        let left = self.vfs.list_dir(&self.left.cwd, self.show_hidden)?;
        let right = self.vfs.list_dir(&self.right.cwd, self.show_hidden)?;
        self.left.set_entries(self.map_dir_entries(left));
        self.right.set_entries(self.map_dir_entries(right));
        Ok(())
    }

    fn map_dir_entries(&self, entries: Vec<DirEntry>) -> Vec<FileEntry> {
        entries
            .into_iter()
            .map(|e| FileEntry {
                name: e.name,
                path: e.path,
                is_dir: e.meta.is_dir,
                is_symlink: e.meta.is_symlink,
                is_exe: e.meta.is_executable,
                size: e.meta.size,
                modified: e.meta.modified,
                permissions: e.meta.permissions,
                owner: e.meta.owner,
                group: e.meta.group,
            })
            .collect()
    }

    pub fn handle_action(&mut self, action: Action) -> Result<()> {
        use Action::*;
        match action {
            Quit => self.quit = true,
            Refresh => {
                self.reload_panels()?;
            }
            ToggleHidden => {
                self.show_hidden = !self.show_hidden;
                self.reload_panels()?;
            }
            SwapPanels => {
                std::mem::swap(&mut self.left, &mut self.right);
                self.active = match self.active {
                    PaneSide::Left => PaneSide::Right,
                    PaneSide::Right => PaneSide::Left,
                };
            }
            FocusMenu => self.ui_mode = UiMode::MenuFocused,
            ShowHelp => self.ui_mode = UiMode::Help,
            MoveUp => self.active_panel_mut().move_up(),
            MoveDown => self.active_panel_mut().move_down(),
            PageUp => {
                let page = 10;
                self.active_panel_mut().page_up(page);
            }
            PageDown => {
                let page = 10;
                self.active_panel_mut().page_down(page);
            }
            Home => self.active_panel_mut().home(),
            End => self.active_panel_mut().end(),
            Enter => {
                let ent_opt = self.active_panel().current_entry().cloned();
                if let Some(ent) = ent_opt {
                    if ent.is_dir {
                        self.change_dir(&ent.path)?;
                    } else {
                        // No-op for files (ext associations to be added later)
                    }
                }
            }
            ParentDir => {
                let parent = self.active_panel().cwd.parent().map(Path::to_path_buf);
                if let Some(p) = parent {
                    self.change_dir(&p)?;
                }
            }
            SwitchPanel => {
                self.active = match self.active {
                    PaneSide::Left => PaneSide::Right,
                    PaneSide::Right => PaneSide::Left,
                }
            }
            ToggleSelect => {
                let idx = self.active_panel().cursor;
                self.active_panel_mut().selection.toggle(idx);
            }
            Sort(sb) => {
                let (by, _dir) = match sb {
                    SortByAction::Name => (SortBy::Name, self.active_panel().sort_dir),
                    SortByAction::Size => (SortBy::Size, self.active_panel().sort_dir),
                    SortByAction::Time => (SortBy::Time, self.active_panel().sort_dir),
                };
                let p = self.active_panel_mut();
                p.sort_by = by;
                p.apply_sort();
            }
            ViewFile => {
                if let Some(ent) = self.active_panel().current_entry() {
                    if !ent.is_dir {
                        self.ui_mode = UiMode::Viewer { path: ent.path.clone(), hex: false };
                    }
                }
            }
            Copy | Move | Mkdir | Delete => {
                // UI layer opens dialogs; core provides helpers
            }
            ViewerQuit => self.ui_mode = UiMode::Normal,
            ViewerToggleHex => {
                if let UiMode::Viewer { path: _, hex } = &mut self.ui_mode {
                    *hex = !*hex;
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub fn change_dir(&mut self, path: &Path) -> Result<()> {
        let new_cwd = path.to_path_buf();
        // Acquire listing before mutably borrowing panel to avoid aliasing
        let list = self.vfs.list_dir(&new_cwd, self.show_hidden)?;
        let entries = self.map_dir_entries(list);
        let p = self.active_panel_mut();
        p.cwd = new_cwd;
        p.set_entries(entries);
        Ok(())
    }
}

impl App {
    pub fn page_up_by(&mut self, rows: usize) {
        self.active_panel_mut().page_up(rows);
    }
    pub fn page_down_by(&mut self, rows: usize) {
        self.active_panel_mut().page_down(rows);
    }
}
