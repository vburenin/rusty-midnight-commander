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
    let a = km.resolve(&KeyEvent::new(KeyCode::Char('g'), KeyModifiers::ALT));
    assert!(matches!(a, Some(Action::PanelJumpTop)));
    let a = km.resolve(&KeyEvent::new(KeyCode::Char('r'), KeyModifiers::ALT));
    assert!(matches!(a, Some(Action::PanelJumpMiddle)));
    let a = km.resolve(&KeyEvent::new(KeyCode::Char('j'), KeyModifiers::ALT));
    assert!(matches!(a, Some(Action::PanelJumpBottom)));
    let a = km.resolve(&KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    assert!(matches!(a, Some(Action::QuickSearch)));
    let a = km.resolve(&KeyEvent::new(KeyCode::Char('s'), KeyModifiers::ALT));
    assert!(matches!(a, Some(Action::QuickSearch)));
    let a = km.resolve(&KeyEvent::new(KeyCode::Insert, KeyModifiers::NONE));
    assert!(matches!(a, Some(Action::ToggleSelect)));
    let a = km.resolve(&KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL));
    assert!(matches!(a, Some(Action::ToggleSelect)));
    let a = km.resolve(&KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL));
    assert!(matches!(a, Some(Action::Repaint)));
    let a = km.resolve(&KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
    assert!(matches!(a, Some(Action::Refresh)));
    for (n, want) in [
        (1, "ShowHelp"),
        (2, "ShowUserMenu"),
        (3, "ViewFile"),
        (4, "FunctionKey4"),
        (5, "Copy"),
        (6, "Move"),
        (7, "Mkdir"),
        (8, "Delete"),
        (9, "FocusMenu"),
        (10, "Quit"),
    ] {
        let a = km.resolve(&KeyEvent::new(KeyCode::F(n), KeyModifiers::NONE));
        match (n, a) {
            (1, Some(Action::ShowHelp)) => {}
            (2, Some(Action::ShowUserMenu)) => {}
            (3, Some(Action::ViewFile)) => {}
            (4, Some(Action::FunctionKey(4))) => {}
            (5, Some(Action::Copy)) => {}
            (6, Some(Action::Move)) => {}
            (7, Some(Action::Mkdir)) => {}
            (8, Some(Action::Delete)) => {}
            (9, Some(Action::FocusMenu)) => {}
            (10, Some(Action::Quit)) => {}
            other => panic!("F{n} must bind {want}, got {other:?}"),
        }
    }
    assert!(
        matches!(
            km.resolve(&KeyEvent::new(KeyCode::F(13), KeyModifiers::NONE)),
            Some(Action::FunctionKey(13))
        ),
        "F13 View raw"
    );
    assert!(
        matches!(
            km.resolve(&KeyEvent::new(KeyCode::F(3), KeyModifiers::SHIFT)),
            Some(Action::FunctionKey(13))
        ),
        "S-F3 == F13"
    );
    assert!(
        matches!(
            km.resolve(&KeyEvent::new(KeyCode::F(14), KeyModifiers::NONE)),
            Some(Action::FunctionKey(14))
        ),
        "F14 Edit new"
    );
    assert!(
        matches!(
            km.resolve(&KeyEvent::new(KeyCode::F(4), KeyModifiers::SHIFT)),
            Some(Action::FunctionKey(14))
        ),
        "S-F4 == F14"
    );
    assert!(
        matches!(
            km.resolve(&KeyEvent::new(KeyCode::F(15), KeyModifiers::NONE)),
            Some(Action::FunctionKey(15))
        ),
        "F15 Copy to current"
    );
    assert!(
        matches!(
            km.resolve(&KeyEvent::new(KeyCode::F(5), KeyModifiers::SHIFT)),
            Some(Action::FunctionKey(15))
        ),
        "S-F5 == F15"
    );
    assert!(
        matches!(
            km.resolve(&KeyEvent::new(KeyCode::F(16), KeyModifiers::NONE)),
            Some(Action::FunctionKey(16))
        ),
        "F16 Move to current"
    );
    assert!(
        matches!(
            km.resolve(&KeyEvent::new(KeyCode::F(6), KeyModifiers::SHIFT)),
            Some(Action::FunctionKey(16))
        ),
        "S-F6 == F16"
    );
    assert!(
        matches!(
            km.resolve(&KeyEvent::new(KeyCode::F(20), KeyModifiers::NONE)),
            Some(Action::Quit)
        ),
        "F20 Quit"
    );
    assert!(
        matches!(
            km.resolve(&KeyEvent::new(KeyCode::F(10), KeyModifiers::SHIFT)),
            Some(Action::Quit)
        ),
        "S-F10 == F20 Quit"
    );
}
