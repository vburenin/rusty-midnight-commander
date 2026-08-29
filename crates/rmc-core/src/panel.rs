use crate::dirtree::DirectoryTreeState;
use crate::matchutil;
use crate::selection::Selection;
use crate::sorting::{self, SortDir};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VisibleJump {
    Top,
    Middle,
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelMode {
    Listing,
    QuickView,
    Info,
    Tree,
}

#[derive(Debug, Clone)]
pub struct TreeEntry {
    pub path: PathBuf,
    pub depth: usize,
}

/// Per-panel tree figure for Left/Right → Tree (not the Command-menu dialog).
///
/// Reuses [`DirectoryTreeState`] so dynamic/static navigation, forget, rescan,
/// and search stay on one engine.
#[derive(Debug, Clone)]
pub struct TreeState {
    pub figure: DirectoryTreeState,
    /// GNU tree view: typed characters extend search only after C-s / Alt-s.
    pub search_active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ListingFormat {
    Full,
    Brief,
    Long,
    /// User-defined format string stored on the panel (`user_format`).
    User,
}

impl ListingFormat {
    /// GNU mc(1) Alt-t: cycle Full → Brief → Long → User → Full.
    ///
    /// Public manpage: switch to brief, long, user-defined, then back to the default (Full).
    pub fn cycle(self) -> Self {
        match self {
            Self::Full => Self::Brief,
            Self::Brief => Self::Long,
            Self::Long => Self::User,
            Self::User => Self::Full,
        }
    }
}

/// Default User listing format (Listing Format dialog prefills this; Alt-t User
/// reuses it when the panel has no configured string). GNU-ish subset; `half`
/// is ignored by the parser. Do not invent a second field language.
pub const DEFAULT_USER_LISTING_FORMAT: &str = "half type name | size | perm";

/// GNU brief listing packs names into 1–9 side-by-side columns (mc(1) default is 2).
pub const BRIEF_COLUMNS_DEFAULT: u8 = 2;
pub const BRIEF_COLUMNS_MAX: u8 = 9;

/// Clamp a Brief column count to the GNU 1–9 range.
pub fn clamp_brief_columns(n: u8) -> u8 {
    n.clamp(1, BRIEF_COLUMNS_MAX)
}

/// Visible listing slots for `page_rows` of panel body height.
///
/// Brief multiplies by the column count so PageUp/ensure_visible walk packed names.
/// Callers must pass `page_rows` from `handle_key` (never `crossterm::terminal::size()`).
pub fn listing_page_capacity(listing: ListingFormat, brief_columns: u8, page_rows: usize) -> usize {
    let rows = page_rows.max(1);
    match listing {
        ListingFormat::Brief => rows * clamp_brief_columns(brief_columns) as usize,
        ListingFormat::Full | ListingFormat::Long | ListingFormat::User => rows,
    }
}

/// Inner width of one Brief name column (panel width includes the frame).
pub fn brief_column_width(panel_width: u16, columns: u8) -> u16 {
    let n = clamp_brief_columns(columns) as u16;
    let inner = panel_width.saturating_sub(2);
    let seps = n.saturating_sub(1);
    inner.saturating_sub(seps) / n.max(1)
}

/// Column-major Brief cell: fill each column top-to-bottom, then left-to-right.
pub fn brief_entry_index(scroll_top: usize, row: usize, col: usize, rows: usize) -> usize {
    scroll_top + row + col * rows
}

/// Which Brief column contains `inner_x` (x relative to the panel's left edge).
pub fn brief_column_at_x(inner_x: u16, per_col_width: u16, columns: u8) -> usize {
    let n = clamp_brief_columns(columns) as usize;
    let stride = per_col_width.saturating_add(1).max(1);
    let col = inner_x.saturating_sub(1) / stride;
    (col as usize).min(n.saturating_sub(1))
}

/// GNU mc(1) user-defined listing format tokens (public manpage names).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserFormatToken {
    Name,
    Size,
    /// Directory placeholder size: `SUB-DIR` / `UP--DIR` (files use [`Size`]).
    Bsize,
    Perm,
    /// Octal permission bits (`mode` in mc(1); not the `perm` rwx string).
    Mode,
    Type,
    /// `*` when the file is tagged, space otherwise.
    Mark,
    Mtime,
    Atime,
    Ctime,
    Nlink,
    Owner,
    Group,
    /// Numeric uid (`nuid`).
    Nuid,
    /// Numeric gid (`ngid`).
    Ngid,
    Inode,
    /// `|` in the format string: a literal column gap.
    Gap,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub is_symlink: bool,
    /// Stored `readlink` text (not canonicalized). `None` if this is not a symlink
    /// or if reading the target failed.
    pub symlink_target: Option<String>,
    pub is_exe: bool,
    pub size: u64,
    pub modified: SystemTime,
    /// Last access time (`st_atime`). Archives and remote copy `modified`.
    pub accessed: SystemTime,
    /// Inode status-change time (`st_ctime`). Archives and remote copy `modified`.
    pub changed: SystemTime,
    pub permissions: u32,
    pub owner: Option<String>,
    pub group: Option<String>,
    /// Hard-link count (`st_nlink`). Missing stat falls back to 1. Local listing
    /// `..` uses the parent directory's real nlink; panelize markers stay 1.
    pub nlink: u64,
    /// Filesystem inode (`st_ino`). Archives and remote use 0.
    pub inode: u64,
    /// Unix uid (`st_uid`) for user-format `nuid`. Archives and remote use 0.
    #[serde(default)]
    pub uid: u32,
    /// Unix gid (`st_gid`) for user-format `ngid`. Archives and remote use 0.
    #[serde(default)]
    pub gid: u32,
    /// GNU mc(1) `type` `!`: symlink whose target does not exist.
    #[serde(default)]
    pub is_stale_symlink: bool,
}

impl FileEntry {
    pub fn is_parent_marker(&self) -> bool {
        self.name == ".."
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortBy {
    Name,
    Ext,
    Size,
    /// Modification time (`mtime`).
    Time,
    /// Access time (`atime`).
    Atime,
    /// Change time (`ctime` / inode-status change).
    Ctime,
    Inode,
    /// Listing order from `list_dir` after the `..` marker.
    Unsorted,
}

/// Local-directory identity used by GNU mc Fast reload.
///
/// GNU mc skips a re-list when POSIX `stat()` of the panel cwd still matches the
/// snapshot from the last listing (`st_mtime` and `st_ctime`). Remote/archive/extfs
/// paths fail local `stat()` and are never skipped here (those use Directory cache
/// timeout instead). `nlink` and `size` are included because directory growth and
/// subdirectory create/delete often show up there even when timestamps are coarse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirReloadStamp {
    pub mtime: SystemTime,
    pub ctime_sec: i64,
    pub ctime_nsec: i64,
    pub nlink: u64,
    pub size: u64,
}

impl DirReloadStamp {
    /// Snapshot `path` via local `stat()`. `None` if it is not a readable local directory.
    pub fn from_local_dir(path: &Path) -> Option<Self> {
        let md = std::fs::metadata(path).ok()?;
        if !md.is_dir() {
            return None;
        }
        let mtime = md.modified().ok()?;
        #[cfg(unix)]
        let (ctime_sec, ctime_nsec, nlink) = {
            use std::os::unix::fs::MetadataExt;
            (md.ctime(), md.ctime_nsec(), md.nlink())
        };
        #[cfg(not(unix))]
        let (ctime_sec, ctime_nsec, nlink) = (0_i64, 0_i64, 1_u64);
        Some(Self {
            mtime,
            ctime_sec,
            ctime_nsec,
            nlink,
            size: md.len(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct PanelState {
    pub cwd: PathBuf,
    pub entries: Vec<FileEntry>,
    pub cursor: usize,
    pub scroll_top: usize,
    /// Current panel mode (Listing/QuickView/Info/Tree)
    pub mode: PanelMode,
    /// Tree state when mode == Tree
    pub tree: Option<TreeState>,
    pub show_hidden: bool,
    pub sort_by: SortBy,
    pub sort_dir: SortDir,
    pub dirs_first: bool,
    pub listing: ListingFormat,
    /// User-defined listing format string used when `listing == ListingFormat::User`.
    pub user_format: String,
    /// Brief listing column count (GNU 1–9; default 2).
    pub brief_columns: u8,
    pub selection: Selection,
    // When panelized, entries show a virtual list; pressing `..` or leaving mode restores saved state.
    pub panelized: Option<PanelizeSaved>,
    /// Optional filename filter. `None`, empty, or "*" shows all.
    /// Glob vs regex and case sensitivity come from the per-panel Filter dialog
    /// flags below (not the global Use shell patterns option).
    pub filter_glob: Option<String>,
    /// Filter dialog **Regular expression**: pattern is a regex instead of a glob.
    pub filter_regex: bool,
    /// Filter dialog **Files only**: when true, directories always stay listed
    /// (only files are matched). When false, directories must match too. `..`
    /// is always kept.
    pub filter_files_only: bool,
    /// Filter dialog **Case sensitive**. Default true (GNU mc).
    pub filter_case_sensitive: bool,
    /// Local dir mtime/ctime/nlink/size from the last listing (Fast reload).
    pub dir_reload_stamp: Option<DirReloadStamp>,
    /// `show_hidden` used for the last listing (Ctrl-H must re-list even if mtime is unchanged).
    pub dir_reload_show_hidden: Option<bool>,
    /// Filter glob used for the last listing (changing the filter must re-list).
    pub dir_reload_filter: Option<String>,
    /// Filter flags used for the last listing (changing them must re-list).
    pub dir_reload_filter_regex: bool,
    pub dir_reload_filter_files_only: bool,
    pub dir_reload_filter_case_sensitive: bool,
    /// Byte offset for the reduced Quick view viewer (panel mode, not F3).
    pub preview_offset: u64,
    /// Path last shown in Quick view; used to reset `preview_offset` on change.
    pub preview_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct PanelizeSaved {
    pub cwd: PathBuf,
    pub entries: Vec<FileEntry>,
    pub cursor: usize,
    pub scroll_top: usize,
}

impl PanelState {
    pub fn new<P: AsRef<Path>>(cwd: P) -> Self {
        Self {
            cwd: cwd.as_ref().to_path_buf(),
            entries: Vec::new(),
            cursor: 0,
            scroll_top: 0,
            mode: PanelMode::Listing,
            tree: None,
            show_hidden: false,
            sort_by: SortBy::Name,
            sort_dir: SortDir::Asc,
            dirs_first: true,
            listing: ListingFormat::Full,
            user_format: DEFAULT_USER_LISTING_FORMAT.to_string(),
            brief_columns: BRIEF_COLUMNS_DEFAULT,
            selection: Selection::default(),
            panelized: None,
            filter_glob: None,
            filter_regex: false,
            filter_files_only: false,
            filter_case_sensitive: true,
            dir_reload_stamp: None,
            dir_reload_show_hidden: None,
            dir_reload_filter: None,
            dir_reload_filter_regex: false,
            dir_reload_filter_files_only: false,
            dir_reload_filter_case_sensitive: true,
            preview_offset: 0,
            preview_path: None,
        }
    }

    /// Store Left/Right Filter dialog values on this panel.
    /// Empty / `"*"` clears the pattern (show all, still honoring `show_hidden`).
    pub fn set_filename_filter(
        &mut self,
        pattern: &str,
        regular_expression: bool,
        files_only: bool,
        case_sensitive: bool,
    ) {
        let pat = pattern.trim();
        if pat.is_empty() || pat == "*" {
            self.filter_glob = None;
        } else {
            self.filter_glob = Some(pat.to_string());
        }
        self.filter_regex = regular_expression;
        self.filter_files_only = files_only;
        self.filter_case_sensitive = case_sensitive;
    }

    /// Record the local-directory stamp after a successful `list_dir`.
    pub fn capture_dir_reload_stamp(&mut self, show_hidden: bool) {
        self.dir_reload_stamp = DirReloadStamp::from_local_dir(&self.cwd);
        self.dir_reload_show_hidden = Some(show_hidden);
        self.dir_reload_filter = self.filter_glob.clone();
        self.dir_reload_filter_regex = self.filter_regex;
        self.dir_reload_filter_files_only = self.filter_files_only;
        self.dir_reload_filter_case_sensitive = self.filter_case_sensitive;
    }

    /// GNU mc Fast reload: reuse this panel's listing when the local dir is unchanged.
    /// Always `false` for remote/archive/extfs (local `stat()` fails).
    pub fn fast_reload_listing_is_current(&self, show_hidden: bool) -> bool {
        let Some(now) = DirReloadStamp::from_local_dir(&self.cwd) else {
            return false;
        };
        self.dir_reload_stamp == Some(now)
            && self.dir_reload_show_hidden == Some(show_hidden)
            && self.dir_reload_filter == self.filter_glob
            && self.dir_reload_filter_regex == self.filter_regex
            && self.dir_reload_filter_files_only == self.filter_files_only
            && self.dir_reload_filter_case_sensitive == self.filter_case_sensitive
    }

    pub fn set_entries(&mut self, entries: Vec<FileEntry>) {
        self.set_entries_with(entries, true);
    }

    pub fn set_entries_with(&mut self, mut entries: Vec<FileEntry>, reverse_files_only: bool) {
        // Separate parent marker (if any) to keep it visible and first.
        let mut parent_marker: Option<FileEntry> = None;
        entries.retain(|e| {
            if e.is_dir && e.name == ".." && parent_marker.is_none() {
                parent_marker = Some(e.clone());
                false
            } else {
                true
            }
        });
        // Apply filename filter if present and not equal to "*". Empty / "*" shows all.
        if let Some(pat) = self
            .filter_glob
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty() && *s != "*")
        {
            let glob = !self.filter_regex;
            let case_sensitive = self.filter_case_sensitive;
            let files_only = self.filter_files_only;
            entries.retain(|e| {
                if files_only && e.is_dir {
                    true
                } else {
                    matchutil::filename_pattern_matches_ex(pat, &e.name, glob, case_sensitive)
                }
            });
        }
        // Put parent marker back on top if exists.
        if let Some(pm) = parent_marker {
            let mut filtered = Vec::with_capacity(entries.len() + 1);
            filtered.push(pm);
            filtered.extend(entries);
            self.entries = filtered;
        } else {
            self.entries = entries;
        }
        self.apply_sort_with(reverse_files_only);
        if self.cursor >= self.entries.len() {
            self.cursor = self.entries.len().saturating_sub(1);
        }
    }

    /// After a directory re-list: keep the cursor on the same name when it still
    /// exists, otherwise clamp to the previous index (nearest remaining entry).
    /// Re-bind Insert marks by name so added/removed files do not shift tags.
    pub fn restore_selection_after_reload(
        &mut self,
        cursor_name: Option<&str>,
        cursor_idx: usize,
        marked_names: &[String],
    ) {
        if let Some(name) = cursor_name {
            if let Some(idx) = self.entries.iter().position(|e| e.name == name) {
                self.cursor = idx;
            } else {
                self.cursor = cursor_idx.min(self.entries.len().saturating_sub(1));
            }
        }
        // No previous name (empty listing): keep the index `set_entries_with` already clamped.
        if self.cursor < self.scroll_top {
            self.scroll_top = self.cursor;
        }
        self.selection.clear();
        for name in marked_names {
            if let Some(idx) = self.entries.iter().position(|e| e.name == *name) {
                self.selection.select(idx);
            }
        }
    }

    /// Default-true wrapper: GNU mc `reverse_files_only` defaults on.
    pub fn apply_sort(&mut self) {
        self.apply_sort_with(true);
    }

    /// Sort panel entries, honoring GNU mc `reverse_files_only`.
    ///
    /// When `dirs_first` and the panel sort is reverse:
    /// - `reverse_files_only == true` (default): directories stay name-ascending;
    ///   only the file group is reversed.
    /// - `reverse_files_only == false`: both the directory group and the file group reverse.
    ///
    /// When mixed (`dirs_first == false`), reverse applies to the whole list either way.
    pub fn apply_sort_with(&mut self, reverse_files_only: bool) {
        // Detach a possible parent marker at the top to keep it first
        let mut items = std::mem::take(&mut self.entries);
        let mut parent_marker: Option<FileEntry> = None;
        if !items.is_empty() && items[0].is_dir && items[0].name == ".." {
            parent_marker = Some(items.remove(0));
        }

        if self.dirs_first {
            let (mut dirs, mut rest): (Vec<_>, Vec<_>) = items.into_iter().partition(|e| e.is_dir);
            // Directories always sort by name; optionally ignore reverse.
            // Unsorted keeps list_dir order within each group (Reverse still flips).
            let dir_dir = if reverse_files_only {
                SortDir::Asc
            } else {
                self.sort_dir
            };
            if matches!(self.sort_by, SortBy::Unsorted) {
                sorting::sort_unsorted(&mut dirs, dir_dir);
                sorting::sort_unsorted(&mut rest, self.sort_dir);
            } else {
                sorting::sort_by_name(&mut dirs, dir_dir);
                sorting::sort_entries(&mut rest, self.sort_by, self.sort_dir);
            }
            self.entries = [dirs, rest].concat();
        } else {
            // Mixed directories/files together sorted uniformly (except parent marker)
            sorting::sort_entries(&mut items, self.sort_by, self.sort_dir);
            self.entries = items;
        }
        if let Some(pm) = parent_marker {
            self.entries.insert(0, pm);
        }
    }

    pub fn current_entry(&self) -> Option<&FileEntry> {
        self.entries.get(self.cursor)
    }
    pub fn current_entry_mut(&mut self) -> Option<&mut FileEntry> {
        self.entries.get_mut(self.cursor)
    }

    pub fn move_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }
    pub fn move_down(&mut self) {
        if self.cursor + 1 < self.entries.len() {
            self.cursor += 1;
        }
    }
    pub fn page_up(&mut self, page: usize) {
        self.cursor = self.cursor.saturating_sub(page);
    }
    pub fn page_down(&mut self, page: usize) {
        self.cursor = (self.cursor + page).min(self.entries.len().saturating_sub(1));
    }
    pub fn home(&mut self) {
        self.cursor = 0;
    }
    pub fn end(&mut self) {
        if !self.entries.is_empty() {
            self.cursor = self.entries.len() - 1;
        }
    }

    /// GNU mc(1) Alt-g: select the top file currently drawn in the panel.
    ///
    /// `page_rows` is the panel body height from `handle_key` (never TTY size).
    pub fn jump_visible_top(&mut self, page_rows: usize) {
        self.jump_visible(page_rows, VisibleJump::Top);
    }

    /// GNU mc(1) Alt-r: select the middle file currently drawn in the panel.
    pub fn jump_visible_middle(&mut self, page_rows: usize) {
        self.jump_visible(page_rows, VisibleJump::Middle);
    }

    /// GNU mc(1) Alt-j: select the bottom file currently drawn in the panel.
    pub fn jump_visible_bottom(&mut self, page_rows: usize) {
        self.jump_visible(page_rows, VisibleJump::Bottom);
    }

    fn jump_visible(&mut self, page_rows: usize, which: VisibleJump) {
        let len = self.entries.len();
        if len == 0 {
            return;
        }
        let last = len - 1;
        let slots = listing_page_capacity(self.listing, self.brief_columns, page_rows).max(1);
        let offset = self.scroll_top.min(last);
        // Files actually drawn (short listing / partial last page), not empty slots.
        let visible = slots.min(len - offset);
        let idx = match which {
            VisibleJump::Top => offset,
            VisibleJump::Middle => offset + visible / 2,
            VisibleJump::Bottom => offset + visible.saturating_sub(1),
        };
        self.cursor = idx.min(last);
        self.ensure_visible(slots);
    }

    pub fn ensure_visible(&mut self, content_rows: usize) {
        if self.cursor < self.scroll_top {
            self.scroll_top = self.cursor;
        } else if self.cursor >= self.scroll_top + content_rows {
            self.scroll_top = self.cursor.saturating_sub(content_rows.saturating_sub(1));
        }
    }

    pub fn is_panelized(&self) -> bool {
        self.panelized.is_some()
    }

    pub fn set_panelized_entries(&mut self, cwd_for_caption: PathBuf, entries: Vec<FileEntry>) {
        self.set_panelized_entries_with(cwd_for_caption, entries, true);
    }

    pub fn set_panelized_entries_with(
        &mut self,
        cwd_for_caption: PathBuf,
        entries: Vec<FileEntry>,
        reverse_files_only: bool,
    ) {
        // Save current state
        self.panelized = Some(PanelizeSaved {
            cwd: self.cwd.clone(),
            entries: std::mem::take(&mut self.entries),
            cursor: self.cursor,
            scroll_top: self.scroll_top,
        });
        // In panelized mode, keep the original cwd for caption to hint origin
        self.cwd = cwd_for_caption;
        self.cursor = 0;
        self.scroll_top = 0;
        self.set_entries_with(entries, reverse_files_only);
    }

    pub fn unpanelize(&mut self) {
        if let Some(saved) = self.panelized.take() {
            self.cwd = saved.cwd;
            self.entries = saved.entries;
            self.cursor = saved.cursor;
            self.scroll_top = saved.scroll_top;
        }
    }
}

/// Parse a GNU mc(1) user listing format string.
///
/// Tokens are case-insensitive and space-separated. `|` is a column gap.
/// Unknown tokens and layout specifiers (`half` / `full` / `space` / `1`–`9`)
/// are ignored. An optional `:width` suffix (e.g. `size:7`) is stripped so the
/// field still matches.
pub fn parse_user_listing_format(fmt: &str) -> Vec<UserFormatToken> {
    let mut out = Vec::new();
    for raw in fmt.split_whitespace() {
        let mut rest = raw;
        loop {
            if let Some((left, right)) = rest.split_once('|') {
                if let Some(tok) = classify_user_token(left) {
                    out.push(tok);
                }
                out.push(UserFormatToken::Gap);
                rest = right;
            } else {
                if let Some(tok) = classify_user_token(rest) {
                    out.push(tok);
                }
                break;
            }
        }
    }
    out
}

fn classify_user_token(raw: &str) -> Option<UserFormatToken> {
    if raw.is_empty() {
        return None;
    }
    let key = raw.split(':').next().unwrap_or(raw);
    match key.to_ascii_lowercase().as_str() {
        "name" => Some(UserFormatToken::Name),
        "size" => Some(UserFormatToken::Size),
        "bsize" => Some(UserFormatToken::Bsize),
        "perm" => Some(UserFormatToken::Perm),
        "mode" => Some(UserFormatToken::Mode),
        "type" => Some(UserFormatToken::Type),
        "mark" => Some(UserFormatToken::Mark),
        "mtime" | "time" => Some(UserFormatToken::Mtime),
        "atime" => Some(UserFormatToken::Atime),
        "ctime" => Some(UserFormatToken::Ctime),
        "nlink" => Some(UserFormatToken::Nlink),
        "owner" => Some(UserFormatToken::Owner),
        "group" => Some(UserFormatToken::Group),
        "nuid" => Some(UserFormatToken::Nuid),
        "ngid" => Some(UserFormatToken::Ngid),
        "inode" => Some(UserFormatToken::Inode),
        "|" => Some(UserFormatToken::Gap),
        _ => None,
    }
}

fn user_token_width(tok: UserFormatToken) -> usize {
    match tok {
        UserFormatToken::Name => 0,
        UserFormatToken::Size | UserFormatToken::Bsize => 8,
        UserFormatToken::Perm => 10,
        UserFormatToken::Mode => 4,
        UserFormatToken::Type | UserFormatToken::Mark => 1,
        UserFormatToken::Mtime | UserFormatToken::Atime | UserFormatToken::Ctime => 12,
        UserFormatToken::Nlink => 4,
        UserFormatToken::Owner | UserFormatToken::Group => 8,
        UserFormatToken::Nuid | UserFormatToken::Ngid | UserFormatToken::Inode => 7,
        UserFormatToken::Gap => 1,
    }
}

fn fit_left(s: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let n = s.chars().count();
    if n > width {
        if width == 1 {
            s.chars().take(1).collect()
        } else {
            s.chars()
                .take(width.saturating_sub(1))
                .chain("…".chars())
                .collect()
        }
    } else {
        let mut out = s.to_string();
        out.push_str(&" ".repeat(width - n));
        out
    }
}

fn fit_right(s: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let n = s.chars().count();
    if n > width {
        s.chars().skip(n - width).collect()
    } else {
        let mut out = " ".repeat(width - n);
        out.push_str(s);
        out
    }
}

fn user_perm_string(mode: u32, is_dir: bool) -> String {
    let mut s = String::new();
    s.push(if is_dir { 'd' } else { '-' });
    let bits = [
        (0o400, 'r'),
        (0o200, 'w'),
        (0o100, 'x'),
        (0o040, 'r'),
        (0o020, 'w'),
        (0o010, 'x'),
        (0o004, 'r'),
        (0o002, 'w'),
        (0o001, 'x'),
    ];
    for (bit, ch) in bits {
        s.push(if mode & bit != 0 { ch } else { '-' });
    }
    s
}

/// GNU mc(1) `type` token (Full listing `half type name | size | mtime`).
///
/// Always one cell: `*` executable, `/` directory (including parent `..`), `@`
/// symlink, `=` socket, `-` char device, `+` block device, `|` FIFO, `~`
/// symlink to a directory, `!` stale symlink, and a space for a regular file.
/// The space is not optional — names must stay aligned with `/docs` and
/// `*run.sh`. Live GNU Full listing paints `/..`, not a leading space.
pub fn listing_type_char(ent: &FileEntry) -> char {
    if ent.is_stale_symlink {
        return '!';
    }
    if ent.is_symlink && ent.is_dir {
        return '~';
    }
    if ent.is_symlink {
        return '@';
    }
    if ent.is_dir {
        return '/';
    }
    // Unix `st_mode` file-type bits when present (local `lstat`).
    match ent.permissions & 0o170_000 {
        0o140_000 => '=',
        0o020_000 => '-',
        0o060_000 => '+',
        0o010_000 => '|',
        _ if ent.is_exe => '*',
        _ => ' ',
    }
}

/// GNU Full listing header sort mark (mc 4.8 screenshots / mc-devel).
/// `.` = primary field ascending; `,` = reverse. Letter: n/e/m/a/c/s/i/u.
pub fn full_listing_sort_indicator(by: SortBy, dir: SortDir) -> [char; 2] {
    let mark = match dir {
        SortDir::Asc => '.',
        SortDir::Desc => ',',
    };
    let letter = match by {
        SortBy::Name => 'n',
        SortBy::Ext => 'e',
        SortBy::Time => 'm',
        SortBy::Atime => 'a',
        SortBy::Ctime => 'c',
        SortBy::Size => 's',
        SortBy::Inode => 'i',
        SortBy::Unsorted => 'u',
    };
    [mark, letter]
}

/// Full listing size cell: raw bytes by default (GNU `size`), SI abbreviations
/// when Options → Panels → Use SI size units is on. Only `..` is `UP--DIR`.
/// Directories show the directory inode size, not a blank or `UP--DIR`.
pub fn format_full_listing_size(ent: &FileEntry, si: bool) -> String {
    if ent.is_parent_marker() {
        "UP--DIR".to_string()
    } else if si {
        format_byte_size(ent.size, true)
    } else {
        ent.size.to_string()
    }
}

/// Format a byte count for panel size columns and mini-status.
///
/// GNU Midnight Commander `kilobyte_si` (Options → Panels → Use SI size units):
/// - `si == false` (default): powers of 1024 with suffixes K, M, G, T (not KiB).
///   Values below 1024 are a plain integer with no suffix.
/// - `si == true`: powers of 1000 with suffixes B, kB, MB, GB, TB.
pub fn format_byte_size(bytes: u64, si: bool) -> String {
    let base = if si { 1000u64 } else { 1024u64 };
    let units: &[&str] = if si {
        &["B", "kB", "MB", "GB", "TB", "PB", "EB"]
    } else {
        &["", "K", "M", "G", "T", "P", "E"]
    };

    if bytes < base {
        return if units[0].is_empty() {
            bytes.to_string()
        } else {
            format!("{bytes}{}", units[0])
        };
    }

    let mut unit = 1usize;
    let mut div = base;
    while unit + 1 < units.len() {
        let Some(next) = div.checked_mul(base) else {
            break;
        };
        if bytes / div < base {
            break;
        }
        div = next;
        unit += 1;
    }

    let whole = bytes / div;
    // One fractional digit only when the whole part is a single digit so the
    // panel column stays compact (1.5K, 1.2kB) while 976K stays an integer.
    if whole < 10 {
        let tenth = (bytes % div) * 10 / div;
        if tenth == 0 {
            format!("{whole}{}", units[unit])
        } else {
            format!("{whole}.{tenth}{}", units[unit])
        }
    } else {
        format!("{whole}{}", units[unit])
    }
}

/// GNU mc Options → Panels → Show mini-status: reserve the panel footer row
/// when the option is on, or when quick search is active on this panel.
pub fn reserve_panel_mini_status(
    show_mini_status: bool,
    is_active_panel: bool,
    quick_search_active: bool,
) -> bool {
    show_mini_status || (is_active_panel && quick_search_active)
}

/// Listing body rows inside a panel of height `panel_h` (including frames).
/// Chrome is top frame + header + bottom frame, plus the mini-status row when reserved.
/// GNU Full/Brief also draw a `├─┴─┴─┤` split-line above mini-status (`column_split_sep`);
/// that line is not a listing row. Mini-status off: no extra line, `┴` sits on the bottom frame.
pub fn panel_listing_content_rows(
    panel_h: u16,
    reserve_mini_status: bool,
    column_split_sep: bool,
) -> u16 {
    let mut chrome = if reserve_mini_status { 4 } else { 3 };
    if reserve_mini_status && column_split_sep {
        chrome += 1;
    }
    panel_h.saturating_sub(chrome)
}

/// GNU `|` tokens in Full, and Brief packed-name gaps (2+ columns).
pub fn listing_has_column_split_sep(listing: ListingFormat, brief_columns: u8) -> bool {
    match listing {
        ListingFormat::Full => true,
        ListingFormat::Brief => clamp_brief_columns(brief_columns) > 1,
        ListingFormat::Long | ListingFormat::User => false,
    }
}

/// Current-entry mini-status: perms, owner, group, size ([`format_byte_size`]), mtime.
///
/// GNU mc(1): a symlink's mini-status is the stored target (`-> path`), not the
/// regular-file stat line. Parent `..` is `UP--DIR`.
pub fn format_mini_status(ent: &FileEntry, si: bool) -> String {
    // GNU mc(1): parent `..` mini-status is UP--DIR, not a fake Unix-epoch stat line.
    if ent.is_parent_marker() {
        return "UP--DIR".to_string();
    }
    if ent.is_symlink {
        return match ent.symlink_target.as_deref().filter(|s| !s.is_empty()) {
            Some(target) => format!("-> {target}"),
            None => "<readlink failed>".to_string(),
        };
    }
    let perms = user_perm_string(ent.permissions, ent.is_dir);
    let owner = ent.owner.as_deref().unwrap_or("-");
    let group = ent.group.as_deref().unwrap_or("-");
    let size = if ent.is_dir { 0 } else { ent.size };
    let size_s = format_byte_size(size, si);
    let ts = user_mtime_string(ent);
    format!("{perms}  {owner:>8} {group:>8} {size_s:>8} {ts}")
}

/// Text for the panel mini-status row, if that row is shown.
///
/// - Option on (default): current-entry status, or empty when the panel has no entry.
/// - Option off: `None` (do not draw). Quick search on the active panel still
///   returns ` Search: …` so the search string can use that row.
pub fn panel_mini_status_line(
    show_mini_status: bool,
    is_active_panel: bool,
    quick_search: Option<&str>,
    current: Option<&FileEntry>,
    si: bool,
) -> Option<String> {
    if is_active_panel {
        if let Some(pattern) = quick_search {
            return Some(format!(" Search: {pattern}"));
        }
    }
    if !show_mini_status {
        return None;
    }
    Some(match current {
        Some(ent) => format_mini_status(ent, si),
        None => String::new(),
    })
}

/// Mini-status for Left/Right Tree panel mode.
///
/// After C-s, GNU shows the search string on this row. We include Dynamic/Static
/// plus the string so tests and the user can see both mode and query.
pub fn tree_panel_mini_status(
    tree: &TreeState,
    show_mini_status: bool,
    is_active_panel: bool,
) -> Option<String> {
    if is_active_panel && tree.search_active {
        let mode = if tree.figure.is_dynamic() {
            "Dynamic"
        } else {
            "Static"
        };
        return Some(format!("{mode}  Search: {}", tree.figure.search));
    }
    if !show_mini_status {
        return None;
    }
    Some(tree.figure.selected_path().display().to_string())
}

fn user_size_string(ent: &FileEntry, si: bool) -> String {
    if ent.is_parent_marker() {
        "UP--DIR".to_string()
    } else {
        format_byte_size(ent.size, si)
    }
}

fn user_bsize_string(ent: &FileEntry, si: bool) -> String {
    if ent.is_parent_marker() {
        "UP--DIR".to_string()
    } else if ent.is_dir {
        "SUB-DIR".to_string()
    } else {
        format_byte_size(ent.size, si)
    }
}

fn user_mode_string(permissions: u32) -> String {
    format!("{:04o}", permissions & 0o7777)
}

fn format_listing_time(ts: SystemTime) -> String {
    let dt: time::OffsetDateTime = ts.into();
    dt.format(&time::macros::format_description!(
        "[month repr:short] [day padding:space] [hour]:[minute]"
    ))
    .unwrap_or_default()
}

fn user_mtime_string(ent: &FileEntry) -> String {
    format_listing_time(ent.modified)
}

fn name_column_width(tokens: &[UserFormatToken], width: usize) -> usize {
    let name_count = tokens
        .iter()
        .filter(|t| matches!(t, UserFormatToken::Name))
        .count();
    if name_count == 0 {
        return 0;
    }
    let reserved =
        tokens.len().saturating_sub(1) + tokens.iter().map(|t| user_token_width(*t)).sum::<usize>();
    width.saturating_sub(reserved) / name_count
}

fn render_user_token(
    tok: UserFormatToken,
    ent: Option<&FileEntry>,
    name_width: usize,
    si: bool,
    marked: bool,
) -> String {
    match tok {
        UserFormatToken::Name => {
            let name = ent.map(|e| e.name.as_str()).unwrap_or("Name");
            fit_left(name, name_width.max(1))
        }
        UserFormatToken::Size => {
            let s = match ent {
                Some(e) => user_size_string(e, si),
                None => "Size".to_string(),
            };
            if ent.is_none() {
                fit_left(&s, 8)
            } else {
                fit_right(&s, 8)
            }
        }
        UserFormatToken::Bsize => {
            let s = match ent {
                Some(e) => user_bsize_string(e, si),
                None => "Size".to_string(),
            };
            if ent.is_none() {
                fit_left(&s, 8)
            } else {
                fit_right(&s, 8)
            }
        }
        UserFormatToken::Perm => match ent {
            Some(e) => user_perm_string(e.permissions, e.is_dir),
            None => fit_left("Perm", 10),
        },
        UserFormatToken::Mode => match ent {
            Some(e) => user_mode_string(e.permissions),
            None => fit_left("Mode", 4),
        },
        UserFormatToken::Type => match ent {
            Some(e) => listing_type_char(e).to_string(),
            None => " ".to_string(),
        },
        UserFormatToken::Mark => {
            if ent.is_some() && marked {
                "*".to_string()
            } else {
                " ".to_string()
            }
        }
        UserFormatToken::Mtime => match ent {
            Some(e) => fit_left(&format_listing_time(e.modified), 12),
            None => fit_left("Modify time", 12),
        },
        UserFormatToken::Atime => match ent {
            Some(e) => fit_left(&format_listing_time(e.accessed), 12),
            None => fit_left("Access time", 12),
        },
        UserFormatToken::Ctime => match ent {
            Some(e) => fit_left(&format_listing_time(e.changed), 12),
            None => fit_left("Change time", 12),
        },
        UserFormatToken::Nlink => match ent {
            Some(e) => fit_right(&e.nlink.to_string(), 4),
            None => fit_left("Nl", 4),
        },
        UserFormatToken::Owner => {
            let s = match ent {
                Some(e) => e.owner.as_deref().unwrap_or("-"),
                None => "Owner",
            };
            if ent.is_none() {
                fit_left(s, 8)
            } else {
                fit_right(s, 8)
            }
        }
        UserFormatToken::Group => {
            let s = match ent {
                Some(e) => e.group.as_deref().unwrap_or("-"),
                None => "Group",
            };
            if ent.is_none() {
                fit_left(s, 8)
            } else {
                fit_right(s, 8)
            }
        }
        UserFormatToken::Nuid => match ent {
            Some(e) => fit_right(&e.uid.to_string(), 7),
            None => fit_left("UID", 7),
        },
        UserFormatToken::Ngid => match ent {
            Some(e) => fit_right(&e.gid.to_string(), 7),
            None => fit_left("GID", 7),
        },
        UserFormatToken::Inode => match ent {
            Some(e) => fit_right(&e.inode.to_string(), 7),
            None => fit_left("Inode", 7),
        },
        UserFormatToken::Gap => "|".to_string(),
    }
}

fn join_user_parts(
    tokens: &[UserFormatToken],
    width: usize,
    ent: Option<&FileEntry>,
    si: bool,
    marked: bool,
) -> String {
    if tokens.is_empty() {
        return String::new();
    }
    let name_width = name_column_width(tokens, width);
    let parts: Vec<String> = tokens
        .iter()
        .map(|t| render_user_token(*t, ent, name_width, si, marked))
        .collect();
    let line = parts.join(" ");
    let n = line.chars().count();
    if n <= width {
        line
    } else {
        line.chars()
            .take(width.saturating_sub(1))
            .chain("…".chars())
            .collect()
    }
}

/// Render one user-format listing row, truncated to `width`.
/// `si` selects SI (1000: B/kB/MB/…) size units vs 1024-based (K/M/G).
/// `marked` is the GNU `mark` token (`*` if tagged).
pub fn format_user_listing_line(
    ent: &FileEntry,
    tokens: &[UserFormatToken],
    width: usize,
    si: bool,
    marked: bool,
) -> String {
    join_user_parts(tokens, width, Some(ent), si, marked)
}

/// Column header for a parsed user listing format, truncated to `width`.
pub fn format_user_listing_header(tokens: &[UserFormatToken], width: usize) -> String {
    join_user_parts(tokens, width, None, false, false)
}

/// `ls -l`–like Long listing prefix: perm, nlink, owner, group, size, mtime, trailing spaces.
/// The file name is painted separately so filehighlight can color it.
pub fn format_long_listing_prefix(ent: &FileEntry, si: bool) -> String {
    let perms = user_perm_string(ent.permissions, ent.is_dir);
    let owner = ent.owner.as_deref().unwrap_or("-");
    let group = ent.group.as_deref().unwrap_or("-");
    let size = if ent.is_dir { 0 } else { ent.size };
    let size_s = format!("{:>8}", format_byte_size(size, si));
    let tm = user_mtime_string(ent);
    format!(
        "{perms} {nlink:>4} {owner:>8} {group:>8} {size_s} {tm}  ",
        nlink = ent.nlink
    )
}

/// Full Long listing line including the file name.
pub fn format_long_listing_line(ent: &FileEntry, si: bool) -> String {
    format!("{}{}", format_long_listing_prefix(ent, si), ent.name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    fn make_entry(name: &str, size: u64, modified: SystemTime, is_dir: bool) -> FileEntry {
        FileEntry {
            name: name.to_string(),
            path: PathBuf::from(name),
            is_dir,
            is_symlink: false,
            symlink_target: None,
            is_exe: false,
            size,
            modified,
            accessed: modified,
            changed: modified,
            permissions: 0o755,
            owner: Some("user".into()),
            group: Some("group".into()),
            nlink: 1,
            inode: 0,
            uid: 0,
            gid: 0,
            is_stale_symlink: false,
        }
    }

    #[test]
    fn listing_cursor_moves_one_entry_without_wrap() {
        let now = SystemTime::now();
        let mut p = PanelState::new(".");
        p.set_entries(vec![
            make_entry("..", 0, now, true),
            make_entry("a.txt", 1, now, false),
            make_entry("b.txt", 1, now, false),
        ]);
        assert_eq!(p.cursor, 0);
        p.move_up();
        assert_eq!(p.cursor, 0, "Up on the first entry stays there");
        p.move_down();
        assert_eq!(p.cursor, 1);
        p.move_down();
        assert_eq!(p.cursor, 2);
        p.move_down();
        assert_eq!(p.cursor, 2, "Down on the last entry stays there");
        p.move_up();
        assert_eq!(p.cursor, 1);
    }

    #[test]
    fn listing_home_end_and_empty_are_noops() {
        let now = SystemTime::now();
        let mut p = PanelState::new(".");
        p.entries.clear();
        p.cursor = 0;
        p.scroll_top = 0;
        p.move_up();
        p.move_down();
        p.page_up(5);
        p.page_down(5);
        p.home();
        p.end();
        assert!(p.entries.is_empty());
        assert_eq!(p.cursor, 0);
        assert_eq!(p.scroll_top, 0);

        p.set_entries(vec![
            make_entry("..", 0, now, true),
            make_entry("z.txt", 1, now, false),
            make_entry("a.txt", 1, now, false),
        ]);
        p.cursor = 2;
        p.home();
        assert_eq!(p.cursor, 0);
        assert_eq!(p.entries[0].name, "..", "Home lands on .. when present");
        p.end();
        assert_eq!(p.cursor, p.entries.len() - 1);
        assert_eq!(p.current_entry().map(|e| e.name.as_str()), Some("z.txt"));
    }

    #[test]
    fn listing_page_and_ensure_visible_use_capacity() {
        let now = SystemTime::now();
        let mut p = PanelState::new(".");
        p.listing = ListingFormat::Full;
        p.entries = (0..30)
            .map(|i| make_entry(&format!("f{i:02}"), 1, now, false))
            .collect();
        p.cursor = 0;
        p.scroll_top = 0;
        let page_rows = 5;
        let cap = listing_page_capacity(p.listing, p.brief_columns, page_rows);
        assert_eq!(cap, 5, "Full page is page_rows");
        p.page_down(cap);
        p.ensure_visible(cap);
        assert_eq!(p.cursor, 5);
        assert_eq!(p.scroll_top, 1, "cursor stays in the drawn window");
        p.page_up(cap);
        p.ensure_visible(cap);
        assert_eq!(p.cursor, 0);
        assert_eq!(p.scroll_top, 0);
        p.cursor = 28;
        p.page_down(cap);
        assert_eq!(p.cursor, 29, "PageDown clamps to last; never wraps");
        p.home();
        p.ensure_visible(cap);
        assert_eq!(p.cursor, 0);
        assert_eq!(p.scroll_top, 0);
        p.end();
        p.ensure_visible(cap);
        assert_eq!(p.cursor, 29);
        assert_eq!(p.scroll_top, 29 - (cap - 1));

        p.listing = ListingFormat::Brief;
        p.brief_columns = 2;
        p.cursor = 0;
        p.scroll_top = 0;
        let brief_cap = listing_page_capacity(p.listing, p.brief_columns, page_rows);
        assert_eq!(brief_cap, 10, "Brief 2-col page is page_rows * 2");
        p.page_down(brief_cap);
        p.ensure_visible(brief_cap);
        assert_eq!(p.cursor, 10);
        p.page_down(brief_cap);
        p.page_down(brief_cap);
        assert_eq!(p.cursor, 29, "Brief PageDown clamps, never wraps");
        p.page_up(brief_cap);
        assert_eq!(p.cursor, 19);
    }

    #[test]
    fn jump_visible_empty_listing_is_noop() {
        let mut p = PanelState::new(".");
        p.entries.clear();
        p.cursor = 0;
        p.scroll_top = 0;
        p.jump_visible_top(5);
        p.jump_visible_middle(5);
        p.jump_visible_bottom(5);
        assert!(p.entries.is_empty());
        assert_eq!(p.cursor, 0);
        assert_eq!(p.scroll_top, 0);
    }

    #[test]
    fn restore_selection_after_reload_keeps_name_and_marks() {
        let now = SystemTime::now();
        let mut p = PanelState::new(".");
        p.set_entries(vec![
            make_entry("alpha.txt", 1, now, false),
            make_entry("beta.txt", 2, now, false),
            make_entry("gamma.txt", 3, now, false),
        ]);
        let beta = p.entries.iter().position(|e| e.name == "beta.txt").unwrap();
        p.cursor = beta;
        p.selection.select(beta);

        p.set_entries(vec![
            make_entry("aaa.txt", 0, now, false),
            make_entry("alpha.txt", 1, now, false),
            make_entry("beta.txt", 2, now, false),
            make_entry("gamma.txt", 3, now, false),
        ]);
        p.restore_selection_after_reload(Some("beta.txt"), beta, &["beta.txt".to_string()]);
        assert_eq!(p.current_entry().map(|e| e.name.as_str()), Some("beta.txt"));
        let marked: Vec<_> = p
            .selection
            .iter()
            .filter_map(|i| p.entries.get(i).map(|e| e.name.as_str()))
            .collect();
        assert_eq!(marked, ["beta.txt"]);
    }

    #[test]
    fn restore_selection_after_reload_clamps_when_name_gone() {
        let now = SystemTime::now();
        let mut p = PanelState::new(".");
        p.set_entries(vec![
            make_entry("alpha.txt", 1, now, false),
            make_entry("beta.txt", 2, now, false),
            make_entry("gamma.txt", 3, now, false),
        ]);
        let beta = p.entries.iter().position(|e| e.name == "beta.txt").unwrap();
        p.cursor = beta;
        p.set_entries(vec![
            make_entry("alpha.txt", 1, now, false),
            make_entry("gamma.txt", 3, now, false),
        ]);
        p.restore_selection_after_reload(Some("beta.txt"), beta, &[]);
        assert_eq!(
            p.cursor,
            beta.min(p.entries.len().saturating_sub(1)),
            "missing name keeps relative index, clamped"
        );
        assert!(p.selection.is_empty());
    }

    #[test]
    fn jump_visible_fits_and_scrolled_viewport() {
        let now = SystemTime::now();
        let mut p = PanelState::new(".");
        p.entries = (0..6)
            .map(|i| make_entry(&format!("e{i}"), 1, now, false))
            .collect();
        p.scroll_top = 0;
        p.cursor = 4;
        // Listing fits: visible = min(slots, len) = 6; middle = offset + visible/2.
        p.jump_visible_top(20);
        assert_eq!(p.cursor, 0);
        p.jump_visible_bottom(20);
        assert_eq!(p.cursor, 5);
        p.jump_visible_middle(20);
        assert_eq!(p.cursor, 3);

        p.entries = (0..21)
            .map(|i| make_entry(&format!("f{i:02}"), 1, now, false))
            .collect();
        p.scroll_top = 5;
        p.cursor = 9;
        let slots = 4;
        p.jump_visible_top(slots);
        assert_eq!(p.cursor, 5, "Alt-g is the first visible row, not index 0");
        assert_eq!(p.scroll_top, 5);
        p.jump_visible_bottom(slots);
        assert_eq!(p.cursor, 8, "Alt-j is last visible (5+4-1), not len-1");
        assert_eq!(p.scroll_top, 5);
        p.jump_visible_middle(slots);
        assert_eq!(p.cursor, 7, "middle = offset + visible/2 = 5 + 4/2");
        assert_eq!(p.scroll_top, 5);
    }

    #[test]
    fn sort_entries() {
        let now = SystemTime::now();
        let mut p = PanelState::new(".");
        p.set_entries(vec![
            make_entry("b.txt", 2, now, false),
            make_entry("a", 0, now - Duration::from_secs(10), true),
            make_entry("c.bin", 10, now - Duration::from_secs(20), false),
        ]);
        p.sort_by = SortBy::Name;
        p.apply_sort();
        assert_eq!(p.entries[0].name, "a");
        assert_eq!(p.entries[1].name, "b.txt");
        assert_eq!(p.entries[2].name, "c.bin");
        p.sort_by = SortBy::Size;
        p.apply_sort();
        assert_eq!(p.entries[1].name, "b.txt");
        assert_eq!(p.entries[2].name, "c.bin");
    }

    #[test]
    fn sort_by_extension_and_mix() {
        let now = SystemTime::now();
        let mut p = PanelState::new(".");
        p.set_entries(vec![
            make_entry("..", 0, now, true),
            make_entry("z.log", 1, now, false),
            make_entry("b.txt", 2, now, false),
            make_entry("alpha", 0, now, true),
            make_entry("c.bin", 10, now, false),
            make_entry("noext", 3, now, false),
        ]);
        p.sort_by = SortBy::Ext;
        p.dirs_first = false; // mixed sorting
        p.apply_sort();
        // '..' stays first; then files ordered by ext: "" (noext), bin, log, txt with ties by name
        assert_eq!(p.entries[0].name, "..");
        let names: Vec<_> = p.entries.iter().skip(1).map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "noext", "c.bin", "z.log", "b.txt"]);
        // Now reverse
        p.sort_dir = sorting::SortDir::Desc;
        p.apply_sort();
        let names: Vec<_> = p.entries.iter().skip(1).map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["b.txt", "z.log", "c.bin", "noext", "alpha"]);
    }

    #[test]
    fn reverse_files_only_dirs_first() {
        let now = SystemTime::now();
        let mut p = PanelState::new(".");
        p.dirs_first = true;
        p.sort_by = SortBy::Name;
        p.sort_dir = sorting::SortDir::Desc;
        p.set_entries(vec![
            make_entry("..", 0, now, true),
            make_entry("b", 0, now, true),
            make_entry("a", 0, now, true),
            make_entry("z", 1, now, false),
            make_entry("y", 1, now, false),
        ]);
        // Default reverse_files_only=true: dirs stay a,b (asc); files reverse z,y
        let names: Vec<_> = p.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["..", "a", "b", "z", "y"]);
        // Flag false: both groups reverse
        p.apply_sort_with(false);
        let names: Vec<_> = p.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["..", "b", "a", "z", "y"]);
    }

    fn make_entry_full(
        name: &str,
        size: u64,
        modified: SystemTime,
        accessed: SystemTime,
        changed: SystemTime,
        inode: u64,
        is_dir: bool,
    ) -> FileEntry {
        FileEntry {
            name: name.to_string(),
            path: PathBuf::from(name),
            is_dir,
            is_symlink: false,
            symlink_target: None,
            is_exe: false,
            size,
            modified,
            accessed,
            changed,
            permissions: 0o644,
            owner: None,
            group: None,
            nlink: 1,
            inode,
            uid: 0,
            gid: 0,
            is_stale_symlink: false,
        }
    }

    #[test]
    fn sort_by_atime_ctime_inode_from_constructed_entries() {
        let t0 = SystemTime::UNIX_EPOCH;
        let t1 = t0 + Duration::from_secs(10);
        let t2 = t0 + Duration::from_secs(20);
        let mut p = PanelState::new(".");
        p.dirs_first = false;
        p.set_entries(vec![
            make_entry_full("..", 0, t0, t0, t0, 0, true),
            make_entry_full("late-access", 1, t0, t2, t0, 30, false),
            make_entry_full("early-access", 1, t2, t1, t2, 10, false),
            make_entry_full("mid-change", 1, t1, t0, t1, 20, false),
        ]);

        p.sort_by = SortBy::Atime;
        p.sort_dir = sorting::SortDir::Asc;
        p.apply_sort();
        let names: Vec<_> = p.entries.iter().skip(1).map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["mid-change", "early-access", "late-access"]);

        p.sort_by = SortBy::Ctime;
        p.apply_sort();
        let names: Vec<_> = p.entries.iter().skip(1).map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["late-access", "mid-change", "early-access"]);

        p.sort_by = SortBy::Inode;
        p.apply_sort();
        let names: Vec<_> = p.entries.iter().skip(1).map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["early-access", "mid-change", "late-access"]);
    }

    #[test]
    fn unsorted_keeps_list_dir_order_and_reverse_flips() {
        let now = SystemTime::now();
        let mut p = PanelState::new(".");
        p.dirs_first = false;
        p.sort_by = SortBy::Unsorted;
        p.set_entries(vec![
            make_entry("..", 0, now, true),
            make_entry("z", 1, now, false),
            make_entry("a", 1, now, false),
            make_entry("m", 1, now, false),
        ]);
        let names: Vec<_> = p.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["..", "z", "a", "m"]);
        p.sort_dir = sorting::SortDir::Desc;
        p.apply_sort();
        let names: Vec<_> = p.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["..", "m", "a", "z"]);
    }

    #[test]
    fn unsorted_reverse_honors_dirs_first_and_reverse_files_only() {
        let now = SystemTime::now();
        let listing = vec![
            make_entry("..", 0, now, true),
            make_entry("d2", 0, now, true),
            make_entry("d1", 0, now, true),
            make_entry("f2", 1, now, false),
            make_entry("f1", 1, now, false),
        ];
        let mut p = PanelState::new(".");
        p.dirs_first = true;
        p.sort_by = SortBy::Unsorted;
        p.sort_dir = sorting::SortDir::Desc;
        p.set_entries(listing.clone());
        // reverse_files_only default: dirs keep list order d2,d1; files reverse f1,f2
        let names: Vec<_> = p.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["..", "d2", "d1", "f1", "f2"]);
        // Restore list_dir order, then reverse both groups.
        p.sort_dir = sorting::SortDir::Asc;
        p.set_entries(listing);
        p.sort_dir = sorting::SortDir::Desc;
        p.apply_sort_with(false);
        let names: Vec<_> = p.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["..", "d1", "d2", "f1", "f2"]);
    }

    #[test]
    fn user_format_parses_default_and_ignores_half() {
        let t = parse_user_listing_format("half type name | size | perm");
        assert_eq!(
            t,
            vec![
                UserFormatToken::Type,
                UserFormatToken::Name,
                UserFormatToken::Gap,
                UserFormatToken::Size,
                UserFormatToken::Gap,
                UserFormatToken::Perm,
            ]
        );
        assert!(parse_user_listing_format("").is_empty());
        assert!(parse_user_listing_format("half").is_empty());
        assert!(parse_user_listing_format("   ").is_empty());
    }

    #[test]
    fn user_format_case_insensitive_aliases_and_unknown() {
        let t = parse_user_listing_format("FULL MODE TIME nlink owner group foo size:7");
        assert_eq!(
            t,
            vec![
                UserFormatToken::Mode,
                UserFormatToken::Mtime,
                UserFormatToken::Nlink,
                UserFormatToken::Owner,
                UserFormatToken::Group,
                UserFormatToken::Size,
            ]
        );
        let glued = parse_user_listing_format("name|size|perm");
        assert_eq!(
            glued,
            vec![
                UserFormatToken::Name,
                UserFormatToken::Gap,
                UserFormatToken::Size,
                UserFormatToken::Gap,
                UserFormatToken::Perm,
            ]
        );
    }

    #[test]
    fn user_format_line_draws_name_size_perm() {
        let now = SystemTime::now();
        let ent = make_entry("readme.txt", 42, now, false);
        let tokens = parse_user_listing_format("half type name | size | perm");
        let line = format_user_listing_line(&ent, &tokens, 80, false, false);
        assert!(line.contains("readme.txt"), "line={line:?}");
        assert!(line.contains("42"), "line={line:?}");
        assert!(line.contains("rwx"), "line={line:?}");
        assert!(line.contains('|'), "line={line:?}");
        let header = format_user_listing_header(&tokens, 80);
        assert!(header.contains("Name"), "header={header:?}");
        assert!(header.contains("Size"), "header={header:?}");
        assert!(header.contains("Perm"), "header={header:?}");
    }

    #[test]
    fn user_format_type_marks_and_truncation() {
        let now = SystemTime::now();
        let tokens = parse_user_listing_format("type name");
        let dir = make_entry("src", 0, now, true);
        assert!(
            format_user_listing_line(&dir, &tokens, 40, false, false).starts_with('/'),
            "dir type"
        );
        let mut exe = make_entry("a.out", 10, now, false);
        exe.is_exe = true;
        assert!(
            format_user_listing_line(&exe, &tokens, 40, false, false).starts_with('*'),
            "exe type"
        );
        let mut link = make_entry("link", 1, now, false);
        link.is_symlink = true;
        assert!(
            format_user_listing_line(&link, &tokens, 40, false, false).starts_with('@'),
            "symlink type"
        );
        let mut sock = make_entry("sock", 0, now, false);
        sock.permissions = 0o140_000 | 0o666;
        assert_eq!(listing_type_char(&sock), '=');
        assert!(
            format_user_listing_line(&sock, &tokens, 40, false, false).starts_with('='),
            "socket type"
        );
        let mut fifo = make_entry("pipe", 0, now, false);
        fifo.permissions = 0o010_000 | 0o666;
        assert_eq!(listing_type_char(&fifo), '|');
        assert!(
            format_user_listing_line(&fifo, &tokens, 40, false, false).starts_with('|'),
            "fifo type"
        );
        let mut chr = make_entry("tty", 0, now, false);
        chr.permissions = 0o020_000 | 0o666;
        assert_eq!(listing_type_char(&chr), '-');
        assert!(
            format_user_listing_line(&chr, &tokens, 40, false, false).starts_with('-'),
            "char device type"
        );
        let mut blk = make_entry("sda", 0, now, false);
        blk.permissions = 0o060_000 | 0o666;
        assert_eq!(listing_type_char(&blk), '+');
        assert!(
            format_user_listing_line(&blk, &tokens, 40, false, false).starts_with('+'),
            "block device type"
        );
        let mut stale = make_entry("gone", 1, now, false);
        stale.is_symlink = true;
        stale.is_stale_symlink = true;
        assert_eq!(listing_type_char(&stale), '!');
        assert!(
            format_user_listing_line(&stale, &tokens, 40, false, false).starts_with('!'),
            "stale symlink type"
        );
        let mut linkdir = make_entry("linked-dir", 1, now, true);
        linkdir.is_symlink = true;
        assert_eq!(listing_type_char(&linkdir), '~');
        assert!(
            format_user_listing_line(&linkdir, &tokens, 40, false, false).starts_with('~'),
            "symlink-to-dir type"
        );
        assert_eq!(listing_type_char(&make_entry("..", 0, now, true)), '/');
        let regular = make_entry("Cargo.lock", 72688, now, false);
        assert_eq!(listing_type_char(&regular), ' ');
        let regular_line = format_user_listing_line(&regular, &tokens, 40, false, false);
        assert_eq!(
            regular_line.chars().next(),
            Some(' '),
            "type cell space is not optional for regular files: {regular_line:?}"
        );
        assert!(regular_line.contains("Cargo.lock"), "{regular_line:?}");
        let parent_line =
            format_user_listing_line(&make_entry("..", 0, now, true), &tokens, 40, false, false);
        assert_eq!(
            parent_line.chars().next(),
            Some('/'),
            "parent `..` is a directory type cell `/..` on live GNU: {parent_line:?}"
        );
        assert_eq!(
            full_listing_sort_indicator(SortBy::Name, SortDir::Asc),
            ['.', 'n']
        );
        assert_eq!(
            full_listing_sort_indicator(SortBy::Size, SortDir::Desc),
            [',', 's']
        );
        let dir = make_entry("src", 4096, now, true);
        assert_eq!(format_full_listing_size(&dir, false), "4096");
        let file = make_entry("Cargo.lock", 72688, now, false);
        assert_eq!(format_full_listing_size(&file, false), "72688");
        assert_eq!(
            format_full_listing_size(&make_entry("..", 4096, now, true), false),
            "UP--DIR"
        );
        assert_eq!(format_full_listing_size(&file, true), "72kB");
        let long = make_entry("very-long-filename-that-should-clip", 1, now, false);
        let clipped = format_user_listing_line(&long, &tokens, 12, false, false);
        assert!(clipped.chars().count() <= 12);
    }

    #[test]
    fn format_byte_size_1024_vs_1000() {
        // Flag off (default): 1024-based, GNU mc K/M/G (not KiB).
        assert!(!crate::app::PanelOptions::default().kilobyte_si);
        assert_eq!(format_byte_size(42, false), "42");
        assert_eq!(format_byte_size(1000, false), "1000");
        assert_eq!(format_byte_size(1024, false), "1K");
        assert_eq!(format_byte_size(1536, false), "1.5K");
        assert_eq!(format_byte_size(1_000_000, false), "976K");
        assert_eq!(format_byte_size(1_048_576, false), "1M");
        // Flag on: decimal SI (1000) B / kB / MB / GB / TB.
        assert_eq!(format_byte_size(42, true), "42B");
        assert_eq!(format_byte_size(999, true), "999B");
        assert_eq!(format_byte_size(1000, true), "1kB");
        assert_eq!(format_byte_size(1024, true), "1kB");
        assert_eq!(format_byte_size(1_200, true), "1.2kB");
        assert_eq!(format_byte_size(1_000_000, true), "1MB");
        assert_eq!(format_byte_size(1_048_576, true), "1MB");
        assert_eq!(format_byte_size(3_400_000, true), "3.4MB");
        assert_eq!(format_byte_size(5_600_000_000, true), "5.6GB");
        assert_eq!(
            format_byte_size(1024, crate::app::PanelOptions::default().kilobyte_si),
            "1K"
        );
    }

    #[test]
    fn show_mini_status_defaults_true() {
        assert!(
            crate::app::PanelOptions::default().show_mini_status,
            "GNU mc Show mini-status defaults to on"
        );
        assert!(reserve_panel_mini_status(true, false, false));
        assert_eq!(
            panel_listing_content_rows(20, true, false),
            16,
            "Long/User mini-status: top+header+status+bottom"
        );
        assert_eq!(
            panel_listing_content_rows(20, true, true),
            15,
            "Full/Brief mini-status also reserves the ├────────┤ split-line"
        );
    }

    #[test]
    fn mini_status_on_draws_current_entry() {
        let mut ent = make_entry("readme.txt", 1024, SystemTime::UNIX_EPOCH, false);
        ent.permissions = 0o644;
        ent.owner = Some("alice".into());
        ent.group = Some("staff".into());
        let line = panel_mini_status_line(true, true, None, Some(&ent), false)
            .expect("mini-status drawn when option is on");
        assert!(
            line.starts_with("-rw-r--r--"),
            "perms in mini-status: {line:?}"
        );
        assert!(line.contains("alice"), "owner in mini-status: {line:?}");
        assert!(line.contains("staff"), "group in mini-status: {line:?}");
        let size = format!("{:>8}", format_byte_size(1024, false));
        assert!(
            line.contains(&size),
            "1024-based size {size:?} in mini-status: {line:?}"
        );
        assert_eq!(line, format_mini_status(&ent, false));
        // Parent `..` is GNU UP--DIR, never d--------- / Jan 1 00:00 from UNIX_EPOCH.
        let parent = make_entry("..", 0, SystemTime::UNIX_EPOCH, true);
        let parent_line = panel_mini_status_line(true, true, None, Some(&parent), false)
            .expect("parent mini-status");
        assert_eq!(parent_line, "UP--DIR");
        assert_eq!(format_mini_status(&parent, false), "UP--DIR");
        assert!(
            !parent_line.contains("d---------")
                && !parent_line.contains("Jan")
                && !parent_line.contains("00:00"),
            "parent mini-status must not be epoch zeros: {parent_line:?}"
        );
        // SI size units still apply when the line is shown.
        let si_line = panel_mini_status_line(true, false, None, Some(&ent), true)
            .expect("inactive panel still draws mini-status when option is on");
        let si_size = format!("{:>8}", format_byte_size(1024, true));
        assert!(
            si_line.contains(&si_size),
            "SI size {si_size:?} in mini-status: {si_line:?}"
        );
    }

    #[test]
    fn mini_status_symlink_shows_stored_target() {
        let mut file_link = make_entry("link", 11, SystemTime::UNIX_EPOCH, false);
        file_link.permissions = 0o777;
        file_link.owner = Some("alice".into());
        file_link.group = Some("staff".into());
        file_link.is_symlink = true;
        file_link.symlink_target = Some("readme.txt".into());
        let line = panel_mini_status_line(true, true, None, Some(&file_link), false)
            .expect("symlink mini-status");
        assert_eq!(line, "-> readme.txt");
        assert_eq!(format_mini_status(&file_link, false), "-> readme.txt");
        assert!(
            !line.contains("alice") && !line.contains("rwx"),
            "symlink mini-status is the target, not the regular stat line: {line:?}"
        );

        let mut dir_link = make_entry("todir", 4, SystemTime::UNIX_EPOCH, true);
        dir_link.is_symlink = true;
        dir_link.symlink_target = Some("../other".into());
        assert_eq!(format_mini_status(&dir_link, false), "-> ../other");

        let mut abs = make_entry("abslink", 9, SystemTime::UNIX_EPOCH, false);
        abs.is_symlink = true;
        abs.symlink_target = Some("/etc/passwd".into());
        assert_eq!(format_mini_status(&abs, false), "-> /etc/passwd");

        let mut failed = make_entry("broken", 1, SystemTime::UNIX_EPOCH, false);
        failed.is_symlink = true;
        failed.symlink_target = None;
        assert_eq!(format_mini_status(&failed, false), "<readlink failed>");

        // Parent marker wins even if a symlink flag were set.
        let mut parent = make_entry("..", 0, SystemTime::UNIX_EPOCH, true);
        parent.is_symlink = true;
        parent.symlink_target = Some("/nowhere".into());
        assert_eq!(format_mini_status(&parent, false), "UP--DIR");
        assert_eq!(
            panel_mini_status_line(true, true, None, Some(&parent), false).as_deref(),
            Some("UP--DIR")
        );

        let regular = make_entry("readme.txt", 1024, SystemTime::UNIX_EPOCH, false);
        let regular_line = format_mini_status(&regular, false);
        assert!(
            regular_line.starts_with("-"),
            "regular file mini-status: {regular_line:?}"
        );
        assert!(
            !regular_line.contains("->"),
            "regular file must not show a link arrow: {regular_line:?}"
        );

        assert_eq!(
            panel_mini_status_line(false, true, None, Some(&file_link), false),
            None,
            "Show mini-status off still omits the row for a symlink"
        );
        assert_eq!(
            panel_mini_status_line(false, false, None, Some(&dir_link), false),
            None,
            "inactive panel omits symlink mini-status when option is off"
        );
    }

    #[test]
    fn mini_status_off_does_not_draw() {
        let ent = make_entry("readme.txt", 1024, SystemTime::UNIX_EPOCH, false);
        assert_eq!(
            panel_mini_status_line(false, true, None, Some(&ent), false),
            None,
            "active panel omits mini-status when option is off"
        );
        assert_eq!(
            panel_mini_status_line(false, false, None, Some(&ent), false),
            None,
            "inactive panel omits mini-status when option is off"
        );
        assert!(!reserve_panel_mini_status(false, false, false));
        assert!(!reserve_panel_mini_status(false, true, false));
        assert_eq!(panel_listing_content_rows(20, false, false), 17);
        assert_eq!(
            panel_listing_content_rows(20, false, true),
            17,
            "mini-status off: ┴ is on the bottom frame, no extra row"
        );
        // Quick search on the active panel still uses the row.
        assert_eq!(
            panel_mini_status_line(false, true, Some("foo"), Some(&ent), false).as_deref(),
            Some(" Search: foo")
        );
        assert!(reserve_panel_mini_status(false, true, true));
        assert_eq!(
            panel_mini_status_line(false, false, Some("foo"), Some(&ent), false),
            None,
            "inactive panel does not show quick search on mini-status"
        );
    }

    #[test]
    fn tree_panel_mini_status_shows_mode_and_search() {
        let known = vec![
            TreeEntry {
                path: PathBuf::from("/"),
                depth: 0,
            },
            TreeEntry {
                path: PathBuf::from("/tmp"),
                depth: 1,
            },
        ];
        let mut tree = TreeState {
            figure: crate::dirtree::DirectoryTreeState::new(known, Path::new("/tmp")),
            search_active: false,
        };
        assert!(tree_panel_mini_status(&tree, false, true).is_none());
        tree.search_active = true;
        tree.figure.search = "tm".to_string();
        let line = tree_panel_mini_status(&tree, false, true).expect("search row");
        assert!(line.contains("Dynamic"), "mode in mini-status: {line:?}");
        assert!(
            line.contains("Search: tm"),
            "string in mini-status: {line:?}"
        );
        tree.figure.toggle_mode();
        tree.search_active = true;
        tree.figure.search = "tm".to_string();
        let line = tree_panel_mini_status(&tree, false, true).expect("static search row");
        assert!(line.contains("Static"), "mode in mini-status: {line:?}");
        assert!(line.contains("Search: tm"), "{line:?}");
    }

    #[test]
    fn user_format_size_si_flag() {
        let now = SystemTime::now();
        let ent = make_entry("big.bin", 1_000_000, now, false);
        let tokens = parse_user_listing_format("size");
        let iec = format_user_listing_line(&ent, &tokens, 16, false, false);
        assert!(iec.contains("976K"), "1024-based={iec:?}");
        let si = format_user_listing_line(&ent, &tokens, 16, true, false);
        assert!(si.contains("1MB"), "SI={si:?}");
    }

    #[test]
    fn user_format_nlink_is_right_aligned() {
        let now = SystemTime::now();
        let mut ent = make_entry("file", 1, now, false);
        let tokens = parse_user_listing_format("nlink name");
        let line = format_user_listing_line(&ent, &tokens, 40, false, false);
        assert!(line.contains("   1"), "nlink 1 line={line:?}");
        ent.nlink = 2;
        let line = format_user_listing_line(&ent, &tokens, 40, false, false);
        assert!(line.contains("   2"), "nlink 2 line={line:?}");
        let header = format_user_listing_header(&tokens, 40);
        assert!(header.contains("Nl"), "header={header:?}");
    }

    #[test]
    fn user_format_parses_mc1_field_tokens() {
        let t = parse_user_listing_format(
            "half type name mark | size bsize perm mode nlink owner group nuid ngid inode mtime atime ctime space foo",
        );
        assert_eq!(
            t,
            vec![
                UserFormatToken::Type,
                UserFormatToken::Name,
                UserFormatToken::Mark,
                UserFormatToken::Gap,
                UserFormatToken::Size,
                UserFormatToken::Bsize,
                UserFormatToken::Perm,
                UserFormatToken::Mode,
                UserFormatToken::Nlink,
                UserFormatToken::Owner,
                UserFormatToken::Group,
                UserFormatToken::Nuid,
                UserFormatToken::Ngid,
                UserFormatToken::Inode,
                UserFormatToken::Mtime,
                UserFormatToken::Atime,
                UserFormatToken::Ctime,
            ]
        );
        assert_eq!(
            parse_user_listing_format("perm space nlink"),
            vec![UserFormatToken::Perm, UserFormatToken::Nlink]
        );
        assert_eq!(
            parse_user_listing_format("ATIME CTIME BSIZE MARK INODE NUID NGID"),
            vec![
                UserFormatToken::Atime,
                UserFormatToken::Ctime,
                UserFormatToken::Bsize,
                UserFormatToken::Mark,
                UserFormatToken::Inode,
                UserFormatToken::Nuid,
                UserFormatToken::Ngid,
            ]
        );
    }

    #[test]
    fn user_format_mc1_tokens_render() {
        let t_m = SystemTime::UNIX_EPOCH;
        let t_a = t_m + Duration::from_secs(40 * 24 * 3600);
        let t_c = t_m + Duration::from_secs(80 * 24 * 3600);
        let mut file = make_entry("readme.txt", 42, t_m, false);
        file.accessed = t_a;
        file.changed = t_c;
        file.permissions = 0o644;
        file.owner = Some("alice".into());
        file.group = Some("staff".into());
        file.nlink = 3;
        file.inode = 123456;
        file.uid = 1001;
        file.gid = 1002;

        let size =
            format_user_listing_line(&file, &parse_user_listing_format("size"), 16, false, false);
        let bsize =
            format_user_listing_line(&file, &parse_user_listing_format("bsize"), 16, false, false);
        assert!(size.contains("42"), "size file={size:?}");
        assert!(bsize.contains("42"), "bsize file={bsize:?}");
        assert_eq!(size, bsize, "file size and bsize match");

        let mut dir = make_entry("src", 4096, t_m, true);
        dir.permissions = 0o755;
        let dir_size =
            format_user_listing_line(&dir, &parse_user_listing_format("size"), 16, false, false);
        let dir_bsize =
            format_user_listing_line(&dir, &parse_user_listing_format("bsize"), 16, false, false);
        assert!(
            dir_size.contains("4K"),
            "size shows directory inode size: {dir_size:?}"
        );
        assert!(
            dir_bsize.contains("SUB-DIR"),
            "bsize directories are SUB-DIR: {dir_bsize:?}"
        );
        assert!(
            !dir_size.contains("SUB-DIR") && !dir_size.contains("UP--DIR"),
            "size is not bsize for a real dir: {dir_size:?}"
        );

        let mut parent = make_entry("..", 4096, t_m, true);
        parent.nlink = 12;
        parent.inode = 99;
        parent.uid = 0;
        parent.gid = 0;
        parent.permissions = 0o755;
        let parent_size = format_user_listing_line(
            &parent,
            &parse_user_listing_format("size"),
            16,
            false,
            false,
        );
        let parent_bsize = format_user_listing_line(
            &parent,
            &parse_user_listing_format("bsize"),
            16,
            false,
            false,
        );
        assert!(parent_size.contains("UP--DIR"), "size ..={parent_size:?}");
        assert!(
            parent_bsize.contains("UP--DIR"),
            "bsize ..={parent_bsize:?}"
        );
        let parent_nlink = format_user_listing_line(
            &parent,
            &parse_user_listing_format("nlink"),
            16,
            false,
            false,
        );
        let parent_inode = format_user_listing_line(
            &parent,
            &parse_user_listing_format("inode"),
            16,
            false,
            false,
        );
        let parent_perm = format_user_listing_line(
            &parent,
            &parse_user_listing_format("perm"),
            16,
            false,
            false,
        );
        assert!(
            !parent_nlink.contains("UP--DIR") && parent_nlink.contains("12"),
            "UP--DIR is size-only: nlink={parent_nlink:?}"
        );
        assert!(
            !parent_inode.contains("UP--DIR") && parent_inode.contains("99"),
            "UP--DIR is size-only: inode={parent_inode:?}"
        );
        assert!(
            !parent_perm.contains("UP--DIR") && parent_perm.contains("rwx"),
            "UP--DIR is size-only: perm={parent_perm:?}"
        );

        let perm =
            format_user_listing_line(&file, &parse_user_listing_format("perm"), 16, false, false);
        let mode =
            format_user_listing_line(&file, &parse_user_listing_format("mode"), 16, false, false);
        assert!(perm.contains("rw-r--r--"), "perm={perm:?}");
        assert!(mode.contains("0644"), "mode={mode:?}");
        assert!(!mode.contains("rw-"), "mode is octal, not rwx: {mode:?}");

        let nlink =
            format_user_listing_line(&file, &parse_user_listing_format("nlink"), 16, false, false);
        assert!(nlink.contains("3"), "nlink={nlink:?}");

        let owner =
            format_user_listing_line(&file, &parse_user_listing_format("owner"), 16, false, false);
        let group =
            format_user_listing_line(&file, &parse_user_listing_format("group"), 16, false, false);
        let nuid =
            format_user_listing_line(&file, &parse_user_listing_format("nuid"), 16, false, false);
        let ngid =
            format_user_listing_line(&file, &parse_user_listing_format("ngid"), 16, false, false);
        assert!(owner.contains("alice"), "owner={owner:?}");
        assert!(group.contains("staff"), "group={group:?}");
        assert!(
            nuid.contains("1001") && !nuid.contains("alice"),
            "nuid={nuid:?}"
        );
        assert!(
            ngid.contains("1002") && !ngid.contains("staff"),
            "ngid={ngid:?}"
        );

        let inode =
            format_user_listing_line(&file, &parse_user_listing_format("inode"), 16, false, false);
        assert!(inode.contains("123456"), "inode={inode:?}");

        let unmarked = format_user_listing_line(
            &file,
            &parse_user_listing_format("mark name"),
            24,
            false,
            false,
        );
        let marked = format_user_listing_line(
            &file,
            &parse_user_listing_format("mark name"),
            24,
            false,
            true,
        );
        assert!(
            unmarked.starts_with(' '),
            "unmarked mark field is a space: {unmarked:?}"
        );
        assert!(
            marked.starts_with('*'),
            "tagged mark field is *: {marked:?}"
        );
        assert!(
            marked.contains("readme.txt"),
            "mark keeps the name: {marked:?}"
        );

        let mtime =
            format_user_listing_line(&file, &parse_user_listing_format("mtime"), 16, false, false);
        let atime =
            format_user_listing_line(&file, &parse_user_listing_format("atime"), 16, false, false);
        let ctime =
            format_user_listing_line(&file, &parse_user_listing_format("ctime"), 16, false, false);
        let mut same = file.clone();
        same.modified = t_a;
        let mtime_of_ta =
            format_user_listing_line(&same, &parse_user_listing_format("mtime"), 16, false, false);
        same.modified = t_c;
        let mtime_of_tc =
            format_user_listing_line(&same, &parse_user_listing_format("mtime"), 16, false, false);
        assert_eq!(
            atime.trim(),
            mtime_of_ta.trim(),
            "atime uses accessed, atime={atime:?} mtime(t_a)={mtime_of_ta:?}"
        );
        assert_eq!(
            ctime.trim(),
            mtime_of_tc.trim(),
            "ctime uses changed, ctime={ctime:?} mtime(t_c)={mtime_of_tc:?}"
        );
        assert_ne!(mtime.trim(), atime.trim());
        assert_ne!(atime.trim(), ctime.trim());

        let header = format_user_listing_header(
            &parse_user_listing_format("size bsize perm mode mark nuid ngid inode atime ctime"),
            120,
        );
        assert!(header.contains("Size"), "{header:?}");
        assert!(header.contains("Perm"), "{header:?}");
        assert!(header.contains("Mode"), "{header:?}");
        assert!(header.contains("UID"), "{header:?}");
        assert!(header.contains("GID"), "{header:?}");
        assert!(header.contains("Inode"), "{header:?}");
        assert!(header.contains("Access time"), "{header:?}");
        assert!(header.contains("Change time"), "{header:?}");
    }

    #[test]
    fn filter_files_only_keeps_directories() {
        let now = SystemTime::now();
        let mut p = PanelState::new(".");
        p.filter_glob = Some("*.txt".to_string());
        p.filter_files_only = false;
        p.set_entries(vec![
            make_entry("..", 0, now, true),
            make_entry("keep.txt", 1, now, false),
            make_entry("skip.dat", 1, now, false),
            make_entry("subdir", 0, now, true),
        ]);
        let names: Vec<_> = p.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["..", "keep.txt"]);

        p.filter_files_only = true;
        p.set_entries(vec![
            make_entry("..", 0, now, true),
            make_entry("keep.txt", 1, now, false),
            make_entry("skip.dat", 1, now, false),
            make_entry("subdir", 0, now, true),
        ]);
        let names: Vec<_> = p.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["..", "subdir", "keep.txt"]);
    }

    #[test]
    fn listing_format_cycles_gnu_alt_t_order() {
        use ListingFormat::*;
        assert_eq!(Full.cycle(), Brief);
        assert_eq!(Brief.cycle(), Long);
        assert_eq!(Long.cycle(), User);
        assert_eq!(User.cycle(), Full);
    }

    #[test]
    fn brief_columns_clamp_and_capacity() {
        assert_eq!(clamp_brief_columns(0), 1);
        assert_eq!(clamp_brief_columns(2), 2);
        assert_eq!(clamp_brief_columns(9), 9);
        assert_eq!(clamp_brief_columns(10), 9);
        assert_eq!(
            listing_page_capacity(ListingFormat::Brief, 2, 10),
            20,
            "default Brief is two columns"
        );
        assert_eq!(listing_page_capacity(ListingFormat::Brief, 1, 10), 10);
        assert_eq!(listing_page_capacity(ListingFormat::Brief, 9, 10), 90);
        assert_eq!(listing_page_capacity(ListingFormat::Full, 9, 10), 10);
        assert_eq!(listing_page_capacity(ListingFormat::Long, 9, 10), 10);
        // Column-major: col 1 starts after `rows` entries.
        assert_eq!(brief_entry_index(0, 0, 0, 10), 0);
        assert_eq!(brief_entry_index(0, 3, 1, 10), 13);
        assert_eq!(brief_entry_index(0, 0, 2, 10), 20);
        assert_eq!(brief_column_width(30, 2), (30 - 3) / 2);
        assert_eq!(brief_column_at_x(1, 13, 2), 0);
        assert_eq!(brief_column_at_x(2 + 13, 13, 2), 1);
        let p = PanelState::new(".");
        assert_eq!(p.brief_columns, BRIEF_COLUMNS_DEFAULT);
        assert_eq!(p.listing, ListingFormat::Full);
        assert!(listing_has_column_split_sep(ListingFormat::Full, 2));
        assert!(listing_has_column_split_sep(ListingFormat::Brief, 2));
        assert!(!listing_has_column_split_sep(ListingFormat::Brief, 1));
        assert!(!listing_has_column_split_sep(ListingFormat::Long, 2));
        assert!(!listing_has_column_split_sep(ListingFormat::User, 2));
    }

    #[test]
    fn long_listing_includes_perm_nlink_owner_group_size_name() {
        let mut ent = make_entry("readme.txt", 42, SystemTime::UNIX_EPOCH, false);
        ent.permissions = 0o644;
        ent.nlink = 3;
        ent.owner = Some("alice".into());
        ent.group = Some("staff".into());
        let line = format_long_listing_line(&ent, false);
        assert!(line.contains("rw-r--r--"), "perms line={line:?}");
        assert!(line.contains("   3"), "nlink line={line:?}");
        assert!(line.contains("alice"), "owner line={line:?}");
        assert!(line.contains("staff"), "group line={line:?}");
        assert!(line.contains("42"), "size line={line:?}");
        assert!(line.contains("readme.txt"), "name line={line:?}");
        let prefix = format_long_listing_prefix(&ent, false);
        assert!(
            !prefix.contains("readme.txt"),
            "prefix should omit the name so the renderer can color it"
        );
    }
}
