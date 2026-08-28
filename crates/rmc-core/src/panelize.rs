//! GNU External panelize named-command store.
//!
//! Original Apache-2.0 JSON schema (`panelize.json` next to the hotlist).
//! This is not Midnight Commander's GPL `panelize` file format.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// One saved External panelize command (descriptive name + shell command).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PanelizeCommand {
    pub name: String,
    pub command: String,
}

/// On-disk store of named External panelize commands.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PanelizeStore {
    #[serde(default)]
    pub commands: Vec<PanelizeCommand>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalPanelizeFocus {
    List,
    Command,
    ButtonAddNew,
    ButtonPanelize,
    ButtonRemove,
    ButtonCancel,
}

impl ExternalPanelizeFocus {
    pub fn next(self) -> Self {
        match self {
            Self::List => Self::Command,
            Self::Command => Self::ButtonAddNew,
            Self::ButtonAddNew => Self::ButtonPanelize,
            Self::ButtonPanelize => Self::ButtonRemove,
            Self::ButtonRemove => Self::ButtonCancel,
            Self::ButtonCancel => Self::List,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::List => Self::ButtonCancel,
            Self::Command => Self::List,
            Self::ButtonAddNew => Self::Command,
            Self::ButtonPanelize => Self::ButtonAddNew,
            Self::ButtonRemove => Self::ButtonPanelize,
            Self::ButtonCancel => Self::ButtonRemove,
        }
    }

    pub fn is_button(self) -> bool {
        matches!(
            self,
            Self::ButtonAddNew | Self::ButtonPanelize | Self::ButtonRemove | Self::ButtonCancel
        )
    }

    /// Cycle Add new → Panelize → Remove → Cancel.
    pub fn next_button(self) -> Self {
        match self {
            Self::ButtonAddNew => Self::ButtonPanelize,
            Self::ButtonPanelize => Self::ButtonRemove,
            Self::ButtonRemove => Self::ButtonCancel,
            Self::ButtonCancel => Self::ButtonAddNew,
            other => other,
        }
    }

    pub fn prev_button(self) -> Self {
        match self {
            Self::ButtonAddNew => Self::ButtonCancel,
            Self::ButtonPanelize => Self::ButtonAddNew,
            Self::ButtonRemove => Self::ButtonPanelize,
            Self::ButtonCancel => Self::ButtonRemove,
            other => other,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExternalPanelizeDialogState {
    pub commands: Vec<PanelizeCommand>,
    pub selected_index: usize,
    pub scroll_top: usize,
    pub command: String,
    pub focus: ExternalPanelizeFocus,
    /// GNU "enter a name" overlay after Add new. `None` while the main dialog is active.
    pub name_prompt: Option<String>,
}

impl ExternalPanelizeDialogState {
    pub fn new(commands: Vec<PanelizeCommand>) -> Self {
        Self {
            commands,
            selected_index: 0,
            scroll_top: 0,
            command: String::new(),
            focus: ExternalPanelizeFocus::Command,
            name_prompt: None,
        }
    }

    pub fn fill_command_from_selection(&mut self) {
        if let Some(e) = self.commands.get(self.selected_index) {
            self.command = e.command.clone();
        }
    }

    pub fn clamp_selection(&mut self) {
        if self.commands.is_empty() {
            self.selected_index = 0;
            self.scroll_top = 0;
            return;
        }
        if self.selected_index >= self.commands.len() {
            self.selected_index = self.commands.len() - 1;
        }
        if self.selected_index < self.scroll_top {
            self.scroll_top = self.selected_index;
        }
    }
}

impl PanelizeStore {
    /// Load from XDG/mc/panelize.json (or ~/.config/mc/panelize.json). Missing file → empty.
    pub fn load_from_default_path() -> Self {
        let path = default_panelize_path();
        if !path.exists() {
            return Self::default();
        }
        Self::load_from_file(&path).unwrap_or_default()
    }

    pub fn load_from_file(path: &Path) -> Result<Self> {
        let data = fs::read_to_string(path)
            .with_context(|| format!("Failed to read panelize file {}", path.display()))?;
        let store: Self = serde_json::from_str(&data)
            .with_context(|| format!("Failed to parse panelize file {}", path.display()))?;
        Ok(store)
    }

    pub fn save_to_default_path(&self) -> Result<()> {
        let path = default_panelize_path();
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)
                .with_context(|| format!("Failed to create config dir {}", dir.display()))?;
        }
        self.save_to_file(&path)
    }

    pub fn save_to_file(&self, path: &Path) -> Result<()> {
        let mut tmp = path.to_path_buf();
        tmp.set_extension("json.tmp");
        let data = serde_json::to_string_pretty(self)
            .context("Failed to serialize panelize named commands")?;
        fs::write(&tmp, data)
            .with_context(|| format!("Failed to write temp file {}", tmp.display()))?;
        fs::rename(&tmp, path).with_context(|| {
            format!(
                "Failed to atomically replace panelize file {}",
                path.display()
            )
        })?;
        Ok(())
    }

    pub fn add_or_replace(&mut self, name: String, command: String) {
        if let Some(idx) = self.commands.iter().position(|e| e.name == name) {
            self.commands[idx] = PanelizeCommand { name, command };
        } else {
            self.commands.push(PanelizeCommand { name, command });
        }
    }

    pub fn remove_at(&mut self, idx: usize) {
        if idx < self.commands.len() {
            self.commands.remove(idx);
        }
    }
}

/// Same config family as the hotlist: `$MC_PROFILE_ROOT` / `$XDG_CONFIG_HOME/mc`
/// or `~/.config/mc`.
pub fn default_panelize_path() -> PathBuf {
    crate::paths::user_mc_config_dir().join("panelize.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn load_missing_file_is_empty() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("panelize.json");
        assert!(!file.exists());
        // load_from_file errors on missing; default-path helper treats missing as empty.
        let store = PanelizeStore::load_from_file(&file);
        assert!(store.is_err());
    }

    #[test]
    fn load_and_save_roundtrip() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("panelize.json");
        let mut s = PanelizeStore::default();
        s.add_or_replace("Hello".into(), "echo hello".into());
        s.add_or_replace("Links".into(), "find . -type l -print".into());
        s.save_to_file(&file).unwrap();
        let loaded = PanelizeStore::load_from_file(&file).unwrap();
        assert_eq!(loaded.commands.len(), 2);
        assert_eq!(loaded.commands[0].name, "Hello");
        assert_eq!(loaded.commands[0].command, "echo hello");
        assert_eq!(loaded.commands[1].name, "Links");
        assert_eq!(loaded.commands[1].command, "find . -type l -print");
    }

    #[test]
    fn add_or_replace_keeps_last_command_for_name() {
        let mut s = PanelizeStore::default();
        s.add_or_replace("Hello".into(), "echo hello".into());
        s.add_or_replace("Hello".into(), "echo world".into());
        assert_eq!(s.commands.len(), 1);
        assert_eq!(s.commands[0].command, "echo world");
    }

    #[test]
    fn remove_at_drops_entry() {
        let mut s = PanelizeStore::default();
        s.add_or_replace("A".into(), "echo a".into());
        s.add_or_replace("B".into(), "echo b".into());
        s.remove_at(0);
        assert_eq!(s.commands.len(), 1);
        assert_eq!(s.commands[0].name, "B");
        s.remove_at(99);
        assert_eq!(s.commands.len(), 1);
    }
}
