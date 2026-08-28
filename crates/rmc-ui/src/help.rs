// Help index loader and simple hypertext node model (Apache-2.0 original content).
use anyhow::{anyhow, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use rmc_core::app::HelpState;
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Node opened by F1 while the help viewer is already showing.
pub const KEYS_TOPIC: &str = "Keys";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelpAction {
    Stay,
    Quit,
}

#[derive(Debug, Clone)]
pub enum HelpItem {
    Text(String),
    Link { label: String, target: String },
}

#[derive(Debug, Clone)]
pub struct HelpNode {
    pub name: String,
    pub title: String,
    pub items: Vec<HelpItem>,
}

impl HelpNode {
    pub fn link_count(&self) -> usize {
        self.items
            .iter()
            .filter(|it| matches!(it, HelpItem::Link { .. }))
            .count()
    }

    pub fn link_target(&self, idx: usize) -> Option<&str> {
        self.items
            .iter()
            .filter_map(|it| match it {
                HelpItem::Link { target, .. } => Some(target.as_str()),
                HelpItem::Text(_) => None,
            })
            .nth(idx)
    }
}

#[derive(Debug, Default)]
pub struct HelpIndex {
    nodes: HashMap<String, HelpNode>, // keyed by case-sensitive name
}

impl HelpIndex {
    pub fn load_default() -> Result<Self> {
        if let Ok(p) = std::env::var("MC_HELPDIR") {
            if let Ok(idx) = Self::load_from_dir(Path::new(&p)) {
                if !idx.nodes.is_empty() {
                    return Ok(idx);
                }
            }
        }
        let crate_fallback = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/help");
        for cand in rmc_core::paths::data_file_candidates("help", crate_fallback) {
            if let Ok(idx) = Self::load_from_dir(&cand) {
                if !idx.nodes.is_empty() {
                    return Ok(idx);
                }
            }
        }
        Ok(Self::embedded())
    }

    pub fn embedded() -> Self {
        let mut idx = HelpIndex::default();
        for (stem, src) in EMBEDDED_HELP {
            if let Ok(node) = parse_help_text(stem, src) {
                idx.nodes.insert(node.name.clone(), node);
            }
        }
        idx
    }

    pub fn load_from_dir(dir: &Path) -> Result<Self> {
        let mut idx = HelpIndex::default();
        for ent in fs::read_dir(dir)? {
            let ent = ent?;
            let path = ent.path();
            if path.is_file() {
                let node = parse_help_file(&path)?;
                idx.nodes.insert(node.name.clone(), node);
            }
        }
        if idx.nodes.is_empty() {
            return Err(anyhow!("help: no nodes in {}", dir.display()));
        }
        Ok(idx)
    }

    pub fn get(&self, name: &str) -> Option<&HelpNode> {
        if let Some(n) = self.nodes.get(name) {
            return Some(n);
        }
        let lower = name.to_ascii_lowercase();
        self.nodes
            .values()
            .find(|n| n.name.to_ascii_lowercase() == lower)
    }

    pub fn contents_name(&self) -> String {
        if self.nodes.contains_key("Contents") {
            "Contents".to_string()
        } else {
            self.nodes
                .keys()
                .next()
                .cloned()
                .unwrap_or_else(|| "Contents".to_string())
        }
    }
}

const EMBEDDED_HELP: &[(&str, &str)] = &[
    ("contents", include_str!("../../../data/help/contents.txt")),
    ("keys", include_str!("../../../data/help/keys.txt")),
    ("panels", include_str!("../../../data/help/panels.txt")),
    (
        "find-file",
        include_str!("../../../data/help/find-file.txt"),
    ),
    ("editor", include_str!("../../../data/help/editor.txt")),
    ("viewer", include_str!("../../../data/help/viewer.txt")),
    ("diff", include_str!("../../../data/help/diff.txt")),
    ("vfs", include_str!("../../../data/help/vfs.txt")),
    (
        "directory-tree",
        include_str!("../../../data/help/directory-tree.txt"),
    ),
    (
        "external-panelize",
        include_str!("../../../data/help/external-panelize.txt"),
    ),
    (
        "screen-list",
        include_str!("../../../data/help/screen-list.txt"),
    ),
    (
        "user-menu",
        include_str!("../../../data/help/user-menu.txt"),
    ),
    ("dialogs", include_str!("../../../data/help/dialogs.txt")),
    ("copy", include_str!("../../../data/help/copy.txt")),
    ("move", include_str!("../../../data/help/move.txt")),
    ("mkdir", include_str!("../../../data/help/mkdir.txt")),
    ("delete", include_str!("../../../data/help/delete.txt")),
];

fn parse_help_file(path: &Path) -> Result<HelpNode> {
    let f = fs::File::open(path)?;
    let rdr = BufReader::new(f);
    let mut body = String::new();
    for line in rdr.lines() {
        body.push_str(&line?);
        body.push('\n');
    }
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Node");
    parse_help_text(stem, &body)
}

fn parse_help_text(fallback_name: &str, src: &str) -> Result<HelpNode> {
    let mut title = String::new();
    let mut name = fallback_name.to_string();
    let mut items = Vec::new();
    for raw in src.lines() {
        let s = raw.trim();
        if s.is_empty() || s.starts_with('#') || s.starts_with(';') {
            continue;
        }
        if let Some(rest) = s.strip_prefix("= ") {
            title = rest.trim().to_string();
            let tmp = title.clone();
            if let Some((t, n)) = tmp.split_once('|') {
                title = t.trim().to_string();
                name = n.trim().to_string();
            } else {
                name = title.clone();
            }
            continue;
        }
        if let Some(rest) = s.strip_prefix("* ") {
            if let Some((label, tgt)) = rest.split_once("->") {
                items.push(HelpItem::Link {
                    label: label.trim().to_string(),
                    target: tgt.trim().to_string(),
                });
            } else {
                let tgt = rest.trim().to_string();
                items.push(HelpItem::Link {
                    label: tgt.clone(),
                    target: tgt,
                });
            }
        } else {
            items.push(HelpItem::Text(raw.to_string()));
        }
    }
    if title.is_empty() {
        title = name.clone();
    }
    if name.is_empty() {
        return Err(anyhow!("help: empty node name"));
    }
    Ok(HelpNode { name, title, items })
}

pub fn initial_topic_or_contents(index: &HelpIndex, st: &HelpState) -> String {
    if index.get(&st.topic).is_some() {
        st.topic.clone()
    } else {
        index.contents_name()
    }
}

static HELP_INDEX: OnceLock<HelpIndex> = OnceLock::new();
pub fn global_index() -> &'static HelpIndex {
    HELP_INDEX.get_or_init(|| HelpIndex::load_default().unwrap_or_else(|_| HelpIndex::embedded()))
}

/// GNU mc(1) help viewer keys. Uses `page_rows` from `handle_key` (no TTY size).
pub fn apply_help_key(
    state: &mut HelpState,
    index: &HelpIndex,
    key: &KeyEvent,
    page_rows: usize,
) -> HelpAction {
    let page = page_rows.max(1);
    let half = (page / 2).max(1);
    let none = key.modifiers.is_empty();
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    if alt {
        return HelpAction::Stay;
    }

    match key.code {
        KeyCode::Esc | KeyCode::F(10) => HelpAction::Quit,
        KeyCode::Char('q' | 'Q') if none || (shift && !ctrl) => HelpAction::Quit,
        KeyCode::F(1) => {
            jump_topic(state, KEYS_TOPIC);
            HelpAction::Stay
        }
        KeyCode::F(2) => {
            let dest = index.contents_name();
            jump_topic(state, &dest);
            HelpAction::Stay
        }
        KeyCode::Char('c' | 'i') if none => {
            let dest = index.contents_name();
            jump_topic(state, &dest);
            HelpAction::Stay
        }
        KeyCode::F(3) => {
            history_back(state);
            HelpAction::Stay
        }
        KeyCode::F(4) | KeyCode::Enter => {
            follow_selected(state, index);
            HelpAction::Stay
        }
        KeyCode::Char('\n') if none => {
            follow_selected(state, index);
            HelpAction::Stay
        }
        KeyCode::Tab => {
            cycle_link(state, index, 1);
            HelpAction::Stay
        }
        KeyCode::BackTab => {
            cycle_link(state, index, -1);
            HelpAction::Stay
        }
        KeyCode::Right if none => {
            follow_selected(state, index);
            HelpAction::Stay
        }
        KeyCode::Left if none => {
            history_back(state);
            HelpAction::Stay
        }
        KeyCode::Down if none => {
            state.scroll_top = state.scroll_top.saturating_add(1);
            HelpAction::Stay
        }
        KeyCode::Up if none => {
            state.scroll_top = state.scroll_top.saturating_sub(1);
            HelpAction::Stay
        }
        KeyCode::PageDown => {
            state.scroll_top = state.scroll_top.saturating_add(page);
            HelpAction::Stay
        }
        KeyCode::PageUp => {
            state.scroll_top = state.scroll_top.saturating_sub(page);
            HelpAction::Stay
        }
        KeyCode::Char(' ') if none => {
            state.scroll_top = state.scroll_top.saturating_add(page);
            HelpAction::Stay
        }
        KeyCode::Backspace | KeyCode::Delete => {
            state.scroll_top = state.scroll_top.saturating_sub(page);
            HelpAction::Stay
        }
        KeyCode::Char('b') if none || ctrl => {
            state.scroll_top = state.scroll_top.saturating_sub(page);
            HelpAction::Stay
        }
        KeyCode::Char('h') if ctrl => {
            state.scroll_top = state.scroll_top.saturating_sub(page);
            HelpAction::Stay
        }
        KeyCode::Char('u') if none => {
            state.scroll_top = state.scroll_top.saturating_sub(half);
            HelpAction::Stay
        }
        KeyCode::Char('d') if none => {
            state.scroll_top = state.scroll_top.saturating_add(half);
            HelpAction::Stay
        }
        KeyCode::Home => {
            state.scroll_top = 0;
            HelpAction::Stay
        }
        KeyCode::End => {
            scroll_end(state, index, page);
            HelpAction::Stay
        }
        KeyCode::Char('g') if none => {
            state.scroll_top = 0;
            HelpAction::Stay
        }
        KeyCode::Char('G') if none || shift => {
            scroll_end(state, index, page);
            HelpAction::Stay
        }
        KeyCode::Char('l' | 'p') if none => {
            history_back(state);
            HelpAction::Stay
        }
        KeyCode::Char('n') if none => {
            follow_selected(state, index);
            HelpAction::Stay
        }
        _ => HelpAction::Stay,
    }
}

fn jump_topic(state: &mut HelpState, topic: &str) {
    if state.topic != topic {
        state.history.push(state.topic.clone());
        state.topic = topic.to_string();
        state.cursor = 0;
        state.scroll_top = 0;
    }
}

fn history_back(state: &mut HelpState) {
    if let Some(prev) = state.history.pop() {
        state.topic = prev;
        state.cursor = 0;
        state.scroll_top = 0;
    }
}

fn follow_selected(state: &mut HelpState, index: &HelpIndex) {
    let Some(node) = index.get(&state.topic) else {
        return;
    };
    let Some(target) = node.link_target(state.cursor) else {
        return;
    };
    if target == state.topic {
        return;
    }
    state.history.push(state.topic.clone());
    state.topic = target.to_string();
    state.cursor = 0;
    state.scroll_top = 0;
}

fn cycle_link(state: &mut HelpState, index: &HelpIndex, dir: isize) {
    let Some(node) = index.get(&state.topic) else {
        return;
    };
    let links = node.link_count();
    if links == 0 {
        return;
    }
    let next = state.cursor as isize + dir;
    state.cursor = if next < 0 {
        links - 1
    } else {
        next as usize % links
    };
}

fn scroll_end(state: &mut HelpState, index: &HelpIndex, page: usize) {
    let n = index
        .get(&state.topic)
        .map(|node| node.items.len())
        .unwrap_or(0);
    state.scroll_top = n.saturating_sub(page);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_index_has_required_nodes() {
        let idx = HelpIndex::embedded();
        for name in [
            "Contents",
            "Keys",
            "Panels",
            "Find File",
            "Editor",
            "Viewer",
            "VFS",
            "Directory Tree",
            "External panelize",
        ] {
            assert!(idx.get(name).is_some(), "missing help node {name}");
        }
        let contents = idx.get("Contents").unwrap();
        assert!(contents.link_count() >= 2, "Contents needs hypertext links");
    }

    #[test]
    fn parse_header_pipe_sets_node_name() {
        let node = parse_help_text("stem", "= Title | NodeName\n* A -> B\n").unwrap();
        assert_eq!(node.name, "NodeName");
        assert_eq!(node.title, "Title");
        assert_eq!(node.link_count(), 1);
        assert_eq!(node.link_target(0), Some("B"));
    }
}
