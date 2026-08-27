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
    pub show_hidden: bool,     // default false — match current App.show_hidden
    pub mix_all_files: bool,   // default false — match current dirs_first=true
    pub mark_moves_down: bool, // default true — GNU mc Insert-mark then cursor down
    /// GNU mc Options → Panels → Show mini-status. When true (default), draw the
    /// mini-status line at the bottom of each listing panel (perms/owner/group/
    /// size/mtime). When false, omit that row so the listing uses the extra line.
    /// Quick search still uses the row on the active panel.
    pub show_mini_status: bool,
    pub kilobyte_si: bool, // default false; panel/mini-status SI (1000) vs 1024 units
    /// Skip local panel re-list when the directory mtime/ctime/nlink/size is unchanged.
    /// Default false. C-r / Refresh always re-lists. Remote/archive/extfs use dir cache timeout.
    pub fast_reload: bool,
    pub reverse_files_only: bool, // default true — dirs-first reverse: files only (dirs stay name-asc)
    pub simple_swap: bool,        // default false — swap panes without flipping active
    /// GNU mc Options → Panels → Auto save setup. Default false.
    /// When true, Quit calls [`crate::config::save_setup`].
    pub auto_save_setup: bool,
    pub lynx_like: bool, // default false — Left=parent, Right=enter in listing
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
    /// GNU mc Options → Configuration → Verbose operation. Default **true**.
    /// When true, Copy/Move progress shows the current file name. When false,
    /// the per-file name is omitted (totals/bar only).
    pub verbose: bool,
    /// GNU mc Options → Configuration → Compute totals. Default **true**.
    /// When true, Copy/Move pre-scans total size/count so the progress bar has
    /// a real denominator. When false, skip the pre-scan (progress may be
    /// indeterminate / file-by-file only).
    pub compute_totals: bool,
    /// GNU mc Options → Configuration → Classic progressbar. Default **true**.
    /// When true, Copy/Move uses GNU mc's classic one-line `****` bar. When
    /// false, use two bars (File + Total).
    pub classic_progressbar: bool,
    pub use_internal_view: bool, // default true — keep F3 internal viewer by default
    pub use_internal_edit: bool, // default true — keep F4 internal editor by default
    /// After a waited external command (Enter-execute, external F3/$PAGER), show a
    /// "Press any key to continue..." prompt before panels redraw. Default false.
    /// Does not apply to fire-and-forget desktop open (`xdg-open` `.spawn()`).
    pub pause_after_run: bool,
    /// GNU mc Options → Configuration → Use shell patterns. When true (default),
    /// Select group, Unselect group, and the panel filter treat patterns as
    /// shell globs (`*.c`, `?`, `[abc]`). When false, those patterns are regexes.
    pub shell_patterns: bool,
    /// GNU mc Options → Configuration → Auto menus. When true, a successful
    /// `change_dir` into a different directory that contains a local `.mc.menu`
    /// opens the User menu (same as F2). Default false. Reload, C-r, panelize,
    /// and same-cwd `change_dir` do not auto-open.
    pub auto_menus: bool,
    /// GNU mc Options → Configuration → Drop down menus. When true, F9
    /// immediately opens the current top-level pull-down (item 0 selected).
    /// When false (default), F9 highlights the menu bar until Down or Enter.
    pub drop_menus: bool,
    /// GNU mc Options → Configuration → Mkdir autoname. When true, F7 prefills
    /// the Mkdir name with the current panel entry (not `..`). Default false.
    pub mkdir_autoname: bool,
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
    /// Directory cache timeout in seconds (GNU mc Virtual FS). Pushed into the
    /// VFS on panel reload and cwd change so remote/archive/extfs listings honor it.
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

#[allow(clippy::large_enum_variant)] // Editor owns EditorBuffer; overlays are boxed where needed
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
        /// GNU mcview F9 format/unformat (nroff overstrike). Off = unformat.
        format_nroff: bool,
        /// GNU mcview F8 Parsed vs Raw. True shows mc.ext `[view]` filter output.
        parsed: bool,
        /// Selection anchor byte offset (None = no highlight). Cursor is `sel_cursor`.
        sel_anchor: Option<u64>,
        /// Selection/cursor byte offset (Shift+arrows extend from `sel_anchor`).
        sel_cursor: u64,
        /// Viewer File/Command/Options pull-down (GNU: click the topmost line).
        viewer_menu: Option<ViewerMenu>,
        /// GNU mcview F7 Search dialog (None while viewing).
        search_dialog: Option<Box<ViewerSearchDialog>>,
        /// Viewer display-options dialog (Options → Display options). None while viewing.
        display_dialog: Option<Box<ViewerDisplayDialog>>,
        /// Last Search-dialog flags, used by n / F17.
        search_case_sensitive: bool,
        search_backwards: bool,
        search_whole_words: bool,
        search_regexp: bool,
        status_msg: Option<String>,
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
        /// GNU mcedit F9 pull-down. `None` while editing; `Some` drops File/Edit/Search/Command/Options.
        show_menu: Option<EditorMenu>,
        status_msg: Option<String>,
        search_input: Option<String>,
        /// GNU mcedit Save as dialog (F12 / Shift-F2). None while editing.
        save_as_dialog: Option<Box<EditorSaveAsDialog>>,
        /// GNU mcedit F7 Search dialog (None while editing).
        search_dialog: Option<Box<EditorSearchDialog>>,
        /// GNU mcedit F4 Replace dialog (None while editing).
        replace_dialog: Option<Box<EditorReplaceDialog>>,
        /// GNU mcedit `|` Pipe dialog (None while editing).
        pipe_dialog: Option<EditorPipeDialog>,
        /// GNU mcedit Alt-l Goto line dialog (None while editing).
        goto_dialog: Option<Box<EditorGotoDialog>>,
        /// GNU mcedit Options → Tab spacing dialog (None while editing).
        tab_spacing_dialog: Option<Box<EditorTabSpacingDialog>>,
        pending_quit: bool,
        confirm_exit: Option<YncDialog>,
        /// When the editor was opened from mcdiff, restore this mode on quit
        /// (Help uses the same nesting). Diff hunks are rebuilt from disk.
        return_to: Option<Box<UiMode>>,
    },
    Menu {
        top_index: usize,
        selected_index: usize,
        /// False: menu-bar highlight only (GNU mc drop_menus off).
        /// True: pull-down is open with `selected_index` on an item.
        dropped: bool,
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
    /// GNU mc file-operations progress dialog (Copy/Move).
    FileOpProgress {
        op: CopyMoveOp,
        src: PathBuf,
        dst: PathBuf,
        state: crate::fileop::FileOpProgressState,
        /// True once the transfer has been queued on [`crate::jobs::JobQueue`].
        started: bool,
        job_id: crate::jobs::JobId,
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
    /// GNU mc command-line History list (Alt-h / M-h while the input line has focus).
    HistoryDialog {
        selected_index: usize,
        scroll_top: usize,
        focus: HistoryDialogFocus,
        /// When true, show the GNU mc "clean this history?" confirmation.
        confirm_clean: bool,
    },
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
    /// Pause after a waited external command when `config_opts.pause_after_run`.
    /// Any key dismisses so the panels can redraw. Not used for fire-and-forget
    /// desktop open.
    PauseAfterRun,
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

/// Focus within the GNU mcedit F7 Search dialog.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum EditorSearchFocus {
    #[default]
    Search,
    CaseSensitive,
    Backwards,
    WholeWords,
    RegularExpression,
    Ok,
    Cancel,
}

impl EditorSearchFocus {
    /// True when focus is one of the four GNU Search checkboxes.
    pub fn is_checkbox(self) -> bool {
        matches!(
            self,
            Self::CaseSensitive | Self::Backwards | Self::WholeWords | Self::RegularExpression
        )
    }
}

/// GNU mcedit-style Search dialog: needle field, four option checkboxes,
/// and OK / Cancel. Defaults match GNU (all checkboxes off).
#[derive(Clone)]
pub struct EditorSearchDialog {
    pub search: String,
    pub case_sensitive: bool,
    pub backwards: bool,
    pub whole_words: bool,
    pub regular_expression: bool,
    pub focus: EditorSearchFocus,
}

/// GNU mcview F7 Search dialog: same chrome, labels, and defaults as mcedit Search
/// (title ` Search `, prompt `Enter search string:`, four checkboxes all off).
/// All charsets is omitted, as on editor Search.
pub type ViewerSearchDialog = EditorSearchDialog;
/// Focus order matches editor Search: field → four checkboxes → OK → Cancel.
pub type ViewerSearchFocus = EditorSearchFocus;

/// GNU mcview top-line menus (File / Command / Options). Opened by clicking
/// the topmost line — the same path mc(1) documents for the menu bar.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ViewerMenu {
    File { selected: usize },
    Command { selected: usize },
    Options { selected: usize },
}

impl ViewerMenu {
    pub fn selected(self) -> usize {
        match self {
            Self::File { selected } | Self::Command { selected } | Self::Options { selected } => {
                selected
            }
        }
    }

    pub fn with_selected(self, selected: usize) -> Self {
        match self {
            Self::File { .. } => Self::File { selected },
            Self::Command { .. } => Self::Command { selected },
            Self::Options { .. } => Self::Options { selected },
        }
    }

    /// Item labels for the dropped menu. Options contains Display options
    /// (GNU mcview Options menu — F9 itself is format/unformat).
    pub fn items(self) -> &'static [&'static str] {
        match self {
            Self::File { .. } => &["Quit"],
            Self::Command { .. } => &["Search"],
            Self::Options { .. } => &["Display options"],
        }
    }
}

/// GNU mcedit F9 pull-down titles (mcedit(1) / mc(1) Internal File Editor).
/// Items are only those with existing editor actions — no stub labels.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EditorMenu {
    File { selected: usize },
    Edit { selected: usize },
    Search { selected: usize },
    Command { selected: usize },
    Options { selected: usize },
}

impl EditorMenu {
    /// Packed ` File  Edit  Search  Command  Options ` (GNU mcedit menu bar).
    pub const TITLES: [&'static str; 5] =
        [" File ", " Edit ", " Search ", " Command ", " Options "];

    /// F9 drops the File menu with Save selected (first item).
    pub fn default_open() -> Self {
        Self::File { selected: 0 }
    }

    pub fn selected(self) -> usize {
        match self {
            Self::File { selected }
            | Self::Edit { selected }
            | Self::Search { selected }
            | Self::Command { selected }
            | Self::Options { selected } => selected,
        }
    }

    pub fn index(self) -> usize {
        match self {
            Self::File { .. } => 0,
            Self::Edit { .. } => 1,
            Self::Search { .. } => 2,
            Self::Command { .. } => 3,
            Self::Options { .. } => 4,
        }
    }

    pub fn from_index(index: usize, selected: usize) -> Self {
        match index % 5 {
            0 => Self::File { selected },
            1 => Self::Edit { selected },
            2 => Self::Search { selected },
            3 => Self::Command { selected },
            _ => Self::Options { selected },
        }
    }

    pub fn with_selected(self, selected: usize) -> Self {
        let n = self.items().len();
        let selected = if n == 0 { 0 } else { selected.min(n - 1) };
        match self {
            Self::File { .. } => Self::File { selected },
            Self::Edit { .. } => Self::Edit { selected },
            Self::Search { .. } => Self::Search { selected },
            Self::Command { .. } => Self::Command { selected },
            Self::Options { .. } => Self::Options { selected },
        }
    }

    /// Item labels under the current title. Options wires Auto indent (toggle)
    /// and Tab spacing (mcedit(1) `editor_tab_spacing` dialog).
    pub fn items(self) -> &'static [&'static str] {
        match self {
            Self::File { .. } => &["Save", "Save as", "Quit"],
            Self::Edit { .. } => &["Undo", "Copy", "Move", "Delete", "Mark"],
            Self::Search { .. } => &["Search", "Replace"],
            Self::Command { .. } => &["Go to line", "Pipe"],
            Self::Options { .. } => &["Auto indent", "Tab spacing"],
        }
    }

    pub fn left(self) -> Self {
        Self::from_index(self.index() + 4, 0)
    }

    pub fn right(self) -> Self {
        Self::from_index(self.index() + 1, 0)
    }

    pub fn up(self) -> Self {
        let n = self.items().len();
        if n == 0 {
            return self;
        }
        self.with_selected((self.selected() + n - 1) % n)
    }

    pub fn down(self) -> Self {
        let n = self.items().len();
        if n == 0 {
            return self;
        }
        self.with_selected((self.selected() + 1) % n)
    }

    /// Label of the highlighted item, if this menu has any.
    pub fn current_item(self) -> Option<&'static str> {
        self.items().get(self.selected()).copied()
    }
}

/// Focus within the mcview display-options dialog (Options → Display options).
/// Checkboxes first, then OK / Cancel — same cycle as Search.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum ViewerDisplayFocus {
    #[default]
    ShowLineNumbers,
    ShowCr,
    WrapMode,
    HexMode,
    Ok,
    Cancel,
}

impl ViewerDisplayFocus {
    /// True when focus is one of the display-option checkboxes.
    pub fn is_checkbox(self) -> bool {
        matches!(
            self,
            Self::ShowLineNumbers | Self::ShowCr | Self::WrapMode | Self::HexMode
        )
    }
}

/// mcview display-options dialog: checkboxes for GNU Internal File Viewer
/// display flags (line numbers, CR as ^M, wrap, hex). Title ` Display options `.
/// Checkboxes seed from the current Viewer flags (unlike Search, which resets).
#[derive(Clone)]
pub struct ViewerDisplayDialog {
    pub show_line_numbers: bool,
    pub show_cr: bool,
    pub wrap: bool,
    pub hex: bool,
    pub focus: ViewerDisplayFocus,
}

impl ViewerDisplayDialog {
    /// Seed checkboxes from the live Viewer flags. Focus starts on the first checkbox.
    pub fn from_viewer(show_line_numbers: bool, show_cr: bool, wrap: bool, hex: bool) -> Self {
        Self {
            show_line_numbers,
            show_cr,
            wrap,
            hex,
            focus: ViewerDisplayFocus::ShowLineNumbers,
        }
    }

    /// Toggle the focused checkbox. Returns false when focus is not a checkbox.
    pub fn toggle_focused_checkbox(&mut self) -> bool {
        match self.focus {
            ViewerDisplayFocus::ShowLineNumbers => self.show_line_numbers = !self.show_line_numbers,
            ViewerDisplayFocus::ShowCr => self.show_cr = !self.show_cr,
            ViewerDisplayFocus::WrapMode => self.wrap = !self.wrap,
            ViewerDisplayFocus::HexMode => self.hex = !self.hex,
            _ => return false,
        }
        true
    }
}

impl EditorSearchDialog {
    /// Prefill the search field from the editor's last Search needle.
    /// Checkboxes always start at GNU defaults (unchecked).
    pub fn from_last_search(last_search: &[u8]) -> Self {
        Self {
            search: String::from_utf8_lossy(last_search).into_owned(),
            case_sensitive: false,
            backwards: false,
            whole_words: false,
            regular_expression: false,
            focus: EditorSearchFocus::Search,
        }
    }

    /// Toggle the focused checkbox. Returns false when focus is not a checkbox.
    pub fn toggle_focused_checkbox(&mut self) -> bool {
        match self.focus {
            EditorSearchFocus::CaseSensitive => self.case_sensitive = !self.case_sensitive,
            EditorSearchFocus::Backwards => self.backwards = !self.backwards,
            EditorSearchFocus::WholeWords => self.whole_words = !self.whole_words,
            EditorSearchFocus::RegularExpression => {
                self.regular_expression = !self.regular_expression
            }
            _ => return false,
        }
        true
    }
}

/// Focus within the GNU mcedit F4 Replace dialog.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum EditorReplaceFocus {
    #[default]
    Search,
    Replacement,
    CaseSensitive,
    Backwards,
    WholeWords,
    RegularExpression,
    Replace,
    All,
    Skip,
    Cancel,
}

impl EditorReplaceFocus {
    /// True when focus is one of the four GNU Replace checkboxes.
    pub fn is_checkbox(self) -> bool {
        matches!(
            self,
            Self::CaseSensitive | Self::Backwards | Self::WholeWords | Self::RegularExpression
        )
    }

    /// True when focus is Replace / All / Skip / Cancel.
    pub fn is_button(self) -> bool {
        matches!(self, Self::Replace | Self::All | Self::Skip | Self::Cancel)
    }
}

/// GNU mcedit-style Replace dialog: Search plus a replacement field, the same
/// four option checkboxes (all default off), and Replace / All / Skip / Cancel.
///
/// GNU mcedit puts Skip on the “replace this occurrence?” prompt after a match
/// is found. This replica keeps a single combined dialog (already Replace / All
/// / Cancel) and adds Skip there. All charsets is omitted, as on Search.
/// Replace next keeps the dialog open; All closes after reporting the count.
#[derive(Clone)]
pub struct EditorReplaceDialog {
    pub search: String,
    pub replacement: String,
    pub case_sensitive: bool,
    pub backwards: bool,
    pub whole_words: bool,
    pub regular_expression: bool,
    pub focus: EditorReplaceFocus,
    /// True after Skip landed on a match so the next Skip advances past it.
    pub on_match: bool,
}

impl EditorReplaceDialog {
    /// Prefill the search field from the editor's last Search (F7) needle.
    /// Checkboxes always start at GNU defaults (unchecked). Replacement starts empty.
    pub fn from_last_search(last_search: &[u8]) -> Self {
        Self {
            search: String::from_utf8_lossy(last_search).into_owned(),
            replacement: String::new(),
            case_sensitive: false,
            backwards: false,
            whole_words: false,
            regular_expression: false,
            focus: EditorReplaceFocus::Search,
            on_match: false,
        }
    }

    /// Toggle the focused checkbox. Returns false when focus is not a checkbox.
    pub fn toggle_focused_checkbox(&mut self) -> bool {
        match self.focus {
            EditorReplaceFocus::CaseSensitive => self.case_sensitive = !self.case_sensitive,
            EditorReplaceFocus::Backwards => self.backwards = !self.backwards,
            EditorReplaceFocus::WholeWords => self.whole_words = !self.whole_words,
            EditorReplaceFocus::RegularExpression => {
                self.regular_expression = !self.regular_expression
            }
            _ => return false,
        }
        self.on_match = false;
        true
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum EditorPipeFocus {
    #[default]
    Command,
    Ok,
    Cancel,
}

/// GNU mcedit-style Pipe dialog: filter the selection (or whole buffer)
/// through `sh -c <command>` and replace it with stdout.
#[derive(Clone, Default)]
pub struct EditorPipeDialog {
    pub command: String,
    pub focus: EditorPipeFocus,
}

/// Focus within the GNU mcedit Alt-l Goto line dialog.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum EditorGotoFocus {
    #[default]
    Line,
    Ok,
    Cancel,
}

/// GNU mcedit-style Goto line dialog: 1-based line field and OK / Cancel.
#[derive(Clone)]
pub struct EditorGotoDialog {
    pub line: String,
    pub focus: EditorGotoFocus,
}

impl EditorGotoDialog {
    /// Prefill the line field from the buffer's current 0-based cursor row.
    pub fn from_cursor_row(row: usize) -> Self {
        Self {
            line: (row + 1).to_string(),
            focus: EditorGotoFocus::Line,
        }
    }
}

/// Focus within the GNU mcedit Options → Tab spacing dialog.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum EditorTabSpacingFocus {
    #[default]
    Width,
    Ok,
    Cancel,
}

/// GNU mcedit-style Tab spacing dialog (mcedit(1) `editor_tab_spacing`).
/// Title ` Tab spacing `, prompt `Enter tab spacing:`.
#[derive(Clone)]
pub struct EditorTabSpacingDialog {
    pub width: String,
    pub focus: EditorTabSpacingFocus,
}

impl EditorTabSpacingDialog {
    /// Prefill from the buffer's current tab width.
    pub fn from_tab_width(width: usize) -> Self {
        Self {
            width: width.to_string(),
            focus: EditorTabSpacingFocus::Width,
        }
    }
}

/// Focus within the GNU mcedit Save as dialog.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum EditorSaveAsFocus {
    #[default]
    Filename,
    Ok,
    Cancel,
}

/// GNU mcedit-style Save as dialog: filename field and OK / Cancel.
/// Title ` Save as `, prompt `Enter file name:` (public mcedit File menu / Save as).
#[derive(Clone)]
pub struct EditorSaveAsDialog {
    pub filename: String,
    pub focus: EditorSaveAsFocus,
    /// GNU overwrite confirm when the destination already exists.
    pub overwrite: Option<YncDialog>,
}

impl EditorSaveAsDialog {
    /// Prefill the filename field from the buffer's current path (empty if unnamed).
    pub fn from_buffer_path(path: Option<&Path>) -> Self {
        Self {
            filename: path
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
            focus: EditorSaveAsFocus::Filename,
            overwrite: None,
        }
    }
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
pub enum HistoryDialogFocus {
    /// The history list has focus; Up/Down change the selected row.
    List,
    Ok,
    Cancel,
    /// GNU mc History widget "Clear" (not a generic "Delete all").
    Clear,
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

    /// Push live Options → Virtual FS directory cache timeout into the VFS.
    fn sync_vfs_dir_cache_timeout(&self) {
        self.vfs
            .set_dir_cache_timeout_secs(self.vfs_opts.dir_cache_timeout_secs);
    }

    pub fn reload_panels(&mut self) -> Result<()> {
        self.reload_panels_impl(false)
    }

    /// Re-list both panels. `force` (C-r / Refresh) always calls `list_dir`, even when
    /// Fast reload is on and the local directory stamp is unchanged.
    fn reload_panels_impl(&mut self, force: bool) -> Result<()> {
        self.sync_vfs_dir_cache_timeout();
        let reverse_files_only = self.panel_opts.reverse_files_only;
        let shell_patterns = self.config_opts.shell_patterns;
        let show_hidden = self.show_hidden;
        let fast = self.panel_opts.fast_reload;

        let left_cwd = self.left.cwd.clone();
        if force || !fast || !self.left.fast_reload_listing_is_current(show_hidden) {
            let left = self.vfs.list_dir(&left_cwd, show_hidden)?;
            self.left.set_entries_with(
                self.map_dir_entries(left),
                reverse_files_only,
                shell_patterns,
            );
            self.left.capture_dir_reload_stamp(show_hidden);
        }

        let right_cwd = self.right.cwd.clone();
        if force || !fast || !self.right.fast_reload_listing_is_current(show_hidden) {
            let right = self.vfs.list_dir(&right_cwd, show_hidden)?;
            self.right.set_entries_with(
                self.map_dir_entries(right),
                reverse_files_only,
                shell_patterns,
            );
            self.right.capture_dir_reload_stamp(show_hidden);
        }
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
                nlink: e.meta.nlink,
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
                // C-r: force re-list even when Fast reload or the VFS dir cache is still fresh.
                let left = self.left.cwd.clone();
                let right = self.right.cwd.clone();
                self.vfs.invalidate_dir_cache(Some(&left));
                self.vfs.invalidate_dir_cache(Some(&right));
                self.reload_panels_impl(true)?;
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
                        app.apply_group_pattern(pattern.trim(), true);
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
                        app.apply_group_pattern(pattern.trim(), false);
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
                    dropped: self.config_opts.drop_menus,
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
                        self.ui_mode = UiMode::new_viewer(ent.path.clone());
                    }
                }
            }
            Copy | Move | Mkdir | Delete => {
                // UI layer opens dialogs; core provides helpers
            }
            ShowUserMenu => self.try_open_user_menu(),
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
        let cwd_changed = self.active_panel().cwd != new_cwd;
        let reverse_files_only = self.panel_opts.reverse_files_only;
        let shell_patterns = self.config_opts.shell_patterns;
        let auto_menus = self.config_opts.auto_menus;
        self.sync_vfs_dir_cache_timeout();
        // Re-entering a remote/archive/extfs dir within the timeout reuses the
        // cached listing (GNU mc). C-r / Refresh is the force-reload.
        // Acquire listing before mutably borrowing panel to avoid aliasing
        let list = self.vfs.list_dir(&new_cwd, self.show_hidden)?;
        let entries = self.map_dir_entries(list);
        let show_hidden = self.show_hidden;
        let p = self.active_panel_mut();
        p.cwd = new_cwd.clone();
        p.set_entries_with(entries, reverse_files_only, shell_patterns);
        p.capture_dir_reload_stamp(show_hidden);
        // Auto menus: only after a real directory change, and only for a local
        // `.mc.menu` in the new panel cwd (not a parent, not reload/C-r/panelize).
        if cwd_changed && auto_menus {
            self.try_auto_open_local_user_menu(&new_cwd);
        }
        Ok(())
    }

    /// Open the User menu the same way F2 does (`load_menu` fallbacks). Missing
    /// menu is a no-op.
    fn try_open_user_menu(&mut self) {
        let cwd = self.active_panel().cwd.clone();
        if let Ok(menu) = crate::user_menu::load_menu(&cwd) {
            self.open_user_menu(menu);
        }
    }

    /// Auto menus: open User menu only when `cwd/.mc.menu` itself is present
    /// and readable. Does not walk parents or fall back to `~/.config/mc/menu`.
    fn try_auto_open_local_user_menu(&mut self, cwd: &Path) {
        if let Some(menu) = crate::user_menu::try_load_local_menu(cwd) {
            self.open_user_menu(menu);
        }
    }

    fn open_user_menu(&mut self, menu: crate::user_menu::UserMenu) {
        self.ui_mode = UiMode::UserMenu {
            title: menu.title,
            entries: menu.entries,
            selected_index: 0,
        };
    }

    /// Select (`select == true`) or unselect files whose names match `pattern`.
    /// Honors Options → Configuration → Use shell patterns. Skips `..`.
    fn apply_group_pattern(&mut self, pattern: &str, select: bool) {
        let shell_patterns = self.config_opts.shell_patterns;
        let matches: Vec<usize> = self
            .active_panel()
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| !e.is_parent_marker())
            .filter(|(_, e)| {
                crate::matchutil::filename_pattern_matches(pattern, &e.name, shell_patterns)
            })
            .map(|(i, _)| i)
            .collect();
        let panel = self.active_panel_mut();
        for idx in matches {
            if select {
                panel.selection.select(idx);
            } else {
                panel.selection.unselect(idx);
            }
        }
    }
}

impl UiMode {
    /// Internal viewer with GNU mcview defaults (parsed on, format off, no selection).
    pub fn new_viewer(path: PathBuf) -> Self {
        UiMode::Viewer {
            path,
            hex: false,
            wrap: false,
            offset: 0,
            search: None,
            search_prompt: None,
            goto_prompt: None,
            show_line_numbers: false,
            show_cr: false,
            format_nroff: false,
            parsed: true,
            sel_anchor: None,
            sel_cursor: 0,
            viewer_menu: None,
            search_dialog: None,
            display_dialog: None,
            search_case_sensitive: false,
            search_backwards: false,
            search_whole_words: false,
            search_regexp: false,
            status_msg: None,
        }
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
            UiMode::FileOpProgress { op, .. } => match op {
                CopyMoveOp::Copy => "Copy".to_string(),
                CopyMoveOp::Move => "Move".to_string(),
            },
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
            UiMode::HistoryDialog { .. } => "Panels".to_string(),
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
            UiMode::PauseAfterRun => "Panels".to_string(),
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
            nlink: 1,
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
                nlink: meta.nlink,
            });
        }
        let caption = self.active_panel().cwd.clone();
        let reverse_files_only = self.panel_opts.reverse_files_only;
        let shell_patterns = self.config_opts.shell_patterns;
        self.active_panel_mut().set_panelized_entries_with(
            caption,
            entries,
            reverse_files_only,
            shell_patterns,
        );
        Ok(())
    }

    /// Open the GNU mc Copy/Move progress dialog and queue the transfer.
    /// Local→local uses the jobs worker (64 KiB chunks, live bars, Abort).
    /// Archive/ftp/sftp/extfs paths use [`rmc_fs::Vfs::copy`] / `move_path`
    /// on that same worker; Abort is honored between files (a non-chunked
    /// VFS op finishes the current file first).
    pub fn begin_file_op(&mut self, op: CopyMoveOp, src: PathBuf, dst: PathBuf) -> Result<()> {
        let state = crate::fileop::FileOpProgressState::prepare(
            self.vfs.as_ref(),
            op,
            &src,
            &dst,
            &self.config_opts,
        )?;
        let job_id = match op {
            CopyMoveOp::Copy => self.jobs.spawn_copy(&src, &dst),
            CopyMoveOp::Move => self.jobs.spawn_move(&src, &dst),
        };
        self.ui_mode = UiMode::FileOpProgress {
            op,
            src,
            dst,
            state,
            started: true,
            job_id,
        };
        Ok(())
    }

    /// Apply the jobs worker snapshot into the progress dialog. Never blocks on
    /// the whole copy: the worker copies in 64 KiB chunks with a cancel flag.
    pub fn poll_file_op_progress(&mut self) -> Result<()> {
        let job_id = match &self.ui_mode {
            UiMode::FileOpProgress {
                started: true,
                job_id,
                ..
            } => *job_id,
            _ => return Ok(()),
        };
        let Some(job) = self.jobs.get(job_id) else {
            return Ok(());
        };
        if let UiMode::FileOpProgress { state, .. } = &mut self.ui_mode {
            state.apply_counters(
                job.file_done,
                job.file_total,
                job.bytes_done,
                job.files_done,
                &job.current_name,
            );
        }
        match job.status {
            crate::jobs::JobStatus::Queued | crate::jobs::JobStatus::Running => Ok(()),
            crate::jobs::JobStatus::Done | crate::jobs::JobStatus::Cancelled => {
                self.finish_file_op_dialog()
            }
            crate::jobs::JobStatus::Failed => {
                let message = job.error.unwrap_or_else(|| "file operation failed".into());
                self.ui_mode = UiMode::DialogConfirm {
                    title: "Error".into(),
                    message,
                    on_ok: Box::new(|_| Ok(())),
                };
                Ok(())
            }
        }
    }

    fn finish_file_op_dialog(&mut self) -> Result<()> {
        let dst = match &self.ui_mode {
            UiMode::FileOpProgress { dst, .. } => dst.clone(),
            _ => {
                self.ui_mode = UiMode::Normal;
                self.reload_panels()?;
                return Ok(());
            }
        };
        self.vfs.invalidate_dir_cache(dst.parent());
        self.ui_mode = UiMode::Normal;
        self.reload_panels()?;
        Ok(())
    }

    /// Cancel an in-flight foreground copy/move (GNU mc Abort). Leaves a
    /// partial destination as the jobs worker does; returns to Normal.
    /// For VFS (archive/ftp/sftp/extfs) transfers the current `vfs.copy` is
    /// not byte-chunked: Abort finishes that file, then stops.
    pub fn abort_file_op(&mut self) -> Result<()> {
        let job_id = match &self.ui_mode {
            UiMode::FileOpProgress { job_id, .. } => *job_id,
            _ => return Ok(()),
        };
        self.jobs.cancel(job_id);
        self.ui_mode = UiMode::Normal;
        self.reload_panels()?;
        Ok(())
    }

    /// Back-compat name used by the UI loop: non-blocking poll of the worker.
    pub fn drive_pending_file_op(&mut self) -> Result<()> {
        self.poll_file_op_progress()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::KeyMap;
    use rmc_fs::local::LocalFs;
    use std::io::Write;

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

    fn assert_menu(app: &App, top: usize, sel: usize, dropped: bool) {
        match &app.ui_mode {
            UiMode::Menu {
                top_index,
                selected_index,
                dropped: d,
            } => {
                assert_eq!(*top_index, top, "top_index");
                assert_eq!(*selected_index, sel, "selected_index");
                assert_eq!(*d, dropped, "dropped");
            }
            _ => panic!("expected UiMode::Menu"),
        }
    }

    #[test]
    fn drop_menus_default_is_false() {
        assert!(!ConfigOptions::default().drop_menus);
    }

    #[test]
    fn focus_menu_bar_only_when_drop_menus_false() {
        let (mut app, _tmp, _, _) = app_with_distinct_panes();
        app.config_opts.drop_menus = false;
        app.handle_action(Action::FocusMenu).unwrap();
        assert_menu(&app, 0, 0, false);
    }

    #[test]
    fn focus_menu_drops_left_when_drop_menus_true() {
        let (mut app, _tmp, _, _) = app_with_distinct_panes();
        app.config_opts.drop_menus = true;
        app.handle_action(Action::FocusMenu).unwrap();
        assert_menu(&app, 0, 0, true);
    }

    #[test]
    fn begin_file_op_prescans_when_compute_totals_on() {
        let (mut app, tmp, _, _) = app_with_distinct_panes();
        let src = tmp.path().join("src.bin");
        let dst = tmp.path().join("dst.bin");
        std::fs::write(&src, vec![0xCDu8; 1800]).unwrap();
        app.config_opts.compute_totals = true;
        app.config_opts.verbose = true;
        app.begin_file_op(CopyMoveOp::Copy, src, dst).unwrap();
        match &app.ui_mode {
            UiMode::FileOpProgress { state, started, .. } => {
                assert_eq!(state.bytes_total, Some(1800));
                assert_eq!(state.source_name, "src.bin");
                assert!(*started, "transfer is queued on the jobs worker");
                let view = state.view(47, false);
                assert_eq!(view.source_name.as_deref(), Some("src.bin"));
            }
            _ => panic!("expected FileOpProgress"),
        }
    }

    #[test]
    fn begin_file_op_skips_prescan_when_compute_totals_off() {
        let (mut app, tmp, _, _) = app_with_distinct_panes();
        let src = tmp.path().join("src.bin");
        let dst = tmp.path().join("dst.bin");
        std::fs::write(&src, vec![0xCDu8; 900]).unwrap();
        app.config_opts.compute_totals = false;
        app.begin_file_op(CopyMoveOp::Copy, src, dst).unwrap();
        match &app.ui_mode {
            UiMode::FileOpProgress { state, .. } => {
                assert_eq!(state.bytes_total, None);
                let view = state.view(47, false);
                assert!(view.total_bytes.is_none());
            }
            _ => panic!("expected FileOpProgress"),
        }
    }

    #[test]
    fn poll_file_op_progress_copies_and_returns_to_normal() {
        let (mut app, tmp, _, _) = app_with_distinct_panes();
        let src = tmp.path().join("src.bin");
        let dst = tmp.path().join("dst.bin");
        std::fs::write(&src, b"hello-progress").unwrap();
        app.config_opts.compute_totals = true;
        app.begin_file_op(CopyMoveOp::Copy, src.clone(), dst.clone())
            .unwrap();
        wait_until_file_op_settled(&mut app, 5_000);
        assert!(matches!(app.ui_mode, UiMode::Normal));
        assert_eq!(std::fs::read(&dst).unwrap(), b"hello-progress");
        assert_eq!(std::fs::read(&src).unwrap(), b"hello-progress");
    }

    #[test]
    fn abort_accepted_while_started_in_flight() {
        let (mut app, tmp, _, _) = app_with_distinct_panes();
        let src = tmp.path().join("big.bin");
        let dst = tmp.path().join("big.dst");
        std::fs::write(&src, vec![0xABu8; 4 * 1024 * 1024]).unwrap();
        app.begin_file_op(CopyMoveOp::Copy, src.clone(), dst.clone())
            .unwrap();
        let job_id = match &app.ui_mode {
            UiMode::FileOpProgress {
                started, job_id, ..
            } => {
                assert!(*started);
                *job_id
            }
            _ => panic!("expected FileOpProgress"),
        };
        let start = std::time::Instant::now();
        loop {
            if let Some(j) = app.jobs.get(job_id) {
                if j.status == crate::jobs::JobStatus::Running && j.bytes_done > 0 {
                    break;
                }
                if matches!(
                    j.status,
                    crate::jobs::JobStatus::Done
                        | crate::jobs::JobStatus::Failed
                        | crate::jobs::JobStatus::Cancelled
                ) {
                    panic!("job finished before abort: {:?}", j.status);
                }
            }
            if start.elapsed() > std::time::Duration::from_millis(5_000) {
                panic!("job never started running");
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        match &app.ui_mode {
            UiMode::FileOpProgress { started: true, .. } => {}
            _ => panic!("dialog must still be in-flight"),
        }
        app.abort_file_op().unwrap();
        assert!(matches!(app.ui_mode, UiMode::Normal));
        let start = std::time::Instant::now();
        loop {
            if let Some(j) = app.jobs.get(job_id) {
                if j.status != crate::jobs::JobStatus::Running
                    && j.status != crate::jobs::JobStatus::Queued
                {
                    assert_eq!(j.status, crate::jobs::JobStatus::Cancelled);
                    break;
                }
            }
            if start.elapsed() > std::time::Duration::from_millis(5_000) {
                panic!("cancel did not land");
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        if let Ok(meta) = std::fs::metadata(&dst) {
            let src_len = std::fs::metadata(&src).unwrap().len();
            assert!(
                meta.len() < src_len,
                "aborted destination should be incomplete"
            );
        }
    }

    #[test]
    fn counters_advance_before_completion() {
        let (mut app, tmp, _, _) = app_with_distinct_panes();
        let src = tmp.path().join("mid.bin");
        let dst = tmp.path().join("mid.dst");
        std::fs::write(&src, vec![0xCDu8; 4 * 1024 * 1024]).unwrap();
        app.config_opts.compute_totals = true;
        app.begin_file_op(CopyMoveOp::Copy, src, dst).unwrap();
        let start = std::time::Instant::now();
        loop {
            app.poll_file_op_progress().unwrap();
            match &app.ui_mode {
                UiMode::FileOpProgress { state, started, .. } => {
                    assert!(*started);
                    if state.bytes_done > 0 {
                        assert!(state.bytes_done < state.bytes_total.unwrap_or(u64::MAX));
                        break;
                    }
                }
                _ => panic!("dialog closed before counters advanced"),
            }
            if start.elapsed() > std::time::Duration::from_millis(5_000) {
                panic!("bytes_done never advanced");
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        wait_until_file_op_settled(&mut app, 10_000);
    }

    fn write_zip(path: &Path, files: &[(&str, &[u8])]) {
        let f = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(f);
        let options = zip::write::FileOptions::default();
        for (name, data) in files {
            zip.start_file(*name, options).unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn begin_file_op_copy_from_archive_uses_vfs_and_writes_dest() {
        let tmp = tempfile::tempdir().unwrap();
        let zip_path = tmp.path().join("sample.zip");
        write_zip(&zip_path, &[("hello.txt", b"archive-hello")]);
        let mut src = zip_path.as_os_str().to_string_lossy().into_owned();
        src.push('#');
        let src = PathBuf::from(src).join("hello.txt");
        let dst = tmp.path().join("out.txt");

        let vfs = rmc_fs::composite::CompositeFs::new();
        let mut app = App::new(Box::new(vfs), KeyMap::mc_defaults()).unwrap();
        app.config_opts.compute_totals = true;
        app.begin_file_op(CopyMoveOp::Copy, src, dst.clone())
            .unwrap();
        match &app.ui_mode {
            UiMode::FileOpProgress { started, state, .. } => {
                assert!(*started, "progress dialog opens for VFS copy");
                assert_eq!(state.source_name, "hello.txt");
                let view = state.view(47, false);
                assert!(view.files_processed.contains("Files processed:"));
                assert_eq!(view.title, "Copy");
            }
            _ => panic!("expected FileOpProgress"),
        }
        wait_until_file_op_settled(&mut app, 5_000);
        assert!(matches!(app.ui_mode, UiMode::Normal));
        assert_eq!(std::fs::read(&dst).unwrap(), b"archive-hello");
    }

    fn wait_until_file_op_settled(app: &mut App, timeout_ms: u64) {
        let start = std::time::Instant::now();
        loop {
            app.poll_file_op_progress().unwrap();
            if !matches!(app.ui_mode, UiMode::FileOpProgress { .. }) {
                return;
            }
            if start.elapsed() > std::time::Duration::from_millis(timeout_ms) {
                panic!("file op did not settle");
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn editor_menu_gnu_titles_and_wired_items() {
        assert_eq!(
            EditorMenu::TITLES,
            [" File ", " Edit ", " Search ", " Command ", " Options "]
        );
        let file = EditorMenu::default_open();
        assert_eq!(file.items(), &["Save", "Save as", "Quit"][..]);
        assert_eq!(file.current_item(), Some("Save"));
        assert_eq!(
            file.down().current_item(),
            Some("Save as"),
            "Down from Save lands on Save as"
        );
        assert_eq!(EditorMenu::Edit { selected: 0 }.items()[0], "Undo");
        assert_eq!(
            EditorMenu::Search { selected: 0 }.items(),
            &["Search", "Replace"][..]
        );
        assert_eq!(
            EditorMenu::Command { selected: 0 }.items(),
            &["Go to line", "Pipe"][..]
        );
        assert_eq!(
            EditorMenu::Options { selected: 0 }.items(),
            &["Auto indent", "Tab spacing"][..]
        );
        let search = file.right().right();
        assert!(matches!(search, EditorMenu::Search { selected: 0 }));
        let options = search.right().right();
        assert!(matches!(options, EditorMenu::Options { selected: 0 }));
        assert!(matches!(options.right(), EditorMenu::File { selected: 0 }));
        assert!(matches!(file.left(), EditorMenu::Options { selected: 0 }));
        let opts = EditorMenu::Options { selected: 0 };
        assert_eq!(
            opts.down().current_item(),
            Some("Tab spacing"),
            "Down from Auto indent lands on Tab spacing"
        );
    }
}
