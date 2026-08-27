//! GNU mc(1) directory-tree figure engine.
//!
//! Shared by the Command-menu Directory tree dialog (`UiMode::DirectoryTree`)
//! and Left/Right panel [`crate::panel::PanelMode::Tree`]. Not the Find File
//! Tree picker. Dynamic mode shows parent/siblings/children; static mode shows
//! every known directory.

use crate::panel::TreeEntry;
use std::path::{Path, PathBuf};

/// Same cap as the panel tree / Find File tree picker.
pub const DIRECTORY_TREE_MAX_ENTRIES: usize = 2048;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectoryTreeMode {
    /// Default: Up/Down siblings, Left parent, Right child; only that neighborhood.
    Dynamic,
    /// Up/Down through all known directories.
    Static,
}

#[derive(Debug, Clone)]
pub struct DirectoryTreeState {
    /// All directories that have been scanned into the figure.
    pub known: Vec<TreeEntry>,
    /// Currently displayed rows (filtered in dynamic mode).
    pub entries: Vec<TreeEntry>,
    pub selected_index: usize,
    pub scroll_top: usize,
    pub mode: DirectoryTreeMode,
    /// Incremental name search (C-s / typed characters).
    pub search: String,
}

impl DirectoryTreeState {
    pub fn new(known: Vec<TreeEntry>, selected: &Path) -> Self {
        let mut st = Self {
            known,
            entries: Vec::new(),
            selected_index: 0,
            scroll_top: 0,
            mode: DirectoryTreeMode::Dynamic,
            search: String::new(),
        };
        st.rebuild_shown(Some(selected));
        st
    }

    pub fn is_dynamic(&self) -> bool {
        matches!(self.mode, DirectoryTreeMode::Dynamic)
    }

    pub fn selected_path(&self) -> PathBuf {
        self.entries
            .get(self.selected_index)
            .map(|e| e.path.clone())
            .or_else(|| self.known.first().map(|e| e.path.clone()))
            .unwrap_or_else(|| PathBuf::from("/"))
    }

    pub fn selected_depth(&self) -> usize {
        self.entries
            .get(self.selected_index)
            .map(|e| e.depth)
            .unwrap_or(0)
    }

    pub fn rebuild_shown(&mut self, keep: Option<&Path>) {
        let keep = keep
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| self.selected_path());
        match self.mode {
            DirectoryTreeMode::Static => {
                let mut all = self.known.clone();
                all.sort_by(|a, b| a.path.cmp(&b.path));
                self.entries = all;
            }
            DirectoryTreeMode::Dynamic => {
                self.entries = dynamic_neighborhood(&self.known, &keep);
            }
        }
        self.select_path(&keep);
    }

    pub fn select_path(&mut self, path: &Path) {
        if let Some(i) = self.entries.iter().position(|e| e.path == path) {
            self.selected_index = i;
        } else if self.entries.is_empty() {
            self.selected_index = 0;
        } else {
            self.selected_index = self.selected_index.min(self.entries.len() - 1);
        }
    }

    pub fn ensure_visible(&mut self, list_rows: usize) {
        if list_rows == 0 || self.entries.is_empty() {
            self.scroll_top = 0;
            return;
        }
        if self.selected_index < self.scroll_top {
            self.scroll_top = self.selected_index;
        } else if self.selected_index >= self.scroll_top + list_rows {
            self.scroll_top = self
                .selected_index
                .saturating_sub(list_rows.saturating_sub(1));
        }
        let max_top = self.entries.len().saturating_sub(list_rows);
        if self.scroll_top > max_top {
            self.scroll_top = max_top;
        }
    }

    pub fn toggle_mode(&mut self) {
        self.mode = match self.mode {
            DirectoryTreeMode::Dynamic => DirectoryTreeMode::Static,
            DirectoryTreeMode::Static => DirectoryTreeMode::Dynamic,
        };
        self.rebuild_shown(None);
    }

    pub fn move_up(&mut self) {
        match self.mode {
            DirectoryTreeMode::Dynamic => self.move_sibling(-1),
            DirectoryTreeMode::Static => {
                if self.selected_index > 0 {
                    self.selected_index -= 1;
                }
            }
        }
    }

    pub fn move_down(&mut self) {
        match self.mode {
            DirectoryTreeMode::Dynamic => self.move_sibling(1),
            DirectoryTreeMode::Static => {
                if self.selected_index + 1 < self.entries.len() {
                    self.selected_index += 1;
                }
            }
        }
    }

    pub fn page_up(&mut self, list_rows: usize) {
        let step = list_rows.max(1);
        match self.mode {
            DirectoryTreeMode::Dynamic => {
                let (lo, _) = self.sibling_range();
                self.selected_index = self.selected_index.saturating_sub(step).max(lo);
                let path = self.selected_path();
                self.rebuild_shown(Some(&path));
            }
            DirectoryTreeMode::Static => {
                self.selected_index = self.selected_index.saturating_sub(step);
            }
        }
    }

    pub fn page_down(&mut self, list_rows: usize) {
        let step = list_rows.max(1);
        match self.mode {
            DirectoryTreeMode::Dynamic => {
                let (_, hi) = self.sibling_range();
                if hi > 0 {
                    self.selected_index = self
                        .selected_index
                        .saturating_add(step)
                        .min(hi.saturating_sub(1));
                }
                let path = self.selected_path();
                self.rebuild_shown(Some(&path));
            }
            DirectoryTreeMode::Static => {
                if !self.entries.is_empty() {
                    let max = self.entries.len() - 1;
                    self.selected_index = self.selected_index.saturating_add(step).min(max);
                }
            }
        }
    }

    pub fn move_home(&mut self) {
        match self.mode {
            DirectoryTreeMode::Dynamic => {
                let (lo, _) = self.sibling_range();
                self.selected_index = lo;
                let path = self.selected_path();
                self.rebuild_shown(Some(&path));
            }
            DirectoryTreeMode::Static => {
                self.selected_index = 0;
            }
        }
    }

    pub fn move_end(&mut self) {
        match self.mode {
            DirectoryTreeMode::Dynamic => {
                let (_, hi) = self.sibling_range();
                if hi > 0 {
                    self.selected_index = hi - 1;
                }
                let path = self.selected_path();
                self.rebuild_shown(Some(&path));
            }
            DirectoryTreeMode::Static => {
                if !self.entries.is_empty() {
                    self.selected_index = self.entries.len() - 1;
                }
            }
        }
    }

    /// Left: parent directory.
    pub fn move_parent(&mut self) {
        let sel = self.selected_path();
        if let Some(parent) = parent_of(&sel) {
            if self.known.iter().any(|e| e.path == parent) {
                self.rebuild_shown(Some(&parent));
            }
        }
    }

    /// Right: first known child.
    pub fn move_child(&mut self) {
        let sel = self.selected_path();
        if let Some(child) = first_child(&self.known, &sel) {
            let p = child.clone();
            self.rebuild_shown(Some(&p));
        }
    }

    /// Drop this directory (and descendants) from the figure. Root is kept.
    pub fn forget_selected(&mut self) {
        let sel = self.selected_path();
        if is_root(&sel) {
            return;
        }
        let parent = parent_of(&sel);
        self.known
            .retain(|e| e.path != sel && !is_strict_descendant(&e.path, &sel));
        let keep = parent
            .filter(|p| self.known.iter().any(|e| e.path == *p))
            .or_else(|| self.known.first().map(|e| e.path.clone()));
        self.rebuild_shown(keep.as_deref());
    }

    /// Replace immediate children of the selected directory after a rescan.
    pub fn apply_rescan(&mut self, children: Vec<PathBuf>, parent_depth: usize) {
        let sel = self.selected_path();
        self.known.retain(|e| !is_strict_descendant(&e.path, &sel));
        for p in children {
            if self.known.iter().any(|e| e.path == p) {
                continue;
            }
            if self.known.len() >= DIRECTORY_TREE_MAX_ENTRIES {
                break;
            }
            self.known.push(TreeEntry {
                path: p,
                depth: parent_depth + 1,
            });
        }
        self.rebuild_shown(Some(&sel));
    }

    /// C-s / typed prefix: next name starting with `search`. No hit: one row down.
    pub fn search_next(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        if self.search.is_empty() {
            self.move_down();
            return;
        }
        let start = self.selected_index.saturating_add(1);
        let n = self.entries.len();
        for off in 0..n {
            let i = (start + off) % n;
            if name_starts_with(&self.entries[i].path, &self.search) {
                let path = self.entries[i].path.clone();
                self.rebuild_shown(Some(&path));
                return;
            }
        }
        self.move_down();
    }

    fn move_sibling(&mut self, dir: i32) {
        let (lo, hi) = self.sibling_range();
        if hi <= lo {
            return;
        }
        let next = if dir < 0 {
            self.selected_index.saturating_sub(1).max(lo)
        } else {
            (self.selected_index + 1).min(hi.saturating_sub(1))
        };
        if next == self.selected_index {
            return;
        }
        self.selected_index = next;
        let path = self.selected_path();
        self.rebuild_shown(Some(&path));
    }

    /// Inclusive start, exclusive end of the sibling block in `entries`.
    fn sibling_range(&self) -> (usize, usize) {
        if self.entries.is_empty() {
            return (0, 0);
        }
        let sel = self.selected_path();
        let indices: Vec<usize> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| same_parent(&e.path, &sel))
            .map(|(i, _)| i)
            .collect();
        match (indices.first(), indices.last()) {
            (Some(&lo), Some(&hi)) => (lo, hi + 1),
            _ => (self.selected_index, self.selected_index + 1),
        }
    }
}

fn is_root(path: &Path) -> bool {
    path == Path::new("/") || path.parent().is_none()
}

fn parent_of(path: &Path) -> Option<PathBuf> {
    if is_root(path) {
        None
    } else {
        path.parent().map(|p| p.to_path_buf())
    }
}

fn same_parent(a: &Path, b: &Path) -> bool {
    parent_of(a) == parent_of(b)
}

fn is_strict_descendant(path: &Path, ancestor: &Path) -> bool {
    path.starts_with(ancestor) && path != ancestor
}

fn first_child<'a>(known: &'a [TreeEntry], parent: &Path) -> Option<&'a PathBuf> {
    known
        .iter()
        .filter(|e| e.path.parent() == Some(parent))
        .map(|e| &e.path)
        .min()
}

fn dynamic_neighborhood(known: &[TreeEntry], selected: &Path) -> Vec<TreeEntry> {
    let mut out = Vec::new();
    if let Some(parent) = parent_of(selected) {
        if let Some(e) = known.iter().find(|e| e.path == parent) {
            out.push(e.clone());
        }
    }
    let mut siblings: Vec<TreeEntry> = known
        .iter()
        .filter(|e| same_parent(&e.path, selected))
        .cloned()
        .collect();
    siblings.sort_by(|a, b| a.path.cmp(&b.path));
    out.extend(siblings);
    let mut children: Vec<TreeEntry> = known
        .iter()
        .filter(|e| e.path.parent() == Some(selected))
        .cloned()
        .collect();
    children.sort_by(|a, b| a.path.cmp(&b.path));
    out.extend(children);
    if out.is_empty() {
        if let Some(e) = known.iter().find(|e| e.path == selected) {
            out.push(e.clone());
        }
    }
    out
}

fn name_starts_with(path: &Path, prefix: &str) -> bool {
    let name = if path == Path::new("/") {
        "/"
    } else {
        path.file_name().and_then(|s| s.to_str()).unwrap_or("")
    };
    name.starts_with(prefix)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(path: &str, depth: usize) -> TreeEntry {
        TreeEntry {
            path: PathBuf::from(path),
            depth,
        }
    }

    fn sample() -> DirectoryTreeState {
        // /home/u/{a/c, b}
        let known = vec![
            e("/", 0),
            e("/home", 1),
            e("/home/u", 2),
            e("/home/u/a", 3),
            e("/home/u/b", 3),
            e("/home/u/a/c", 4),
        ];
        DirectoryTreeState::new(known, Path::new("/home/u/a"))
    }

    fn paths(st: &DirectoryTreeState) -> Vec<String> {
        st.entries
            .iter()
            .map(|e| e.path.display().to_string())
            .collect()
    }

    #[test]
    fn default_is_dynamic_neighborhood() {
        let st = sample();
        assert!(st.is_dynamic());
        assert_eq!(
            paths(&st),
            vec!["/home/u", "/home/u/a", "/home/u/b", "/home/u/a/c"]
        );
        assert_eq!(st.selected_path(), PathBuf::from("/home/u/a"));
    }

    #[test]
    fn dynamic_up_down_move_siblings_not_children() {
        let mut st = sample();
        st.move_down();
        assert_eq!(st.selected_path(), PathBuf::from("/home/u/b"));
        // Child of a is not shown once we leave a.
        assert!(!paths(&st).iter().any(|p| p == "/home/u/a/c"));
        st.move_up();
        assert_eq!(st.selected_path(), PathBuf::from("/home/u/a"));
        assert!(paths(&st).iter().any(|p| p == "/home/u/a/c"));
    }

    #[test]
    fn left_parent_right_child() {
        let mut st = sample();
        st.move_parent();
        assert_eq!(st.selected_path(), PathBuf::from("/home/u"));
        st.move_child();
        assert_eq!(st.selected_path(), PathBuf::from("/home/u/a"));
        st.move_child();
        assert_eq!(st.selected_path(), PathBuf::from("/home/u/a/c"));
    }

    #[test]
    fn f4_toggles_static_shows_all_known() {
        let mut st = sample();
        st.toggle_mode();
        assert!(!st.is_dynamic());
        assert_eq!(st.mode, DirectoryTreeMode::Static);
        assert_eq!(
            paths(&st),
            vec![
                "/",
                "/home",
                "/home/u",
                "/home/u/a",
                "/home/u/a/c",
                "/home/u/b",
            ]
        );
        st.select_path(Path::new("/home/u/a"));
        st.move_down();
        assert_eq!(st.selected_path(), PathBuf::from("/home/u/a/c"));
        st.toggle_mode();
        assert!(st.is_dynamic());
    }

    #[test]
    fn forget_drops_dir_and_descendants() {
        let mut st = sample();
        st.forget_selected();
        assert!(!st.known.iter().any(|e| e.path == Path::new("/home/u/a")));
        assert!(!st.known.iter().any(|e| e.path == Path::new("/home/u/a/c")));
        assert_eq!(st.selected_path(), PathBuf::from("/home/u"));
    }

    #[test]
    fn forget_refuses_root() {
        let mut st = sample();
        st.rebuild_shown(Some(Path::new("/")));
        st.forget_selected();
        assert!(st.known.iter().any(|e| e.path == Path::new("/")));
    }

    #[test]
    fn rescan_replaces_children() {
        let mut st = sample();
        st.apply_rescan(vec![PathBuf::from("/home/u/a/n")], st.selected_depth());
        assert!(!st.known.iter().any(|e| e.path == Path::new("/home/u/a/c")));
        assert!(st.known.iter().any(|e| e.path == Path::new("/home/u/a/n")));
    }
}
