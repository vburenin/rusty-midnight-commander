use crate::actions::{Action, SortBy};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyMap {
    // Map of KeyEvent signature to Action
    #[serde(skip)]
    bindings: Vec<(KeyEvent, Action)>,
}

impl KeyMap {
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
