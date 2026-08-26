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

/// Parse a minimal INI with two sections:
/// [pairs] keys with "fg;bg"
/// [filehighlight] keys with single color for file types
pub fn parse_skin(text: &str) -> Result<McPalette> {
    #[derive(Clone, Copy)]
    enum Section {
        None,
        Pairs,
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
            section = match &line[1..line.len() - 1] {
                "pairs" | "Pairs" => Section::Pairs,
                "filehighlight" | "FileHighlight" => Section::FileHighlight,
                _ => Section::None,
            };
            continue;
        }
        let (k, v) = line
            .split_once('=')
            .map(|(k, v)| (k.trim(), v.trim()))
            .ok_or_else(|| anyhow!("Invalid line {}: {raw}", lineno + 1))?;
        match section {
            Section::Pairs => {
                let (fgs, bgs) = v
                    .split_once(';')
                    .map(|(a, b)| (a.trim(), b.trim()))
                    .ok_or_else(|| anyhow!("Invalid pair at {}: expected fg;bg", lineno + 1))?;
                let fg = parse_color_name(fgs)
                    .ok_or_else(|| anyhow!("Unknown color '{}' on line {}", fgs, lineno + 1))?;
                let bg = parse_color_name(bgs)
                    .ok_or_else(|| anyhow!("Unknown color '{}' on line {}", bgs, lineno + 1))?;
                match k {
                    "core_default" => { pal.core_default_fg = fg; pal.core_default_bg = bg; }
                    "selected" => { pal.selected_fg = fg; pal.selected_bg = bg; }
                    "marked" => { pal.marked_fg = fg; pal.marked_bg = bg; }
                    "markselect" => { pal.markselect_fg = fg; pal.markselect_bg = bg; }
                    "header" => { pal.header_fg = fg; pal.header_bg = bg; }
                    "frame" => { pal.frame_fg = fg; pal.frame_bg = bg; }
                    "shadow" => { pal.shadow_fg = fg; pal.shadow_bg = bg; }
                    "dialog_default" => { pal.dialog_default_fg = fg; pal.dialog_default_bg = bg; }
                    "dfocus" => { pal.dfocus_fg = fg; pal.dfocus_bg = bg; }
                    "dtitle" => { pal.dtitle_fg = fg; pal.dtitle_bg = bg; }
                    "menu" => { pal.menu_fg = fg; pal.menu_bg = bg; }
                    "menusel" => { pal.menusel_fg = fg; pal.menusel_bg = bg; }
                    "menuhot" => { pal.menuhot_fg = fg; pal.menuhot_bg = bg; }
                    "buttonbar_hotkey" => { pal.buttonbar_hotkey_fg = fg; pal.buttonbar_hotkey_bg = bg; }
                    "buttonbar_button" => { pal.buttonbar_button_fg = fg; pal.buttonbar_button_bg = bg; }
                    "statusbar" => { pal.statusbar_fg = fg; pal.statusbar_bg = bg; }
                    _ => {
                        // Ignore unknown key
                    }
                }
            }
            Section::FileHighlight => {
                let col = parse_color_name(v)
                    .ok_or_else(|| anyhow!("Unknown color '{}' on line {}", v, lineno + 1))?;
                match k {
                    "dir" => pal.dir_color = col,
                    "exec" => pal.exec_color = col,
                    "archive" => pal.archive_color = col,
                    "source" => pal.source_color = col,
                    "symlink" => pal.symlink_color = col,
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

