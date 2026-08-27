use crate::actions::{Action, PaneSide, SortBy as SortByAction};
use crate::config::KeyMap;
use crate::find::FindDialogState;
use crate::hotlist::{Hotlist, HotlistDialogState};
use crate::panel::{FileEntry, PanelState, SortBy};
use crate::subshell::Subshell;
use anyhow::Result;
use crossterm::event::KeyEvent;
use rmc_diff;
use rmc_edit::EditorBuffer;
use rmc_fs::{DirEntry, Vfs};
use std::path::{Path, PathBuf};

type UiOkCb = Box<dyn FnOnce(&mut App) -> Result<()> + Send>;
type UiPromptCb = Box<dyn FnOnce(&mut App, String) -> Result<()> + Send>;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CompareDirsMode {
    Quick,
    SizeOnly,
    Thorough,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CompareDirsFocus {
    RadioQuick,
    RadioSizeOnly,
    RadioThorough,
    Ok,
    Cancel,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum JobsDialogFocus {
    /// The list of jobs has focus; Up/Down change selection.
    List,
    /// Bottom-row buttons focus: Cancel selected job.
    Cancel,
    /// Bottom-row buttons focus: Drop finished jobs.
    Cleanup,
    /// Bottom-row buttons focus: Close dialog (OK).
    Ok,
}

#[derive(Clone, Copy, Debug)]
pub struct LayoutOptions {
    pub menubar_visible: bool,
    pub command_prompt: bool,
    pub keybar_visible: bool,
    pub hintbar_visible: bool,
    // Optional extras; default ON to match GNU mc
    pub xterm_title: bool,
    pub show_free_space: bool,
}

impl Default for LayoutOptions {
    fn default() -> Self {
        Self {
            menubar_visible: true,
            command_prompt: true,
            keybar_visible: true,
            hintbar_visible: true,
            xterm_title: true,
            show_free_space: true,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ConfirmOptions {
    pub delete: bool,
    pub overwrite: bool,
    pub execute: bool,
    pub exit: bool,
    pub directory_hotlist: bool,
    pub history_cleanup: bool,
}

impl Default for ConfirmOptions {
    fn default() -> Self {
        Self {
            delete: true,
            overwrite: true,
            execute: false,
            exit: false,
            directory_hotlist: false,
            history_cleanup: true,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PanelOptions {
    pub show_hidden: bool,        // default false — match current App.show_hidden
    pub mix_all_files: bool,      // default false — match current dirs_first=true
    pub mark_moves_down: bool,    // default true — GNU mc Insert-mark then cursor down
    pub show_mini_status: bool,   // default true; store only
    pub kilobyte_si: bool,        // default false; store only
    pub fast_reload: bool,        // default false; store only
    pub reverse_files_only: bool, // default true — dirs-first reverse: files only (dirs stay name-asc)
    pub simple_swap: bool,        // default false — swap panes without flipping active
    pub auto_save_setup: bool,    // default false; store only
    pub lynx_like: bool,          // default false — Left=parent, Right=enter in listing
}

impl Default for PanelOptions {
    fn default() -> Self {
        Self {
            show_hidden: false,
            mix_all_files: false,
            mark_moves_down: true,
            show_mini_status: true,
            kilobyte_si: false,
            fast_reload: false,
            reverse_files_only: true,
            simple_swap: false,
            auto_save_setup: false,
            lynx_like: false,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ConfigOptions {
    pub verbose: bool,             // default true; store only unless wired later
    pub compute_totals: bool,      // default true; store only
    pub classic_progressbar: bool, // default true; store only
    pub use_internal_view: bool,   // default true — keep F3 internal viewer by default
    pub use_internal_edit: bool,   // default true — keep F4 internal editor by default
    pub pause_after_run: bool,     // default false; store only
    pub shell_patterns: bool,      // default true; store only
    pub auto_menus: bool,          // default false; store only
    pub drop_menus: bool,          // default false; store only
    pub mkdir_autoname: bool,      // default false; store only
}

impl Default for ConfigOptions {
    fn default() -> Self {
        Self {
            verbose: true,
            compute_totals: true,
            classic_progressbar: true,
            use_internal_view: true,
            use_internal_edit: true,
            pause_after_run: false,
            shell_patterns: true,
            auto_menus: false,
            drop_menus: false,
            mkdir_autoname: false,
        }
    }
}

/// GNU mc Options → Virtual FS
#[derive(Clone, Debug)]
pub struct VfsOptions {
    /// Always use ftp proxy
    pub always_use_ftp_proxy: bool,
    /// FTP proxy host (host[:port])
    pub ftp_proxy_host: String,
    /// Use ~/.netrc for FTP login
    pub use_netrc: bool,
    /// Default password for anonymous FTP
    pub ftp_anon_password: String,
    /// Directory cache timeout in seconds (stored; not currently applied)
    pub dir_cache_timeout_secs: u32,
}

impl Default for VfsOptions {
    fn default() -> Self {
        Self {
            always_use_ftp_proxy: false,
            ftp_proxy_host: String::new(),
            use_netrc: true,
            ftp_anon_password: "anonymous@".to_string(),
            dir_cache_timeout_secs: 900,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LayoutFocus {
    MenuBar,
    CommandPrompt,
    KeyBar,
    HintBar,
    XtermTitle,
    ShowFreeSpace,
    Ok,
    Cancel,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ConfigOptionsFocus {
    Verbose,
    ComputeTotals,
    ClassicProgressbar,
    UseInternalViewer,
    UseInternalEditor,
    PauseAfterRun,
    ShellPatterns,
    AutoMenus,
    DropMenus,
    MkdirAutoname,
    Ok,
    Cancel,
}

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

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CopyMoveOp {
    Copy,
    Move,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OverwriteFocus {
    Yes,
    No,
    All,
    Older,
    None,
    Smaller,
    SizeDiffers,
    Append,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ListingModeFocus {
    RadioFull,
    RadioBrief,
    RadioLong,
    RadioUser,
    Input,
    Ok,
    Cancel,
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
        // Inline overlays and toggles specific to viewer
        goto_prompt: Option<String>,
        show_line_numbers: bool,
        show_cr: bool,
    },
    Diff(DiffState),
    // Sort order dialog for Left/Right panel
    SortDialog {
        side: PaneSide,
        focus_index: usize,
        by: SortBy,
        reverse: bool,
        dirs_first: bool,
    },
    /// GNU mc Left/Right → Listing mode dialog
    ListingModeDialog {
        side: PaneSide,
        listing: crate::panel::ListingFormat,
        user_format: String,
        focus: ListingModeFocus,
    },
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
    /// Overwrite/Replace dialog shown when destination exists for Copy/Move.
    OverwriteDialog {
        op: CopyMoveOp,
        src_path: PathBuf,
        dst_path: PathBuf,
        focus: OverwriteFocus,
    },
    // Permissions dialog
    ChmodDialog {
        name: String,
        mode: u32,
        // Bit flags
        ur: bool,
        uw: bool,
        ux: bool,
        gr: bool,
        gw: bool,
        gx: bool,
        or_: bool,
        ow: bool,
        ox: bool,
        suid: bool,
        sgid: bool,
        sticky: bool,
        recursive: bool,
        focus_index: usize,
    },
    // Ownership dialog
    ChownDialog {
        owner: String,
        group: String,
        recursive: bool,
        // 0=owner,1=group,2=recursive,3=ok,4=cancel
        focus_index: usize,
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
    // Generic input dialog with a prompt line and OK/Cancel
    InputDialog {
        title: String,
        prompt: String,
        value: String,
        on_submit: UiPromptCb,
        focus_ok: bool,
    },
    /// FTP/SFTP connect dialog (GNU mc-style multi-field form)
    FtpConnectDialog {
        /// "ftp" or "sftp"
        scheme: String,
        host: String,
        port: String,      // empty -> default (21/22)
        user: String,      // empty -> none (or anonymous if checkbox on)
        password: String,  // may be empty
        directory: String, // defaults to "/"
        anonymous: bool,
        /// 0=host,1=port,2=user,3=password,4=directory,5=anonymous
        focus_index: usize,
        /// true -> OK button focused; false -> fields focused
        focus_ok: bool,
    },
    MenuFocused,
    Help {
        state: HelpState,
        prev: Box<UiMode>,
    },
    /// Command line at bottom has focus for editing/executing.
    ShellInput,
    // Directory Hotlist
    HotlistDialog(HotlistDialogState),
    /// Background jobs list dialog (C-x j).
    JobsDialog {
        /// Selected row in the jobs list.
        selected_index: usize,
        /// Which part of the dialog is focused (list or which button).
        focus: JobsDialogFocus,
    },
    /// GNU mc-style Compare directories dialog
    CompareDirsDialog {
        mode: CompareDirsMode,
        focus: CompareDirsFocus,
    },
    /// GNU mc Options → Layout dialog
    LayoutDialog {
        draft: LayoutOptions,
        focus: LayoutFocus,
    },
    /// GNU mc Options → Confirmations dialog
    ConfirmationsDialog {
        draft: ConfirmOptions,
        focus: ConfirmationsFocus,
    },
    /// GNU mc Options → Panel options dialog
    PanelOptionsDialog {
        draft: PanelOptions,
        focus: PanelOptionsFocus,
    },
    /// GNU mc Options → Learn keys dialog
    LearnKeysDialog {
        /// Draft bindings for the limited set of actions we expose.
        draft: Vec<(crate::actions::Action, KeyEvent)>,
        /// Selected row index; when equal to draft.len(), the bottom buttons are focused.
        selected: usize,
        /// True while waiting for the next key press to assign to the selected row.
        capturing: bool,
        /// When buttons are focused, true => OK, false => Cancel.
        focus_ok: bool,
    },
    /// GNU mc Options → Appearance dialog
    AppearanceDialog {
        /// Working copy of selected skin name
        draft_skin: String,
        /// Working copy of shadow toggle
        draft_shadows: bool,
        /// Available skin names without .ini extension (always contains "default")
        skins: Vec<String>,
        /// Selected row in the skin list
        selected: usize,
        /// Which UI element is focused
        focus: AppearanceFocus,
    },
    /// GNU mc Options → Configuration dialog
    ConfigurationDialog {
        draft: ConfigOptions,
        focus: ConfigOptionsFocus,
    },
    /// GNU mc Options → Virtual FS dialog
    VfsOptionsDialog {
        draft: VfsOptions,
        focus: VfsOptionsFocus,
    },
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

#[derive(Clone)]
pub struct HelpState {
    pub topic: String,        // current node name
    pub cursor: usize,        // index within current node links
    pub scroll_top: usize,    // first visible content line
    pub history: Vec<String>, // simple back stack
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
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ConfirmationsFocus {
    Delete,
    Overwrite,
    Execute,
    Exit,
    DirectoryHotlist,
    HistoryCleanup,
    Ok,
    Cancel,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PanelOptionsFocus {
    ShowHidden,
    MixAllFiles,
    MarkMovesDown,
    ShowMiniStatus,
    UseSiUnits,
    FastReload,
    ReverseFilesOnly,
    SimpleSwap,
    AutoSaveSetup,
    LynxLikeMotion,
    Ok,
    Cancel,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AppearanceFocus {
    SkinList,
    Shadows,
    Ok,
    Cancel,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum VfsOptionsFocus {
    AlwaysUseFtpProxy,
    FtpProxyHost,
    UseNetrc,
    FtpAnonPassword,
    DirCacheTimeout,
    Ok,
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
    /// MC-style incremental quick search state for the active panel.
    pub quick_search: Option<crate::quicksearch::QuickSearchState>,
    /// Background job queue (copy/move on worker thread).
    pub jobs: crate::jobs::JobQueue,
    /// Options controlling UI chrome visibility/layout.
    pub layout: LayoutOptions,
    /// Options controlling confirmation prompts (GNU mc-style).
    pub confirm: ConfirmOptions,
    /// GNU mc-style Options → Panels
    pub panel_opts: PanelOptions,
    /// GNU mc-style Options → Configuration
    pub config_opts: ConfigOptions,
    /// GNU mc-style Options → Virtual FS
    pub vfs_opts: VfsOptions,
    /// Selected skin name (e.g., "default")
    pub skin_name: String,
    /// Whether to draw drop shadows for dialogs/menus
    pub shadows: bool,
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
            quick_search: None,
            jobs: crate::jobs::JobQueue::new(),
            layout: LayoutOptions::default(),
            confirm: ConfirmOptions::default(),
            panel_opts: PanelOptions::default(),
            config_opts: ConfigOptions::default(),
            vfs_opts: VfsOptions::default(),
            skin_name: "default".to_string(),
            shadows: true,
        };
        // Overlay user setup (if available) over defaults, then refresh panels.
        let _ = crate::config::load_user_setup(&mut app);
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
        let reverse_files_only = self.panel_opts.reverse_files_only;
        let left = self.vfs.list_dir(&self.left.cwd, self.show_hidden)?;
        let right = self.vfs.list_dir(&self.right.cwd, self.show_hidden)?;
        self.left
            .set_entries_with(self.map_dir_entries(left), reverse_files_only);
        self.right
            .set_entries_with(self.map_dir_entries(right), reverse_files_only);
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
            Quit => {
                if self.confirm.exit {
                    self.ui_mode = UiMode::DialogConfirm {
                        title: "Confirmation".to_string(),
                        message: "Are you sure you want to quit?".to_string(),
                        on_ok: Box::new(|app| {
                            if app.panel_opts.auto_save_setup {
                                let _ = crate::config::save_setup(app);
                            }
                            app.quit = true;
                            Ok(())
                        }),
                    };
                } else {
                    if self.panel_opts.auto_save_setup {
                        let _ = crate::config::save_setup(self);
                    }
                    self.quit = true;
                }
            }
            Refresh => {
                self.reload_panels()?;
            }
            ToggleSubshell => {
                // Toggle full-screen subshell/output view.
                self.subshell.toggle_output_screen();
                // Ensure we leave any transient prompt/dialog modes.
                self.ui_mode = UiMode::Normal;
            }
            CycleListingFormat => {
                use crate::panel::ListingFormat::*;
                let p = self.active_panel_mut();
                p.listing = match p.listing {
                    Full => Brief,
                    Brief => Long,
                    Long | User => Full,
                };
            }
            ToggleHidden => {
                self.show_hidden = !self.show_hidden;
                // Keep Options → Panels in sync with Ctrl-H
                self.panel_opts.show_hidden = self.show_hidden;
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
                // GNU mc "Simple swap": keep focus on the same side (now showing the other dir).
                if !self.panel_opts.simple_swap {
                    self.active = match self.active {
                        PaneSide::Left => PaneSide::Right,
                        PaneSide::Right => PaneSide::Left,
                    };
                }
            }
            FocusMenu => {
                self.ui_mode = UiMode::Menu {
                    top_index: 0,
                    selected_index: 0,
                }
            }
            ShowHelp => {
                let prev = std::mem::replace(&mut self.ui_mode, UiMode::MenuFocused);
                let topic = Self::context_help_topic_from_mode(&prev);
                self.ui_mode = UiMode::Help {
                    state: HelpState {
                        topic,
                        cursor: 0,
                        scroll_top: 0,
                        history: Vec::new(),
                    },
                    prev: Box::new(prev),
                };
            }
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
                            // No-op here; UI Enter path may Open via mc.ext after VFS/exe fail.
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
                // GNU mc: when enabled, marking moves the cursor down
                if self.panel_opts.mark_moves_down {
                    self.active_panel_mut().move_down();
                }
            }
            Sort(sb) => {
                let (by, _dir) = match sb {
                    SortByAction::Name => (SortBy::Name, self.active_panel().sort_dir),
                    SortByAction::Ext => (SortBy::Ext, self.active_panel().sort_dir),
                    SortByAction::Size => (SortBy::Size, self.active_panel().sort_dir),
                    SortByAction::Time => (SortBy::Time, self.active_panel().sort_dir),
                };
                let reverse_files_only = self.panel_opts.reverse_files_only;
                let p = self.active_panel_mut();
                p.sort_by = by;
                p.apply_sort_with(reverse_files_only);
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
                            goto_prompt: None,
                            show_line_numbers: false,
                            show_cr: false,
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
        let reverse_files_only = self.panel_opts.reverse_files_only;
        // Acquire listing before mutably borrowing panel to avoid aliasing
        let list = self.vfs.list_dir(&new_cwd, self.show_hidden)?;
        let entries = self.map_dir_entries(list);
        let p = self.active_panel_mut();
        p.cwd = new_cwd;
        p.set_entries_with(entries, reverse_files_only);
        Ok(())
    }
}

impl App {
    fn context_help_topic_from_mode(mode: &UiMode) -> String {
        match mode {
            UiMode::Normal | UiMode::Menu { .. } => "Panels".to_string(),
            UiMode::Viewer { .. } => "Viewer".to_string(),
            UiMode::Diff(_) => "Diff".to_string(),
            UiMode::UserMenu { .. } => "User Menu".to_string(),
            UiMode::SortDialog { .. } => "Panels".to_string(),
            UiMode::ListingModeDialog { .. } => "Panels".to_string(),
            UiMode::Editor { .. } => "Editor".to_string(),
            UiMode::FindDialog(_) => "Find File".to_string(),
            UiMode::CopyDialog { title, .. } => {
                if title == "Copy" {
                    "Copy".to_string()
                } else {
                    "Move".to_string()
                }
            }
            UiMode::OverwriteDialog { op, .. } => match op {
                CopyMoveOp::Copy => "Copy".to_string(),
                CopyMoveOp::Move => "Move".to_string(),
            },
            UiMode::MkdirDialog { .. } => "Mkdir".to_string(),
            UiMode::DeleteDialog { .. } => "Delete".to_string(),
            UiMode::DialogConfirm { .. } => "Confirmations".to_string(),
            UiMode::PromptInput { .. } => "Prompts".to_string(),
            UiMode::InputDialog { .. } => "Prompts".to_string(),
            UiMode::FtpConnectDialog { .. } => "FTP".to_string(),
            UiMode::ChmodDialog { .. } => "Chmod".to_string(),
            UiMode::ChownDialog { .. } => "Chown".to_string(),
            UiMode::MenuFocused => "Menus".to_string(),
            UiMode::Help { state, .. } => state.topic.clone(),
            UiMode::ShellInput => "Panels".to_string(),
            UiMode::HotlistDialog(_) => "Panels".to_string(),
            UiMode::JobsDialog { .. } => "Panels".to_string(),
            UiMode::CompareDirsDialog { .. } => "Panels".to_string(),
            UiMode::LayoutDialog { .. } => "Panels".to_string(),
            UiMode::ConfirmationsDialog { .. } => "Panels".to_string(),
            UiMode::PanelOptionsDialog { .. } => "Panels".to_string(),
            UiMode::LearnKeysDialog { .. } => "Panels".to_string(),
            UiMode::AppearanceDialog { .. } => "Panels".to_string(),
            UiMode::ConfigurationDialog { .. } => "Panels".to_string(),
            UiMode::VfsOptionsDialog { .. } => "Panels".to_string(),
        }
    }
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
        let reverse_files_only = self.panel_opts.reverse_files_only;
        self.active_panel_mut()
            .set_panelized_entries_with(caption, entries, reverse_files_only);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::KeyMap;
    use rmc_fs::local::LocalFs;

    fn app_with_distinct_panes() -> (App, tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let left_dir = tmp.path().join("left");
        let right_dir = tmp.path().join("right");
        std::fs::create_dir(&left_dir).unwrap();
        std::fs::create_dir(&right_dir).unwrap();
        let vfs = LocalFs::new();
        let mut app = App::new(Box::new(vfs), KeyMap::mc_defaults()).unwrap();
        app.change_dir(&left_dir).unwrap();
        app.active = PaneSide::Right;
        app.change_dir(&right_dir).unwrap();
        app.active = PaneSide::Left;
        (app, tmp, left_dir, right_dir)
    }

    #[test]
    fn swap_panels_flips_active_by_default() {
        let (mut app, _tmp, left_dir, right_dir) = app_with_distinct_panes();
        app.panel_opts.simple_swap = false;
        app.handle_action(Action::SwapPanels).unwrap();
        assert_eq!(app.left.cwd, right_dir);
        assert_eq!(app.right.cwd, left_dir);
        assert_eq!(app.active, PaneSide::Right);
    }

    #[test]
    fn simple_swap_keeps_active_side() {
        let (mut app, _tmp, left_dir, right_dir) = app_with_distinct_panes();
        app.panel_opts.simple_swap = true;
        app.handle_action(Action::SwapPanels).unwrap();
        assert_eq!(app.left.cwd, right_dir);
        assert_eq!(app.right.cwd, left_dir);
        assert_eq!(app.active, PaneSide::Left);
    }
}
