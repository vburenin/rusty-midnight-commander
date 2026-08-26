use crate::actions::{Action, SortBy};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Serialize};
use anyhow::Result;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyMap {
    // Map of KeyEvent signature to Action
    #[serde(skip)]
    bindings: Vec<(KeyEvent, Action)>,
}

impl KeyMap {
    /// Try to load keymap from data/mc.keymap or MC_KEYMAP; fall back to mc_defaults.
    pub fn load_default() -> Self {
        if let Ok(p) = std::env::var("MC_KEYMAP") {
            if let Ok(km) = Self::load_from_file(Path::new(&p)) {
                return km;
            }
        }
        let candidates = [
            PathBuf::from("data/mc.keymap"),
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/mc.keymap"),
        ];
        for p in candidates {
            if let Ok(km) = Self::load_from_file(&p) {
                return km;
            }
        }
        Self::mc_defaults()
    }

    /// Load keymap from a given file path.
    pub fn load_from_file(path: &Path) -> Result<Self> {
        let f = File::open(path)?;
        let mut km = Self { bindings: Vec::new() };
        for (lineno, line) in BufReader::new(f).lines().enumerate() {
            let raw = line?;
            let s = raw.trim();
            if s.is_empty() || s.starts_with('#') || s.starts_with(';') {
                continue;
            }
            let (key_str, action_str) = match s.split_once('=') {
                Some((k, v)) => (k.trim(), v.trim()),
                None => continue, // ignore invalid
            };
            if let (Some(key), Some(action)) = (parse_key(key_str), parse_action(action_str)) {
                km.bind(key, action);
            } else {
                eprintln!("Warning: could not parse keymap at line {}: {}", lineno + 1, raw);
            }
        }
        Ok(km)
    }

    pub fn mc_defaults() -> Self {
        use Action::*;
        let mut m = Self { bindings: Vec::new() };
        m.bind(new_event(KeyCode::Up), MoveUp);
        m.bind(new_event(KeyCode::Down), MoveDown);
        m.bind(new_event(KeyCode::PageUp), PageUp);
        m.bind(new_event(KeyCode::PageDown), PageDown);
        m.bind(new_event(KeyCode::Home), Home);
        m.bind(new_event(KeyCode::End), End);
        m.bind(new_event(KeyCode::Tab), SwitchPanel);
        m.bind(new_event(KeyCode::Enter), Enter);
        m.bind(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE), ParentDir);
        m.bind(KeyEvent::new(KeyCode::PageUp, KeyModifiers::CONTROL), ParentDir);
        m.bind(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL), ToggleHidden);
        m.bind(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL), SwapPanels);
        m.bind(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL), Refresh);
        // Function keys
        m.bind(new_event(KeyCode::F(1)), ShowHelp);
        m.bind(new_event(KeyCode::F(3)), ViewFile);
        m.bind(new_event(KeyCode::F(5)), Copy);
        m.bind(new_event(KeyCode::F(6)), Move);
        m.bind(new_event(KeyCode::F(7)), Mkdir);
        m.bind(new_event(KeyCode::F(8)), Delete);
        m.bind(new_event(KeyCode::F(9)), FocusMenu);
        m.bind(new_event(KeyCode::F(10)), Quit);
        // Sorting shortcuts (stub: Shift+N/S/T)
        m.bind(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::ALT), Sort(SortBy::Name));
        m.bind(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::ALT), Sort(SortBy::Size));
        m.bind(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::ALT), Sort(SortBy::Time));
        // Selection toggles
        m.bind(KeyEvent::new(KeyCode::Insert, KeyModifiers::NONE), ToggleSelect);
        m.bind(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE), ToggleSelect);
        m
    }

    pub fn bind(&mut self, key: KeyEvent, action: Action) {
        self.bindings.push((key, action));
    }

    pub fn resolve(&self, ev: &KeyEvent) -> Option<Action> {
        // Simple resolution by exact match
        for (k, a) in &self.bindings {
            if k == ev {
                return Some(a.clone());
            }
        }
        None
    }
}

fn new_event(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn parse_key(s: &str) -> Option<KeyEvent> {
    // Recognize modifiers C- and Alt-
    let mut mods = KeyModifiers::NONE;
    let mut rem = s.trim();
    // Support "C-PageUp" and similar with '-' in name
    if let Some(rest) = rem.strip_prefix("C-") {
        mods |= KeyModifiers::CONTROL;
        rem = rest;
    }
    if let Some(rest) = rem.strip_prefix("Alt-") {
        mods |= KeyModifiers::ALT;
        rem = rest;
    }
    // Names
    let code = match rem {
        "Up" => KeyCode::Up,
        "Down" => KeyCode::Down,
        "Left" => KeyCode::Left,
        "Right" => KeyCode::Right,
        "Home" => KeyCode::Home,
        "End" => KeyCode::End,
        "PageUp" => KeyCode::PageUp,
        "PageDown" => KeyCode::PageDown,
        "Tab" => KeyCode::Tab,
        "Enter" => KeyCode::Enter,
        "Backspace" => KeyCode::Backspace,
        "Insert" => KeyCode::Insert,
        "Space" => KeyCode::Char(' '),
        // Function keys
        s if s.starts_with('F') => {
            if let Ok(n) = s[1..].parse::<u8>() {
                KeyCode::F(n)
            } else {
                return None;
            }
        }
        // Single character (e.g., "h" after modifiers)
        s if s.len() == 1 => {
            let ch = s.chars().next().unwrap();
            KeyCode::Char(ch)
        }
        // Special combined names
        "Page-Up" => KeyCode::PageUp,
        "Page-Down" => KeyCode::PageDown,
        _ => return None,
    };
    Some(KeyEvent::new(code, mods))
}

fn parse_action(s: &str) -> Option<Action> {
    use Action::*;
    match s {
        "Quit" => Some(Quit),
        "Refresh" => Some(Refresh),
        "ToggleHidden" => Some(ToggleHidden),
        "SwapPanels" => Some(SwapPanels),
        "FocusMenu" => Some(FocusMenu),
        "ShowHelp" => Some(ShowHelp),
        "MoveUp" => Some(MoveUp),
        "MoveDown" => Some(MoveDown),
        "PageUp" => Some(PageUp),
        "PageDown" => Some(PageDown),
        "Home" => Some(Home),
        "End" => Some(End),
        "Enter" => Some(Enter),
        "ParentDir" => Some(ParentDir),
        "SwitchPanel" => Some(SwitchPanel),
        "ToggleSelect" => Some(ToggleSelect),
        "ViewFile" => Some(ViewFile),
        "Copy" => Some(Copy),
        "Move" => Some(Move),
        "Mkdir" => Some(Mkdir),
        "Delete" => Some(Delete),
        "ViewerQuit" => Some(ViewerQuit),
        "ViewerToggleHex" => Some(ViewerToggleHex),
        "SortName" => Some(Sort(SortBy::Name)),
        "SortSize" => Some(Sort(SortBy::Size)),
        "SortTime" => Some(Sort(SortBy::Time)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};

    #[test]
    fn resolve_some_default_keymap() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../data/mc.keymap");
        let km = KeyMap::load_from_file(&path).expect("load keymap");
        assert!(matches!(km.resolve(&KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)), Some(Action::MoveUp)));
        assert!(matches!(km.resolve(&KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE)), Some(Action::Copy)));
        assert!(matches!(km.resolve(&KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)), Some(Action::Refresh)));
        assert!(matches!(km.resolve(&KeyEvent::new(KeyCode::Char('n'), KeyModifiers::ALT)), Some(Action::Sort(SortBy::Name))));
        assert!(matches!(km.resolve(&KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)), Some(Action::ParentDir)));
    }
}
