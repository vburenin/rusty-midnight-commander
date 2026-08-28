use crate::mc_colors::McPalette;
use anyhow::{anyhow, Context, Result};
use crossterm::style::Color;
use std::fs;
use std::path::{Path, PathBuf};

/// Non-empty trimmed `MC_SKIN` (skin name or path). `None` if unset or blank.
/// GNU mc(1): this sits between `-S`/`--skin` and the ini `skin=` value.
pub fn mc_skin_env_name() -> Option<String> {
    match std::env::var("MC_SKIN") {
        Ok(val) => {
            let val = val.trim();
            if val.is_empty() {
                None
            } else {
                Some(val.to_string())
            }
        }
        Err(_) => None,
    }
}

/// Load the shipped default palette (`default.ini` via the skin search order).
/// Unknown / unreadable named skins fall back here — not to `MC_SKIN`.
/// `$MC_COLOR_TABLE` overlays pairs after the skin file (legacy Colors / mcview(1)).
pub fn load_default_palette() -> McPalette {
    apply_mc_color_table_env(load_default_palette_unoverlaid())
}

fn load_default_palette_unoverlaid() -> McPalette {
    if let Some(path) = find_skin_path_by_name("default") {
        if let Ok(pal) = load_from_file(&path) {
            return pal;
        }
    }
    let bundled = [
        PathBuf::from("data/skins/default.ini"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/skins/default.ini"),
    ];
    for p in bundled {
        if let Ok(pal) = load_from_file(&p) {
            return pal;
        }
    }
    McPalette::default()
}

/// Load a palette from a given skin INI file path.
pub fn load_from_file(path: &Path) -> Result<McPalette> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("Failed to read skin file: {}", path.display()))?;
    parse_skin(&text)
}

/// Parse an MC-like skin INI format:
/// Sections: [core], [dialog], [error], [menu], [buttonbar], [statusbar],
/// [filehighlight], [viewer], [editor].
/// Keys in UI sections are color pairs "fg;bg" (extra attributes after a third ';' are ignored).
/// [filehighlight] values are single colors.
pub fn parse_skin(text: &str) -> Result<McPalette> {
    #[derive(Clone, Copy)]
    enum Section {
        None,
        Core,
        Dialog,
        Error,
        Menu,
        ButtonBar,
        StatusBar,
        FileHighlight,
        Viewer,
        Editor,
    }
    let mut section = Section::None;
    let mut pal = McPalette::default();
    for (lineno, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            let sec = &line[1..line.len() - 1];
            let low = sec.to_ascii_lowercase();
            section = match low.as_str() {
                "core" => Section::Core,
                "dialog" => Section::Dialog,
                "error" => Section::Error,
                "menu" => Section::Menu,
                "buttonbar" => Section::ButtonBar,
                "statusbar" => Section::StatusBar,
                "filehighlight" => Section::FileHighlight,
                "viewer" => Section::Viewer,
                "editor" => Section::Editor,
                // Back-compat with early RMC prototype
                "pairs" => Section::Core,
                _ => Section::None,
            };
            continue;
        }
        let (k, v) = line
            .split_once('=')
            .map(|(k, v)| (k.trim(), v.trim()))
            .ok_or_else(|| anyhow!("Invalid line {}: {raw}", lineno + 1))?;
        match section {
            Section::Core => assign_pair(&mut pal, "core", k, v, lineno + 1)?,
            Section::Dialog => assign_pair(&mut pal, "dialog", k, v, lineno + 1)?,
            Section::Error => assign_pair(&mut pal, "error", k, v, lineno + 1)?,
            Section::Menu => assign_pair(&mut pal, "menu", k, v, lineno + 1)?,
            Section::ButtonBar => assign_pair(&mut pal, "buttonbar", k, v, lineno + 1)?,
            Section::StatusBar => assign_pair(&mut pal, "statusbar", k, v, lineno + 1)?,
            Section::FileHighlight => {
                let col = parse_color_name(v)
                    .ok_or_else(|| anyhow!("Unknown color '{}' on line {}", v, lineno + 1))?;
                match &k.to_ascii_lowercase()[..] {
                    "dir" | "directory" => pal.dir_color = col,
                    "exec" | "executable" => pal.exec_color = col,
                    "archive" => pal.archive_color = col,
                    "source" => pal.source_color = col,
                    "symlink" | "link" => pal.symlink_color = col,
                    _ => {}
                }
            }
            Section::Viewer => assign_pair(&mut pal, "viewer", k, v, lineno + 1)?,
            Section::Editor => assign_pair(&mut pal, "editor", k, v, lineno + 1)?,
            Section::None => {
                // ignore top-level assignments
            }
        }
    }
    Ok(pal)
}

fn assign_pair(
    pal: &mut McPalette,
    section: &str,
    key: &str,
    val: &str,
    lineno: usize,
) -> Result<()> {
    // Accept "fg;bg" and ignore a third ";..." suffix if present
    let mut parts = val.split(';').map(|s| s.trim());
    let fgs = parts
        .next()
        .ok_or_else(|| anyhow!("Invalid pair at {lineno}: expected fg;bg"))?;
    let bgs = parts
        .next()
        .ok_or_else(|| anyhow!("Invalid pair at {lineno}: expected fg;bg"))?;
    let fg = parse_color_name(fgs).ok_or_else(|| anyhow!("Unknown color '{fgs}'"))?;
    let bg = parse_color_name(bgs).ok_or_else(|| anyhow!("Unknown color '{bgs}'"))?;
    let k = key.to_ascii_lowercase();
    match section {
        "core" => match k.as_str() {
            "_default_" => {
                pal.core_default_fg = fg;
                pal.core_default_bg = bg;
            }
            "selected" => {
                pal.selected_fg = fg;
                pal.selected_bg = bg;
            }
            "marked" => {
                pal.marked_fg = fg;
                pal.marked_bg = bg;
            }
            "markselect" => {
                pal.markselect_fg = fg;
                pal.markselect_bg = bg;
            }
            "header" => {
                pal.header_fg = fg;
                pal.header_bg = bg;
            }
            "frame" => {
                pal.frame_fg = fg;
                pal.frame_bg = bg;
            }
            "shadow" => {
                pal.shadow_fg = fg;
                pal.shadow_bg = bg;
            }
            _ => {}
        },
        "dialog" => match k.as_str() {
            "_default_" => {
                pal.dialog_default_fg = fg;
                pal.dialog_default_bg = bg;
            }
            "dfocus" => {
                pal.dfocus_fg = fg;
                pal.dfocus_bg = bg;
            }
            "dtitle" => {
                pal.dtitle_fg = fg;
                pal.dtitle_bg = bg;
            }
            _ => {}
        },
        "menu" => match k.as_str() {
            "_default_" => {
                pal.menu_fg = fg;
                pal.menu_bg = bg;
            }
            "menusel" => {
                pal.menusel_fg = fg;
                pal.menusel_bg = bg;
            }
            "menuhot" => {
                pal.menuhot_fg = fg;
                pal.menuhot_bg = bg;
            }
            "menuhotsel" => {
                pal.menuhotsel_fg = fg;
                pal.menuhotsel_bg = bg;
            }
            _ => {}
        },
        "error" => match k.as_str() {
            "_default_" => {
                pal.error_default_fg = fg;
                pal.error_default_bg = bg;
            }
            "errdfocus" | "dfocus" => {
                pal.errdfocus_fg = fg;
                pal.errdfocus_bg = bg;
            }
            _ => {}
        },
        "buttonbar" => match k.as_str() {
            "hotkey" => {
                pal.buttonbar_hotkey_fg = fg;
                pal.buttonbar_hotkey_bg = bg;
            }
            "button" => {
                pal.buttonbar_button_fg = fg;
                pal.buttonbar_button_bg = bg;
            }
            _ => {}
        },
        "statusbar" if k.as_str() == "_default_" => {
            pal.statusbar_fg = fg;
            pal.statusbar_bg = bg;
        }
        "statusbar" => {}
        "viewer" => match k.as_str() {
            "_default_" => {
                pal.viewer_default_fg = fg;
                pal.viewer_default_bg = bg;
            }
            // Apache skins use `selected`; public GNU default.ini uses `viewselected`.
            "selected" | "viewselected" => {
                pal.viewer_selected_fg = fg;
                pal.viewer_selected_bg = bg;
            }
            _ => {}
        },
        "editor" => match k.as_str() {
            "_default_" | "editnormal" => {
                pal.edit_normal_fg = fg;
                pal.edit_normal_bg = bg;
            }
            "editbold" => {
                pal.edit_bold_fg = fg;
                pal.edit_bold_bg = bg;
            }
            "editmarked" => {
                pal.edit_marked_fg = fg;
                pal.edit_marked_bg = bg;
            }
            "editwhitespace" => {
                pal.edit_whitespace_fg = fg;
                pal.edit_whitespace_bg = bg;
            }
            "editlinestate" => {
                pal.edit_linestate_fg = fg;
                pal.edit_linestate_bg = bg;
            }
            _ => {}
        },
        _ => {}
    }
    Ok(())
}

fn parse_color_name(name: &str) -> Option<Color> {
    match name.to_ascii_lowercase().as_str() {
        "black" => Some(Color::Black),
        "red" => Some(Color::Red),
        "green" => Some(Color::Green),
        "yellow" => Some(Color::Yellow),
        "blue" => Some(Color::Blue),
        "magenta" => Some(Color::Magenta),
        "cyan" => Some(Color::Cyan),
        "white" => Some(Color::White),
        "lightgray" | "lightgrey" | "gray" | "grey" => Some(Color::Grey),
        "darkgray" | "darkgrey" => Some(Color::DarkGrey),
        "brightgreen" => Some(Color::Green),
        "brightmagenta" => Some(Color::Magenta),
        // Distinct from `blue` (already Color::Blue) so editwhitespace stays visible.
        "brightblue" => Some(Color::Cyan),
        "darkblue" => Some(Color::DarkBlue),
        "darkcyan" => Some(Color::DarkCyan),
        "darkred" => Some(Color::DarkRed),
        "darkgreen" => Some(Color::DarkGreen),
        "darkyellow" => Some(Color::DarkYellow),
        "darkmagenta" => Some(Color::DarkMagenta),
        _ => None,
    }
}

/// Extra color names from the public legacy Colors section (mcview(1) `MC_COLOR_TABLE`).
fn parse_color_table_name(name: &str) -> Option<Color> {
    let n = name.trim();
    if n.is_empty() {
        return None;
    }
    parse_color_name(n).or_else(|| match n.to_ascii_lowercase().as_str() {
        "brown" => Some(Color::DarkYellow),
        "brightred" => Some(Color::Red),
        "brightcyan" => Some(Color::Cyan),
        "brightwhite" => Some(Color::White),
        _ => None,
    })
}

fn apply_optional_pair(
    fg_slot: &mut Color,
    bg_slot: &mut Color,
    fg: Option<Color>,
    bg: Option<Color>,
) {
    if let Some(c) = fg {
        *fg_slot = c;
    }
    if let Some(c) = bg {
        *bg_slot = c;
    }
}

/// Overlay a legacy `MC_COLOR_TABLE` string onto `pal`.
///
/// Format (public mcview(1) / older Colors docs): `key=fg,bg:key=fg,bg:…`.
/// Empty fg or bg keeps the previous component. Unknown keys/colors are skipped.
/// Does not change editor or viewer pairs (those stay with the skin).
pub fn apply_color_table(pal: &mut McPalette, table: &str) {
    for part in table.split(':') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let Some((k, v)) = part.split_once('=') else {
            continue;
        };
        let key = k.trim().to_ascii_lowercase();
        let val = v.trim();
        let (fg_s, bg_s) = match val.split_once(',') {
            Some((a, b)) => (a.trim(), b.trim()),
            None => (val, ""),
        };
        let fg = parse_color_table_name(fg_s);
        let bg = parse_color_table_name(bg_s);
        match key.as_str() {
            "normal" => {
                apply_optional_pair(&mut pal.core_default_fg, &mut pal.core_default_bg, fg, bg)
            }
            "selected" => apply_optional_pair(&mut pal.selected_fg, &mut pal.selected_bg, fg, bg),
            "marked" => apply_optional_pair(&mut pal.marked_fg, &mut pal.marked_bg, fg, bg),
            "markselect" => {
                apply_optional_pair(&mut pal.markselect_fg, &mut pal.markselect_bg, fg, bg)
            }
            "menu" => apply_optional_pair(&mut pal.menu_fg, &mut pal.menu_bg, fg, bg),
            "menusel" => apply_optional_pair(&mut pal.menusel_fg, &mut pal.menusel_bg, fg, bg),
            "menuhot" => apply_optional_pair(&mut pal.menuhot_fg, &mut pal.menuhot_bg, fg, bg),
            "menuhotsel" => {
                apply_optional_pair(&mut pal.menuhotsel_fg, &mut pal.menuhotsel_bg, fg, bg)
            }
            "dnormal" => apply_optional_pair(
                &mut pal.dialog_default_fg,
                &mut pal.dialog_default_bg,
                fg,
                bg,
            ),
            "dfocus" => apply_optional_pair(&mut pal.dfocus_fg, &mut pal.dfocus_bg, fg, bg),
            "errors" => {
                apply_optional_pair(&mut pal.error_default_fg, &mut pal.error_default_bg, fg, bg)
            }
            "header" => apply_optional_pair(&mut pal.header_fg, &mut pal.header_bg, fg, bg),
            "directory" => {
                if let Some(c) = fg.or(bg) {
                    pal.dir_color = c;
                }
            }
            "executable" => {
                if let Some(c) = fg.or(bg) {
                    pal.exec_color = c;
                }
            }
            "link" => {
                if let Some(c) = fg.or(bg) {
                    pal.symlink_color = c;
                }
            }
            _ => {}
        }
    }
}

fn apply_mc_color_table_env(mut pal: McPalette) -> McPalette {
    if let Ok(table) = std::env::var("MC_COLOR_TABLE") {
        if !table.trim().is_empty() {
            apply_color_table(&mut pal, &table);
        }
    }
    pal
}

/// GNU mc(1) directories searched for a named skin (first match wins):
/// `$MC_PROFILE_ROOT`/`$HOME` `~/.local/share/mc/skins` and `~/.config/mc/skins`,
/// `/etc/mc/skins`, `$MC_DATADIR/skins` (replaces `%pkgdatadir%`), `/usr/share/mc/skins`,
/// then the Apache-2.0 skins shipped in `data/skins`.
pub fn skin_search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let root = rmc_core::paths::profile_root();
    if root != PathBuf::from(".") {
        dirs.push(root.join(".local/share/mc/skins"));
        dirs.push(root.join(".config/mc/skins"));
    }
    dirs.push(PathBuf::from("/etc/mc/skins"));
    if let Some(data) = rmc_core::paths::pkg_data_dir() {
        dirs.push(data.join("skins"));
    }
    dirs.push(PathBuf::from("/usr/share/mc/skins"));
    dirs.push(PathBuf::from("data/skins"));
    dirs.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/skins"));
    dirs
}

fn is_explicit_path(name: &str) -> bool {
    Path::new(name).is_absolute() || name.contains('/')
}

/// Resolve `path` as a skin file, trying with and without a `.ini` suffix.
fn existing_skin_file(path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    let ext = path.extension().and_then(|s| s.to_str());
    if ext.is_some_and(|e| e.eq_ignore_ascii_case("ini")) {
        let no_ext = path.with_extension("");
        if no_ext.is_file() {
            return Some(no_ext);
        }
    } else {
        let mut with_ini = path.to_path_buf();
        with_ini.set_extension("ini");
        if with_ini.is_file() {
            return Some(with_ini);
        }
    }
    None
}

fn scan_skin_dir(dir: &Path, out: &mut std::collections::BTreeSet<String>) {
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    for ent in rd.flatten() {
        let p = ent.path();
        if !p.is_file() {
            continue;
        }
        let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if stem.is_empty() {
            continue;
        }
        let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("");
        if ext.is_empty() || ext.eq_ignore_ascii_case("ini") {
            out.insert(stem.to_string());
        }
    }
}

/// Return available skin names (without `.ini`), always including `"default"`.
/// Names come from [`skin_search_dirs`] (user, system, then shipped files).
pub fn list_available_skins() -> Vec<String> {
    use std::collections::BTreeSet;
    let mut names: BTreeSet<String> = BTreeSet::new();
    names.insert("default".to_string());
    for dir in skin_search_dirs() {
        scan_skin_dir(&dir, &mut names);
    }
    names.into_iter().collect()
}

/// Load the palette for a skin name or path. Missing or unreadable names fall
/// back to [`load_default_palette`] — the same path Options → Appearance uses.
/// `$MC_COLOR_TABLE` is applied once after the skin (not twice on fallback).
pub fn load_palette_by_name(name: &str) -> McPalette {
    let pal = if let Some(path) = find_skin_path_by_name(name) {
        load_from_file(&path).unwrap_or_else(|_| load_default_palette_unoverlaid())
    } else {
        load_default_palette_unoverlaid()
    };
    apply_mc_color_table_env(pal)
}

/// Resolve a skin name or path. Command-line / `MC_SKIN` / ini may be an
/// absolute path (with or without `.ini`); otherwise search [`skin_search_dirs`].
pub fn find_skin_path_by_name(name: &str) -> Option<PathBuf> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    if is_explicit_path(name) {
        return existing_skin_file(Path::new(name));
    }
    for dir in skin_search_dirs() {
        if let Some(path) = existing_skin_file(&dir.join(name)) {
            return Some(path);
        }
    }
    None
}

#[cfg(test)]
pub(crate) fn lock_skin_env() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn parse_default_skin_file() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/skins/default.ini");
        let pal = load_from_file(&path).expect("load default skin");
        assert_eq!(pal.core_default_bg, Color::Blue);
        assert_eq!(pal.core_default_fg, Color::Grey);
        assert_eq!(pal.selected_fg, Color::Black);
        assert_eq!(pal.selected_bg, Color::Cyan);
        assert_eq!(pal.header_fg, Color::Yellow);
        assert_eq!(pal.header_bg, Color::Blue);
        assert_eq!(pal.dialog_default_fg, Color::Black);
        assert_eq!(pal.dialog_default_bg, Color::Grey);
        assert_eq!(pal.dfocus_fg, Color::Black);
        assert_eq!(pal.dfocus_bg, Color::Cyan);
        assert_eq!(pal.dtitle_fg, Color::Blue);
        assert_eq!(pal.dtitle_bg, Color::Grey);
        assert_eq!(pal.menu_fg, Color::White);
        assert_eq!(pal.menu_bg, Color::Cyan);
        assert_eq!(pal.menusel_fg, Color::White);
        assert_eq!(pal.menusel_bg, Color::Black);
        assert_eq!(pal.menuhot_fg, Color::Yellow);
        assert_eq!(pal.menuhot_bg, Color::Cyan);
        assert_eq!(pal.menuhotsel_fg, Color::Yellow);
        assert_eq!(pal.menuhotsel_bg, Color::Black);
        assert_eq!(pal.error_default_fg, Color::White);
        assert_eq!(pal.error_default_bg, Color::Red);
        assert_eq!(pal.errdfocus_fg, Color::Black);
        assert_eq!(pal.errdfocus_bg, Color::Grey);
        assert_eq!(pal.dir_color, Color::White);
        assert_eq!(pal.exec_color, Color::Green);
        assert_eq!(pal.viewer_default_fg, Color::Grey);
        assert_eq!(pal.viewer_default_bg, Color::Blue);
        assert_eq!(pal.viewer_selected_fg, Color::Yellow);
        assert_eq!(pal.viewer_selected_bg, Color::Cyan);
        assert_ne!(pal.viewer_selected_fg, pal.selected_fg);
        assert_eq!(pal.edit_normal_fg, Color::Grey);
        assert_eq!(pal.edit_normal_bg, Color::Blue);
        assert_eq!(pal.edit_bold_fg, Color::Yellow);
        assert_eq!(pal.edit_bold_bg, Color::Green);
        assert_eq!(pal.edit_marked_fg, Color::Black);
        assert_eq!(pal.edit_marked_bg, Color::Cyan);
        assert_eq!(pal.edit_whitespace_fg, Color::Cyan);
        assert_eq!(pal.edit_linestate_fg, Color::White);
        assert_eq!(pal.edit_linestate_bg, Color::Cyan);
        assert_ne!(pal.edit_marked_fg, pal.edit_bold_fg);
        assert_ne!(pal.edit_marked_bg, pal.marked_bg);
    }

    #[test]
    fn viewer_default_pair_is_independent_of_core() {
        let pal = parse_skin(
            "[core]\n\
             _default_ = white;red\n\
             selected = black;cyan\n\
             [viewer]\n\
             _default_ = lightgray;blue\n\
             selected = yellow;cyan\n",
        )
        .expect("parse");
        assert_eq!(pal.core_default_fg, Color::White);
        assert_eq!(pal.core_default_bg, Color::Red);
        assert_eq!(pal.viewer_default_fg, Color::Grey);
        assert_eq!(pal.viewer_default_bg, Color::Blue);
        assert_eq!(pal.viewer_selected_fg, Color::Yellow);
        assert_eq!(pal.viewer_selected_bg, Color::Cyan);
        assert_ne!(
            (pal.viewer_default_fg, pal.viewer_default_bg),
            (pal.core_default_fg, pal.core_default_bg),
            "mcview _default_ must not reuse [core] _default_"
        );
        assert_ne!(pal.viewer_selected_fg, pal.selected_fg);
    }

    #[test]
    fn viewer_viewselected_alias_maps_yellow_cyan() {
        let pal = parse_skin(
            "[viewer]\n\
             _default_ = lightgray;blue\n\
             viewselected = yellow;cyan\n",
        )
        .expect("parse");
        assert_eq!(pal.viewer_default_fg, Color::Grey);
        assert_eq!(pal.viewer_default_bg, Color::Blue);
        assert_eq!(pal.viewer_selected_fg, Color::Yellow);
        assert_eq!(pal.viewer_selected_bg, Color::Cyan);
    }

    fn section_keys(text: &str) -> Vec<(String, String)> {
        let mut section = String::new();
        let mut keys = Vec::new();
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                section = line[1..line.len() - 1].to_ascii_lowercase();
                continue;
            }
            if let Some((k, _)) = line.split_once('=') {
                keys.push((section.clone(), k.trim().to_ascii_lowercase()));
            }
        }
        keys
    }

    #[test]
    fn extra_skins_parse_and_match_default_keys() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/skins");
        let default_text = fs::read_to_string(dir.join("default.ini")).expect("default.ini");
        let default_keys = section_keys(&default_text);
        let listed = list_available_skins();
        for name in [
            "dark",
            "gray-green",
            "sand",
            "modarcon16",
            "gotar",
            "nicedark",
            "darkfar",
            "mc32",
            "xoria256",
            "classic",
            "featured",
            "sand256",
            "xoria256root",
        ] {
            assert!(
                listed.iter().any(|s| s == name),
                "Appearance list missing {name}: {listed:?}"
            );
            let path = dir.join(format!("{name}.ini"));
            let text = fs::read_to_string(&path).unwrap_or_else(|_| panic!("read {name}.ini"));
            load_from_file(&path).unwrap_or_else(|e| panic!("parse {name}.ini: {e}"));
            assert_eq!(section_keys(&text), default_keys, "{name}.ini keys");
        }
    }

    #[test]
    fn named_skin_loads_and_unknown_falls_back_to_default_loader() {
        let _lock = lock_skin_env();
        let dark = load_palette_by_name("dark");
        let default = load_default_palette();
        assert_ne!(
            dark.core_default_bg, default.core_default_bg,
            "dark.ini must not be the default palette"
        );
        assert!(find_skin_path_by_name("no-such-skin-rmc").is_none());
        let missing = load_palette_by_name("no-such-skin-rmc");
        assert_eq!(missing.core_default_bg, default.core_default_bg);
        assert_eq!(missing.core_default_fg, default.core_default_fg);
        let named_default = load_palette_by_name("default");
        assert_eq!(named_default.core_default_bg, default.core_default_bg);
        assert!(
            find_skin_path_by_name("default").is_some(),
            "shipped default.ini must resolve by name"
        );
    }

    fn lock_skin_env() -> std::sync::MutexGuard<'static, ()> {
        super::lock_skin_env()
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

    fn write_core_skin(path: &Path, pair: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(
            path,
            format!("[core]\n_default_ = {pair}\nselected = black;cyan\n"),
        )
        .unwrap();
    }

    #[test]
    fn mc_skin_env_selects_named_skin_and_missing_falls_back() {
        let _lock = lock_skin_env();
        let _skin = EnvRestore::set("MC_SKIN", "dark");
        let name = mc_skin_env_name().expect("MC_SKIN");
        assert_eq!(name, "dark");
        let dark = load_palette_by_name(&name);
        let default = load_default_palette();
        assert_ne!(
            dark.core_default_bg, default.core_default_bg,
            "MC_SKIN=dark must load the dark palette"
        );

        let _skin = EnvRestore::set("MC_SKIN", "no-such-skin-rmc");
        let missing_name = mc_skin_env_name().expect("MC_SKIN");
        assert_eq!(missing_name, "no-such-skin-rmc");
        assert!(find_skin_path_by_name(&missing_name).is_none());
        let missing = load_palette_by_name(&missing_name);
        assert_eq!(missing.core_default_bg, default.core_default_bg);
        assert_eq!(missing.core_default_fg, default.core_default_fg);
    }

    #[test]
    fn skin_search_user_dirs_first_match_wins_with_or_without_ini() {
        let _lock = lock_skin_env();
        let root = std::env::temp_dir().join(format!(
            "rmc-skin-search-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let home = root.join("home");
        let local = home.join(".local/share/mc/skins");
        let config = home.join(".config/mc/skins");
        write_core_skin(&local.join("orderskin.ini"), "red;blue");
        write_core_skin(&config.join("orderskin.ini"), "white;green");
        write_core_skin(&config.join("bareext"), "yellow;red");

        let _home = EnvRestore::set("HOME", home.to_str().unwrap());
        let listed = list_available_skins();
        assert!(
            listed.iter().any(|s| s == "orderskin"),
            "Appearance list missing orderskin: {listed:?}"
        );
        assert!(listed.iter().any(|s| s == "bareext"));
        assert!(listed.iter().any(|s| s == "default"));
        assert!(listed.iter().any(|s| s == "dark"));

        let order_path = find_skin_path_by_name("orderskin").expect("orderskin");
        assert_eq!(order_path, local.join("orderskin.ini"));
        let pal = load_from_file(&order_path).unwrap();
        assert_eq!(pal.core_default_bg, Color::Blue, "local share wins");

        let by_ext = find_skin_path_by_name("orderskin.ini").expect("orderskin.ini");
        assert_eq!(by_ext, local.join("orderskin.ini"));

        let bare = find_skin_path_by_name("bareext").expect("bareext");
        assert_eq!(bare, config.join("bareext"));
        let bare_ini = find_skin_path_by_name("bareext.ini").expect("bareext.ini");
        assert_eq!(bare_ini, config.join("bareext"));

        let abs = local.join("orderskin.ini");
        assert_eq!(
            find_skin_path_by_name(abs.to_str().unwrap()).as_deref(),
            Some(abs.as_path())
        );
        assert_eq!(
            find_skin_path_by_name(abs.with_extension("").to_str().unwrap()).as_deref(),
            Some(abs.as_path()),
            "absolute path without .ini finds the .ini file"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn mc_skin_path_loads_without_searching_dirs() {
        let _lock = lock_skin_env();
        let root = std::env::temp_dir().join(format!(
            "rmc-skin-path-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let file = root.join("custom.ini");
        write_core_skin(&file, "black;magenta");
        let _skin = EnvRestore::set("MC_SKIN", file.to_str().unwrap());
        let name = mc_skin_env_name().unwrap();
        let pal = load_palette_by_name(&name);
        assert_eq!(pal.core_default_bg, Color::Magenta);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn parse_editor_pairs() {
        let pal = parse_skin(
            "[editor]\n\
             _default_ = lightgray;blue\n\
             editbold = yellow;green\n\
             editmarked = black;cyan\n",
        )
        .expect("parse editor pairs");
        assert_eq!(pal.edit_normal_fg, Color::Grey);
        assert_eq!(pal.edit_normal_bg, Color::Blue);
        assert_eq!(pal.edit_bold_fg, Color::Yellow);
        assert_eq!(pal.edit_bold_bg, Color::Green);
        assert_eq!(pal.edit_marked_fg, Color::Black);
        assert_eq!(pal.edit_marked_bg, Color::Cyan);
        assert_ne!(
            pal.edit_marked_bg, pal.marked_bg,
            "editmarked is not panel marked yellow;blue"
        );
        assert_ne!(
            (pal.edit_bold_fg, pal.edit_bold_bg),
            (pal.edit_normal_fg, pal.edit_normal_bg)
        );

        let alias = parse_skin("[editor]\neditnormal = white;red\n").expect("editnormal alias");
        assert_eq!(alias.edit_normal_fg, Color::White);
        assert_eq!(alias.edit_normal_bg, Color::Red);
    }

    #[test]
    fn parse_error_and_menuhotsel_pairs() {
        let pal = parse_skin(
            "[menu]\n\
             menuhotsel = yellow;black\n\
             [error]\n\
             _default_ = white;red\n\
             errdfocus = black;lightgray\n",
        )
        .expect("parse error/menu pairs");
        assert_eq!(pal.menuhotsel_fg, Color::Yellow);
        assert_eq!(pal.menuhotsel_bg, Color::Black);
        assert_eq!(pal.error_default_fg, Color::White);
        assert_eq!(pal.error_default_bg, Color::Red);
        assert_eq!(pal.errdfocus_fg, Color::Black);
        assert_eq!(pal.errdfocus_bg, Color::Grey);
    }

    #[test]
    fn color_table_overlays_pairs_and_skips_editor_viewer() {
        let mut pal = McPalette::default();
        let viewer_fg = pal.viewer_selected_fg;
        let viewer_bg = pal.viewer_selected_bg;
        let edit_fg = pal.edit_normal_fg;
        let edit_bg = pal.edit_normal_bg;
        apply_color_table(
            &mut pal,
            "normal=lightgray,black:selected=black,green:directory=red,blue:marked=,magenta:bogus=nope,red:viewunderline=yellow,red",
        );
        assert_eq!(pal.core_default_fg, Color::Grey);
        assert_eq!(pal.core_default_bg, Color::Black);
        assert_eq!(pal.selected_fg, Color::Black);
        assert_eq!(pal.selected_bg, Color::Green);
        assert_eq!(pal.dir_color, Color::Red);
        assert_eq!(pal.marked_bg, Color::Magenta, "empty fg keeps previous");
        assert_eq!(pal.marked_fg, Color::Yellow);
        assert_eq!(
            pal.viewer_selected_fg, viewer_fg,
            "must not restaff viewer colors"
        );
        assert_eq!(pal.viewer_selected_bg, viewer_bg);
        assert_eq!(
            pal.edit_normal_fg, edit_fg,
            "must not restaff editor colors"
        );
        assert_eq!(pal.edit_normal_bg, edit_bg);
    }

    #[test]
    fn mc_color_table_env_overlays_loaded_palette() {
        let _lock = lock_skin_env();
        let _tbl = EnvRestore::set("MC_COLOR_TABLE", "normal=white,red:selected=black,green");
        let pal = load_default_palette();
        assert_eq!(pal.core_default_fg, Color::White);
        assert_eq!(pal.core_default_bg, Color::Red);
        assert_eq!(pal.selected_bg, Color::Green);
        let dark = load_palette_by_name("dark");
        assert_eq!(
            dark.core_default_bg,
            Color::Red,
            "table overlays the named skin once"
        );
    }

    #[test]
    fn mc_datadir_and_profile_root_are_skin_search_dirs() {
        let _lock = lock_skin_env();
        let root = std::env::temp_dir().join(format!(
            "rmc-env-skin-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let profile = root.join("profile");
        let datadir = root.join("datadir");
        write_core_skin(
            &profile.join(".local/share/mc/skins/proskin.ini"),
            "white;red",
        );
        write_core_skin(&datadir.join("skins/dataskin.ini"), "black;yellow");
        let _prof = EnvRestore::set("MC_PROFILE_ROOT", profile.to_str().unwrap());
        let _data = EnvRestore::set("MC_DATADIR", datadir.to_str().unwrap());
        let listed = list_available_skins();
        assert!(
            listed.iter().any(|s| s == "proskin"),
            "MC_PROFILE_ROOT skins missing: {listed:?}"
        );
        assert!(
            listed.iter().any(|s| s == "dataskin"),
            "MC_DATADIR skins missing: {listed:?}"
        );
        let pal = load_from_file(&find_skin_path_by_name("dataskin").unwrap()).unwrap();
        assert_eq!(pal.core_default_bg, Color::Yellow);
        let _ = fs::remove_dir_all(&root);
    }
}
