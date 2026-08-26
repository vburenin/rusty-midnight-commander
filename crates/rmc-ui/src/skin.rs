use crate::mc_colors::McPalette;
use anyhow::{anyhow, Context, Result};
use crossterm::style::Color;
use std::fs;
use std::path::{Path, PathBuf};

/// Load the default palette from data/skins/default.ini.
/// Search order:
/// 1) MC_SKIN (path to a skin ini file)
/// 2) ./data/skins/default.ini relative to current working directory
/// 3) Workspace default relative to crate: <workspace>/data/skins/default.ini
///
/// Falls back to McPalette::default() if not found or parse fails.
pub fn load_default_palette() -> McPalette {
    if let Ok(p) = std::env::var("MC_SKIN") {
        if let Ok(pal) = load_from_file(Path::new(&p)) {
            return pal;
        }
    }
    let candidates = [
        PathBuf::from("data/skins/default.ini"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../data/skins/default.ini"),
    ];
    for p in candidates {
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
/// Sections: [core], [dialog], [menu], [buttonbar], [statusbar], [filehighlight]
/// Keys in UI sections are color pairs "fg;bg" (extra attributes after a third ';' are ignored).
/// [filehighlight] values are single colors.
pub fn parse_skin(text: &str) -> Result<McPalette> {
    #[derive(Clone, Copy)]
    enum Section {
        None,
        Core,
        Dialog,
        Menu,
        ButtonBar,
        StatusBar,
        FileHighlight,
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
                "menu" => Section::Menu,
                "buttonbar" => Section::ButtonBar,
                "statusbar" => Section::StatusBar,
                "filehighlight" => Section::FileHighlight,
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
            Section::None => {
                // ignore top-level assignments
            }
        }
    }
    Ok(pal)
}

fn assign_pair(pal: &mut McPalette, section: &str, key: &str, val: &str, lineno: usize) -> Result<()> {
    // Accept "fg;bg" and ignore a third ";..." suffix if present
    let mut parts = val.split(';').map(|s| s.trim());
    let fgs = parts.next().ok_or_else(|| anyhow!("Invalid pair at {lineno}: expected fg;bg"))?;
    let bgs = parts.next().ok_or_else(|| anyhow!("Invalid pair at {lineno}: expected fg;bg"))?;
    let fg = parse_color_name(fgs)
        .ok_or_else(|| anyhow!("Unknown color '{fgs}'"))?;
    let bg = parse_color_name(bgs)
        .ok_or_else(|| anyhow!("Unknown color '{bgs}'"))?;
    let k = key.to_ascii_lowercase();
    match section {
        "core" => match k.as_str() {
            "_default_" => { pal.core_default_fg = fg; pal.core_default_bg = bg; }
            "selected" => { pal.selected_fg = fg; pal.selected_bg = bg; }
            "marked" => { pal.marked_fg = fg; pal.marked_bg = bg; }
            "markselect" => { pal.markselect_fg = fg; pal.markselect_bg = bg; }
            "header" => { pal.header_fg = fg; pal.header_bg = bg; }
            "frame" => { pal.frame_fg = fg; pal.frame_bg = bg; }
            "shadow" => { pal.shadow_fg = fg; pal.shadow_bg = bg; }
            _ => {}
        },
        "dialog" => match k.as_str() {
            "_default_" => { pal.dialog_default_fg = fg; pal.dialog_default_bg = bg; }
            "dfocus" => { pal.dfocus_fg = fg; pal.dfocus_bg = bg; }
            "dtitle" => { pal.dtitle_fg = fg; pal.dtitle_bg = bg; }
            _ => {}
        },
        "menu" => match k.as_str() {
            "_default_" => { pal.menu_fg = fg; pal.menu_bg = bg; }
            "menusel" => { pal.menusel_fg = fg; pal.menusel_bg = bg; }
            "menuhot" => { pal.menuhot_fg = fg; pal.menuhot_bg = bg; }
            // "menuhotsel" exists in MC but unused here
            _ => {}
        },
        "buttonbar" => match k.as_str() {
            "hotkey" => { pal.buttonbar_hotkey_fg = fg; pal.buttonbar_hotkey_bg = bg; }
            "button" => { pal.buttonbar_button_fg = fg; pal.buttonbar_button_bg = bg; }
            _ => {}
        },
        "statusbar" => {
            if k.as_str() == "_default_" {
                pal.statusbar_fg = fg;
                pal.statusbar_bg = bg;
            }
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
        "darkblue" => Some(Color::DarkBlue),
        "darkcyan" => Some(Color::DarkCyan),
        "darkred" => Some(Color::DarkRed),
        "darkgreen" => Some(Color::DarkGreen),
        "darkyellow" => Some(Color::DarkYellow),
        "darkmagenta" => Some(Color::DarkMagenta),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn parse_default_skin_file() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../data/skins/default.ini");
        let pal = load_from_file(&path).expect("load default skin");
        assert_eq!(pal.core_default_bg, Color::Blue);
        assert_eq!(pal.core_default_fg, Color::Grey);
        assert_eq!(pal.selected_bg, Color::Cyan);
        assert_eq!(pal.dfocus_bg, Color::Cyan);
        assert_eq!(pal.menu_bg, Color::Cyan);
        assert_eq!(pal.menusel_bg, Color::Black);
        assert_eq!(pal.dir_color, Color::White);
        assert_eq!(pal.exec_color, Color::Green);
    }
}

