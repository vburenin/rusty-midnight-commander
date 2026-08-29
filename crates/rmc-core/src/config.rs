use crate::actions::{Action, SortBy};
use crate::dirtree::DirectoryTreeState;
use crate::panel::{
    clamp_brief_columns, ListingFormat, PanelMode, PanelState, SortBy as PanelSortBy, TreeEntry,
    TreeState,
};
use crate::sorting::SortDir;
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyMap {
    // Map of KeyEvent signature to Action
    #[serde(skip)]
    bindings: Vec<(KeyEvent, Action)>,
    /// GNU `C-x <key>` prefix chords (second key after Control-x).
    #[serde(skip)]
    ctrl_x_bindings: Vec<(KeyEvent, Action)>,
}

impl KeyMap {
    /// Try to load keymap from `MC_KEYMAP`, then `%pkgdatadir%`/`data/mc.keymap`.
    ///
    /// GNU mc(1) “Redefine hotkey bindings”: `MC_KEYMAP` may be an absolute path
    /// (with or without `.keymap`) or a name searched in `~/.config/mc`,
    /// `$MC_DATADIR`, and the shipped `data/` directory.
    pub fn load_default() -> Self {
        if let Some(spec) = std::env::var("MC_KEYMAP")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
        {
            if let Some(path) =
                crate::paths::resolve_keymap_spec(&spec, &crate::paths::keymap_search_dirs())
            {
                if let Ok(km) = Self::load_from_file(&path) {
                    return km;
                }
            }
        }
        let crate_fallback = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/mc.keymap");
        for p in crate::paths::data_file_candidates("mc.keymap", crate_fallback) {
            if let Ok(km) = Self::load_from_file(&p) {
                return km;
            }
        }
        Self::mc_defaults()
    }

    /// Load keymap from a given file path.
    pub fn load_from_file(path: &Path) -> Result<Self> {
        let (km, warnings) = Self::load_from_file_with_warnings(path)?;
        for w in &warnings {
            eprintln!("Warning: {w}");
        }
        Ok(km)
    }

    /// Load keymap and return parse warnings instead of printing them.
    ///
    /// Warning strings match the startup `eprintln!` text without the `Warning: `
    /// prefix: `could not parse keymap at line N: <raw>`.
    pub fn load_from_file_with_warnings(path: &Path) -> Result<(Self, Vec<String>)> {
        let f = File::open(path)?;
        // Start from defaults and overlay file bindings (file wins)
        let mut km = Self::mc_defaults();
        let mut warnings = Vec::new();
        let mut in_section_main = true; // top-level allowed
        for (lineno, line) in BufReader::new(f).lines().enumerate() {
            let raw = line?;
            let s = raw.trim();
            if s.is_empty() || s.starts_with('#') || s.starts_with(';') {
                continue;
            }
            if s.starts_with('[') && s.ends_with(']') {
                let sec = &s[1..s.len() - 1];
                in_section_main = sec.eq_ignore_ascii_case("main");
                continue;
            }
            if !in_section_main {
                continue;
            }
            let (lhs, rhs) = match s.split_once('=') {
                Some((a, b)) => (a.trim(), b.trim()),
                None => continue, // ignore invalid
            };
            // Accept either "Key = Action" (current) or "Action = key" (MC-like)
            if let Some((spec, action)) = parse_binding_line(lhs, rhs) {
                match spec {
                    KeySpec::Simple(key) => km.set_binding(key, action),
                    KeySpec::CtrlX(key) => km.set_ctrl_x_binding(key, action),
                }
            } else {
                warnings.push(format!(
                    "could not parse keymap at line {}: {}",
                    lineno + 1,
                    raw
                ));
            }
        }
        Ok((km, warnings))
    }

    pub fn mc_defaults() -> Self {
        use Action::*;
        let mut m = Self {
            bindings: Vec::new(),
            ctrl_x_bindings: Vec::new(),
        };
        m.bind(new_event(KeyCode::Up), MoveUp);
        m.bind(new_event(KeyCode::Down), MoveDown);
        m.bind(new_event(KeyCode::PageUp), PageUp);
        m.bind(new_event(KeyCode::PageDown), PageDown);
        m.bind(new_event(KeyCode::Home), Home);
        m.bind(new_event(KeyCode::End), End);
        // GNU mc(1) Directory Panels: C-p / C-n / Alt-v / C-v aliases.
        // Not Emacs cmdline keys (history stays Alt-p / Alt-n).
        m.bind(
            KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL),
            MoveUp,
        );
        m.bind(
            KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL),
            MoveDown,
        );
        m.bind(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::ALT), PageUp);
        m.bind(
            KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL),
            PageDown,
        );
        // GNU mc(1) Directory Panels: Alt-g/r/j jump to top/middle/bottom visible file.
        m.bind(
            KeyEvent::new(KeyCode::Char('g'), KeyModifiers::ALT),
            PanelJumpTop,
        );
        m.bind(
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::ALT),
            PanelJumpMiddle,
        );
        m.bind(
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::ALT),
            PanelJumpBottom,
        );
        m.bind(new_event(KeyCode::Tab), SwitchPanel);
        // GNU mc(1) Directory Panels: C-i is the same ChangePanel as Tab.
        m.bind(
            KeyEvent::new(KeyCode::Char('i'), KeyModifiers::CONTROL),
            SwitchPanel,
        );
        m.bind(new_event(KeyCode::Enter), Enter);
        m.bind(
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
            ParentDir,
        );
        m.bind(
            KeyEvent::new(KeyCode::PageUp, KeyModifiers::CONTROL),
            ParentDir,
        );
        m.bind(
            KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL),
            ToggleHidden,
        );
        m.bind(
            KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
            SwapPanels,
        );
        // GNU mc SplitEqual (mc.keymap): Alt-=
        m.bind(
            KeyEvent::new(KeyCode::Char('='), KeyModifiers::ALT),
            EqualizePanels,
        );
        // GNU mc Layout Panel split toggle: Alt-, (Alt-comma)
        m.bind(
            KeyEvent::new(KeyCode::Char(','), KeyModifiers::ALT),
            TogglePanelSplit,
        );
        m.bind(
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL),
            Refresh,
        );
        // GNU mc(1) Miscellaneous Keys: C-l redraw / repaint the screen (not C-r Reload).
        m.bind(
            KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL),
            Repaint,
        );
        // Subshell toggle (C-o)
        m.bind(
            KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL),
            Action::ToggleSubshell,
        );
        // Function keys
        m.bind(new_event(KeyCode::F(1)), ShowHelp);
        // F2: User Menu
        m.bind(new_event(KeyCode::F(2)), Action::ShowUserMenu);
        m.bind(new_event(KeyCode::F(3)), ViewFile);
        m.bind(new_event(KeyCode::F(4)), Action::FunctionKey(4));
        m.bind(new_event(KeyCode::F(5)), Copy);
        m.bind(new_event(KeyCode::F(6)), Move);
        m.bind(new_event(KeyCode::F(7)), Mkdir);
        m.bind(new_event(KeyCode::F(8)), Delete);
        m.bind(new_event(KeyCode::F(9)), FocusMenu);
        m.bind(new_event(KeyCode::F(10)), Quit);
        // GNU mc(1) File menu Shift-F / F13–F20 (terminals send F(13) or Shift+F3).
        m.bind(new_event(KeyCode::F(13)), Action::FunctionKey(13));
        m.bind(
            KeyEvent::new(KeyCode::F(3), KeyModifiers::SHIFT),
            Action::FunctionKey(13),
        );
        m.bind(new_event(KeyCode::F(14)), Action::FunctionKey(14));
        m.bind(
            KeyEvent::new(KeyCode::F(4), KeyModifiers::SHIFT),
            Action::FunctionKey(14),
        );
        m.bind(new_event(KeyCode::F(15)), Action::FunctionKey(15));
        m.bind(
            KeyEvent::new(KeyCode::F(5), KeyModifiers::SHIFT),
            Action::FunctionKey(15),
        );
        m.bind(new_event(KeyCode::F(16)), Action::FunctionKey(16));
        m.bind(
            KeyEvent::new(KeyCode::F(6), KeyModifiers::SHIFT),
            Action::FunctionKey(16),
        );
        m.bind(new_event(KeyCode::F(20)), Quit);
        m.bind(KeyEvent::new(KeyCode::F(10), KeyModifiers::SHIFT), Quit);
        // Selection group keys
        m.bind(new_event(KeyCode::Char('+')), Action::SelectGroup);
        m.bind(new_event(KeyCode::Char('\\')), Action::UnselectGroup);
        m.bind(new_event(KeyCode::Char('*')), Action::InvertSelection);
        // GNU mc(1) Directory Panels: C-\ directory hotlist.
        m.bind(
            KeyEvent::new(KeyCode::Char('\\'), KeyModifiers::CONTROL),
            OpenHotlist,
        );
        // GNU mc 4.8.33 `mc.default.keymap`: `Find = alt-question` (help: Alt-?).
        // F17 / S-F7 is viewer/editor SearchContinue, not Find File.
        m.bind(
            KeyEvent::new(KeyCode::Char('?'), KeyModifiers::ALT),
            Action::FindFile,
        );
        // GNU mc(1) C-x prefix chords (UI still consumes Control-x as a prefix).
        m.set_ctrl_x_binding(new_event(KeyCode::Char('c')), Chmod);
        m.set_ctrl_x_binding(new_event(KeyCode::Char('o')), Chown);
        m.set_ctrl_x_binding(new_event(KeyCode::Char('l')), LinkHard);
        m.set_ctrl_x_binding(new_event(KeyCode::Char('s')), SymlinkAbs);
        m.set_ctrl_x_binding(new_event(KeyCode::Char('v')), SymlinkRel);
        // GNU mc(1) Quick search: C-s / Alt-s. Sort by size stays on the Sort order dialog.
        m.bind(
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
            QuickSearch,
        );
        m.bind(
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::ALT),
            QuickSearch,
        );
        // Sorting shortcuts (stub: Shift+N/S/T)
        m.bind(
            KeyEvent::new(KeyCode::Char('n'), KeyModifiers::ALT),
            Sort(SortBy::Name),
        );
        // GNU mc(1) Alt-t: cycle listing format Full → Brief → Long → User → Full
        m.bind(
            KeyEvent::new(KeyCode::Char('t'), KeyModifiers::ALT),
            Action::CycleListingFormat,
        );
        // GNU mc(1) Insert / C-t: toggle mark. Space remains a ToggleSelect alias.
        m.bind(
            KeyEvent::new(KeyCode::Insert, KeyModifiers::NONE),
            ToggleSelect,
        );
        m.bind(
            KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL),
            ToggleSelect,
        );
        m.bind(
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
            ToggleSelect,
        );
        m
    }

    pub fn bind(&mut self, key: KeyEvent, action: Action) {
        self.bindings.push((key, action));
    }

    /// Overwrite any existing binding for this key event, then set it.
    pub fn set_binding(&mut self, key: KeyEvent, action: Action) {
        self.bindings.retain(|(k, _)| *k != key);
        self.bind(key, action);
    }

    /// Overwrite any existing C-x chord for this second key, then set it.
    pub fn set_ctrl_x_binding(&mut self, key: KeyEvent, action: Action) {
        self.ctrl_x_bindings.retain(|(k, _)| *k != key);
        self.ctrl_x_bindings.push((key, action));
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

    /// Resolve the second key of a GNU `C-x <key>` chord.
    pub fn resolve_ctrl_x(&self, ev: &KeyEvent) -> Option<Action> {
        for (k, a) in &self.ctrl_x_bindings {
            if k == ev {
                return Some(a.clone());
            }
        }
        None
    }

    /// Return the first key bound to the given action, if any.
    pub fn first_key_for_action(&self, action: &Action) -> Option<KeyEvent> {
        for (k, a) in &self.bindings {
            if a == action {
                return Some(*k);
            }
        }
        None
    }

    /// Expose bindings as a slice for iteration.
    pub fn bindings(&self) -> &[(KeyEvent, Action)] {
        &self.bindings
    }

    /// Save the keymap to a file in a simple INI-like format.
    /// Uses the same names that `load_from_file` accepts.
    pub fn save_to_file(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                fs::create_dir_all(dir)?;
            }
        }
        let mut f = File::create(path)?;
        // Header
        writeln!(f, "# rmc keymap")?;
        writeln!(f, "[main]")?;
        for (key, action) in &self.bindings {
            // Skip mouse pseudo-actions; skip unknown/unrepresentable keys.
            if matches!(
                action,
                Action::MouseClick { .. } | Action::MouseScroll { .. }
            ) {
                continue;
            }
            let k = format_key(key);
            if k == "Unknown" {
                continue;
            }
            let a = format_action(action);
            writeln!(f, "{k} = {a}")?;
        }
        for (key, action) in &self.ctrl_x_bindings {
            if matches!(
                action,
                Action::MouseClick { .. } | Action::MouseScroll { .. }
            ) {
                continue;
            }
            let k = format_key(key);
            if k == "Unknown" {
                continue;
            }
            let a = format_action(action);
            writeln!(f, "C-x {k} = {a}")?;
        }
        Ok(())
    }

    /// Remove all bindings associated with the given action.
    pub fn remove_action_bindings(&mut self, action: &Action) {
        self.bindings.retain(|(_, a)| a != action);
        self.ctrl_x_bindings.retain(|(_, a)| a != action);
    }
}

fn new_event(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

enum KeySpec {
    Simple(KeyEvent),
    /// GNU `C-x <key>` two-key chord; value is the second key.
    CtrlX(KeyEvent),
}

fn parse_binding_line(lhs: &str, rhs: &str) -> Option<(KeySpec, Action)> {
    if let Some(spec) = parse_key_spec(lhs) {
        if let Some(act) = parse_action(rhs) {
            return Some((spec, act));
        }
    }
    if let Some(act) = parse_action(lhs) {
        if let Some(spec) = parse_key_spec(rhs) {
            return Some((spec, act));
        }
    }
    None
}

/// Parse a keymap key token, including GNU `C-x c` chords and `C-\\` / `\\`.
fn parse_key_spec(s: &str) -> Option<KeySpec> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(idx) = s.find(char::is_whitespace) {
        let first = s[..idx].trim();
        let rest = s[idx..].trim();
        if rest.is_empty() {
            return parse_key(first).map(KeySpec::Simple);
        }
        let prefix = parse_key(first)?;
        let second = parse_key(rest)?;
        if matches!(prefix.code, KeyCode::Char('x') | KeyCode::Char('X'))
            && prefix.modifiers.contains(KeyModifiers::CONTROL)
        {
            return Some(KeySpec::CtrlX(second));
        }
        return None;
    }
    parse_key(s).map(KeySpec::Simple)
}

fn parse_key(s: &str) -> Option<KeyEvent> {
    // Recognize modifiers C- and Alt-
    let mut mods = KeyModifiers::NONE;
    let mut rem = s.trim();
    // Support "C-PageUp" and similar with '-' in name
    if rem.to_ascii_lowercase().starts_with("c-") {
        rem = &rem[2..];
        mods |= KeyModifiers::CONTROL;
    }
    if rem.to_ascii_lowercase().starts_with("alt-") || rem.to_ascii_lowercase().starts_with("m-") {
        rem = &rem[4..];
        mods |= KeyModifiers::ALT;
    }
    if rem.to_ascii_lowercase().starts_with("s-") {
        rem = &rem[2..];
        mods |= KeyModifiers::SHIFT;
    }
    // Names
    let lc = rem.to_ascii_lowercase();
    let code = match lc.as_str() {
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" | "page-up" => KeyCode::PageUp,
        "pagedown" | "page-down" => KeyCode::PageDown,
        "tab" => KeyCode::Tab,
        "enter" => KeyCode::Enter,
        "backspace" => KeyCode::Backspace,
        "insert" => KeyCode::Insert,
        "space" => KeyCode::Char(' '),
        // GNU mc.keymap writes `?` as `question` (`Find = alt-question`).
        "question" => KeyCode::Char('?'),
        // GNU mc.keymap writes backslash as `\\` (and `C-\\`).
        "\\" | "\\\\" => KeyCode::Char('\\'),
        // Function keys
        s if s.starts_with('f') => {
            if let Ok(n) = s[1..].parse::<u8>() {
                KeyCode::F(n)
            } else {
                return None;
            }
        }
        // Single character (e.g., "h" after modifiers)
        s if s.len() == 1 => {
            let ch = lc.chars().next().unwrap();
            KeyCode::Char(ch)
        }
        _ => return None,
    };
    Some(KeyEvent::new(code, mods))
}

fn parse_action(s: &str) -> Option<Action> {
    use Action::*;
    match s {
        "Quit" => Some(Quit),
        "Refresh" => Some(Refresh),
        "Repaint" => Some(Repaint),
        "ToggleSubshell" => Some(Action::ToggleSubshell),
        "ToggleHidden" => Some(ToggleHidden),
        "SwapPanels" => Some(SwapPanels),
        "EqualizePanels" => Some(EqualizePanels),
        "TogglePanelSplit" => Some(TogglePanelSplit),
        "ShowUserMenu" | "UserMenu" => Some(ShowUserMenu),
        "FocusMenu" => Some(FocusMenu),
        "ShowHelp" => Some(ShowHelp),
        "CycleListingFormat" => Some(CycleListingFormat),
        "MoveUp" => Some(MoveUp),
        "MoveDown" => Some(MoveDown),
        "PageUp" => Some(PageUp),
        "PageDown" => Some(PageDown),
        "Home" => Some(Home),
        "End" => Some(End),
        "PanelJumpTop" => Some(PanelJumpTop),
        "PanelJumpMiddle" => Some(PanelJumpMiddle),
        "PanelJumpBottom" => Some(PanelJumpBottom),
        "QuickSearch" | "StartOrNextQuickSearch" => Some(QuickSearch),
        "Enter" => Some(Enter),
        "ParentDir" => Some(ParentDir),
        "SwitchPanel" => Some(SwitchPanel),
        "ToggleSelect" => Some(ToggleSelect),
        "ViewFile" => Some(ViewFile),
        "Copy" => Some(Copy),
        "Move" => Some(Move),
        "Mkdir" => Some(Mkdir),
        "Delete" => Some(Delete),
        "SelectGroup" => Some(Action::SelectGroup),
        "UnselectGroup" => Some(Action::UnselectGroup),
        "InvertSelection" => Some(Action::InvertSelection),
        "Chmod" => Some(Chmod),
        "Chown" => Some(Chown),
        "LinkHard" => Some(LinkHard),
        "SymlinkAbs" => Some(SymlinkAbs),
        "SymlinkRel" => Some(SymlinkRel),
        "ViewerQuit" => Some(ViewerQuit),
        "ViewerToggleHex" => Some(ViewerToggleHex),
        "SortName" => Some(Sort(SortBy::Name)),
        "SortExt" => Some(Sort(SortBy::Ext)),
        "SortSize" => Some(Sort(SortBy::Size)),
        "SortTime" => Some(Sort(SortBy::Time)),
        "OpenHotlist" => Some(OpenHotlist),
        "Find" | "FindFile" => Some(Action::FindFile),
        _ => {
            // FunctionKeyN pattern (e.g., FunctionKey4)
            if let Some(num) = s.strip_prefix("FunctionKey") {
                if let Ok(n) = num.parse::<u8>() {
                    return Some(Action::FunctionKey(n));
                }
            }
            None
        }
    }
}

fn format_action(a: &Action) -> String {
    use Action::*;
    match a {
        Quit => "Quit",
        Refresh => "Refresh",
        Repaint => "Repaint",
        Action::ToggleSubshell => "ToggleSubshell",
        ToggleHidden => "ToggleHidden",
        SwapPanels => "SwapPanels",
        EqualizePanels => "EqualizePanels",
        TogglePanelSplit => "TogglePanelSplit",
        ShowUserMenu => "ShowUserMenu",
        FocusMenu => "FocusMenu",
        ShowHelp => "ShowHelp",
        CycleListingFormat => "CycleListingFormat",
        MoveUp => "MoveUp",
        MoveDown => "MoveDown",
        PageUp => "PageUp",
        PageDown => "PageDown",
        Home => "Home",
        End => "End",
        PanelJumpTop => "PanelJumpTop",
        PanelJumpMiddle => "PanelJumpMiddle",
        PanelJumpBottom => "PanelJumpBottom",
        QuickSearch => "QuickSearch",
        Enter => "Enter",
        ParentDir => "ParentDir",
        SwitchPanel => "SwitchPanel",
        ToggleSelect => "ToggleSelect",
        ViewFile => "ViewFile",
        Copy => "Copy",
        Move => "Move",
        Mkdir => "Mkdir",
        Delete => "Delete",
        Action::SelectGroup => "SelectGroup",
        Action::UnselectGroup => "UnselectGroup",
        Action::InvertSelection => "InvertSelection",
        Chmod => "Chmod",
        Chown => "Chown",
        LinkHard => "LinkHard",
        SymlinkAbs => "SymlinkAbs",
        SymlinkRel => "SymlinkRel",
        ViewerQuit => "ViewerQuit",
        ViewerToggleHex => "ViewerToggleHex",
        Action::Sort(SortBy::Name) => "SortName",
        Action::Sort(SortBy::Ext) => "SortExt",
        Action::Sort(SortBy::Size) => "SortSize",
        Action::Sort(SortBy::Time) => "SortTime",
        Action::OpenHotlist => "OpenHotlist",
        Action::FindFile => "FindFile",
        Action::FunctionKey(n) => return format!("FunctionKey{}", n),
        Action::MouseClick { .. } | Action::MouseScroll { .. } => "Mouse",
    }
    .to_string()
}

fn format_key(ev: &KeyEvent) -> String {
    let mut out = String::new();
    if ev.modifiers.contains(KeyModifiers::CONTROL) {
        out.push_str("C-");
    }
    if ev.modifiers.contains(KeyModifiers::ALT) {
        out.push_str("Alt-");
    }
    if ev.modifiers.contains(KeyModifiers::SHIFT) {
        out.push_str("S-");
    }
    match ev.code {
        KeyCode::Up => out.push_str("Up"),
        KeyCode::Down => out.push_str("Down"),
        KeyCode::Left => out.push_str("Left"),
        KeyCode::Right => out.push_str("Right"),
        KeyCode::Home => out.push_str("Home"),
        KeyCode::End => out.push_str("End"),
        KeyCode::PageUp => out.push_str("PageUp"),
        KeyCode::PageDown => out.push_str("PageDown"),
        KeyCode::Tab => out.push_str("Tab"),
        KeyCode::Enter => out.push_str("Enter"),
        KeyCode::Backspace => out.push_str("Backspace"),
        KeyCode::Insert => out.push_str("Insert"),
        KeyCode::Char(' ') => out.push_str("Space"),
        KeyCode::Char('\\') => out.push_str("\\\\"),
        KeyCode::Char(ch) => out.push(ch),
        KeyCode::F(n) => out.push_str(&format!("F{n}")),
        _ => out.push_str("Unknown"),
    }
    out
}

/// Return the config directory path honoring `$MCR_CONFIG_DIR`, then
/// GNU `~/.config/mc` (`$MC_PROFILE_ROOT` / `$XDG_CONFIG_HOME` / `$HOME`).
pub fn default_config_dir() -> PathBuf {
    crate::paths::default_config_dir()
}

/// Save App options (layout/confirm/panels/Left/Right) and keymap to the user
/// config dir (`~/.config/mc` unless relocated). Creates the directory as needed.
pub fn save_setup(app: &crate::app::App) -> Result<()> {
    save_setup_to(app, &default_config_dir())
}

/// Save App options and keymap into `dir` (`ini` + `keymap`).
///
/// Known keys are upserted; unknown sections and keys in an existing `ini` are
/// left in place (mc(1) users may hand-edit Special Settings).
pub fn save_setup_to(app: &crate::app::App, dir: &Path) -> Result<()> {
    fs::create_dir_all(dir)?;
    let ini_path = dir.join("ini");
    let mut contents = if ini_path.exists() {
        fs::read_to_string(&ini_path)?
    } else {
        String::new()
    };
    contents = merge_setup_into_ini(&contents, app);
    for (term, pairs) in app.learned_keys.sections_to_write() {
        if pairs.is_empty() {
            continue;
        }
        let body = crate::learn_keys::format_terminal_section_body(&pairs);
        contents =
            crate::learn_keys::upsert_ini_section(&contents, &format!("terminal:{term}"), &body);
    }
    fs::write(&ini_path, contents)?;
    app.keymap.save_to_file(&dir.join("keymap"))?;
    Ok(())
}

fn merge_setup_into_ini(contents: &str, app: &crate::app::App) -> String {
    let mut out = contents.to_string();
    for (section, keys) in setup_kv_sections(app) {
        out = upsert_ini_keys(&out, section, &keys);
    }
    out
}

fn setup_kv_sections(app: &crate::app::App) -> Vec<(&'static str, Vec<(&'static str, String)>)> {
    vec![
        (
            "layout",
            vec![
                ("menubar_visible", app.layout.menubar_visible.to_string()),
                ("command_prompt", app.layout.command_prompt.to_string()),
                ("keybar_visible", app.layout.keybar_visible.to_string()),
                ("hintbar_visible", app.layout.hintbar_visible.to_string()),
                ("xterm_title", app.layout.xterm_title.to_string()),
                ("show_free_space", app.layout.show_free_space.to_string()),
                ("panel_ratio", app.layout.panel_ratio.to_string()),
                ("horizontal_split", app.layout.horizontal_split.to_string()),
                ("equal_split", app.layout.equal_split.to_string()),
            ],
        ),
        (
            "confirm",
            vec![
                ("delete", app.confirm.delete.to_string()),
                ("overwrite", app.confirm.overwrite.to_string()),
                ("execute", app.confirm.execute.to_string()),
                ("exit", app.confirm.exit.to_string()),
                (
                    "directory_hotlist",
                    app.confirm.directory_hotlist.to_string(),
                ),
                ("history_cleanup", app.confirm.history_cleanup.to_string()),
            ],
        ),
        (
            "panels",
            vec![
                ("show_hidden", app.panel_opts.show_hidden.to_string()),
                ("mix_all_files", app.panel_opts.mix_all_files.to_string()),
                (
                    "mark_moves_down",
                    app.panel_opts.mark_moves_down.to_string(),
                ),
                (
                    "show_mini_status",
                    app.panel_opts.show_mini_status.to_string(),
                ),
                ("kilobyte_si", app.panel_opts.kilobyte_si.to_string()),
                ("fast_reload", app.panel_opts.fast_reload.to_string()),
                (
                    "reverse_files_only",
                    app.panel_opts.reverse_files_only.to_string(),
                ),
                ("simple_swap", app.panel_opts.simple_swap.to_string()),
                (
                    "auto_save_setup",
                    app.panel_opts.auto_save_setup.to_string(),
                ),
                ("lynx_like", app.panel_opts.lynx_like.to_string()),
            ],
        ),
        (
            "appearance",
            vec![
                ("skin", app.skin_name.clone()),
                ("shadows", app.shadows.to_string()),
            ],
        ),
        (
            "vfs",
            vec![
                (
                    "always_use_ftp_proxy",
                    app.vfs_opts.always_use_ftp_proxy.to_string(),
                ),
                ("ftp_proxy_host", app.vfs_opts.ftp_proxy_host.clone()),
                ("use_netrc", app.vfs_opts.use_netrc.to_string()),
                ("ftp_anon_password", app.vfs_opts.ftp_anon_password.clone()),
                (
                    "dir_cache_timeout_secs",
                    app.vfs_opts.dir_cache_timeout_secs.to_string(),
                ),
            ],
        ),
        (
            "configuration",
            vec![
                ("verbose", app.config_opts.verbose.to_string()),
                ("compute_totals", app.config_opts.compute_totals.to_string()),
                (
                    "classic_progressbar",
                    app.config_opts.classic_progressbar.to_string(),
                ),
                (
                    "use_internal_view",
                    app.config_opts.use_internal_view.to_string(),
                ),
                (
                    "use_internal_edit",
                    app.config_opts.use_internal_edit.to_string(),
                ),
                (
                    "pause_after_run",
                    app.config_opts.pause_after_run.to_string(),
                ),
                ("shell_patterns", app.config_opts.shell_patterns.to_string()),
                ("auto_menus", app.config_opts.auto_menus.to_string()),
                ("drop_menus", app.config_opts.drop_menus.to_string()),
                ("mkdir_autoname", app.config_opts.mkdir_autoname.to_string()),
                (
                    "preallocate_space",
                    app.config_opts.preallocate_space.to_string(),
                ),
                (
                    "use_cow_file_cloning",
                    app.config_opts.use_cow_file_cloning.to_string(),
                ),
                (
                    "complete_show_all",
                    app.config_opts.complete_show_all.to_string(),
                ),
                ("safe_delete", app.config_opts.safe_delete.to_string()),
            ],
        ),
        ("left", panel_kv(&app.left)),
        ("right", panel_kv(&app.right)),
    ]
}

fn panel_kv(p: &PanelState) -> Vec<(&'static str, String)> {
    vec![
        ("listing", listing_to_ini(p.listing).to_string()),
        ("mode", mode_to_ini(p.mode).to_string()),
        ("user_format", p.user_format.clone()),
        ("brief_columns", p.brief_columns.to_string()),
        ("sort_by", panel_sort_to_ini(p.sort_by).to_string()),
        ("sort_dir", sort_dir_to_ini(p.sort_dir).to_string()),
    ]
}

fn listing_to_ini(l: ListingFormat) -> &'static str {
    match l {
        ListingFormat::Full => "full",
        ListingFormat::Brief => "brief",
        ListingFormat::Long => "long",
        ListingFormat::User => "user",
    }
}

fn parse_listing(s: &str) -> Option<ListingFormat> {
    Some(match s.trim().to_ascii_lowercase().as_str() {
        "full" => ListingFormat::Full,
        "brief" => ListingFormat::Brief,
        "long" => ListingFormat::Long,
        "user" => ListingFormat::User,
        _ => return None,
    })
}

fn mode_to_ini(m: PanelMode) -> &'static str {
    match m {
        PanelMode::Listing => "listing",
        PanelMode::QuickView => "quickview",
        PanelMode::Info => "info",
        PanelMode::Tree => "tree",
    }
}

fn parse_mode(s: &str) -> Option<PanelMode> {
    Some(match s.trim().to_ascii_lowercase().as_str() {
        "listing" => PanelMode::Listing,
        "quickview" | "quick_view" => PanelMode::QuickView,
        "info" => PanelMode::Info,
        "tree" => PanelMode::Tree,
        _ => return None,
    })
}

fn panel_sort_to_ini(s: PanelSortBy) -> &'static str {
    match s {
        PanelSortBy::Name => "name",
        PanelSortBy::Ext => "ext",
        PanelSortBy::Size => "size",
        PanelSortBy::Time => "mtime",
        PanelSortBy::Atime => "atime",
        PanelSortBy::Ctime => "ctime",
        PanelSortBy::Inode => "inode",
        PanelSortBy::Unsorted => "unsorted",
    }
}

fn parse_panel_sort(s: &str) -> Option<PanelSortBy> {
    Some(match s.trim().to_ascii_lowercase().as_str() {
        "name" => PanelSortBy::Name,
        "ext" => PanelSortBy::Ext,
        "size" => PanelSortBy::Size,
        "mtime" | "time" => PanelSortBy::Time,
        "atime" => PanelSortBy::Atime,
        "ctime" => PanelSortBy::Ctime,
        "inode" => PanelSortBy::Inode,
        "unsorted" => PanelSortBy::Unsorted,
        _ => return None,
    })
}

fn sort_dir_to_ini(d: SortDir) -> &'static str {
    match d {
        SortDir::Asc => "asc",
        SortDir::Desc => "desc",
    }
}

fn parse_sort_dir(s: &str) -> Option<SortDir> {
    Some(match s.trim().to_ascii_lowercase().as_str() {
        "asc" | "ascending" => SortDir::Asc,
        "desc" | "descending" | "reverse" => SortDir::Desc,
        _ => return None,
    })
}

/// Public INI booleans: mcr writes `true`/`false`; GNU mc.ini often uses `1`/`0`.
fn parse_ini_bool(s: &str) -> Option<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

fn join_ini_lines(lines: Vec<String>) -> String {
    let mut s = lines.join("\n");
    if !s.ends_with('\n') {
        s.push('\n');
    }
    s
}

/// Upsert `keys` in `[section]` without removing unknown keys or other sections.
fn upsert_ini_keys(contents: &str, section: &str, keys: &[(&'static str, String)]) -> String {
    if keys.is_empty() {
        return if contents.is_empty() || contents.ends_with('\n') {
            contents.to_string()
        } else {
            format!("{contents}\n")
        };
    }
    let header = format!("[{section}]");
    let header_l = header.to_ascii_lowercase();
    let lines: Vec<String> = if contents.is_empty() {
        Vec::new()
    } else {
        contents.lines().map(|s| s.to_string()).collect()
    };

    let mut start = None;
    let mut end = lines.len();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') && t.to_ascii_lowercase() == header_l {
            start = Some(i);
            end = lines.len();
            for (j, l2) in lines.iter().enumerate().skip(i + 1) {
                let t2 = l2.trim();
                if t2.starts_with('[') && t2.ends_with(']') {
                    end = j;
                    break;
                }
            }
            break;
        }
    }

    match start {
        None => {
            let mut out = lines;
            if !out.is_empty() && out.last().is_some_and(|x| !x.is_empty()) {
                out.push(String::new());
            }
            out.push(header);
            for (k, v) in keys {
                out.push(format!("{k}={v}"));
            }
            join_ini_lines(out)
        }
        Some(s) => {
            let mut present = vec![false; keys.len()];
            let mut section_lines: Vec<String> = vec![lines[s].clone()];
            for line in &lines[s + 1..end] {
                let t = line.trim();
                if t.is_empty() || t.starts_with('#') || t.starts_with(';') {
                    section_lines.push(line.clone());
                    continue;
                }
                if let Some((k, _)) = t.split_once('=') {
                    let kl = k.trim().to_ascii_lowercase();
                    if let Some(idx) = keys.iter().position(|(ck, _)| ck.eq_ignore_ascii_case(&kl))
                    {
                        section_lines.push(format!("{}={}", keys[idx].0, keys[idx].1));
                        present[idx] = true;
                        continue;
                    }
                }
                section_lines.push(line.clone());
            }
            let mut insert_at = section_lines.len();
            while insert_at > 1 && section_lines[insert_at - 1].trim().is_empty() {
                insert_at -= 1;
            }
            let missing: Vec<String> = keys
                .iter()
                .enumerate()
                .filter(|(i, _)| !present[*i])
                .map(|(_, (k, v))| format!("{k}={v}"))
                .collect();
            for (i, m) in missing.into_iter().enumerate() {
                section_lines.insert(insert_at + i, m);
            }

            let mut out: Vec<String> = Vec::new();
            out.extend(lines[..s].iter().cloned());
            out.extend(section_lines);
            if end < lines.len() {
                if out.last().is_some_and(|x| !x.is_empty()) {
                    out.push(String::new());
                }
                out.extend(lines[end..].iter().cloned());
            }
            join_ini_lines(out)
        }
    }
}

/// Persist Options → Learn keys into `[terminal:TERM]` of the user `ini`.
/// Other ini sections are preserved (mc(1) writes only redefined sequences).
pub fn save_learned_keys(app: &crate::app::App) -> Result<()> {
    save_learned_keys_to(app, &default_config_dir())
}

pub fn save_learned_keys_to(app: &crate::app::App, dir: &Path) -> Result<()> {
    fs::create_dir_all(dir)?;
    let ini_path = dir.join("ini");
    let sections = app.learned_keys.sections_to_write();
    if sections.is_empty() {
        return Ok(());
    }
    let existing = if ini_path.exists() {
        fs::read_to_string(&ini_path)?
    } else {
        String::new()
    };
    let mut contents = existing;
    for (term, pairs) in sections {
        let body = crate::learn_keys::format_terminal_section_body(&pairs);
        let section = format!("terminal:{term}");
        contents = crate::learn_keys::upsert_ini_section(&contents, &section, &body);
    }
    fs::write(&ini_path, contents)?;
    Ok(())
}

/// Compiled defaults, then the first existing system `mc.ini`
/// (`/etc/mc/mc.ini` else `$MC_DATADIR/mc.ini` / `/usr/share/mc/mc.ini`),
/// then the user dir (`~/.config/mc` unless `$MCR_CONFIG_DIR` /
/// `$MC_PROFILE_ROOT` relocates it).
pub fn load_user_setup(app: &mut crate::app::App) -> Result<()> {
    load_setup_layers(
        app,
        crate::paths::first_existing_file(&crate::paths::system_ini_candidates()).as_deref(),
        &default_config_dir(),
    )
}

/// Overlay `system_ini` (if any) then files in `user_dir`.
pub fn load_setup_layers(
    app: &mut crate::app::App,
    system_ini: Option<&Path>,
    user_dir: &Path,
) -> Result<()> {
    if let Some(p) = system_ini {
        apply_ini_path(app, p)?;
    }
    load_user_setup_from(app, user_dir)
}

/// Load setup files from `dir` if they exist.
pub fn load_user_setup_from(app: &mut crate::app::App, dir: &Path) -> Result<()> {
    let keymap_path = dir.join("keymap");
    if keymap_path.exists() {
        if let Ok(km) = KeyMap::load_from_file(&keymap_path) {
            app.keymap = km;
        }
    }
    apply_ini_path(app, &dir.join("ini"))
}

fn apply_ini_path(app: &mut crate::app::App, ini_path: &Path) -> Result<()> {
    if !ini_path.exists() {
        return Ok(());
    }
    let f = File::open(ini_path)?;
    let mut section = String::new();
    for line in BufReader::new(f).lines() {
        apply_ini_line(app, &mut section, &line?);
    }
    apply_derived_flags(app);
    Ok(())
}

fn apply_derived_flags(app: &mut crate::app::App) {
    app.show_hidden = app.panel_opts.show_hidden;
    app.left.dirs_first = !app.panel_opts.mix_all_files;
    app.right.dirs_first = !app.panel_opts.mix_all_files;
    ensure_tree_stub(&mut app.left);
    ensure_tree_stub(&mut app.right);
}

fn ensure_tree_stub(panel: &mut PanelState) {
    if !matches!(panel.mode, PanelMode::Tree) || panel.tree.is_some() {
        return;
    }
    let cwd = if panel.cwd.as_os_str().is_empty() {
        PathBuf::from("/")
    } else {
        panel.cwd.clone()
    };
    panel.tree = Some(TreeState {
        figure: DirectoryTreeState::new(
            vec![TreeEntry {
                path: cwd.clone(),
                depth: 0,
            }],
            &cwd,
        ),
        search_active: false,
    });
}

fn assign_bool(slot: &mut bool, v: &str) {
    if let Some(b) = parse_ini_bool(v) {
        *slot = b;
    }
}

fn apply_ini_line(app: &mut crate::app::App, section: &mut String, raw: &str) {
    let s = raw.trim();
    if s.is_empty() || s.starts_with('#') || s.starts_with(';') {
        return;
    }
    if s.starts_with('[') && s.ends_with(']') {
        *section = s[1..s.len() - 1].to_ascii_lowercase();
        return;
    }
    let (k, v) = match s.split_once('=') {
        Some((a, b)) => (a.trim().to_ascii_lowercase(), b.trim().to_string()),
        None => return,
    };
    match section.as_str() {
        "layout" => match k.as_str() {
            "menubar_visible" => assign_bool(&mut app.layout.menubar_visible, &v),
            "command_prompt" => assign_bool(&mut app.layout.command_prompt, &v),
            "keybar_visible" => assign_bool(&mut app.layout.keybar_visible, &v),
            "hintbar_visible" => assign_bool(&mut app.layout.hintbar_visible, &v),
            "xterm_title" => assign_bool(&mut app.layout.xterm_title, &v),
            "show_free_space" => assign_bool(&mut app.layout.show_free_space, &v),
            "panel_ratio" => {
                if let Ok(n) = v.parse::<f32>() {
                    app.layout.panel_ratio = n.clamp(0.2, 0.8);
                }
            }
            "horizontal_split" => assign_bool(&mut app.layout.horizontal_split, &v),
            "equal_split" => assign_bool(&mut app.layout.equal_split, &v),
            _ => {}
        },
        "confirm" => match k.as_str() {
            "delete" => assign_bool(&mut app.confirm.delete, &v),
            "overwrite" => assign_bool(&mut app.confirm.overwrite, &v),
            "execute" => assign_bool(&mut app.confirm.execute, &v),
            "exit" => assign_bool(&mut app.confirm.exit, &v),
            "directory_hotlist" => assign_bool(&mut app.confirm.directory_hotlist, &v),
            "history_cleanup" => assign_bool(&mut app.confirm.history_cleanup, &v),
            _ => {}
        },
        "panels" => match k.as_str() {
            "show_hidden" => assign_bool(&mut app.panel_opts.show_hidden, &v),
            "mix_all_files" => assign_bool(&mut app.panel_opts.mix_all_files, &v),
            "mark_moves_down" => assign_bool(&mut app.panel_opts.mark_moves_down, &v),
            "show_mini_status" => assign_bool(&mut app.panel_opts.show_mini_status, &v),
            "kilobyte_si" => assign_bool(&mut app.panel_opts.kilobyte_si, &v),
            "fast_reload" => assign_bool(&mut app.panel_opts.fast_reload, &v),
            "reverse_files_only" => assign_bool(&mut app.panel_opts.reverse_files_only, &v),
            "simple_swap" => assign_bool(&mut app.panel_opts.simple_swap, &v),
            "auto_save_setup" => assign_bool(&mut app.panel_opts.auto_save_setup, &v),
            "lynx_like" => assign_bool(&mut app.panel_opts.lynx_like, &v),
            _ => {}
        },
        "vfs" => match k.as_str() {
            "always_use_ftp_proxy" => assign_bool(&mut app.vfs_opts.always_use_ftp_proxy, &v),
            "ftp_proxy_host" => app.vfs_opts.ftp_proxy_host = v,
            "use_netrc" => assign_bool(&mut app.vfs_opts.use_netrc, &v),
            "ftp_anon_password" => app.vfs_opts.ftp_anon_password = v,
            "dir_cache_timeout_secs" => {
                if let Ok(n) = v.parse::<u32>() {
                    app.vfs_opts.dir_cache_timeout_secs = n;
                }
            }
            _ => {}
        },
        "appearance" => match k.as_str() {
            "skin" => app.skin_name = v,
            "shadows" => assign_bool(&mut app.shadows, &v),
            _ => {}
        },
        "configuration" => match k.as_str() {
            "verbose" => assign_bool(&mut app.config_opts.verbose, &v),
            "compute_totals" => assign_bool(&mut app.config_opts.compute_totals, &v),
            "classic_progressbar" => assign_bool(&mut app.config_opts.classic_progressbar, &v),
            "use_internal_view" => assign_bool(&mut app.config_opts.use_internal_view, &v),
            "use_internal_edit" => assign_bool(&mut app.config_opts.use_internal_edit, &v),
            "pause_after_run" => assign_bool(&mut app.config_opts.pause_after_run, &v),
            "shell_patterns" => assign_bool(&mut app.config_opts.shell_patterns, &v),
            "auto_menus" => assign_bool(&mut app.config_opts.auto_menus, &v),
            "drop_menus" => assign_bool(&mut app.config_opts.drop_menus, &v),
            "mkdir_autoname" => assign_bool(&mut app.config_opts.mkdir_autoname, &v),
            "preallocate_space" => assign_bool(&mut app.config_opts.preallocate_space, &v),
            "use_cow_file_cloning" => assign_bool(&mut app.config_opts.use_cow_file_cloning, &v),
            "complete_show_all" => assign_bool(&mut app.config_opts.complete_show_all, &v),
            "safe_delete" => assign_bool(&mut app.config_opts.safe_delete, &v),
            _ => {}
        },
        "left" => apply_panel_kv(&mut app.left, &k, &v),
        "right" => apply_panel_kv(&mut app.right, &k, &v),
        _ if section.starts_with("terminal:") => {
            let term = &section["terminal:".len()..];
            app.learned_keys.load_ini_pair(term, &k, &v);
        }
        _ => {}
    }
}

fn apply_panel_kv(panel: &mut PanelState, k: &str, v: &str) {
    match k {
        "listing" => {
            if let Some(l) = parse_listing(v) {
                panel.listing = l;
            }
        }
        "mode" => {
            if let Some(m) = parse_mode(v) {
                panel.mode = m;
            }
        }
        "user_format" => panel.user_format = v.to_string(),
        "brief_columns" => {
            if let Ok(n) = v.parse::<u8>() {
                panel.brief_columns = clamp_brief_columns(n);
            }
        }
        "sort_by" => {
            if let Some(s) = parse_panel_sort(v) {
                panel.sort_by = s;
            }
        }
        "sort_dir" => {
            if let Some(d) = parse_sort_dir(v) {
                panel.sort_dir = d;
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};

    #[test]
    fn resolve_some_default_keymap() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/mc.keymap");
        let (km, warnings) = KeyMap::load_from_file_with_warnings(&path).expect("load keymap");
        assert!(
            warnings.is_empty(),
            "data/mc.keymap must parse without warnings, got {warnings:?}"
        );
        assert!(matches!(
            km.resolve(&KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)),
            Some(Action::MoveUp)
        ));
        assert!(matches!(
            km.resolve(&KeyEvent::new(KeyCode::F(4), KeyModifiers::NONE)),
            Some(Action::FunctionKey(4))
        ));
        assert!(matches!(
            km.resolve(&KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE)),
            Some(Action::Copy)
        ));
        assert!(matches!(
            km.resolve(&KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)),
            Some(Action::Refresh)
        ));
        assert!(matches!(
            km.resolve(&KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL)),
            Some(Action::Repaint)
        ));
        assert!(matches!(
            km.resolve(&KeyEvent::new(KeyCode::Char('n'), KeyModifiers::ALT)),
            Some(Action::Sort(SortBy::Name))
        ));
        assert!(matches!(
            km.resolve(&KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)),
            Some(Action::ParentDir)
        ));
        assert!(matches!(
            km.resolve(&KeyEvent::new(KeyCode::Char('g'), KeyModifiers::ALT)),
            Some(Action::PanelJumpTop)
        ));
        assert!(matches!(
            km.resolve(&KeyEvent::new(KeyCode::Char('r'), KeyModifiers::ALT)),
            Some(Action::PanelJumpMiddle)
        ));
        assert!(matches!(
            km.resolve(&KeyEvent::new(KeyCode::Char('j'), KeyModifiers::ALT)),
            Some(Action::PanelJumpBottom)
        ));
        assert!(matches!(
            km.resolve(&KeyEvent::new(KeyCode::Char('t'), KeyModifiers::ALT)),
            Some(Action::CycleListingFormat)
        ));
        assert!(matches!(
            km.resolve(&KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)),
            Some(Action::QuickSearch)
        ));
        assert!(matches!(
            km.resolve(&KeyEvent::new(KeyCode::Char('s'), KeyModifiers::ALT)),
            Some(Action::QuickSearch)
        ));
        assert!(matches!(
            km.resolve(&KeyEvent::new(KeyCode::Insert, KeyModifiers::NONE)),
            Some(Action::ToggleSelect)
        ));
        assert!(matches!(
            km.resolve(&KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL)),
            Some(Action::ToggleSelect)
        ));
        assert!(matches!(
            km.resolve(&KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL)),
            Some(Action::MoveUp)
        ));
        assert!(matches!(
            km.resolve(&KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL)),
            Some(Action::MoveDown)
        ));
        assert!(matches!(
            km.resolve(&KeyEvent::new(KeyCode::Char('v'), KeyModifiers::ALT)),
            Some(Action::PageUp)
        ));
        assert!(matches!(
            km.resolve(&KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL)),
            Some(Action::PageDown)
        ));
        assert!(matches!(
            km.resolve(&KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)),
            Some(Action::MoveUp)
        ));
        assert!(!matches!(
            km.resolve(&KeyEvent::new(KeyCode::Char('s'), KeyModifiers::ALT)),
            Some(Action::Sort(SortBy::Size))
        ));
        assert!(matches!(
            km.resolve(&KeyEvent::new(KeyCode::Char('\\'), KeyModifiers::CONTROL)),
            Some(Action::OpenHotlist)
        ));
        assert!(matches!(
            km.resolve(&KeyEvent::new(KeyCode::Char('\\'), KeyModifiers::NONE)),
            Some(Action::UnselectGroup)
        ));
        assert!(matches!(
            km.resolve_ctrl_x(&KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE)),
            Some(Action::Chmod)
        ));
        assert!(matches!(
            km.resolve_ctrl_x(&KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE)),
            Some(Action::Chown)
        ));
        assert!(matches!(
            km.resolve_ctrl_x(&KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE)),
            Some(Action::LinkHard)
        ));
        assert!(matches!(
            km.resolve_ctrl_x(&KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE)),
            Some(Action::SymlinkAbs)
        ));
        assert!(matches!(
            km.resolve_ctrl_x(&KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE)),
            Some(Action::SymlinkRel)
        ));
    }

    #[test]
    fn default_keymap_file_parses_gnu_backslash_and_ctrl_x_chords() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/mc.keymap");
        let (km, warnings) = KeyMap::load_from_file_with_warnings(&path).expect("load keymap");
        assert_eq!(warnings, Vec::<String>::new());
        assert!(matches!(
            km.resolve(&KeyEvent::new(KeyCode::Char('\\'), KeyModifiers::CONTROL)),
            Some(Action::OpenHotlist)
        ));
        assert!(matches!(
            km.resolve(&KeyEvent::new(KeyCode::Char('\\'), KeyModifiers::NONE)),
            Some(Action::UnselectGroup)
        ));
        assert!(
            matches!(
                km.resolve(&KeyEvent::new(KeyCode::Char('?'), KeyModifiers::ALT)),
                Some(Action::FindFile)
            ),
            "Alt-question must parse as FindFile"
        );
        for (ch, want) in [
            ('c', "Chmod"),
            ('o', "Chown"),
            ('l', "LinkHard"),
            ('s', "SymlinkAbs"),
            ('v', "SymlinkRel"),
        ] {
            let a = km.resolve_ctrl_x(&KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
            match (ch, a) {
                ('c', Some(Action::Chmod)) => {}
                ('o', Some(Action::Chown)) => {}
                ('l', Some(Action::LinkHard)) => {}
                ('s', Some(Action::SymlinkAbs)) => {}
                ('v', Some(Action::SymlinkRel)) => {}
                other => panic!("C-x {ch} must bind {want}, got {other:?}"),
            }
        }
    }

    #[test]
    fn keymap_save_and_load_roundtrip_overrides_binding() {
        // Create a temporary file path
        let mut p = std::env::temp_dir();
        p.push(format!("rmc_keymap_test_{}.keymap", std::process::id()));
        // Start from defaults, override F5 to Move (instead of Copy)
        let mut km = KeyMap::mc_defaults();
        km.set_binding(
            KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE),
            Action::Move,
        );
        km.save_to_file(&p).expect("save keymap");
        // Load from file and ensure F5 resolves to Move
        let lm = KeyMap::load_from_file(&p).expect("load keymap");
        assert!(matches!(
            lm.resolve(&KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE)),
            Some(Action::Move)
        ));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn keymap_roundtrip_preserves_function_keys() {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "rmc_keymap_test_fkeys_{}.keymap",
            std::process::id()
        ));
        let km = KeyMap::mc_defaults();
        km.save_to_file(&p).expect("save keymap");
        let lm = KeyMap::load_from_file(&p).expect("load keymap");
        assert!(matches!(
            lm.resolve(&KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE)),
            Some(Action::ShowHelp)
        ));
        assert!(matches!(
            lm.resolve(&KeyEvent::new(KeyCode::F(4), KeyModifiers::NONE)),
            Some(Action::FunctionKey(4))
        ));
        assert!(matches!(
            lm.resolve(&KeyEvent::new(KeyCode::F(10), KeyModifiers::NONE)),
            Some(Action::Quit)
        ));
        assert!(matches!(
            lm.resolve(&KeyEvent::new(KeyCode::F(13), KeyModifiers::NONE)),
            Some(Action::FunctionKey(13))
        ));
        assert!(matches!(
            lm.resolve(&KeyEvent::new(KeyCode::F(3), KeyModifiers::SHIFT)),
            Some(Action::FunctionKey(13))
        ));
        assert!(matches!(
            lm.resolve(&KeyEvent::new(KeyCode::F(20), KeyModifiers::NONE)),
            Some(Action::Quit)
        ));
        assert!(matches!(
            lm.resolve(&KeyEvent::new(KeyCode::F(10), KeyModifiers::SHIFT)),
            Some(Action::Quit)
        ));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn keymap_roundtrip_preserves_f4_edit() {
        let mut p = std::env::temp_dir();
        p.push(format!("rmc_keymap_f4_{}.keymap", std::process::id()));
        let km = KeyMap::mc_defaults();
        km.save_to_file(&p).expect("save");
        let lm = KeyMap::load_from_file(&p).expect("load");
        assert!(matches!(
            lm.resolve(&KeyEvent::new(KeyCode::F(4), KeyModifiers::NONE)),
            Some(Action::FunctionKey(4))
        ));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn configuration_copy_flags_ini_roundtrip() {
        use crate::app::App;
        use rmc_fs::local::LocalFs;

        let tmp = tempfile::tempdir().unwrap();
        let vfs = LocalFs::new();
        let mut app = App::new(Box::new(vfs), KeyMap::mc_defaults()).unwrap();
        assert!(
            !app.config_opts.preallocate_space,
            "GNU default: preallocate off"
        );
        assert!(app.config_opts.use_cow_file_cloning, "GNU default: COW on");
        app.config_opts.preallocate_space = true;
        app.config_opts.use_cow_file_cloning = false;
        app.config_opts.verbose = true;
        app.config_opts.compute_totals = false;
        app.config_opts.classic_progressbar = true;
        app.config_opts.mkdir_autoname = true;
        assert!(!app.config_opts.safe_delete, "GNU default: Safe delete off");
        app.config_opts.safe_delete = true;
        save_setup_to(&app, tmp.path()).expect("save_setup");

        let vfs2 = LocalFs::new();
        let mut app2 = App::new(Box::new(vfs2), KeyMap::mc_defaults()).unwrap();
        load_user_setup_from(&mut app2, tmp.path()).expect("load_user_setup");
        assert!(
            app2.config_opts.preallocate_space,
            "preallocate_space should round-trip"
        );
        assert!(
            !app2.config_opts.use_cow_file_cloning,
            "use_cow_file_cloning should round-trip"
        );
        assert!(app2.config_opts.verbose);
        assert!(!app2.config_opts.compute_totals);
        assert!(app2.config_opts.classic_progressbar);
        assert!(app2.config_opts.mkdir_autoname);
        assert!(
            app2.config_opts.safe_delete,
            "safe_delete should round-trip"
        );
    }

    #[test]
    fn learned_keys_terminal_section_roundtrip() {
        use crate::app::App;
        use crate::learn_keys::{KeySig, LearnKey};
        use rmc_fs::local::LocalFs;

        let tmp = tempfile::tempdir().unwrap();
        let vfs = LocalFs::new();
        let mut app = App::new(Box::new(vfs), KeyMap::mc_defaults()).unwrap();
        app.learned_keys.term = "xterm-test".into();
        app.learned_keys.set_binding(
            LearnKey::F(13),
            KeySig::from_event(&crossterm::event::KeyEvent::new(
                KeyCode::F(15),
                KeyModifiers::NONE,
            )),
        );
        save_learned_keys_to(&app, tmp.path()).expect("save learned keys");
        let ini = std::fs::read_to_string(tmp.path().join("ini")).unwrap();
        assert!(
            ini.contains("[terminal:xterm-test]"),
            "GNU [terminal:TERM] section, got {ini}"
        );
        assert!(
            ini.contains("f13="),
            "redefined F13 must be saved, got {ini}"
        );

        let vfs2 = LocalFs::new();
        let mut app2 = App::new(Box::new(vfs2), KeyMap::mc_defaults()).unwrap();
        app2.learned_keys = crate::learn_keys::LearnedKeyStore::for_term("xterm-test".into());
        load_user_setup_from(&mut app2, tmp.path()).expect("load");
        let got = app2.learned_keys.remap(crossterm::event::KeyEvent::new(
            KeyCode::F(15),
            KeyModifiers::NONE,
        ));
        assert_eq!(got.code, KeyCode::F(13), "learned F15 must act as F13");
    }

    #[test]
    fn panel_modes_and_skin_ini_roundtrip() {
        use crate::app::App;
        use crate::panel::{ListingFormat, PanelMode, SortBy as PSort};
        use crate::sorting::SortDir;
        use rmc_fs::local::LocalFs;

        let tmp = tempfile::tempdir().unwrap();
        let vfs = LocalFs::new();
        let mut app = App::new(Box::new(vfs), KeyMap::mc_defaults()).unwrap();
        app.skin_name = "dark".into();
        app.shadows = false;
        app.panel_opts.auto_save_setup = true;
        app.left.listing = ListingFormat::Brief;
        app.left.brief_columns = 3;
        app.left.mode = PanelMode::QuickView;
        app.left.sort_by = PSort::Size;
        app.left.sort_dir = SortDir::Desc;
        app.right.listing = ListingFormat::User;
        app.right.user_format = "name size perm".into();
        app.right.mode = PanelMode::Info;
        save_setup_to(&app, tmp.path()).expect("save_setup");

        let ini = std::fs::read_to_string(tmp.path().join("ini")).unwrap();
        assert!(ini.contains("[left]"), "got {ini}");
        assert!(ini.contains("listing=brief"), "got {ini}");
        assert!(ini.contains("mode=quickview"), "got {ini}");
        assert!(ini.contains("[right]"), "got {ini}");
        assert!(ini.contains("listing=user"), "got {ini}");
        assert!(ini.contains("skin=dark"), "got {ini}");
        assert!(ini.contains("auto_save_setup=true"), "got {ini}");

        let vfs2 = LocalFs::new();
        let mut app2 = App::new(Box::new(vfs2), KeyMap::mc_defaults()).unwrap();
        load_user_setup_from(&mut app2, tmp.path()).expect("load");
        assert_eq!(app2.skin_name, "dark");
        assert!(!app2.shadows);
        assert!(app2.panel_opts.auto_save_setup);
        assert_eq!(app2.left.listing, ListingFormat::Brief);
        assert_eq!(app2.left.brief_columns, 3);
        assert_eq!(app2.left.mode, PanelMode::QuickView);
        assert_eq!(app2.left.sort_by, PSort::Size);
        assert_eq!(app2.left.sort_dir, SortDir::Desc);
        assert_eq!(app2.right.listing, ListingFormat::User);
        assert_eq!(app2.right.user_format, "name size perm");
        assert_eq!(app2.right.mode, PanelMode::Info);
    }

    #[test]
    fn save_setup_preserves_unknown_keys() {
        use crate::app::App;
        use rmc_fs::local::LocalFs;

        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("ini"),
            "[misc]\nfoo=bar\n\n[panels]\nshow_hidden=false\ncustom_panel_flag=1\n",
        )
        .unwrap();
        let vfs = LocalFs::new();
        let mut app = App::new(Box::new(vfs), KeyMap::mc_defaults()).unwrap();
        app.panel_opts.show_hidden = true;
        app.skin_name = "classic".into();
        save_setup_to(&app, tmp.path()).expect("save");
        let ini = std::fs::read_to_string(tmp.path().join("ini")).unwrap();
        assert!(
            ini.contains("foo=bar"),
            "unknown section must remain: {ini}"
        );
        assert!(
            ini.contains("custom_panel_flag=1"),
            "unknown key in [panels] must remain: {ini}"
        );
        assert!(
            ini.contains("show_hidden=true"),
            "known key must update: {ini}"
        );
        assert!(ini.contains("skin=classic"), "got {ini}");
    }

    #[test]
    fn system_ini_then_user_overlay() {
        use crate::app::App;
        use rmc_fs::local::LocalFs;

        let tmp = tempfile::tempdir().unwrap();
        let sys = tmp.path().join("mc.ini");
        std::fs::write(
            &sys,
            "[appearance]\nskin=sysskin\n[panels]\nshow_hidden=1\nauto_save_setup=0\n",
        )
        .unwrap();
        let user = tmp.path().join("user");
        std::fs::create_dir_all(&user).unwrap();
        std::fs::write(
            user.join("ini"),
            "[appearance]\nskin=userskin\n[panels]\nauto_save_setup=true\n",
        )
        .unwrap();

        let vfs = LocalFs::new();
        let mut app = App::new(Box::new(vfs), KeyMap::mc_defaults()).unwrap();
        load_setup_layers(&mut app, Some(&sys), &user).expect("layers");
        assert_eq!(app.skin_name, "userskin", "user ini overlays system skin");
        assert!(
            app.panel_opts.show_hidden && app.show_hidden,
            "system show_hidden=1 kept when user omits the key"
        );
        assert!(
            app.panel_opts.auto_save_setup,
            "user auto_save_setup overlays system 0"
        );
    }

    #[test]
    fn save_setup_creates_user_config_dir() {
        use crate::app::App;
        use rmc_fs::local::LocalFs;

        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".config").join("mc");
        assert!(!dir.exists());
        let vfs = LocalFs::new();
        let app = App::new(Box::new(vfs), KeyMap::mc_defaults()).unwrap();
        save_setup_to(&app, &dir).expect("save");
        assert!(dir.join("ini").is_file());
        assert!(dir.join("keymap").is_file());
    }

    #[test]
    fn load_user_setup_honors_mc_profile_root() {
        use crate::app::App;
        use rmc_fs::local::LocalFs;

        let _lock = crate::paths::lock_mc_env();
        let profile = tempfile::tempdir().unwrap();
        let user = profile.path().join(".config").join("mc");
        std::fs::create_dir_all(&user).unwrap();
        std::fs::write(
            user.join("ini"),
            "[appearance]\nskin=profile-skin\n[panels]\nauto_save_setup=1\n",
        )
        .unwrap();
        let prev_mcr = std::env::var("MCR_CONFIG_DIR").ok();
        std::env::remove_var("MCR_CONFIG_DIR");
        let _prof = EnvRestore::set("MC_PROFILE_ROOT", profile.path().to_str().unwrap());
        let _xdg = EnvRestore::set("XDG_CONFIG_HOME", "/tmp/rmc-xdg-should-not-win");
        let vfs = LocalFs::new();
        let app = App::new(Box::new(vfs), KeyMap::mc_defaults()).unwrap();
        match prev_mcr {
            Some(v) => std::env::set_var("MCR_CONFIG_DIR", v),
            None => std::env::remove_var("MCR_CONFIG_DIR"),
        }
        assert_eq!(app.skin_name, "profile-skin");
        assert!(app.panel_opts.auto_save_setup);
    }

    #[test]
    fn quit_auto_saves_to_user_ini() {
        use crate::actions::Action;
        use crate::app::App;
        use rmc_fs::local::LocalFs;

        let _lock = crate::paths::lock_mc_env();
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("cfg");
        std::fs::create_dir_all(&cfg).unwrap();
        let _dir = EnvRestore::set("MCR_CONFIG_DIR", cfg.to_str().unwrap());
        let vfs = LocalFs::new();
        let mut app = App::new(Box::new(vfs), KeyMap::mc_defaults()).unwrap();
        app.confirm.exit = false;
        app.panel_opts.auto_save_setup = true;
        app.skin_name = "dark".into();
        app.left.listing = crate::panel::ListingFormat::Long;
        app.handle_action(Action::Quit).unwrap();
        assert!(app.quit);
        let ini = std::fs::read_to_string(cfg.join("ini")).unwrap();
        assert!(ini.contains("skin=dark"), "got {ini}");
        assert!(ini.contains("auto_save_setup=true"), "got {ini}");
        assert!(ini.contains("listing=long"), "got {ini}");
    }

    #[test]
    fn upsert_ini_keys_keeps_comments_and_unknown() {
        let src = "# preamble\n[layout]\n# keep\nmenubar_visible=false\nextra=1\n\n[other]\nz=9\n";
        let out = upsert_ini_keys(
            src,
            "layout",
            &[
                ("menubar_visible", "true".into()),
                ("keybar_visible", "false".into()),
            ],
        );
        assert!(out.contains("# preamble"));
        assert!(out.contains("# keep"));
        assert!(out.contains("menubar_visible=true"));
        assert!(out.contains("extra=1"));
        assert!(out.contains("keybar_visible=false"));
        assert!(out.contains("[other]"));
        assert!(out.contains("z=9"));
    }

    struct EnvRestore {
        key: &'static str,
        prev: Option<String>,
    }

    impl EnvRestore {
        fn set(key: &'static str, val: &str) -> Self {
            let prev = std::env::var(key).ok();
            std::env::set_var(key, val);
            Self { key, prev }
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }

    #[test]
    fn load_default_honors_mc_keymap_path() {
        let _lock = crate::paths::lock_mc_env();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("alt.keymap");
        std::fs::write(&file, "F5 = Quit\n").unwrap();
        let _km = EnvRestore::set("MC_KEYMAP", file.to_str().unwrap());
        let km = KeyMap::load_default();
        assert!(
            matches!(
                km.resolve(&KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE)),
                Some(Action::Quit)
            ),
            "MC_KEYMAP path must overlay F5"
        );
    }

    #[test]
    fn load_default_mc_keymap_missing_falls_back_to_shipped() {
        let _lock = crate::paths::lock_mc_env();
        let _km = EnvRestore::set("MC_KEYMAP", "/no/such/rmc-keymap-file.keymap");
        let km = KeyMap::load_default();
        assert!(
            matches!(
                km.resolve(&KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE)),
                Some(Action::Copy)
            ),
            "missing MC_KEYMAP must keep shipped F5=Copy"
        );
    }

    #[test]
    fn load_default_honors_mc_datadir_keymap() {
        let _lock = crate::paths::lock_mc_env();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("mc.keymap"), "F5 = Quit\n").unwrap();
        let _data = EnvRestore::set("MC_DATADIR", dir.path().to_str().unwrap());
        let prev_km = std::env::var("MC_KEYMAP").ok();
        std::env::remove_var("MC_KEYMAP");
        let km = KeyMap::load_default();
        match prev_km {
            Some(v) => std::env::set_var("MC_KEYMAP", v),
            None => std::env::remove_var("MC_KEYMAP"),
        }
        assert!(
            matches!(
                km.resolve(&KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE)),
                Some(Action::Quit)
            ),
            "MC_DATADIR/mc.keymap must be used when MC_KEYMAP is unset"
        );
    }
}
