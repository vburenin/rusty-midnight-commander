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
    /// User-defined format string stored on the panel; rendered like Long for now.
    User,
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
            // GNU-ish default placeholder; parsed later if/when implemented.
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
}
