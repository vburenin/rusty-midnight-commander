use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};

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
    Refresh,
    ToggleHidden,
    SwapPanels,
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

pub fn keyevent_to_function_key(ev: &KeyEvent) -> Option<u8> {
    match ev.code {
        KeyCode::F(n @ 1..=12) => Some(n),
        KeyCode::Char('1') if ev.modifiers.contains(KeyModifiers::ALT) => Some(1),
        KeyCode::Char('2') if ev.modifiers.contains(KeyModifiers::ALT) => Some(2),
        KeyCode::Char('3') if ev.modifiers.contains(KeyModifiers::ALT) => Some(3),
        KeyCode::Char('4') if ev.modifiers.contains(KeyModifiers::ALT) => Some(4),
        KeyCode::Char('5') if ev.modifiers.contains(KeyModifiers::ALT) => Some(5),
        KeyCode::Char('6') if ev.modifiers.contains(KeyModifiers::ALT) => Some(6),
        KeyCode::Char('7') if ev.modifiers.contains(KeyModifiers::ALT) => Some(7),
        KeyCode::Char('8') if ev.modifiers.contains(KeyModifiers::ALT) => Some(8),
        KeyCode::Char('9') if ev.modifiers.contains(KeyModifiers::ALT) => Some(9),
        KeyCode::Char('0') if ev.modifiers.contains(KeyModifiers::ALT) => Some(10),
        _ => None,
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
