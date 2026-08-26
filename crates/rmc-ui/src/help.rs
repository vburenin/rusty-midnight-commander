use anyhow::{anyhow, Result};
use rmc_core::app::HelpState;
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

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

#[derive(Debug, Default)]
pub struct HelpIndex {
    nodes: HashMap<String, HelpNode>, // keyed by case-sensitive name
}

impl HelpIndex {
    pub fn load_default() -> Result<Self> {
        if let Ok(p) = std::env::var("MC_HELPDIR") {
            if let Ok(idx) = Self::load_from_dir(Path::new(&p)) {
                return Ok(idx);
            }
        }
        for cand in [
            PathBuf::from("data/help"),
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/help"),
        ] {
            if let Ok(idx) = Self::load_from_dir(&cand) {
                return Ok(idx);
            }
        }
        Err(anyhow!("help: could not locate data/help directory"))
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

fn parse_help_file(path: &Path) -> Result<HelpNode> {
    let f = fs::File::open(path)?;
    let rdr = BufReader::new(f);
    let mut title = String::new();
    let mut name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Node")
        .to_string();
    let mut items = Vec::new();
    for line in rdr.lines() {
        let raw = line?;
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
            items.push(HelpItem::Text(raw));
        }
    }
    if title.is_empty() {
        title = name.clone();
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
    HELP_INDEX.get_or_init(|| HelpIndex::load_default().unwrap_or_default())
}
