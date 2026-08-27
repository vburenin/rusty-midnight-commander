use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use rmc_core::actions::Action;
use rmc_core::config::KeyMap;

#[test]
fn resolves_mc_defaults() {
    let km = KeyMap::mc_defaults();
    let a = km.resolve(&KeyEvent::new(KeyCode::F(10), KeyModifiers::NONE));
    assert!(matches!(a, Some(Action::Quit)));
    let a = km.resolve(&KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL));
    assert!(matches!(a, Some(Action::ToggleHidden)));
    let a = km.resolve(&KeyEvent::new(KeyCode::Char('='), KeyModifiers::ALT));
    assert!(matches!(a, Some(Action::EqualizePanels)));
    let a = km.resolve(&KeyEvent::new(KeyCode::Char(','), KeyModifiers::ALT));
    assert!(matches!(a, Some(Action::TogglePanelSplit)));
}
