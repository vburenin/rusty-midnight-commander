use crate::actions::{Action, PaneSide, SortBy as SortByAction};
use crate::config::KeyMap;
use crate::find::FindDialogState;
use crate::hotlist::{Hotlist, HotlistDialogState};
use crate::panel::{FileEntry, PanelState, SortBy};
use crate::subshell::Subshell;
use anyhow::Result;
use rmc_diff;
use rmc_edit::EditorBuffer;
use rmc_fs::{DirEntry, Vfs};
use std::path::{Path, PathBuf};

type UiOkCb = Box<dyn FnOnce(&mut App) -> Result<()> + Send>;
type UiPromptCb = Box<dyn FnOnce(&mut App, String) -> Result<()> + Send>;

#[derive(Clone)]
pub struct DiffState {
    pub left_path: PathBuf,
    pub right_path: PathBuf,
    pub left_lines: Vec<String>,
    pub right_lines: Vec<String>,
    pub hunks: Vec<rmc_diff::Hunk>,
    pub current_hunk: usize,
    pub left_modified: bool,
    pub right_modified: bool,
    pub show_line_numbers: bool,
    pub show_hunk_status: bool,
    pub search: Option<String>,
    pub search_prompt: Option<String>,
    pub goto_prompt: Option<String>,
    pub confirm_exit: Option<YncDialog>,
    pub left_scroll: usize,
    pub right_scroll: usize,
    pub panel_ratio: f32, // fraction for left [0.2..0.8]
    pub tab_width: usize,
    pub merge_target_right: bool, // F5 merges into right when true; swap flips this
}

pub enum UiMode {
    Normal,
    /// MC User Menu (F2) – list of user-defined commands with hotkeys
    UserMenu {
        title: String,
        entries: Vec<crate::user_menu::MenuEntry>,
        selected_index: usize,
    },
    Viewer {
        path: PathBuf,
        hex: bool,
        wrap: bool,
        offset: u64,
        search: Option<String>,
        search_prompt: Option<String>,
    },
    Diff(DiffState),
    Editor {
        buf: EditorBuffer,
        show_menu: bool,
        status_msg: Option<String>,
        search_input: Option<String>,
        save_as_input: Option<String>,
        pending_quit: bool,
        confirm_exit: Option<YncDialog>,
    },
    Menu {
        top_index: usize,
        selected_index: usize,
    },
    // Find File dialog state (UI renders and drives this)
    FindDialog(FindDialogState),
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
        focus_ok: bool, // true focuses OK button; false focuses input
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
    /// Command line at bottom has focus for editing/executing.
    ShellInput,
    // Directory Hotlist
    HotlistDialog(HotlistDialogState),
    // Directory Hotlist
    HotlistDialog(HotlistDialogState),
}

// Simple glob matcher supporting '*' (any sequence) and '?' (single char).
// Case-sensitive, anchored to full string.
fn glob_match(pat: &str, name: &str) -> bool {
    glob_match_impl(pat.as_bytes(), name.as_bytes())
}

fn glob_match_impl(p: &[u8], s: &[u8]) -> bool {
    // Two-pointer with backtracking on '*'
    let (mut i, mut j) = (0usize, 0usize);
    let (mut star_i, mut star_j) = (None, 0usize);
    while j < s.len() {
        if i < p.len() && (p[i] == b'?' || p[i] == s[j]) {
            i += 1;
            j += 1;
        } else if i < p.len() && p[i] == b'*' {
            star_i = Some(i);
            i += 1;
            star_j = j;
        } else if let Some(si) = star_i {
            // backtrack: advance match under last '*'
            i = si + 1;
            star_j += 1;
            j = star_j;
        } else {
            return false;
        }
    }
    // Consume trailing '*' in pattern
    while i < p.len() && p[i] == b'*' {
        i += 1;
    }
    i == p.len()
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum YncFocus {
    Yes,
    No,
    Cancel,
}

#[derive(Clone)]
pub struct YncDialog {
    pub title: String,
    pub message: String,
    pub focus: YncFocus,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CopyDialogFocus {
    Mask,
    To,
    Checkbox1,
    Checkbox2,
    Checkbox3,
    Checkbox4,
    Checkbox5,
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
    /// Subshell / command line state and last output.
    pub subshell: Subshell,
    pub hotlist: Hotlist,
    pub pending_ctrl_x: bool,
    pub hotlist: Hotlist,
    pub pending_ctrl_x: bool,
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
            subshell: Subshell::new(),
            hotlist: Hotlist::load_from_default_path(),
            pending_ctrl_x: false,
            hotlist: Hotlist::load_from_default_path(),
            pending_ctrl_x: false,
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
            OpenHotlist => {
                let st = HotlistDialogState::new(self.hotlist.entries.clone());
                self.ui_mode = UiMode::HotlistDialog(st);
            }
            Quit => self.quit = true,
            Refresh => {
                self.reload_panels()?;
            }
            ToggleSubshell => {
                // Toggle full-screen subshell/output view.
                self.subshell.toggle_output_screen();
                // Ensure we leave any transient prompt/dialog modes.
                self.ui_mode = UiMode::Normal;
            }
            ToggleHidden => {
                self.show_hidden = !self.show_hidden;
                self.reload_panels()?;
            }
            InvertSelection => {
                // Invert selection for all non-parent entries in the active panel
                let len = self.active_panel().entries.len();
                for idx in 0..len {
                    let ent = &self.active_panel().entries[idx];
                    if !ent.is_parent_marker() {
                        self.active_panel_mut().selection.toggle(idx);
                    }
                }
            }
            SelectGroup => {
                let title = "Select group (glob):".to_string();
                self.ui_mode = UiMode::PromptInput {
                    title,
                    value: "*".to_string(),
                    on_submit: Box::new(|app, pattern| {
                        let pat = pattern.trim().to_string();
                        let len = app.active_panel().entries.len();
                        for idx in 0..len {
                            let ent = &app.active_panel().entries[idx];
                            if ent.is_parent_marker() {
                                continue;
                            }
                            if glob_match(&pat, &ent.name)
                                && !app.active_panel().selection.is_selected(idx)
                            {
                                app.active_panel_mut().selection.select(idx);
                            }
                        }
                        Ok(())
                    }),
                };
            }
            UnselectGroup => {
                let title = "Unselect group (glob):".to_string();
                self.ui_mode = UiMode::PromptInput {
                    title,
                    value: "*".to_string(),
                    on_submit: Box::new(|app, pattern| {
                        let pat = pattern.trim().to_string();
                        let len = app.active_panel().entries.len();
                        for idx in 0..len {
                            let ent = &app.active_panel().entries[idx];
                            if ent.is_parent_marker() {
                                continue;
                            }
                            if glob_match(&pat, &ent.name)
                                && app.active_panel().selection.is_selected(idx)
                            {
                                app.active_panel_mut().selection.unselect(idx);
                            }
                        }
                        Ok(())
                    }),
                };
            }
            SwapPanels => {
                std::mem::swap(&mut self.left, &mut self.right);
                self.active = match self.active {
                    PaneSide::Left => PaneSide::Right,
                    PaneSide::Right => PaneSide::Left,
                };
            }
            FocusMenu => {
                self.ui_mode = UiMode::Menu {
                    top_index: 0,
                    selected_index: 0,
                }
            }
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
                let panelized = self.active_panel().is_panelized();
                let ent_opt = self.active_panel().current_entry().cloned();
                if let Some(ent) = ent_opt {
                    if panelized && ent.name == ".." {
                        self.active_panel_mut().unpanelize();
                    } else if ent.is_dir {
                        self.change_dir(&ent.path)?;
                    } else {
                        // Ask VFS if this path is enterable (e.g., archives)
                        if let Some(p) = self.vfs.enter_path(&ent.path) {
                            self.change_dir(&p)?;
                        } else {
                            // No-op for regular files (open with View action instead)
                        }
                    }
                }
            }
            ParentDir => {
                if self.active_panel().is_panelized() {
                    self.active_panel_mut().unpanelize();
                } else {
                    let parent = self.active_panel().cwd.parent().map(Path::to_path_buf);
                    if let Some(p) = parent {
                        self.change_dir(&p)?;
                    }
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
                        self.ui_mode = UiMode::Viewer {
                            path: ent.path.clone(),
                            hex: false,
                            wrap: false,
                            offset: 0,
                            search: None,
                            search_prompt: None,
                        };
                    }
                }
            }
            Copy | Move | Mkdir | Delete => {
                // UI layer opens dialogs; core provides helpers
            }
            ViewerQuit => self.ui_mode = UiMode::Normal,
            ViewerToggleHex => {
                if let UiMode::Viewer { hex, .. } = &mut self.ui_mode {
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

    pub fn panelize_paths(&mut self, paths: &[PathBuf], base: Option<&Path>) -> Result<()> {
        // Build FileEntry list from paths, including a `..` parent marker to leave panelized mode.
        let mut entries = Vec::with_capacity(paths.len() + 1);
        // Parent marker that points to current cwd for caption/restore
        entries.push(FileEntry {
            name: "..".to_string(),
            path: self.active_panel().cwd.clone(),
            is_dir: true,
            is_symlink: false,
            is_exe: false,
            size: 0,
            modified: std::time::SystemTime::UNIX_EPOCH,
            permissions: 0,
            owner: None,
            group: None,
        });
        for p in paths {
            let meta = self.vfs.stat(p)?;
            let display_name = if let Some(b) = base {
                if let Ok(rel) = p.strip_prefix(b) {
                    rel.display().to_string()
                } else {
                    p.display().to_string()
                }
            } else {
                p.display().to_string()
            };
            entries.push(FileEntry {
                name: display_name,
                path: p.clone(),
                is_dir: meta.is_dir,
                is_symlink: meta.is_symlink,
                is_exe: meta.is_executable,
                size: meta.size,
                modified: meta.modified,
                permissions: meta.permissions,
                owner: meta.owner,
                group: meta.group,
            });
        }
        let caption = self.active_panel().cwd.clone();
        self.active_panel_mut()
            .set_panelized_entries(caption, entries);
        Ok(())
    }
}
