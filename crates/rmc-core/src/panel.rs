use crate::selection::Selection;
use crate::sorting::{self, SortDir};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

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
    Size,
    Time,
}

#[derive(Debug, Clone)]
pub struct PanelState {
    pub cwd: PathBuf,
    pub entries: Vec<FileEntry>,
    pub cursor: usize,
    pub scroll_top: usize,
    pub show_hidden: bool,
    pub sort_by: SortBy,
    pub sort_dir: SortDir,
    pub selection: Selection,
}

impl PanelState {
    pub fn new<P: AsRef<Path>>(cwd: P) -> Self {
        Self {
            cwd: cwd.as_ref().to_path_buf(),
            entries: Vec::new(),
            cursor: 0,
            scroll_top: 0,
            show_hidden: false,
            sort_by: SortBy::Name,
            sort_dir: SortDir::Asc,
            selection: Selection::default(),
        }
    }

    pub fn set_entries(&mut self, mut entries: Vec<FileEntry>) {
        // Always put parent dir marker first
        entries.sort_by(|a, b| {
            let ap = a.is_dir && a.name == "..";
            let bp = b.is_dir && b.name == "..";
            match (ap, bp) {
                (true, true) | (false, false) => Ordering::Equal,
                (true, false) => Ordering::Less,
                (false, true) => Ordering::Greater,
            }
        });
        self.entries = entries;
        self.apply_sort();
        if self.cursor >= self.entries.len() {
            self.cursor = self.entries.len().saturating_sub(1);
        }
    }

    pub fn apply_sort(&mut self) {
        let (dirs, mut rest): (Vec<_>, Vec<_>) = self.entries.drain(..).partition(|e| e.is_dir);
        let mut dirs = dirs;
        match self.sort_by {
            SortBy::Name => {
                sorting::sort_by_name(&mut dirs, self.sort_dir);
                sorting::sort_by_name(&mut rest, self.sort_dir);
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
}
