use crate::actions::{Action, SortBy};
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
        // Start from defaults and overlay file bindings (file wins)
        let mut km = Self::mc_defaults();
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
            let parse_key_then_action = || -> Option<(KeyEvent, Action)> {
                let key = parse_key(lhs)?;
                let act = parse_action(rhs)?;
                Some((key, act))
            };
            let parse_action_then_key = || -> Option<(KeyEvent, Action)> {
                let act = parse_action(lhs)?;
                let key = parse_key(rhs)?;
                Some((key, act))
            };
            if let Some((key, action)) = parse_key_then_action().or_else(parse_action_then_key) {
                km.set_binding(key, action);
            } else {
                eprintln!(
                    "Warning: could not parse keymap at line {}: {}",
                    lineno + 1,
                    raw
                );
            }
        }
        Ok(km)
    }

    pub fn mc_defaults() -> Self {
        use Action::*;
        let mut m = Self {
            bindings: Vec::new(),
        };
        m.bind(new_event(KeyCode::Up), MoveUp);
        m.bind(new_event(KeyCode::Down), MoveDown);
        m.bind(new_event(KeyCode::PageUp), PageUp);
        m.bind(new_event(KeyCode::PageDown), PageDown);
        m.bind(new_event(KeyCode::Home), Home);
        m.bind(new_event(KeyCode::End), End);
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

    pub fn resolve(&self, ev: &KeyEvent) -> Option<Action> {
        // Simple resolution by exact match
        for (k, a) in &self.bindings {
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
        Ok(())
    }

    /// Remove all bindings associated with the given action.
    pub fn remove_action_bindings(&mut self, action: &Action) {
        self.bindings.retain(|(_, a)| a != action);
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
        KeyCode::Char(ch) => out.push(ch),
        KeyCode::F(n) => out.push_str(&format!("F{n}")),
        _ => out.push_str("Unknown"),
    }
    out
}

/// Return the config directory path honoring $MCR_CONFIG_DIR or ~/.config/mcr.
pub fn default_config_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("MCR_CONFIG_DIR") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir);
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".config/mcr")
}

/// Save App options (layout/confirm/panels) and keymap to default config dir.
pub fn save_setup(app: &crate::app::App) -> Result<()> {
    let dir = default_config_dir();
    fs::create_dir_all(&dir)?;
    // Save options ini
    let ini_path = dir.join("ini");
    let mut f = File::create(&ini_path)?;
    // [layout]
    writeln!(f, "[layout]")?;
    writeln!(f, "menubar_visible={}", app.layout.menubar_visible)?;
    writeln!(f, "command_prompt={}", app.layout.command_prompt)?;
    writeln!(f, "keybar_visible={}", app.layout.keybar_visible)?;
    writeln!(f, "hintbar_visible={}", app.layout.hintbar_visible)?;
    writeln!(f, "xterm_title={}", app.layout.xterm_title)?;
    writeln!(f, "show_free_space={}", app.layout.show_free_space)?;
    writeln!(f, "panel_ratio={}", app.layout.panel_ratio)?;
    writeln!(f, "horizontal_split={}", app.layout.horizontal_split)?;
    writeln!(f, "equal_split={}", app.layout.equal_split)?;
    // [confirm]
    writeln!(f, "\n[confirm]")?;
    writeln!(f, "delete={}", app.confirm.delete)?;
    writeln!(f, "overwrite={}", app.confirm.overwrite)?;
    writeln!(f, "execute={}", app.confirm.execute)?;
    writeln!(f, "exit={}", app.confirm.exit)?;
    writeln!(f, "directory_hotlist={}", app.confirm.directory_hotlist)?;
    writeln!(f, "history_cleanup={}", app.confirm.history_cleanup)?;
    // [panels]
    writeln!(f, "\n[panels]")?;
    writeln!(f, "show_hidden={}", app.panel_opts.show_hidden)?;
    writeln!(f, "mix_all_files={}", app.panel_opts.mix_all_files)?;
    writeln!(f, "mark_moves_down={}", app.panel_opts.mark_moves_down)?;
    writeln!(f, "show_mini_status={}", app.panel_opts.show_mini_status)?;
    writeln!(f, "kilobyte_si={}", app.panel_opts.kilobyte_si)?;
    writeln!(f, "fast_reload={}", app.panel_opts.fast_reload)?;
    writeln!(
        f,
        "reverse_files_only={}",
        app.panel_opts.reverse_files_only
    )?;
    writeln!(f, "simple_swap={}", app.panel_opts.simple_swap)?;
    writeln!(f, "auto_save_setup={}", app.panel_opts.auto_save_setup)?;
    writeln!(f, "lynx_like={}", app.panel_opts.lynx_like)?;
    // [appearance]
    writeln!(f, "\n[appearance]")?;
    writeln!(f, "skin={}", app.skin_name)?;
    writeln!(f, "shadows={}", app.shadows)?;
    // [vfs]
    writeln!(f, "\n[vfs]")?;
    writeln!(
        f,
        "always_use_ftp_proxy={}",
        app.vfs_opts.always_use_ftp_proxy
    )?;
    writeln!(f, "ftp_proxy_host={}", app.vfs_opts.ftp_proxy_host)?;
    writeln!(f, "use_netrc={}", app.vfs_opts.use_netrc)?;
    writeln!(f, "ftp_anon_password={}", app.vfs_opts.ftp_anon_password)?;
    writeln!(
        f,
        "dir_cache_timeout_secs={}",
        app.vfs_opts.dir_cache_timeout_secs
    )?;
    // [configuration]
    writeln!(f, "\n[configuration]")?;
    writeln!(f, "verbose={}", app.config_opts.verbose)?;
    writeln!(f, "compute_totals={}", app.config_opts.compute_totals)?;
    writeln!(
        f,
        "classic_progressbar={}",
        app.config_opts.classic_progressbar
    )?;
    writeln!(f, "use_internal_view={}", app.config_opts.use_internal_view)?;
    writeln!(f, "use_internal_edit={}", app.config_opts.use_internal_edit)?;
    writeln!(f, "pause_after_run={}", app.config_opts.pause_after_run)?;
    writeln!(f, "shell_patterns={}", app.config_opts.shell_patterns)?;
    writeln!(f, "auto_menus={}", app.config_opts.auto_menus)?;
    writeln!(f, "drop_menus={}", app.config_opts.drop_menus)?;
    writeln!(f, "mkdir_autoname={}", app.config_opts.mkdir_autoname)?;
    writeln!(f, "complete_show_all={}", app.config_opts.complete_show_all)?;
    // Save keymap
    let keymap_path = dir.join("keymap");
    app.keymap.save_to_file(&keymap_path)?;
    Ok(())
}

/// If setup files exist, load them over defaults and apply to the App.
pub fn load_user_setup(app: &mut crate::app::App) -> Result<()> {
    let dir = default_config_dir();
    // Keymap (optional)
    let keymap_path = dir.join("keymap");
    if keymap_path.exists() {
        if let Ok(km) = KeyMap::load_from_file(&keymap_path) {
            app.keymap = km;
        }
    }
    // Options ini (optional)
    let ini_path = dir.join("ini");
    if ini_path.exists() {
        let f = File::open(&ini_path)?;
        let mut section = String::new();
        for line in BufReader::new(f).lines() {
            let raw = line?;
            let s = raw.trim();
            if s.is_empty() || s.starts_with('#') || s.starts_with(';') {
                continue;
            }
            if s.starts_with('[') && s.ends_with(']') {
                section = s[1..s.len() - 1].to_ascii_lowercase();
                continue;
            }
            let (k, v) = match s.split_once('=') {
                Some((a, b)) => (a.trim().to_ascii_lowercase(), b.trim().to_string()),
                None => continue,
            };
            let vb = |s: &str| -> bool { s.eq_ignore_ascii_case("true") };
            match section.as_str() {
                "layout" => match k.as_str() {
                    "menubar_visible" => app.layout.menubar_visible = vb(&v),
                    "command_prompt" => app.layout.command_prompt = vb(&v),
                    "keybar_visible" => app.layout.keybar_visible = vb(&v),
                    "hintbar_visible" => app.layout.hintbar_visible = vb(&v),
                    "xterm_title" => app.layout.xterm_title = vb(&v),
                    "show_free_space" => app.layout.show_free_space = vb(&v),
                    "panel_ratio" => {
                        if let Ok(n) = v.parse::<f32>() {
                            app.layout.panel_ratio = n.clamp(0.2, 0.8);
                        }
                    }
                    "horizontal_split" => app.layout.horizontal_split = vb(&v),
                    "equal_split" => app.layout.equal_split = vb(&v),
                    _ => {}
                },
                "confirm" => match k.as_str() {
                    "delete" => app.confirm.delete = vb(&v),
                    "overwrite" => app.confirm.overwrite = vb(&v),
                    "execute" => app.confirm.execute = vb(&v),
                    "exit" => app.confirm.exit = vb(&v),
                    "directory_hotlist" => app.confirm.directory_hotlist = vb(&v),
                    "history_cleanup" => app.confirm.history_cleanup = vb(&v),
                    _ => {}
                },
                "panels" => match k.as_str() {
                    "show_hidden" => app.panel_opts.show_hidden = vb(&v),
                    "mix_all_files" => app.panel_opts.mix_all_files = vb(&v),
                    "mark_moves_down" => app.panel_opts.mark_moves_down = vb(&v),
                    "show_mini_status" => app.panel_opts.show_mini_status = vb(&v),
                    "kilobyte_si" => app.panel_opts.kilobyte_si = vb(&v),
                    "fast_reload" => app.panel_opts.fast_reload = vb(&v),
                    "reverse_files_only" => app.panel_opts.reverse_files_only = vb(&v),
                    "simple_swap" => app.panel_opts.simple_swap = vb(&v),
                    "auto_save_setup" => app.panel_opts.auto_save_setup = vb(&v),
                    "lynx_like" => app.panel_opts.lynx_like = vb(&v),
                    _ => {}
                },
                "vfs" => match k.as_str() {
                    "always_use_ftp_proxy" => app.vfs_opts.always_use_ftp_proxy = vb(&v),
                    "ftp_proxy_host" => {
                        app.vfs_opts.ftp_proxy_host = v;
                    }
                    "use_netrc" => {
                        app.vfs_opts.use_netrc = vb(&v);
                    }
                    "ftp_anon_password" => {
                        app.vfs_opts.ftp_anon_password = v;
                    }
                    "dir_cache_timeout_secs" => {
                        if let Ok(n) = v.parse::<u32>() {
                            app.vfs_opts.dir_cache_timeout_secs = n;
                        }
                    }
                    _ => {}
                },
                "appearance" => match k.as_str() {
                    "skin" => app.skin_name = v,
                    "shadows" => app.shadows = vb(&v),
                    _ => {}
                },
                "configuration" => match k.as_str() {
                    "verbose" => app.config_opts.verbose = vb(&v),
                    "compute_totals" => app.config_opts.compute_totals = vb(&v),
                    "classic_progressbar" => app.config_opts.classic_progressbar = vb(&v),
                    "use_internal_view" => app.config_opts.use_internal_view = vb(&v),
                    "use_internal_edit" => app.config_opts.use_internal_edit = vb(&v),
                    "pause_after_run" => app.config_opts.pause_after_run = vb(&v),
                    "shell_patterns" => app.config_opts.shell_patterns = vb(&v),
                    "auto_menus" => app.config_opts.auto_menus = vb(&v),
                    "drop_menus" => app.config_opts.drop_menus = vb(&v),
                    "mkdir_autoname" => app.config_opts.mkdir_autoname = vb(&v),
                    "complete_show_all" => app.config_opts.complete_show_all = vb(&v),
                    _ => {}
                },
                _ => {}
            }
        }
        // Apply top-level derived flags and panel flags
        app.show_hidden = app.panel_opts.show_hidden;
        app.left.dirs_first = !app.panel_opts.mix_all_files;
        app.right.dirs_first = !app.panel_opts.mix_all_files;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};

    #[test]
    fn resolve_some_default_keymap() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/mc.keymap");
        let km = KeyMap::load_from_file(&path).expect("load keymap");
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
        assert!(!matches!(
            km.resolve(&KeyEvent::new(KeyCode::Char('s'), KeyModifiers::ALT)),
            Some(Action::Sort(SortBy::Size))
        ));
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
}
