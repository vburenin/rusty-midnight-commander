use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HotlistEntry {
    pub label: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Hotlist {
    pub entries: Vec<HotlistEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotlistDialogFocus {
    List,
    ButtonGoto,
    ButtonAdd,
    ButtonRemove,
    ButtonCancel,
}

#[derive(Debug, Clone)]
pub struct HotlistDialogState {
    pub entries: Vec<HotlistEntry>,
    pub selected_index: usize,
    pub scroll_top: usize,
    pub focus: HotlistDialogFocus,
}

impl HotlistDialogState {
    pub fn new(entries: Vec<HotlistEntry>) -> Self {
        Self {
            entries,
            selected_index: 0,
            scroll_top: 0,
            focus: HotlistDialogFocus::List,
        }
    }
}

impl Hotlist {
    /// Load the hotlist from ~/.config/mc/hotlist (or XDG_CONFIG_HOME/mc/hotlist).
    /// Uses a simple custom format:
    /// - Lines: "label = /absolute/path"
    /// - Leading/trailing whitespace around label/path is trimmed
    /// - Empty lines and lines starting with '#' are ignored
    /// - Duplicate labels are allowed (later lines replace earlier with same label)
    pub fn load_from_default_path() -> Self {
        let path = default_hotlist_path();
        if !path.exists() {
            return Self::default();
        }
        Self::load_from_file(&path).unwrap_or_default()
    }

    pub fn load_from_file(path: &Path) -> Result<Self> {
        let f = fs::File::open(path)
            .with_context(|| format!("Failed to open hotlist file {}", path.display()))?;
        let mut entries: Vec<HotlistEntry> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for line in BufReader::new(f).lines() {
            let raw = line?;
            let s = raw.trim();
            if s.is_empty() || s.starts_with('#') || s.starts_with(';') {
                continue;
            }
            if let Some((label, path_str)) = s.split_once('=') {
                let label = label.trim().to_string();
                let path_str = path_str.trim();
                if label.is_empty() || path_str.is_empty() {
                    continue;
                }
                let p = PathBuf::from(path_str);
                // Only accept absolute paths to avoid ambiguity
                if !p.is_absolute() {
                    continue;
                }
                // If duplicate label occurs, keep the last occurrence
                if seen.contains(&label) {
                    if let Some(idx) = entries.iter().position(|e| e.label == label) {
                        entries.remove(idx);
                    }
                }
                seen.insert(label.clone());
                entries.push(HotlistEntry { label, path: p });
            }
        }
        Ok(Hotlist { entries })
    }

    pub fn save_to_default_path(&self) -> Result<()> {
        let path = default_hotlist_path();
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)
                .with_context(|| format!("Failed to create config dir {}", dir.display()))?;
        }
        self.save_to_file(&path)
    }

    pub fn save_to_file(&self, path: &Path) -> Result<()> {
        let mut tmp = path.to_path_buf();
        tmp.set_extension("tmp");
        {
            let mut f = fs::File::create(&tmp)
                .with_context(|| format!("Failed to create temp file {}", tmp.display()))?;
            // Header comment describing the format
            writeln!(
                f,
                "# Rusty Midnight Commander hotlist (Apache-2.0 original simple format)\n# One entry per line: label = /absolute/path\n# Lines starting with '#' are comments"
            )?;
            for e in &self.entries {
                // Normalize to absolute path strings
                let p = if e.path.is_absolute() {
                    e.path.clone()
                } else {
                    std::env::current_dir()?.join(&e.path)
                };
                writeln!(f, "{} = {}", e.label, p.display())?;
            }
            f.flush()?;
        }
        fs::rename(&tmp, path).with_context(|| {
            format!(
                "Failed to atomically replace hotlist file {}",
                path.display()
            )
        })?;
        Ok(())
    }

    pub fn add_or_replace(&mut self, label: String, path: PathBuf) -> Result<()> {
        if !path.is_absolute() {
            bail!("Hotlist only supports absolute paths");
        }
        if let Some(idx) = self.entries.iter().position(|e| e.label == label) {
            self.entries[idx] = HotlistEntry { label, path };
        } else {
            self.entries.push(HotlistEntry { label, path });
        }
        Ok(())
    }

    pub fn remove_at(&mut self, idx: usize) {
        if idx < self.entries.len() {
            self.entries.remove(idx);
        }
    }
}

pub fn default_hotlist_path() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        return Path::new(&xdg).join("mc").join("hotlist");
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    Path::new(&home).join(".config").join("mc").join("hotlist")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn load_and_save_roundtrip() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("hotlist");
        let mut h = Hotlist::default();
        h.add_or_replace("Home".into(), PathBuf::from("/home/test"))
            .unwrap();
        h.add_or_replace("Etc".into(), PathBuf::from("/etc"))
            .unwrap();
        h.save_to_file(&file).unwrap();
        let loaded = Hotlist::load_from_file(&file).unwrap();
        assert_eq!(loaded.entries.len(), 2);
        assert_eq!(loaded.entries[0].label, "Home");
        assert_eq!(loaded.entries[0].path, PathBuf::from("/home/test"));
        assert_eq!(loaded.entries[1].label, "Etc");
        assert_eq!(loaded.entries[1].path, PathBuf::from("/etc"));
    }
}
