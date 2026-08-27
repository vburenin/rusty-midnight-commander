use crate::selection::Selection;
use crate::sorting::{self, SortDir};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

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

#[derive(Debug, Clone)]
pub struct TreeState {
    pub entries: Vec<TreeEntry>,
    pub cursor: usize,
    pub scroll_top: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ListingFormat {
    Full,
    Brief,
    Long,
    /// User-defined format string stored on the panel (`user_format`).
    User,
}

/// GNU-ish subset of mc user-defined listing format tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserFormatToken {
    Name,
    Size,
    Perm,
    Type,
    Mtime,
    Nlink,
    Owner,
    Group,
    /// `|` in the format string: a literal column gap.
    Gap,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub is_exe: bool,
    pub size: u64,
    pub modified: SystemTime,
    pub permissions: u32,
    pub owner: Option<String>,
    pub group: Option<String>,
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
    Time,
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
    pub selection: Selection,
    // When panelized, entries show a virtual list; pressing `..` or leaving mode restores saved state.
    pub panelized: Option<PanelizeSaved>,
    /// Optional filename filter (shell glob like *.c). `None` or "*" shows all.
    pub filter_glob: Option<String>,
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
            // GNU-ish default; `half` is ignored by the subset parser.
            user_format: "half type name | size | perm".to_string(),
            selection: Selection::default(),
            panelized: None,
            filter_glob: None,
        }
    }

    pub fn set_entries(&mut self, mut entries: Vec<FileEntry>) {
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
        // Apply filename filter (shell glob) if present and not equal to "*".
        if let Some(pat) = self
            .filter_glob
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty() && *s != "*")
        {
            entries.retain(|e| glob_match(pat, &e.name));
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
        self.apply_sort();
        if self.cursor >= self.entries.len() {
            self.cursor = self.entries.len().saturating_sub(1);
        }
    }

    pub fn apply_sort(&mut self) {
        // Detach a possible parent marker at the top to keep it first
        let mut items = std::mem::take(&mut self.entries);
        let mut parent_marker: Option<FileEntry> = None;
        if !items.is_empty() && items[0].is_dir && items[0].name == ".." {
            parent_marker = Some(items.remove(0));
        }

        if self.dirs_first {
            let (mut dirs, mut rest): (Vec<_>, Vec<_>) = items.into_iter().partition(|e| e.is_dir);
            match self.sort_by {
                SortBy::Name => {
                    sorting::sort_by_name(&mut dirs, self.sort_dir);
                    sorting::sort_by_name(&mut rest, self.sort_dir);
                }
                SortBy::Ext => {
                    sorting::sort_by_name(&mut dirs, self.sort_dir); // dirs by name
                    sorting::sort_by_ext(&mut rest, self.sort_dir);
                }
                SortBy::Size => {
                    sorting::sort_by_name(&mut dirs, self.sort_dir); // dirs by name
                    sorting::sort_by_size(&mut rest, self.sort_dir);
                }
                SortBy::Time => {
                    sorting::sort_by_name(&mut dirs, self.sort_dir); // dirs by name
                    sorting::sort_by_time(&mut rest, self.sort_dir);
                }
            }
            self.entries = [dirs, rest].concat();
        } else {
            // Mixed directories/files together sorted uniformly (except parent marker)
            match self.sort_by {
                SortBy::Name => sorting::sort_by_name(&mut items, self.sort_dir),
                SortBy::Ext => sorting::sort_by_ext(&mut items, self.sort_dir),
                SortBy::Size => sorting::sort_by_size(&mut items, self.sort_dir),
                SortBy::Time => sorting::sort_by_time(&mut items, self.sort_dir),
            }
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
        self.set_entries(entries);
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

/// Parse a GNU-ish user listing format string.
///
/// Tokens are case-insensitive and space-separated. `|` is a column gap.
/// Unknown tokens (including `half` / `full` panel-size specifiers) are ignored.
/// An optional `:width` suffix (e.g. `size:7`) is stripped so the field still matches.
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
        "perm" | "mode" => Some(UserFormatToken::Perm),
        "type" => Some(UserFormatToken::Type),
        "mtime" | "time" => Some(UserFormatToken::Mtime),
        "nlink" => Some(UserFormatToken::Nlink),
        "owner" => Some(UserFormatToken::Owner),
        "group" => Some(UserFormatToken::Group),
        "|" => Some(UserFormatToken::Gap),
        _ => None,
    }
}

fn user_token_width(tok: UserFormatToken) -> usize {
    match tok {
        UserFormatToken::Name => 0,
        UserFormatToken::Size => 8,
        UserFormatToken::Perm => 10,
        UserFormatToken::Type => 1,
        UserFormatToken::Mtime => 12,
        UserFormatToken::Nlink => 4,
        UserFormatToken::Owner => 8,
        UserFormatToken::Group => 8,
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

fn user_type_char(ent: &FileEntry) -> char {
    if ent.is_dir && ent.is_symlink {
        '~'
    } else if ent.is_dir {
        '/'
    } else if ent.is_symlink {
        '@'
    } else if ent.is_exe {
        '*'
    } else {
        ' '
    }
}

/// GNU-ish SI (base 1000) size: 1.2k, 3.4M, 5.6G. Used when Panel `kilobyte_si` is on.
pub fn format_si_size(size: u64) -> String {
    const K: f64 = 1_000.0;
    const M: f64 = 1_000_000.0;
    const G: f64 = 1_000_000_000.0;
    const T: f64 = 1_000_000_000_000.0;
    let s = size as f64;
    if s >= T {
        format!("{:.1}T", s / T)
    } else if s >= G {
        format!("{:.1}G", s / G)
    } else if s >= M {
        format!("{:.1}M", s / M)
    } else if s >= K {
        format!("{:.1}k", s / K)
    } else {
        size.to_string()
    }
}

fn user_size_string(ent: &FileEntry, si: bool) -> String {
    if ent.name == ".." {
        "UP--DIR".to_string()
    } else if ent.is_dir {
        String::new()
    } else if si {
        format_si_size(ent.size)
    } else {
        ent.size.to_string()
    }
}

fn user_mtime_string(ent: &FileEntry) -> String {
    let dt: time::OffsetDateTime = ent.modified.into();
    dt.format(&time::macros::format_description!(
        "[month repr:short] [day padding:space] [hour]:[minute]"
    ))
    .unwrap_or_default()
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
        UserFormatToken::Perm => match ent {
            Some(e) => user_perm_string(e.permissions, e.is_dir),
            None => fit_left("Perm", 10),
        },
        UserFormatToken::Type => match ent {
            Some(e) => user_type_char(e).to_string(),
            None => " ".to_string(),
        },
        UserFormatToken::Mtime => match ent {
            Some(e) => fit_left(&user_mtime_string(e), 12),
            None => fit_left("Modify time", 12),
        },
        UserFormatToken::Nlink => match ent {
            Some(_) => fit_right("1", 4),
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
        UserFormatToken::Gap => "|".to_string(),
    }
}

fn join_user_parts(
    tokens: &[UserFormatToken],
    width: usize,
    ent: Option<&FileEntry>,
    si: bool,
) -> String {
    if tokens.is_empty() {
        return String::new();
    }
    let name_width = name_column_width(tokens, width);
    let parts: Vec<String> = tokens
        .iter()
        .map(|t| render_user_token(*t, ent, name_width, si))
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
/// `si` selects SI (1000) size units vs raw bytes.
pub fn format_user_listing_line(
    ent: &FileEntry,
    tokens: &[UserFormatToken],
    width: usize,
    si: bool,
) -> String {
    join_user_parts(tokens, width, Some(ent), si)
}

/// Column header for a parsed user listing format, truncated to `width`.
pub fn format_user_listing_header(tokens: &[UserFormatToken], width: usize) -> String {
    join_user_parts(tokens, width, None, false)
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
            is_exe: false,
            size,
            modified,
            permissions: 0o755,
            owner: Some("user".into()),
            group: Some("group".into()),
        }
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
                UserFormatToken::Perm,
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
        let line = format_user_listing_line(&ent, &tokens, 80, false);
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
            format_user_listing_line(&dir, &tokens, 40, false).starts_with('/'),
            "dir type"
        );
        let mut exe = make_entry("a.out", 10, now, false);
        exe.is_exe = true;
        assert!(
            format_user_listing_line(&exe, &tokens, 40, false).starts_with('*'),
            "exe type"
        );
        let mut link = make_entry("link", 1, now, false);
        link.is_symlink = true;
        assert!(
            format_user_listing_line(&link, &tokens, 40, false).starts_with('@'),
            "symlink type"
        );
        let long = make_entry("very-long-filename-that-should-clip", 1, now, false);
        let clipped = format_user_listing_line(&long, &tokens, 12, false);
        assert!(clipped.chars().count() <= 12);
    }

    #[test]
    fn format_si_size_gnu_ish_lowercase() {
        assert_eq!(format_si_size(42), "42");
        assert_eq!(format_si_size(999), "999");
        assert_eq!(format_si_size(1_200), "1.2k");
        assert_eq!(format_si_size(3_400_000), "3.4M");
        assert_eq!(format_si_size(5_600_000_000), "5.6G");
    }

    #[test]
    fn user_format_size_si_flag() {
        let now = SystemTime::now();
        let ent = make_entry("big.bin", 3_400_000, now, false);
        let tokens = parse_user_listing_format("size");
        let raw = format_user_listing_line(&ent, &tokens, 16, false);
        assert!(raw.contains("3400000"), "raw={raw:?}");
        let si = format_user_listing_line(&ent, &tokens, 16, true);
        assert!(si.contains("3.4M"), "si={si:?}");
    }
}
