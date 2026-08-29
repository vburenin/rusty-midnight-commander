use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneSide {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortBy {
    Name,
    Ext,
    Size,
    Time,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    // High level UI mode changes
    Quit,
    /// GNU mc(1) C-r / Left-Right → Reread: force-reload the **active** panel listing.
    /// Distinct from C-l Repaint (screen redraw only). Always `list_dir`s even when
    /// Fast directory reload would skip an automatic reload.
    Refresh,
    /// GNU mc(1) C-l: full terminal repaint. Does not re-list directories (that is Refresh / C-r).
    Repaint,
    ToggleHidden,
    SwapPanels,
    /// GNU mc SplitEqual (Alt-=). 50/50 split; does not swap. Not a Left/Right
    /// drop-down row in GNU 4.8.30 (`create_panel_menu`).
    EqualizePanels,
    /// GNU mc Layout Alt-, (Alt-comma): toggle Vertical ↔ Horizontal panel split.
    TogglePanelSplit,
    /// Open the MC-style User Menu (F2)
    ShowUserMenu,
    FocusMenu,
    ShowHelp,
    // Navigation
    MoveUp,
    MoveDown,
    PageUp,
    PageDown,
    Home,
    End,
    /// GNU mc(1) Alt-g: select the top currently visible listing entry.
    PanelJumpTop,
    /// GNU mc(1) Alt-r: select the middle currently visible listing entry.
    PanelJumpMiddle,
    /// GNU mc(1) Alt-j: select the bottom currently visible listing entry.
    PanelJumpBottom,
    /// GNU mc(1) Quick search: C-s / Alt-s starts; C-s / Alt-s again finds the next match.
    QuickSearch,
    Enter,
    ParentDir,
    SwitchPanel,
    ToggleSelect,
    // View formats
    CycleListingFormat,
    // Sorting
    Sort(SortBy),
    // File operations
    ViewFile,
    Copy,
    Move,
    Mkdir,
    Delete,
    // Permissions/Ownership and Links
    Chmod,
    Chown,
    LinkHard,
    SymlinkAbs,
    SymlinkRel,
    // Viewer specific
    ViewerQuit,
    ViewerToggleHex,
    // Group selection operations
    SelectGroup,
    UnselectGroup,
    InvertSelection,
    /// Toggle full-screen subshell/output view (C-o).
    ToggleSubshell,
    // Hotlist
    OpenHotlist,
    /// GNU mc(1) Command → Find file / `Find = alt-question` (not F17; F17 is viewer search-again).
    FindFile,
    // Mouse interactions (coordinates handled by UI)
    MouseClick {
        x: u16,
        y: u16,
        button: MouseButton,
    },
    MouseScroll {
        up: bool,
    },
    FunctionKey(u8),
}

/// GNU mc Esc-number idle timeout. A pending Esc then 1..9/0 is F1..F9/F10;
/// after this delay the prefix is dropped so later digits are not stolen.
pub const ESC_NUMBER_TIMEOUT: Duration = Duration::from_secs(1);

/// GNU mc(1): Esc then digit `1`..`9`/`0` emulates F1..F9/F10.
pub fn esc_digit_to_function_key(c: char) -> Option<u8> {
    match c {
        '1' => Some(1),
        '2' => Some(2),
        '3' => Some(3),
        '4' => Some(4),
        '5' => Some(5),
        '6' => Some(6),
        '7' => Some(7),
        '8' => Some(8),
        '9' => Some(9),
        '0' => Some(10),
        _ => None,
    }
}

pub fn keyevent_to_function_key(ev: &KeyEvent) -> Option<u8> {
    match ev.code {
        KeyCode::F(n @ 1..=12) => Some(n),
        KeyCode::Char(c) if ev.modifiers.contains(KeyModifiers::ALT) => {
            esc_digit_to_function_key(c)
        }
        _ => None,
    }
}

/// Crossterm maps raw TTY bytes `0x1C..=0x1F` to Ctrl-4..Ctrl-7, not the
/// characters those bytes actually are (`C-\`, `C-]`, `C-^`, `C-_`).
/// GNU mc 4.8.33 `HotList = ctrl-backslash` is the FS byte `0x1C`.
pub fn normalize_crossterm_ctrl_key(key: KeyEvent) -> KeyEvent {
    if !key.modifiers.contains(KeyModifiers::CONTROL) {
        return key;
    }
    let code = match key.code {
        KeyCode::Char('4') => KeyCode::Char('\\'),
        KeyCode::Char('5') => KeyCode::Char(']'),
        KeyCode::Char('6') => KeyCode::Char('^'),
        KeyCode::Char('7') => KeyCode::Char('_'),
        _ => return key,
    };
    KeyEvent {
        code,
        modifiers: key.modifiers,
        kind: key.kind,
        state: key.state,
    }
}

/// GNU mc(1) File menu Shift-F / F13–F20. Terminals send `F(13)`…`F(20)` **or**
/// Shift+F3…Shift+F10. Esc-number stays F1–F10 (`esc_digit_to_function_key`).
pub fn file_menu_shift_function_key(ev: &KeyEvent) -> Option<u8> {
    let shift_only = ev.modifiers.contains(KeyModifiers::SHIFT)
        && !ev.modifiers.contains(KeyModifiers::CONTROL)
        && !ev.modifiers.contains(KeyModifiers::ALT);
    match ev.code {
        KeyCode::F(n @ 13..=20) if ev.modifiers.is_empty() => Some(n),
        KeyCode::F(n @ 3..=10) if shift_only => Some(n.saturating_add(10)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn esc_digit_emulates_f1_through_f10() {
        assert_eq!(esc_digit_to_function_key('1'), Some(1));
        assert_eq!(esc_digit_to_function_key('5'), Some(5));
        assert_eq!(esc_digit_to_function_key('9'), Some(9));
        assert_eq!(esc_digit_to_function_key('0'), Some(10));
        assert_eq!(esc_digit_to_function_key('a'), None);
    }

    #[test]
    fn alt_digit_emulates_f1_through_f10() {
        assert_eq!(
            keyevent_to_function_key(&KeyEvent::new(KeyCode::Char('1'), KeyModifiers::ALT)),
            Some(1)
        );
        assert_eq!(
            keyevent_to_function_key(&KeyEvent::new(KeyCode::Char('0'), KeyModifiers::ALT)),
            Some(10)
        );
        assert_eq!(
            keyevent_to_function_key(&KeyEvent::new(KeyCode::F(3), KeyModifiers::NONE)),
            Some(3)
        );
        assert_eq!(
            keyevent_to_function_key(&KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE)),
            None
        );
    }

    #[test]
    fn file_menu_shift_f_maps_f13_through_f20_and_shift_f3_through_f10() {
        assert_eq!(
            file_menu_shift_function_key(&KeyEvent::new(KeyCode::F(13), KeyModifiers::NONE)),
            Some(13)
        );
        assert_eq!(
            file_menu_shift_function_key(&KeyEvent::new(KeyCode::F(16), KeyModifiers::NONE)),
            Some(16)
        );
        assert_eq!(
            file_menu_shift_function_key(&KeyEvent::new(KeyCode::F(20), KeyModifiers::NONE)),
            Some(20)
        );
        assert_eq!(
            file_menu_shift_function_key(&KeyEvent::new(KeyCode::F(3), KeyModifiers::SHIFT)),
            Some(13)
        );
        assert_eq!(
            file_menu_shift_function_key(&KeyEvent::new(KeyCode::F(4), KeyModifiers::SHIFT)),
            Some(14)
        );
        assert_eq!(
            file_menu_shift_function_key(&KeyEvent::new(KeyCode::F(5), KeyModifiers::SHIFT)),
            Some(15)
        );
        assert_eq!(
            file_menu_shift_function_key(&KeyEvent::new(KeyCode::F(6), KeyModifiers::SHIFT)),
            Some(16)
        );
        assert_eq!(
            file_menu_shift_function_key(&KeyEvent::new(KeyCode::F(10), KeyModifiers::SHIFT)),
            Some(20)
        );
        // Esc-number / plain F3 stay F1–F10, not F13.
        assert_eq!(
            file_menu_shift_function_key(&KeyEvent::new(KeyCode::F(3), KeyModifiers::NONE)),
            None
        );
        assert_eq!(esc_digit_to_function_key('3'), Some(3));
        assert_eq!(
            file_menu_shift_function_key(&KeyEvent::new(
                KeyCode::F(3),
                KeyModifiers::SHIFT | KeyModifiers::CONTROL
            )),
            None
        );
    }

    #[test]
    fn crossterm_fs_byte_is_ctrl_backslash() {
        let raw = KeyEvent::new(KeyCode::Char('4'), KeyModifiers::CONTROL);
        let got = normalize_crossterm_ctrl_key(raw);
        assert_eq!(got.code, KeyCode::Char('\\'));
        assert!(got.modifiers.contains(KeyModifiers::CONTROL));
        let plain = KeyEvent::new(KeyCode::Char('4'), KeyModifiers::NONE);
        assert_eq!(normalize_crossterm_ctrl_key(plain).code, KeyCode::Char('4'));
        let ctrl_s = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
        assert_eq!(
            normalize_crossterm_ctrl_key(ctrl_s).code,
            KeyCode::Char('s'),
            "letter Ctrl chords must stay"
        );
    }
}

#[derive(Debug, Clone)]
pub enum MouseAction {
    Click {
        kind: MouseEventKind,
        x: u16,
        y: u16,
        button: Option<MouseButton>,
    },
    Scroll {
        up: bool,
    },
}
