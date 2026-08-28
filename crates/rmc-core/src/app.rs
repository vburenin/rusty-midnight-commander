use crate::actions::{Action, PaneSide, SortBy as SortByAction};
use crate::config::KeyMap;
use crate::dirtree::DirectoryTreeState;
use crate::find::FindDialogState;
use crate::hotlist::{Hotlist, HotlistDialogState};
use crate::learn_keys::{LearnKeyRow, LearnedKeyStore};
use crate::panel::{
    listing_page_capacity, FileEntry, ListingFormat, PanelMode, PanelState, SortBy,
    DEFAULT_USER_LISTING_FORMAT,
};
use crate::panelize::ExternalPanelizeDialogState;
use crate::sorting::SortDir;
use crate::subshell::Subshell;
use anyhow::Result;
use rmc_diff;
use rmc_edit::EditorBuffer;
use rmc_fs::{DirEntry, Vfs};
use std::path::{Path, PathBuf};
use std::time::Instant;

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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum JobsDialogFocus {
    /// The list of jobs has focus; Up/Down change selection.
    List,
    /// GNU Background jobs **Stop** (pause without cancelling).
    Stop,
    /// GNU Background jobs **Restart** (resume a stopped job, or re-run failed/cancelled).
    Restart,
    /// GNU Background jobs **Kill** (abort and remove).
    Kill,
    /// Drop finished (Done/Failed/Cancelled) jobs.
    Cleanup,
    /// Close dialog (OK). Esc/F10 also close.
    Ok,
}

/// GNU mc Background jobs button row, plus Clean up / OK for finished jobs and dismiss.
pub const JOBS_DIALOG_BUTTONS: &[(JobsDialogFocus, &str)] = &[
    (JobsDialogFocus::Stop, "Stop"),
    (JobsDialogFocus::Restart, "Restart"),
    (JobsDialogFocus::Kill, "Kill"),
    (JobsDialogFocus::Cleanup, "Clean up"),
    (JobsDialogFocus::Ok, "OK"),
];

impl JobsDialogFocus {
    pub fn cycle(self, reverse: bool) -> Self {
        use JobsDialogFocus::*;
        const ORDER: [JobsDialogFocus; 6] = [List, Stop, Restart, Kill, Cleanup, Ok];
        let i = ORDER.iter().position(|f| *f == self).unwrap_or(0);
        if reverse {
            ORDER[(i + ORDER.len() - 1) % ORDER.len()]
        } else {
            ORDER[(i + 1) % ORDER.len()]
        }
    }

    pub fn button_label(self) -> Option<&'static str> {
        JOBS_DIALOG_BUTTONS
            .iter()
            .find(|(f, _)| *f == self)
            .map(|(_, s)| *s)
    }
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
    /// First panel share of the dual-pane split (left when vertical, top when
    /// horizontal). 0.5 is equal. Clamped to [0.2, 0.8].
    pub panel_ratio: f32,
    /// GNU mc Layout "Panel split": false = Vertical (Left | Right, default),
    /// true = Horizontal (Above / Below). Toggled with Alt-,.
    pub horizontal_split: bool,
    /// GNU mc Layout "Equal split". When true, [`Self::panel_ratio`] is 0.5.
    pub equal_split: bool,
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
            panel_ratio: 0.5,
            horizontal_split: false,
            equal_split: true,
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
    /// size/mtime, or `->` plus the stored target when the current entry is a
    /// symlink). When false, omit that row so the listing uses the extra line.
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
    /// filename patterns are shell globs (`*.c`, `?`, `[abc]`). When false, they
    /// are regexes. The Select/Unselect dialog and Left/Right Filter dialog each
    /// have their own Regular expression checkbox (Select/Unselect seeds that
    /// checkbox from this option on first open).
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
    /// GNU mc Options → Configuration → Preallocate space. Default **false**.
    /// When true, local Copy/Move tries to reserve the whole destination size
    /// (`posix_fallocate` / `fallocate`) before writing. Soft-fail if unsupported.
    pub preallocate_space: bool,
    /// GNU mc Options → Configuration → Use COW file cloning. Default **true**.
    /// When true, local Copy/Move tries a copy-on-write clone (`FICLONE` /
    /// `copy_file_range`) and falls back to an ordinary byte copy. When false,
    /// never attempt clone.
    pub use_cow_file_cloning: bool,
    /// GNU mc Options → Configuration → Complete: show all. Default **false**.
    /// When false, the first Alt-Tab on an ambiguous token completes the common
    /// prefix and beeps; the second Alt-Tab shows the list. When true, the first
    /// Alt-Tab shows all possibilities.
    pub complete_show_all: bool,
    /// GNU mc Options → Configuration → Safe delete. Default **false**.
    /// When true, Delete (F8) confirmation starts on No so Enter does not delete.
    /// Independent of Options → Confirmations → delete (whether the dialog appears).
    pub safe_delete: bool,
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
            preallocate_space: false,
            use_cow_file_cloning: true,
            complete_show_all: false,
            safe_delete: false,
        }
    }
}

impl ConfigOptions {
    /// Initial Yes focus for Delete confirmation. GNU Safe delete flips this to No.
    pub fn delete_confirm_focus_ok(&self) -> bool {
        !self.safe_delete
    }

    /// Local Copy/Move flags from Options → Configuration.
    pub fn copy_flags(&self) -> rmc_fs::CopyFlags {
        rmc_fs::CopyFlags {
            preallocate_space: self.preallocate_space,
            use_cow_file_cloning: self.use_cow_file_cloning,
            ..rmc_fs::CopyFlags::default()
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutFocus {
    SplitVertical,
    SplitHorizontal,
    EqualSplit,
    MenuBar,
    CommandPrompt,
    KeyBar,
    HintBar,
    XtermTitle,
    ShowFreeSpace,
    Ok,
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigOptionsFocus {
    Verbose,
    ComputeTotals,
    ClassicProgressbar,
    PreallocateSpace,
    UseCowFileCloning,
    UseInternalViewer,
    UseInternalEditor,
    PauseAfterRun,
    ShellPatterns,
    AutoMenus,
    DropMenus,
    MkdirAutoname,
    CompleteShowAll,
    SafeDelete,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverwriteFocus {
    Yes,
    No,
    All,
    Older,
    None,
    Smaller,
    SizeDiffers,
    Append,
    /// Resume: append source bytes after `dest_size`. Copy only, when
    /// `0 < dest_size < src_size`. Hidden otherwise (not a disabled ghost).
    Reget,
    Abort,
    /// GNU: "Don't overwrite with zero length file".
    ZeroLength,
}

/// GNU replace-dialog checkbox label (mc(1) File Operations).
pub const DONT_OVERWRITE_ZERO_LENGTH_LABEL: &str = "Don't overwrite with zero length file";

impl OverwriteFocus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Yes => "Yes",
            Self::No => "No",
            Self::All => "All",
            Self::Older => "Older",
            Self::None => "None",
            Self::Smaller => "Smaller",
            Self::SizeDiffers => "Size differs",
            Self::Append => "Append",
            Self::Reget => "Reget",
            Self::Abort => "Abort",
            Self::ZeroLength => DONT_OVERWRITE_ZERO_LENGTH_LABEL,
        }
    }

    pub fn is_button(self) -> bool {
        !matches!(self, Self::ZeroLength)
    }
}

/// GNU: Reget is offered only on Copy when dest size is non-zero and smaller
/// than the source. Not offered on Move.
pub fn reget_offered(op: CopyMoveOp, src_size: u64, dst_size: u64) -> bool {
    matches!(op, CopyMoveOp::Copy) && dst_size > 0 && dst_size < src_size
}

/// Skip this file (treat as No) when the zero-length checkbox is on and a
/// zero-sized source would replace a non-zero target.
pub fn skip_zero_length_overwrite(dont_overwrite_zero: bool, src_size: u64, dst_size: u64) -> bool {
    dont_overwrite_zero && src_size == 0 && dst_size > 0
}

/// Tab order: existing buttons, optional Reget, Abort, then the checkbox.
pub fn overwrite_tab_order(op: CopyMoveOp, src_size: u64, dst_size: u64) -> Vec<OverwriteFocus> {
    let mut order = vec![
        OverwriteFocus::Yes,
        OverwriteFocus::No,
        OverwriteFocus::All,
        OverwriteFocus::Older,
        OverwriteFocus::None,
        OverwriteFocus::Smaller,
        OverwriteFocus::SizeDiffers,
        OverwriteFocus::Append,
    ];
    if reget_offered(op, src_size, dst_size) {
        order.push(OverwriteFocus::Reget);
    }
    order.push(OverwriteFocus::Abort);
    order.push(OverwriteFocus::ZeroLength);
    order
}

/// Button rows for the replace dialog. Reget is omitted (not a disabled ghost)
/// when not offered. Abort is always on the last row.
pub fn overwrite_button_rows(
    op: CopyMoveOp,
    src_size: u64,
    dst_size: u64,
) -> Vec<Vec<OverwriteFocus>> {
    let mut row3 = Vec::new();
    if reget_offered(op, src_size, dst_size) {
        row3.push(OverwriteFocus::Reget);
    }
    row3.push(OverwriteFocus::Abort);
    vec![
        vec![
            OverwriteFocus::Yes,
            OverwriteFocus::No,
            OverwriteFocus::All,
            OverwriteFocus::Older,
        ],
        vec![
            OverwriteFocus::None,
            OverwriteFocus::Smaller,
            OverwriteFocus::SizeDiffers,
            OverwriteFocus::Append,
        ],
        row3,
    ]
}

pub fn cycle_overwrite_focus(
    cur: OverwriteFocus,
    order: &[OverwriteFocus],
    back: bool,
) -> OverwriteFocus {
    if order.is_empty() {
        return cur;
    }
    match order.iter().position(|f| *f == cur) {
        Some(i) if back => order[(i + order.len() - 1) % order.len()],
        Some(i) => order[(i + 1) % order.len()],
        None if back => *order.last().unwrap(),
        None => order[0],
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListingModeFocus {
    RadioFull,
    RadioBrief,
    RadioLong,
    RadioUser,
    Input,
    Ok,
    Cancel,
}

/// Focus within the GNU Left/Right → Filter dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterDialogFocus {
    Pattern,
    RegularExpression,
    FilesOnly,
    CaseSensitive,
    Ok,
    Cancel,
}

/// Focus within the GNU Select / Unselect group dialog (mc(1) `+` / `\\`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectGroupDialogFocus {
    Pattern,
    FilesOnly,
    CaseSensitive,
    RegularExpression,
    Ok,
    Cancel,
}

/// Last Select/Unselect pattern and checkbox state for this process.
///
/// Not written to ini; a later session starts from defaults.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectGroupLast {
    pub pattern: String,
    pub files_only: bool,
    pub case_sensitive: bool,
    pub regular_expression: bool,
}

/// Focus within the GNU File → Chmod dialog (C-x c).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChmodDialogFocus {
    UserRead,
    UserWrite,
    UserExec,
    GroupRead,
    GroupWrite,
    GroupExec,
    OtherRead,
    OtherWrite,
    OtherExec,
    SetUid,
    SetGid,
    Sticky,
    Recursive,
    Set,
    Cancel,
}

/// Focus within the GNU File → Chown dialog (C-x o).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChownDialogFocus {
    Owner,
    Group,
    Recursive,
    Ok,
    Cancel,
}

/// Kind of link created by File → Hard link / SymLink / Relative symlink.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkKind {
    Hard,
    Abs,
    Rel,
}

impl LinkKind {
    pub fn title(self) -> &'static str {
        match self {
            Self::Hard => "Link",
            Self::Abs => "Symbolic link",
            Self::Rel => "Relative symlink",
        }
    }

    pub fn prompt(self) -> &'static str {
        match self {
            Self::Hard => "Enter the name of the hard link to:",
            Self::Abs | Self::Rel => "Enter name of the symlink:",
        }
    }
}

/// Focus within the GNU hard-link / symlink name dialog (C-x l / s / v).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkDialogFocus {
    Name,
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
    /// GNU mc Left/Right → Filter dialog (mc(1) Filter…)
    FilterDialog {
        side: PaneSide,
        pattern: String,
        regular_expression: bool,
        files_only: bool,
        case_sensitive: bool,
        focus: FilterDialogFocus,
    },
    /// GNU mc(1) Select group (`+`) / Unselect group (`\\`) dialog.
    SelectGroupDialog {
        /// `true` = Select (mark matches); `false` = Unselect (unmark matches).
        select: bool,
        pattern: String,
        files_only: bool,
        case_sensitive: bool,
        regular_expression: bool,
        focus: SelectGroupDialogFocus,
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
        /// Tagged (or current) sources snapshotted when the dialog opened.
        /// `src_path` is the first entry; F15/F16 store only the current file.
        src_paths: Vec<PathBuf>,
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
        /// GNU `file_op_context` "Don't overwrite with zero length file".
        skip_zero_length: bool,
    },
    // Permissions dialog (GNU File → Chmod / C-x c)
    ChmodDialog {
        name: String,
        paths: Vec<PathBuf>,
        mode: u32,
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
        allow_recursive: bool,
        focus: ChmodDialogFocus,
    },
    // Ownership dialog (GNU File → Chown / C-x o)
    ChownDialog {
        paths: Vec<PathBuf>,
        owner: String,
        group: String,
        recursive: bool,
        allow_recursive: bool,
        focus: ChownDialogFocus,
    },
    /// GNU File → Hard link / SymLink / Relative symlink (C-x l / s / v).
    LinkDialog {
        kind: LinkKind,
        src: PathBuf,
        value: String,
        focus: LinkDialogFocus,
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
    /// GNU mc(1) Alt-Tab completion listbox (ambiguous matches).
    CompletionList {
        items: Vec<crate::complete::CompletionItem>,
        selected: usize,
        scroll_top: usize,
        /// Byte offset of the token in the input text (before the cursor).
        token_start: usize,
        /// Mode that owned the input line (restored on insert or cancel).
        prev: Box<UiMode>,
    },
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
    /// GNU External panelize: named commands + run/panelize.
    ExternalPanelizeDialog(ExternalPanelizeDialogState),
    /// GNU mc(1) Command menu Directory tree figure (not panel Tree mode).
    DirectoryTree(DirectoryTreeState),
    /// GNU mc(1) Screen list: currently open internal modules (and the file manager).
    ScreenList {
        selected: usize,
        scroll_top: usize,
        focus: ScreenListFocus,
        /// Mode to restore on Esc/F10 (the screen that was current).
        prev: Box<UiMode>,
    },
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
        /// One row per teachable key (arrows, F1–F20, keypad, Complete, Back Tab).
        keys: Vec<LearnKeyRow>,
        /// Selected key index; when equal to `keys.len()`, Save/Cancel are focused.
        selected: usize,
        /// True while the “press that key” message box is waiting for a sequence.
        capturing: bool,
        /// When buttons are focused, true => Save, false => Cancel.
        focus_save: bool,
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

fn unwrap_screen_overlay(mode: &UiMode) -> &UiMode {
    match mode {
        UiMode::ScreenList { prev, .. } | UiMode::Help { prev, .. } => unwrap_screen_overlay(prev),
        other => other,
    }
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

/// GNU mc(1) Screen list dialog focus (list vs OK/Cancel).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ScreenListFocus {
    List,
    Ok,
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    /// GNU mc(1) C-l: the event loop clears the terminal and redraws before the next `draw`.
    pub needs_full_clear: bool,
    pub ui_mode: UiMode,
    /// Parked internal modules (editor / viewer / diff). The currently displayed
    /// module lives in `ui_mode`; its slot here is a `Normal` placeholder.
    /// Panels are not stored — they are screen index 0, not extra file managers.
    pub screens: Vec<UiMode>,
    /// 0 = panels; `1..=screens.len()` = that module is current (`ui_mode`).
    pub screen_idx: usize,
    /// Subshell / command line state and last output.
    pub subshell: Subshell,
    pub hotlist: Hotlist,
    pub pending_ctrl_x: bool,
    /// C-q: insert the next key literally into the command line.
    pub pending_quote: bool,
    /// GNU mc(1) Esc-number: time of a pending Esc in Normal listing (`None` = idle).
    pub pending_esc: Option<Instant>,
    /// MC-style incremental quick search state for the active panel.
    pub quick_search: Option<crate::quicksearch::QuickSearchState>,
    /// Last pattern used when listing Quick search ended (GNU double C-s).
    pub quick_search_prev: String,
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
    /// Last Alt-Tab was an ambiguous common-prefix completion (show-all off).
    /// The next Alt-Tab on this token opens the list. Cleared by any other key.
    pub completion_retry: bool,
    /// Ambiguous completion "beep" flag (no TTY / BEL required).
    pub completion_beep: bool,
    /// Test override for command `PATH` (`:`-separated). `None` uses the process PATH.
    pub completion_path_override: Option<String>,
    /// Test override for username completion (`/etc/passwd` by default).
    pub completion_passwd_path: Option<PathBuf>,
    /// Test override for hostname completion (`/etc/hosts` by default).
    pub completion_hosts_path: Option<PathBuf>,
    /// Test override for `~/` filename completion (`$HOME` by default).
    pub completion_home: Option<PathBuf>,
    /// Last applied Select/Unselect pattern and checkboxes (process lifetime).
    pub select_group_last: Option<SelectGroupLast>,
    /// GNU replace-dialog "Don't overwrite with zero length file" for the
    /// current Copy/Move (`file_op_context`). Reset when F5/F6 opens a new dialog.
    pub dont_overwrite_with_zero: bool,
    /// Copy/Move dialog flags for the current operation (Follow links, Preserve
    /// attributes, Dive into subdir, Stable symlinks, plus Configuration
    /// Preallocate/COW). Used by OK, Background, and the overwrite path.
    pub copy_op_flags: rmc_fs::CopyFlags,
    /// Selected skin name (e.g., "default")
    pub skin_name: String,
    /// Whether to draw drop shadows for dialogs/menus
    pub shadows: bool,
    /// GNU mc(1) `-d`/`--nomouse`: when false, do not enable mouse capture
    /// and do not process mouse events. Default true (mouse on). Process-lifetime.
    pub mouse_enabled: bool,
    /// GNU mc(1) concurrent subshell: `-U`/`--subshell` (true) vs `-u`/`--nosubshell`
    /// (false). Default true, matching a build with subshell support. C-o still
    /// toggles the panels/output screen; a PTY is spawned only when this is true.
    pub use_subshell: bool,
    /// Sequences from Options → Learn keys (`[terminal:TERM]` in the user ini).
    pub learned_keys: LearnedKeyStore,
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
            needs_full_clear: false,
            ui_mode: UiMode::Normal,
            screens: Vec::new(),
            screen_idx: 0,
            subshell: Subshell::new(),
            hotlist: Hotlist::load_from_default_path(),
            pending_ctrl_x: false,
            pending_quote: false,
            pending_esc: None,
            quick_search: None,
            quick_search_prev: String::new(),
            jobs: crate::jobs::JobQueue::new(),
            layout: LayoutOptions::default(),
            confirm: ConfirmOptions::default(),
            panel_opts: PanelOptions::default(),
            config_opts: ConfigOptions::default(),
            vfs_opts: VfsOptions::default(),
            skin_name: "default".to_string(),
            shadows: true,
            mouse_enabled: true,
            use_subshell: true,
            learned_keys: LearnedKeyStore::default(),
            completion_retry: false,
            completion_beep: false,
            completion_path_override: None,
            completion_passwd_path: None,
            completion_hosts_path: None,
            completion_home: None,
            select_group_last: None,
            dont_overwrite_with_zero: false,
            copy_op_flags: rmc_fs::CopyFlags::default(),
        };
        // Overlay user setup (if available) over defaults, then refresh panels.
        let _ = crate::config::load_user_setup(&mut app);
        app.reload_panels()?;
        Ok(app)
    }

    /// GNU mc Esc-number: drop a pending Esc prefix after the idle timeout.
    pub fn expire_esc_number_prefix(&mut self) {
        if let Some(at) = self.pending_esc {
            if at.elapsed() >= crate::actions::ESC_NUMBER_TIMEOUT {
                self.pending_esc = None;
            }
        }
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
    pub fn inactive_panel(&self) -> &PanelState {
        match self.active {
            PaneSide::Left => &self.right,
            PaneSide::Right => &self.left,
        }
    }

    /// GNU Quick view / Info follow the listing panel cursor. Reset the other
    /// panel's reduced-viewer offset when the selected path changes.
    fn sync_other_preview_target(&mut self) {
        let src_path = self.active_panel().current_entry().map(|e| e.path.clone());
        let other = self.inactive_panel_mut();
        if matches!(other.mode, PanelMode::QuickView | PanelMode::Info)
            && other.preview_path != src_path
        {
            other.preview_path = src_path;
            other.preview_offset = 0;
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
        self.reload_panel(PaneSide::Left, false)?;
        self.reload_panel(PaneSide::Right, false)?;
        Ok(())
    }

    /// GNU mc(1) C-r / Left-Right → Reread: invalidate VFS cache and force `list_dir`
    /// on **one** panel, even when Fast directory reload would skip an auto reload.
    pub fn refresh_panel(&mut self, side: PaneSide) -> Result<()> {
        let cwd = match side {
            PaneSide::Left => self.left.cwd.clone(),
            PaneSide::Right => self.right.cwd.clone(),
        };
        self.vfs.invalidate_dir_cache(Some(&cwd));
        self.reload_panel(side, true)
    }

    /// Re-list one panel. `force` always calls `list_dir`. When Fast reload is on and
    /// `force` is false, skip if the local directory stamp is unchanged.
    fn reload_panel(&mut self, side: PaneSide, force: bool) -> Result<()> {
        self.sync_vfs_dir_cache_timeout();
        let reverse_files_only = self.panel_opts.reverse_files_only;
        let show_hidden = self.show_hidden;
        let fast = self.panel_opts.fast_reload;

        let panel = match side {
            PaneSide::Left => &self.left,
            PaneSide::Right => &self.right,
        };
        // Tree figure C-r is handled in the UI (`rescan_panel_tree`). Quick view /
        // Info have no separate preview rescan; leftover listing is still reloaded
        // so returning to Listing mode is current.
        if !force && fast && panel.fast_reload_listing_is_current(show_hidden) {
            return Ok(());
        }

        let cwd = panel.cwd.clone();
        let cursor_name = panel.current_entry().map(|e| e.name.clone());
        let cursor_idx = panel.cursor;
        let marked_names: Vec<String> = panel
            .selection
            .iter()
            .filter_map(|i| panel.entries.get(i).map(|e| e.name.clone()))
            .collect();

        let list = self.vfs.list_dir(&cwd, show_hidden)?;
        let entries = self.map_dir_entries(list);
        let panel = match side {
            PaneSide::Left => &mut self.left,
            PaneSide::Right => &mut self.right,
        };
        panel.set_entries_with(entries, reverse_files_only);
        panel.restore_selection_after_reload(cursor_name.as_deref(), cursor_idx, &marked_names);
        panel.capture_dir_reload_stamp(show_hidden);
        Ok(())
    }

    /// GNU `!` type mark: local `lstat` succeeds, follow (`stat`) fails.
    /// Archive/remote paths fail `lstat` and are not treated as stale.
    fn local_symlink_is_stale(path: &Path) -> bool {
        path.symlink_metadata().is_ok() && std::fs::metadata(path).is_err()
    }

    fn map_dir_entries(&self, entries: Vec<DirEntry>) -> Vec<FileEntry> {
        entries
            .into_iter()
            .map(|e| {
                let is_stale_symlink = e.meta.is_symlink && Self::local_symlink_is_stale(&e.path);
                FileEntry {
                    name: e.name,
                    path: e.path,
                    is_dir: e.meta.is_dir,
                    is_symlink: e.meta.is_symlink,
                    symlink_target: e.meta.symlink_target,
                    is_exe: e.meta.is_executable,
                    size: e.meta.size,
                    modified: e.meta.modified,
                    accessed: e.meta.accessed,
                    changed: e.meta.changed,
                    permissions: e.meta.permissions,
                    owner: e.meta.owner,
                    group: e.meta.group,
                    nlink: e.meta.nlink,
                    inode: e.meta.inode,
                    is_stale_symlink,
                }
            })
            .collect()
    }

    /// Apply Left/Right → Sort order… to one panel. Unsorted re-lists that panel
    /// (same cwd) so the order matches `list_dir` after `..`. Other orders re-sort
    /// the current listing in place.
    pub fn apply_panel_sort(
        &mut self,
        side: PaneSide,
        by: SortBy,
        reverse: bool,
        dirs_first: bool,
    ) -> Result<()> {
        let reverse_files_only = self.panel_opts.reverse_files_only;
        let show_hidden = self.show_hidden;
        let sort_dir = if reverse { SortDir::Desc } else { SortDir::Asc };
        if matches!(by, SortBy::Unsorted) {
            let cwd = if matches!(side, PaneSide::Left) {
                self.left.cwd.clone()
            } else {
                self.right.cwd.clone()
            };
            let list = self.vfs.list_dir(&cwd, show_hidden)?;
            let entries = self.map_dir_entries(list);
            let p = if matches!(side, PaneSide::Left) {
                &mut self.left
            } else {
                &mut self.right
            };
            p.sort_by = by;
            p.sort_dir = sort_dir;
            p.dirs_first = dirs_first;
            p.set_entries_with(entries, reverse_files_only);
            p.capture_dir_reload_stamp(show_hidden);
        } else {
            let p = if matches!(side, PaneSide::Left) {
                &mut self.left
            } else {
                &mut self.right
            };
            p.sort_by = by;
            p.sort_dir = sort_dir;
            p.dirs_first = dirs_first;
            p.apply_sort_with(reverse_files_only);
        }
        Ok(())
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
                // C-r / Reread: force re-list the **active** panel only, even when
                // Fast reload or the VFS dir cache is still fresh. The other panel
                // is unchanged. Tree figure rescan is wired in the UI.
                let side = self.active;
                self.refresh_panel(side)?;
                self.sync_other_preview_target();
            }
            Repaint => {
                // C-l: full screen repaint only. Do not reload panels or invalidate VFS cache.
                self.needs_full_clear = true;
            }
            ToggleSubshell => {
                // Toggle full-screen subshell/output view.
                self.subshell.toggle_output_screen();
                // Ensure we leave any transient prompt/dialog modes.
                self.ui_mode = UiMode::Normal;
            }
            CycleListingFormat => {
                // Tests without a TTY pass a dummy page size; handle_key uses
                // the real `page_rows` via `cycle_listing_format_by`.
                self.cycle_listing_format_by(10);
            }
            ToggleHidden => {
                self.show_hidden = !self.show_hidden;
                // Keep Options → Panels in sync with Ctrl-H
                self.panel_opts.show_hidden = self.show_hidden;
                self.reload_panels()?;
            }
            InvertSelection => {
                // GNU invert: marked ↔ unmarked for every entry except `..`.
                // Directories are inverted (not files-only unless a later option).
                let len = self.active_panel().entries.len();
                for idx in 0..len {
                    let ent = &self.active_panel().entries[idx];
                    if !ent.is_parent_marker() {
                        self.active_panel_mut().selection.toggle(idx);
                    }
                }
            }
            SelectGroup => {
                self.open_select_group_dialog(true);
            }
            UnselectGroup => {
                self.open_select_group_dialog(false);
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
            EqualizePanels => {
                // GNU mc Left/Right → Equal panel size: 50/50 in either orientation.
                self.layout.panel_ratio = 0.5;
                self.layout.equal_split = true;
            }
            TogglePanelSplit => {
                // GNU mc Alt-, : Vertical (Left|Right) ↔ Horizontal (Above/Below).
                self.layout.horizontal_split = !self.layout.horizontal_split;
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
            MoveUp => {
                // Tests / mouse without a TTY pass a dummy page size; handle_key
                // uses the real `page_rows` via `move_up_by`.
                self.move_up_by(10);
            }
            MoveDown => {
                self.move_down_by(10);
            }
            PageUp => {
                self.page_up_by(10);
            }
            PageDown => {
                self.page_down_by(10);
            }
            Home => {
                self.home_by(10);
            }
            End => {
                self.end_by(10);
            }
            PanelJumpTop => self.jump_visible_top_by(10),
            PanelJumpMiddle => self.jump_visible_middle_by(10),
            PanelJumpBottom => self.jump_visible_bottom_by(10),
            QuickSearch => {
                // Start only. Repeat / double-C-s restore live in the UI key loop
                // (needs `page_rows` from `handle_key`).
                if matches!(self.ui_mode, UiMode::Normal)
                    && matches!(self.active_panel().mode, crate::panel::PanelMode::Listing)
                    && self.quick_search.is_none()
                {
                    self.quick_search = Some(crate::quicksearch::QuickSearchState::new());
                }
            }
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
                // GNU mc(1) Insert / C-t: toggle the mark on the current listing
                // entry. Never mark `..` (no-op). Directories are markable.
                // “Mark moves down” (GNU default on) advances after a successful toggle.
                if matches!(self.ui_mode, UiMode::Normal)
                    && matches!(self.active_panel().mode, crate::panel::PanelMode::Listing)
                    && self
                        .active_panel()
                        .current_entry()
                        .is_some_and(|e| !e.is_parent_marker())
                {
                    let idx = self.active_panel().cursor;
                    self.active_panel_mut().selection.toggle(idx);
                    if self.panel_opts.mark_moves_down {
                        self.active_panel_mut().move_down();
                    }
                }
            }
            Sort(sb) => {
                let by = match sb {
                    SortByAction::Name => SortBy::Name,
                    SortByAction::Ext => SortBy::Ext,
                    SortByAction::Size => SortBy::Size,
                    SortByAction::Time => SortBy::Time,
                };
                let reverse_files_only = self.panel_opts.reverse_files_only;
                let p = self.active_panel_mut();
                p.sort_by = by;
                p.apply_sort_with(reverse_files_only);
            }
            ViewFile => {
                if let Some(ent) = self.active_panel().current_entry() {
                    if !ent.is_dir {
                        self.push_screen(UiMode::new_viewer(ent.path.clone()));
                    }
                }
            }
            Copy | Move | Mkdir | Delete => {
                // UI layer opens dialogs; core provides helpers
            }
            Chmod => self.open_chmod_dialog(),
            Chown => self.open_chown_dialog(),
            LinkHard => self.open_link_dialog(LinkKind::Hard),
            SymlinkAbs => self.open_link_dialog(LinkKind::Abs),
            SymlinkRel => self.open_link_dialog(LinkKind::Rel),
            ShowUserMenu => self.try_open_user_menu(),
            ViewerQuit => self.close_current_screen(),
            ViewerToggleHex => {
                if let UiMode::Viewer { hex, .. } = &mut self.ui_mode {
                    *hex = !*hex;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Consume the C-l full-repaint request. The event loop checks this before `draw`.
    pub fn take_needs_full_clear(&mut self) -> bool {
        std::mem::take(&mut self.needs_full_clear)
    }

    pub fn change_dir(&mut self, path: &Path) -> Result<()> {
        let new_cwd = path.to_path_buf();
        let cwd_changed = self.active_panel().cwd != new_cwd;
        let reverse_files_only = self.panel_opts.reverse_files_only;
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
        p.set_entries_with(entries, reverse_files_only);
        p.capture_dir_reload_stamp(show_hidden);
        // Auto menus: only after a real directory change, and only for a local
        // `.mc.menu` in the new panel cwd (not a parent, not reload/C-r/panelize).
        if cwd_changed && auto_menus {
            self.try_auto_open_local_user_menu(&new_cwd);
        }
        self.sync_other_preview_target();
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

    /// Tagged entries, or the current entry when nothing is tagged. Never `..`.
    fn tagged_or_current_entries(&self) -> Vec<&FileEntry> {
        let panel = self.active_panel();
        if panel.selection.is_empty() {
            match panel.current_entry() {
                Some(e) if !e.is_parent_marker() => vec![e],
                _ => Vec::new(),
            }
        } else {
            panel
                .selection
                .iter()
                .filter_map(|i| panel.entries.get(i))
                .filter(|e| !e.is_parent_marker())
                .collect()
        }
    }

    /// GNU File → Chmod (C-x c). Seeds bits from the current entry; applies to
    /// tagged files when any are marked.
    pub fn open_chmod_dialog(&mut self) {
        let targets = self.tagged_or_current_entries();
        if targets.is_empty() {
            return;
        }
        let paths: Vec<PathBuf> = targets.iter().map(|e| e.path.clone()).collect();
        let allow_recursive = targets.iter().any(|e| e.is_dir);
        let n = paths.len();
        let name = if !self.active_panel().selection.is_empty() {
            format!("{n} files")
        } else {
            targets[0].name.clone()
        };
        let m = self
            .active_panel()
            .current_entry()
            .map(|e| e.permissions & 0o7777)
            .unwrap_or_else(|| targets[0].permissions & 0o7777);
        self.ui_mode = UiMode::ChmodDialog {
            name,
            paths,
            mode: m,
            ur: (m & 0o400) != 0,
            uw: (m & 0o200) != 0,
            ux: (m & 0o100) != 0,
            gr: (m & 0o040) != 0,
            gw: (m & 0o020) != 0,
            gx: (m & 0o010) != 0,
            or_: (m & 0o004) != 0,
            ow: (m & 0o002) != 0,
            ox: (m & 0o001) != 0,
            suid: (m & 0o4000) != 0,
            sgid: (m & 0o2000) != 0,
            sticky: (m & 0o1000) != 0,
            recursive: false,
            allow_recursive,
            focus: ChmodDialogFocus::UserRead,
        };
    }

    /// GNU File → Chown (C-x o). Owner/group names; empty field = unchanged.
    pub fn open_chown_dialog(&mut self) {
        let targets = self.tagged_or_current_entries();
        if targets.is_empty() {
            return;
        }
        let paths: Vec<PathBuf> = targets.iter().map(|e| e.path.clone()).collect();
        let allow_recursive = targets.iter().any(|e| e.is_dir);
        let seed = self.active_panel().current_entry().unwrap_or(targets[0]);
        self.ui_mode = UiMode::ChownDialog {
            paths,
            owner: seed.owner.clone().unwrap_or_default(),
            group: seed.group.clone().unwrap_or_default(),
            recursive: false,
            allow_recursive,
            focus: ChownDialogFocus::Owner,
        };
    }

    /// GNU File → Hard link / SymLink / Relative symlink (C-x l / s / v).
    ///
    /// Hard-linking a directory (including `..`) is refused with an error
    /// dialog. Destination defaults to the other panel's cwd + current name.
    pub fn open_link_dialog(&mut self, kind: LinkKind) {
        let Some(ent) = self.active_panel().current_entry().cloned() else {
            return;
        };
        if ent.is_parent_marker() {
            return;
        }
        if kind == LinkKind::Hard && ent.is_dir {
            self.show_error_dialog("Cannot hard-link a directory".into());
            return;
        }
        let dst_dir = self.inactive_panel().cwd.clone();
        let default_to = dst_dir.join(&ent.name).display().to_string();
        self.ui_mode = UiMode::LinkDialog {
            kind,
            src: ent.path,
            value: default_to,
            focus: LinkDialogFocus::Name,
        };
    }

    /// Show a modal error (unknown user, dest exists, VFS failure). Does not panic.
    pub fn show_error_dialog(&mut self, message: String) {
        self.ui_mode = UiMode::DialogConfirm {
            title: "Error".into(),
            message,
            on_ok: Box::new(|_| Ok(())),
        };
    }

    /// Create the link named in the dialog. Refuses to clobber an existing dest
    /// (overwrite/replace confirm is out of scope). Relative symlink stores a
    /// path relative to the new link's directory.
    pub fn apply_link_dialog(&mut self, kind: LinkKind, src: &Path, dst_raw: &str) -> Result<()> {
        let dst = PathBuf::from(dst_raw.trim());
        if dst.as_os_str().is_empty() {
            anyhow::bail!("link name is empty");
        }
        let dst = if dst.is_absolute() {
            dst
        } else {
            self.inactive_panel().cwd.join(dst)
        };
        if self.vfs.stat(&dst).is_ok() {
            anyhow::bail!(
                "destination already exists: {} (overwrite confirm is not implemented; refusing to clobber)",
                dst.display()
            );
        }
        let cwd = self.active_panel().cwd.clone();
        let abs_src = if src.is_absolute() {
            src.to_path_buf()
        } else {
            cwd.join(src)
        };
        match kind {
            LinkKind::Hard => {
                let md = self.vfs.stat(&abs_src)?;
                if md.is_dir {
                    anyhow::bail!("Cannot hard-link a directory");
                }
                self.vfs.link_hard(&abs_src, &dst)?;
            }
            LinkKind::Abs => {
                self.vfs.symlink(&abs_src, &dst)?;
            }
            LinkKind::Rel => {
                let base = dst.parent().unwrap_or_else(|| Path::new("."));
                let abs_base = if base.is_absolute() {
                    base.to_path_buf()
                } else {
                    cwd.join(base)
                };
                let rel = relative_path(&abs_base, &abs_src);
                self.vfs.symlink(&rel, &dst)?;
            }
        }
        self.reload_panels()?;
        Ok(())
    }

    fn open_select_group_dialog(&mut self, select: bool) {
        let (pattern, files_only, case_sensitive, regular_expression) =
            if let Some(last) = &self.select_group_last {
                (
                    last.pattern.clone(),
                    last.files_only,
                    last.case_sensitive,
                    last.regular_expression,
                )
            } else {
                (String::new(), false, true, !self.config_opts.shell_patterns)
            };
        self.ui_mode = UiMode::SelectGroupDialog {
            select,
            pattern,
            files_only,
            case_sensitive,
            regular_expression,
            focus: SelectGroupDialogFocus::Pattern,
        };
    }

    /// Select (`select == true`) or unselect names matching `pattern`.
    ///
    /// Uses the same glob/regex matcher as the Filter dialog. Never touches `..`.
    /// When `files_only` is set, directories are left unchanged.
    pub fn apply_group_pattern(
        &mut self,
        pattern: &str,
        select: bool,
        files_only: bool,
        case_sensitive: bool,
        regular_expression: bool,
    ) {
        let shell_glob = !regular_expression;
        let matches: Vec<usize> = self
            .active_panel()
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| !e.is_parent_marker())
            .filter(|(_, e)| !(files_only && e.is_dir))
            .filter(|(_, e)| {
                crate::matchutil::filename_pattern_matches_ex(
                    pattern,
                    &e.name,
                    shell_glob,
                    case_sensitive,
                )
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
        self.select_group_last = Some(SelectGroupLast {
            pattern: pattern.to_string(),
            files_only,
            case_sensitive,
            regular_expression,
        });
    }
}

impl UiMode {
    /// Internal editor with GNU mcedit defaults (no pull-down / overlays).
    pub fn new_editor(buf: EditorBuffer, return_to: Option<Box<UiMode>>) -> Self {
        UiMode::Editor {
            buf,
            show_menu: None,
            status_msg: None,
            search_input: None,
            save_as_dialog: None,
            search_dialog: None,
            replace_dialog: None,
            pipe_dialog: None,
            goto_dialog: None,
            tab_spacing_dialog: None,
            pending_quit: false,
            confirm_exit: None,
            return_to,
        }
    }

    /// Internal viewer with GNU mcview defaults (parsed on, format off, no selection).
    pub fn new_viewer(path: PathBuf) -> Self {
        Self::new_viewer_with_parsed(path, true)
    }

    /// Internal viewer. `parsed = false` is GNU F13 View raw (no mc.ext
    /// `[view]` filter / formatting). F3 uses `parsed = true`.
    pub fn new_viewer_with_parsed(path: PathBuf, parsed: bool) -> Self {
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
            parsed,
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
            UiMode::FilterDialog { .. } => "Panels".to_string(),
            UiMode::SelectGroupDialog { .. } => "Panels".to_string(),
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
            UiMode::LinkDialog { .. } => "Link".to_string(),
            UiMode::MenuFocused => "Menus".to_string(),
            UiMode::Help { state, .. } => state.topic.clone(),
            UiMode::ShellInput => "Panels".to_string(),
            UiMode::CompletionList { .. } => "Panels".to_string(),
            UiMode::HistoryDialog { .. } => "Panels".to_string(),
            UiMode::HotlistDialog(_) => "Panels".to_string(),
            UiMode::ExternalPanelizeDialog(_) => "External panelize".to_string(),
            UiMode::DirectoryTree(_) => "Directory Tree".to_string(),
            UiMode::ScreenList { .. } => "Screen list".to_string(),
            UiMode::JobsDialog { .. } => "Panels".to_string(),
            UiMode::CompareDirsDialog { .. } => "Panels".to_string(),
            UiMode::LayoutDialog { .. } => "Panels".to_string(),
            UiMode::ConfirmationsDialog { .. } => "Panels".to_string(),
            UiMode::PanelOptionsDialog { .. } => "Panels".to_string(),
            UiMode::LearnKeysDialog { .. } => "Learn keys".to_string(),
            UiMode::AppearanceDialog { .. } => "Panels".to_string(),
            UiMode::ConfigurationDialog { .. } => "Panels".to_string(),
            UiMode::VfsOptionsDialog { .. } => "Panels".to_string(),
            UiMode::PauseAfterRun => "Panels".to_string(),
        }
    }
    /// GNU mc(1) listing movement. `page_rows` is panel body rows from
    /// `handle_key` (never `crossterm::terminal::size()`). Page size is
    /// [`listing_page_capacity`]: Full/Long/User = `page_rows`; Brief =
    /// `page_rows * brief_columns`. After the move, `ensure_visible` keeps the
    /// cursor in the drawn window. Empty listings are no-ops.
    fn apply_listing_move(&mut self, page_rows: usize, op: impl FnOnce(&mut PanelState, usize)) {
        {
            let p = self.active_panel_mut();
            let cap = listing_page_capacity(p.listing, p.brief_columns, page_rows);
            op(p, cap);
            p.ensure_visible(cap);
        }
        self.sync_other_preview_target();
    }

    /// GNU mc(1) Up / C-p: one listing entry backward. Does not wrap.
    pub fn move_up_by(&mut self, page_rows: usize) {
        self.apply_listing_move(page_rows, |p, _| p.move_up());
    }

    /// GNU mc(1) Down / C-n: one listing entry forward. Does not wrap.
    pub fn move_down_by(&mut self, page_rows: usize) {
        self.apply_listing_move(page_rows, |p, _| p.move_down());
    }

    /// GNU mc(1) Prev Page / Page Up / Alt-v.
    pub fn page_up_by(&mut self, page_rows: usize) {
        self.apply_listing_move(page_rows, |p, cap| p.page_up(cap));
    }

    /// GNU mc(1) Next Page / Page Down / C-v. Clamps to the last entry.
    pub fn page_down_by(&mut self, page_rows: usize) {
        self.apply_listing_move(page_rows, |p, cap| p.page_down(cap));
    }

    /// GNU mc(1) Home / A1: first listing entry (`..` when present).
    pub fn home_by(&mut self, page_rows: usize) {
        self.apply_listing_move(page_rows, |p, _| p.home());
    }

    /// GNU mc(1) End / C1: last listing entry.
    pub fn end_by(&mut self, page_rows: usize) {
        self.apply_listing_move(page_rows, |p, _| p.end());
    }

    /// GNU mc(1) Alt-t: cycle the **active** panel listing format
    /// (Full → Brief → Long → User → Full) and keep the cursor in view.
    /// Uses `page_rows` from `handle_key` (never `crossterm::terminal::size()`).
    ///
    /// Quick view / Info / Tree: GNU applying a listing format restores Listing
    /// mode, then shows the next format. Alt-t is the same path.
    pub fn cycle_listing_format_by(&mut self, page_rows: usize) {
        let p = self.active_panel_mut();
        p.listing = p.listing.cycle();
        if matches!(p.listing, ListingFormat::User) && p.user_format.trim().is_empty() {
            p.user_format = DEFAULT_USER_LISTING_FORMAT.to_string();
        }
        p.mode = PanelMode::Listing;
        let cap = listing_page_capacity(p.listing, p.brief_columns, page_rows);
        p.ensure_visible(cap);
    }

    /// GNU mc(1) Alt-g: top visible listing row. Uses `page_rows` from `handle_key`.
    pub fn jump_visible_top_by(&mut self, page_rows: usize) {
        self.active_panel_mut().jump_visible_top(page_rows);
        self.sync_other_preview_target();
    }

    /// GNU mc(1) Alt-r: middle visible listing row. Uses `page_rows` from `handle_key`.
    pub fn jump_visible_middle_by(&mut self, page_rows: usize) {
        self.active_panel_mut().jump_visible_middle(page_rows);
        self.sync_other_preview_target();
    }

    /// GNU mc(1) Alt-j: bottom visible listing row. Uses `page_rows` from `handle_key`.
    pub fn jump_visible_bottom_by(&mut self, page_rows: usize) {
        self.active_panel_mut().jump_visible_bottom(page_rows);
        self.sync_other_preview_target();
    }

    /// Push an internal module (editor / viewer / diff) as a new screen.
    /// Parks the currently displayed module, if any. Does not create extra
    /// file-manager screens — panels remain index 0.
    pub fn push_screen(&mut self, mode: UiMode) {
        if self.screen_idx > 0 {
            let slot = self.screen_idx - 1;
            if slot < self.screens.len() {
                std::mem::swap(&mut self.ui_mode, &mut self.screens[slot]);
            }
        }
        self.screens.push(mode);
        self.screen_idx = self.screens.len();
        std::mem::swap(&mut self.ui_mode, &mut self.screens[self.screen_idx - 1]);
    }

    /// Switch to screen `new_idx` (0 = panels). Wraps are the caller's job.
    pub fn switch_screen(&mut self, new_idx: usize) {
        let max = self.screens.len();
        let new_idx = new_idx.min(max);
        if new_idx == self.screen_idx {
            return;
        }
        if self.screen_idx > 0 {
            let slot = self.screen_idx - 1;
            if slot < self.screens.len() {
                std::mem::swap(&mut self.ui_mode, &mut self.screens[slot]);
            }
        }
        self.screen_idx = new_idx;
        if new_idx == 0 {
            self.ui_mode = UiMode::Normal;
        } else {
            std::mem::swap(&mut self.ui_mode, &mut self.screens[new_idx - 1]);
        }
    }

    /// Alt-} / Alt-{ wrap around panels + open modules.
    pub fn cycle_screen(&mut self, delta: isize) {
        let n = self.screens.len() + 1;
        if n <= 1 {
            return;
        }
        let next = (self.screen_idx as isize + delta).rem_euclid(n as isize) as usize;
        self.switch_screen(next);
    }

    /// Close the current internal module. Restores the previous screen, or
    /// panels if none remain. Direct `ui_mode` assignments with `screen_idx == 0`
    /// just return to panels (existing tests).
    pub fn close_current_screen(&mut self) {
        if self.screen_idx == 0 {
            self.ui_mode = UiMode::Normal;
            return;
        }
        let idx = self.screen_idx;
        self.ui_mode = UiMode::Normal;
        if idx > 0 && idx <= self.screens.len() {
            self.screens.remove(idx - 1);
        }
        self.screen_idx = 0;
        let prev = idx.saturating_sub(1);
        if prev > 0 {
            self.switch_screen(prev);
        }
    }

    /// Labels for the Screen list dialog, including the single file manager.
    pub fn screen_list_labels(&self) -> Vec<String> {
        let mut out = Vec::with_capacity(self.screens.len() + 1);
        out.push("Midnight Commander".to_string());
        for i in 1..=self.screens.len() {
            out.push(self.screen_label_at(i));
        }
        out
    }

    fn screen_label_at(&self, idx: usize) -> String {
        if idx == 0 {
            return "Midnight Commander".to_string();
        }
        Self::module_label(self.screen_mode_at(idx))
    }

    fn screen_mode_at(&self, idx: usize) -> &UiMode {
        if self.screen_idx == idx {
            unwrap_screen_overlay(&self.ui_mode)
        } else if idx > 0 && idx <= self.screens.len() {
            unwrap_screen_overlay(&self.screens[idx - 1])
        } else {
            unwrap_screen_overlay(&self.ui_mode)
        }
    }

    fn module_label(mode: &UiMode) -> String {
        match mode {
            UiMode::Editor { buf, .. } => {
                let name = buf
                    .path
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "(no name)".to_string());
                format!("Editor: {name}")
            }
            UiMode::Viewer { path, .. } => {
                let name = path
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                format!("Viewer: {name}")
            }
            UiMode::Diff(s) => {
                let left = s
                    .left_path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| s.left_path.display().to_string());
                let right = s
                    .right_path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| s.right_path.display().to_string());
                format!("Diff: {left} | {right}")
            }
            _ => "Screen".to_string(),
        }
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
            symlink_target: None,
            is_exe: false,
            size: 0,
            modified: std::time::SystemTime::UNIX_EPOCH,
            accessed: std::time::SystemTime::UNIX_EPOCH,
            changed: std::time::SystemTime::UNIX_EPOCH,
            permissions: 0,
            owner: None,
            group: None,
            nlink: 1,
            inode: 0,
            is_stale_symlink: false,
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
                symlink_target: meta.symlink_target,
                is_exe: meta.is_executable,
                size: meta.size,
                modified: meta.modified,
                accessed: meta.accessed,
                changed: meta.changed,
                permissions: meta.permissions,
                owner: meta.owner,
                group: meta.group,
                nlink: meta.nlink,
                inode: meta.inode,
                is_stale_symlink: meta.is_symlink && Self::local_symlink_is_stale(p),
            });
        }
        let caption = self.active_panel().cwd.clone();
        let reverse_files_only = self.panel_opts.reverse_files_only;
        self.active_panel_mut()
            .set_panelized_entries_with(caption, entries, reverse_files_only);
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
        let mut flags = self.config_opts.copy_flags();
        flags.follow_links = self.copy_op_flags.follow_links;
        flags.preserve_attrs = self.copy_op_flags.preserve_attrs;
        flags.dive_into_subdir = self.copy_op_flags.dive_into_subdir;
        flags.stable_symlinks = self.copy_op_flags.stable_symlinks;
        let job_id = match op {
            CopyMoveOp::Copy => self.jobs.spawn_copy_with_flags(&src, &dst, flags),
            CopyMoveOp::Move => self.jobs.spawn_move_with_flags(&src, &dst, flags),
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
            crate::jobs::JobStatus::Queued
            | crate::jobs::JobStatus::Running
            | crate::jobs::JobStatus::Stopped => Ok(()),
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

/// Path of `to` relative to directory `from`. Used for GNU C-x v.
fn relative_path(from: &Path, to: &Path) -> PathBuf {
    let from = from.components().collect::<Vec<_>>();
    let to = to.components().collect::<Vec<_>>();
    let mut i = 0usize;
    while i < from.len() && i < to.len() && from[i] == to[i] {
        i += 1;
    }
    let mut out = PathBuf::new();
    for _ in i..from.len() {
        out.push("..");
    }
    for comp in &to[i..] {
        out.push(comp.as_os_str());
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

/// Recompute octal mode from GNU Chmod checkboxes (permission bits only).
#[allow(clippy::too_many_arguments)]
pub fn chmod_mode_from_bits(
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
) -> u32 {
    let mut m = 0u32;
    if ur {
        m |= 0o400;
    }
    if uw {
        m |= 0o200;
    }
    if ux {
        m |= 0o100;
    }
    if gr {
        m |= 0o040;
    }
    if gw {
        m |= 0o020;
    }
    if gx {
        m |= 0o010;
    }
    if or_ {
        m |= 0o004;
    }
    if ow {
        m |= 0o002;
    }
    if ox {
        m |= 0o001;
    }
    if suid {
        m |= 0o4000;
    }
    if sgid {
        m |= 0o2000;
    }
    if sticky {
        m |= 0o1000;
    }
    m
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
    fn reget_offered_only_for_copy_with_shorter_nonzero_dest() {
        assert!(reget_offered(CopyMoveOp::Copy, 100, 40));
        assert!(!reget_offered(CopyMoveOp::Copy, 100, 0));
        assert!(!reget_offered(CopyMoveOp::Copy, 100, 100));
        assert!(!reget_offered(CopyMoveOp::Copy, 100, 120));
        assert!(!reget_offered(CopyMoveOp::Move, 100, 40));
    }

    #[test]
    fn overwrite_tab_order_includes_reget_abort_and_checkbox() {
        let with = overwrite_tab_order(CopyMoveOp::Copy, 100, 40);
        assert!(with.contains(&OverwriteFocus::Reget));
        assert!(with.contains(&OverwriteFocus::Abort));
        assert_eq!(*with.last().unwrap(), OverwriteFocus::ZeroLength);
        let rows = overwrite_button_rows(CopyMoveOp::Copy, 100, 40);
        let flat: Vec<_> = rows.into_iter().flatten().collect();
        assert!(flat.contains(&OverwriteFocus::Reget));
        assert!(!flat.contains(&OverwriteFocus::ZeroLength));

        let without = overwrite_tab_order(CopyMoveOp::Copy, 100, 0);
        assert!(!without.contains(&OverwriteFocus::Reget));
        assert!(without.contains(&OverwriteFocus::Abort));
        let rows_empty = overwrite_button_rows(CopyMoveOp::Copy, 100, 0);
        let flat_empty: Vec<_> = rows_empty.into_iter().flatten().collect();
        assert!(!flat_empty.contains(&OverwriteFocus::Reget));

        let move_order = overwrite_tab_order(CopyMoveOp::Move, 100, 40);
        assert!(!move_order.contains(&OverwriteFocus::Reget));
        let ge = overwrite_tab_order(CopyMoveOp::Copy, 50, 50);
        assert!(!ge.contains(&OverwriteFocus::Reget));
    }

    #[test]
    fn skip_zero_length_overwrite_only_when_checkbox_and_zero_src() {
        assert!(skip_zero_length_overwrite(true, 0, 10));
        assert!(!skip_zero_length_overwrite(false, 0, 10));
        assert!(!skip_zero_length_overwrite(true, 5, 10));
        assert!(!skip_zero_length_overwrite(true, 0, 0));
    }

    #[test]
    fn cycle_overwrite_focus_wraps() {
        let order = overwrite_tab_order(CopyMoveOp::Copy, 100, 40);
        assert_eq!(
            cycle_overwrite_focus(OverwriteFocus::Yes, &order, false),
            OverwriteFocus::No
        );
        assert_eq!(
            cycle_overwrite_focus(OverwriteFocus::ZeroLength, &order, false),
            OverwriteFocus::Yes
        );
        assert_eq!(
            cycle_overwrite_focus(OverwriteFocus::Yes, &order, true),
            OverwriteFocus::ZeroLength
        );
    }

    #[test]
    fn jobs_dialog_focus_cycles_gnu_buttons() {
        use JobsDialogFocus::*;
        assert_eq!(List.cycle(false), Stop);
        assert_eq!(Stop.cycle(false), Restart);
        assert_eq!(Restart.cycle(false), Kill);
        assert_eq!(Kill.cycle(false), Cleanup);
        assert_eq!(Cleanup.cycle(false), Ok);
        assert_eq!(Ok.cycle(false), List);
        assert_eq!(List.cycle(true), Ok);
        assert_eq!(Stop.cycle(true), List);
        assert_eq!(Stop.button_label(), Some("Stop"));
        assert_eq!(Restart.button_label(), Some("Restart"));
        assert_eq!(Kill.button_label(), Some("Kill"));
        assert_eq!(List.button_label(), None);
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

    #[test]
    fn equalize_panels_restores_equal_split_without_swapping() {
        let (mut app, _tmp, left_dir, right_dir) = app_with_distinct_panes();
        let left_names: Vec<_> = app.left.entries.iter().map(|e| e.name.clone()).collect();
        let right_names: Vec<_> = app.right.entries.iter().map(|e| e.name.clone()).collect();
        app.layout.panel_ratio = 0.8;
        app.handle_action(Action::EqualizePanels).unwrap();
        assert!((app.layout.panel_ratio - 0.5).abs() <= f32::EPSILON);
        assert!(app.layout.equal_split);
        assert_eq!(app.left.cwd, left_dir);
        assert_eq!(app.right.cwd, right_dir);
        assert_eq!(app.active, PaneSide::Left);
        let left_after: Vec<_> = app.left.entries.iter().map(|e| e.name.clone()).collect();
        let right_after: Vec<_> = app.right.entries.iter().map(|e| e.name.clone()).collect();
        assert_eq!(left_after, left_names);
        assert_eq!(right_after, right_names);
    }

    #[test]
    fn default_panel_split_is_vertical() {
        let (app, _tmp, _, _) = app_with_distinct_panes();
        assert!(!app.layout.horizontal_split);
        assert!(app.layout.equal_split);
        assert!((app.layout.panel_ratio - 0.5).abs() <= f32::EPSILON);
    }

    #[test]
    fn toggle_panel_split_flips_orientation_without_equalizing() {
        let (mut app, _tmp, left_dir, right_dir) = app_with_distinct_panes();
        app.layout.panel_ratio = 0.8;
        app.layout.equal_split = false;
        app.handle_action(Action::TogglePanelSplit).unwrap();
        assert!(app.layout.horizontal_split);
        assert!((app.layout.panel_ratio - 0.8).abs() <= f32::EPSILON);
        assert!(!app.layout.equal_split);
        assert_eq!(app.left.cwd, left_dir);
        assert_eq!(app.right.cwd, right_dir);
        assert_eq!(app.active, PaneSide::Left);
        app.handle_action(Action::TogglePanelSplit).unwrap();
        assert!(!app.layout.horizontal_split);
        assert!((app.layout.panel_ratio - 0.8).abs() <= f32::EPSILON);
    }

    #[test]
    fn equalize_panels_sets_ratio_while_horizontal() {
        let (mut app, _tmp, left_dir, right_dir) = app_with_distinct_panes();
        app.layout.horizontal_split = true;
        app.layout.panel_ratio = 0.8;
        app.layout.equal_split = false;
        app.handle_action(Action::EqualizePanels).unwrap();
        assert!(app.layout.horizontal_split);
        assert!((app.layout.panel_ratio - 0.5).abs() <= f32::EPSILON);
        assert!(app.layout.equal_split);
        assert_eq!(app.left.cwd, left_dir);
        assert_eq!(app.right.cwd, right_dir);
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
    fn begin_file_op_copy_with_cow_off_writes_bytes() {
        let (mut app, tmp, _, _) = app_with_distinct_panes();
        let src = tmp.path().join("src.bin");
        let dst = tmp.path().join("dst.bin");
        std::fs::write(&src, b"no-cow-copy").unwrap();
        app.config_opts.use_cow_file_cloning = false;
        app.config_opts.preallocate_space = false;
        app.begin_file_op(CopyMoveOp::Copy, src.clone(), dst.clone())
            .unwrap();
        wait_until_file_op_settled(&mut app, 5_000);
        assert!(matches!(app.ui_mode, UiMode::Normal));
        assert_eq!(std::fs::read(&dst).unwrap(), b"no-cow-copy");
        assert_eq!(std::fs::read(&src).unwrap(), b"no-cow-copy");
    }

    #[test]
    fn begin_file_op_copy_with_preallocate_on_writes_bytes() {
        let (mut app, tmp, _, _) = app_with_distinct_panes();
        let src = tmp.path().join("src.bin");
        let dst = tmp.path().join("dst.bin");
        std::fs::write(&src, vec![0x11u8; 2048]).unwrap();
        app.config_opts.preallocate_space = true;
        app.config_opts.use_cow_file_cloning = false;
        app.begin_file_op(CopyMoveOp::Copy, src.clone(), dst.clone())
            .unwrap();
        wait_until_file_op_settled(&mut app, 5_000);
        assert!(matches!(app.ui_mode, UiMode::Normal));
        assert_eq!(std::fs::read(&dst).unwrap(), vec![0x11u8; 2048]);
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

    #[test]
    fn cycle_listing_format_walks_full_brief_long_user() {
        use crate::panel::ListingFormat;
        let (mut app, _tmp, _, _) = app_with_distinct_panes();
        assert_eq!(app.left.listing, ListingFormat::Full);
        assert_eq!(app.right.listing, ListingFormat::Full);
        let want = [
            ListingFormat::Brief,
            ListingFormat::Long,
            ListingFormat::User,
            ListingFormat::Full,
        ];
        for next in want {
            app.handle_action(Action::CycleListingFormat).unwrap();
            assert_eq!(app.left.listing, next);
            assert_eq!(
                app.right.listing,
                ListingFormat::Full,
                "inactive panel must stay Full across the GNU Alt-t cycle"
            );
        }
    }

    #[test]
    fn cycle_listing_format_right_panel_only() {
        use crate::panel::ListingFormat;
        let (mut app, _tmp, _, _) = app_with_distinct_panes();
        app.active = PaneSide::Right;
        app.handle_action(Action::CycleListingFormat).unwrap();
        assert_eq!(app.right.listing, ListingFormat::Brief);
        assert_eq!(app.left.listing, ListingFormat::Full);
        app.handle_action(Action::CycleListingFormat).unwrap();
        assert_eq!(app.right.listing, ListingFormat::Long);
        assert_eq!(app.left.listing, ListingFormat::Full);
    }

    #[test]
    fn cycle_listing_format_survives_panel_switch() {
        use crate::panel::ListingFormat;
        let (mut app, _tmp, _, _) = app_with_distinct_panes();
        app.handle_action(Action::CycleListingFormat).unwrap();
        app.handle_action(Action::CycleListingFormat).unwrap();
        assert_eq!(app.left.listing, ListingFormat::Long);
        app.handle_action(Action::SwitchPanel).unwrap();
        assert_eq!(app.active, PaneSide::Right);
        assert_eq!(app.left.listing, ListingFormat::Long);
        app.handle_action(Action::SwitchPanel).unwrap();
        assert_eq!(app.active, PaneSide::Left);
        assert_eq!(app.left.listing, ListingFormat::Long);
        assert_eq!(app.right.listing, ListingFormat::Full);
    }

    #[test]
    fn cycle_listing_format_empty_user_string_uses_dialog_default() {
        use crate::panel::{ListingFormat, DEFAULT_USER_LISTING_FORMAT};
        let (mut app, _tmp, _, _) = app_with_distinct_panes();
        app.left.user_format.clear();
        app.left.listing = ListingFormat::Long;
        app.handle_action(Action::CycleListingFormat).unwrap();
        assert_eq!(app.left.listing, ListingFormat::User);
        assert_eq!(app.left.user_format, DEFAULT_USER_LISTING_FORMAT);
    }

    #[test]
    fn cycle_listing_format_horizontal_split_still_active_panel_only() {
        use crate::panel::ListingFormat;
        let (mut app, _tmp, _, _) = app_with_distinct_panes();
        app.layout.horizontal_split = true;
        app.handle_action(Action::CycleListingFormat).unwrap();
        assert_eq!(app.left.listing, ListingFormat::Brief);
        assert_eq!(app.right.listing, ListingFormat::Full);
        app.active = PaneSide::Right;
        app.handle_action(Action::CycleListingFormat).unwrap();
        assert_eq!(app.right.listing, ListingFormat::Brief);
        assert_eq!(app.left.listing, ListingFormat::Brief);
    }

    #[test]
    fn cycle_listing_format_restores_listing_panel_mode() {
        use crate::panel::{ListingFormat, PanelMode};
        let (mut app, _tmp, _, _) = app_with_distinct_panes();
        app.left.mode = PanelMode::QuickView;
        app.handle_action(Action::CycleListingFormat).unwrap();
        assert_eq!(app.left.mode, PanelMode::Listing);
        assert_eq!(app.left.listing, ListingFormat::Brief);
        app.left.mode = PanelMode::Info;
        app.handle_action(Action::CycleListingFormat).unwrap();
        assert_eq!(app.left.mode, PanelMode::Listing);
        assert_eq!(app.left.listing, ListingFormat::Long);
        app.left.mode = PanelMode::Tree;
        app.handle_action(Action::CycleListingFormat).unwrap();
        assert_eq!(app.left.mode, PanelMode::Listing);
        assert_eq!(app.left.listing, ListingFormat::User);
    }
}
