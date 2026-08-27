//! Original Apache-2.0 `[open]` map from `data/mc.ext.ini`.
//!
//! This is not GNU Midnight Commander's GPL `mc.ext.ini`. Only the `[open]`
//! section is used here; `[extfs]` / `[extensions]` stay with the VFS helper.

use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Original Apache-2.0 `data/mc.ext.ini`, baked in so lookup works with no files on disk
/// (package-cwd `cargo test`, installed binary, empty overlay).
const SHIPPED_INI: &str = include_str!("../../../data/mc.ext.ini");

/// Action taken when Enter opens a regular file by extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenAction {
    /// Same internal/external viewer path as F3 (`ViewFile`).
    View,
    /// Desktop opener: `$MC_OPEN` if set, otherwise `xdg-open`.
    XdgOpen,
}

/// Extension → Open mapping (keys are lowercase including the leading dot).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct OpenMap {
    by_ext: HashMap<String, OpenAction>,
}

static OPEN_MAP: Lazy<OpenMap> = Lazy::new(OpenMap::load_default);

impl OpenMap {
    pub(crate) fn parse(text: &str) -> Self {
        let mut section = String::new();
        let mut by_ext = HashMap::new();
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                section = line[1..line.len() - 1].trim().to_ascii_lowercase();
                continue;
            }
            if section != "open" {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let key = k.trim();
            if key.is_empty() {
                continue;
            }
            let Some(action) = parse_open_action(v.trim()) else {
                continue;
            };
            by_ext.insert(normalize_ext(key), action);
        }
        Self { by_ext }
    }

    pub(crate) fn load_default() -> Self {
        let mut map = Self::parse(SHIPPED_INI);
        // Optional overlay: cwd first, then crate-relative. An empty `[open]`
        // section must not wipe the shipped map.
        let candidates = [
            PathBuf::from("data/mc.ext.ini"),
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/mc.ext.ini"),
        ];
        for p in candidates {
            if let Ok(s) = std::fs::read_to_string(&p) {
                map.apply_overlay(&s);
            }
        }
        map
    }

    fn apply_overlay(&mut self, text: &str) {
        let overlay = Self::parse(text);
        if overlay.by_ext.is_empty() {
            return;
        }
        self.by_ext.extend(overlay.by_ext);
    }

    pub(crate) fn lookup(&self, path: &Path) -> Option<OpenAction> {
        let ext = extension_key(path)?;
        self.by_ext.get(&ext).copied()
    }
}

/// Look up the shipped (or cwd) `[open]` map for `path`.
pub(crate) fn lookup_open(path: &Path) -> Option<OpenAction> {
    OPEN_MAP.lookup(path)
}

/// Program used for `xdg-open` / `open` mappings: `$MC_OPEN` or `xdg-open`.
pub(crate) fn desktop_open_program() -> String {
    resolve_desktop_open_program(std::env::var("MC_OPEN").ok().as_deref())
}

fn resolve_desktop_open_program(mc_open: Option<&str>) -> String {
    match mc_open {
        Some(s) if !s.trim().is_empty() => s.trim().to_string(),
        _ => "xdg-open".to_string(),
    }
}

fn parse_open_action(v: &str) -> Option<OpenAction> {
    match v.to_ascii_lowercase().as_str() {
        "view" => Some(OpenAction::View),
        "xdg-open" | "open" => Some(OpenAction::XdgOpen),
        _ => None,
    }
}

fn normalize_ext(key: &str) -> String {
    if key.starts_with('.') {
        key.to_ascii_lowercase()
    } else {
        format!(".{}", key.to_ascii_lowercase())
    }
}

fn extension_key(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_string_lossy();
    let (stem, ext) = name.rsplit_once('.')?;
    if stem.is_empty() || ext.is_empty() {
        return None;
    }
    Some(format!(".{}", ext.to_ascii_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn parse_ignores_extfs_and_unknown_actions() {
        let text = r#"
[extfs]
lsarc = extfs/ls-archive
[extensions]
.lsar = lsarc
[open]
.txt = view
.bogus = not-a-real-action
.png = xdg-open
.pdf = open
"#;
        let map = OpenMap::parse(text);
        assert_eq!(map.lookup(Path::new("readme.txt")), Some(OpenAction::View));
        assert_eq!(map.lookup(Path::new("shot.PNG")), Some(OpenAction::XdgOpen));
        assert_eq!(map.lookup(Path::new("doc.PDF")), Some(OpenAction::XdgOpen));
        assert_eq!(map.lookup(Path::new("archive.lsar")), None);
        assert_eq!(map.lookup(Path::new("file.bogus")), None);
    }

    #[test]
    fn lookup_is_case_insensitive_and_accepts_dotless_keys() {
        let map = OpenMap::parse("[open]\nTXT = View\nmd = VIEW\n");
        assert_eq!(map.lookup(Path::new("A.Txt")), Some(OpenAction::View));
        assert_eq!(map.lookup(Path::new("/tmp/n.MD")), Some(OpenAction::View));
        assert_eq!(map.lookup(Path::new("noext")), None);
        assert_eq!(map.lookup(Path::new(".txt")), None);
        assert_eq!(map.lookup(Path::new("foo.txt.bak")), None);
    }

    #[test]
    fn comments_and_empty_lines_are_skipped() {
        let map = OpenMap::parse("# comment\n; also\n\n[open]\n# inner\n.rs = view\n.c = view\n");
        assert_eq!(map.lookup(Path::new("main.rs")), Some(OpenAction::View));
        assert_eq!(map.lookup(Path::new("x.c")), Some(OpenAction::View));
    }

    #[test]
    fn last_duplicate_key_wins() {
        let map = OpenMap::parse("[open]\n.txt = xdg-open\n.txt = view\n");
        assert_eq!(map.lookup(Path::new("a.txt")), Some(OpenAction::View));
    }

    #[test]
    fn shipped_ini_open_section() {
        let map = OpenMap::parse(SHIPPED_INI);
        assert_eq!(map.lookup(Path::new("notes.txt")), Some(OpenAction::View));
        assert_eq!(map.lookup(Path::new("README.md")), Some(OpenAction::View));
        assert_eq!(map.lookup(Path::new("lib.rs")), Some(OpenAction::View));
        assert_eq!(map.lookup(Path::new("a.c")), Some(OpenAction::View));
        assert_eq!(map.lookup(Path::new("a.h")), Some(OpenAction::View));
        assert_eq!(map.lookup(Path::new("app.py")), Some(OpenAction::View));
        assert_eq!(map.lookup(Path::new("main.cpp")), Some(OpenAction::View));
        assert_eq!(map.lookup(Path::new("cfg.json")), Some(OpenAction::View));
        assert_eq!(
            map.lookup(Path::new("index.html")),
            Some(OpenAction::XdgOpen)
        );
        assert_eq!(map.lookup(Path::new("x.htm")), Some(OpenAction::XdgOpen));
        assert_eq!(map.lookup(Path::new("a.pdf")), Some(OpenAction::XdgOpen));
        assert_eq!(map.lookup(Path::new("a.png")), Some(OpenAction::XdgOpen));
        assert_eq!(map.lookup(Path::new("a.jpg")), Some(OpenAction::XdgOpen));
        assert_eq!(map.lookup(Path::new("a.jpeg")), Some(OpenAction::XdgOpen));
        assert_eq!(map.lookup(Path::new("a.gif")), Some(OpenAction::XdgOpen));
        assert_eq!(map.lookup(Path::new("icon.svg")), Some(OpenAction::XdgOpen));
        assert_eq!(map.lookup(Path::new("clip.mp4")), Some(OpenAction::XdgOpen));
        assert_eq!(map.lookup(Path::new("a.dat")), None);
        assert_eq!(map.lookup(Path::new("sample.lsar")), None);
        assert_eq!(map.lookup(Path::new("foo.py.bak")), None);
    }

    #[test]
    fn load_default_finds_repo_ini() {
        let map = OpenMap::load_default();
        assert_eq!(map.lookup(Path::new("notes.txt")), Some(OpenAction::View));
        assert_eq!(map.lookup(Path::new("a.png")), Some(OpenAction::XdgOpen));
    }

    #[test]
    fn lookup_open_uses_shipped_map_without_disk() {
        // Baked include_str; must not depend on cwd `data/mc.ext.ini`.
        assert_eq!(lookup_open(Path::new("x.rs")), Some(OpenAction::View));
        assert_eq!(lookup_open(Path::new("notes.txt")), Some(OpenAction::View));
        assert_eq!(lookup_open(Path::new("app.py")), Some(OpenAction::View));
        assert_eq!(
            lookup_open(Path::new("icon.svg")),
            Some(OpenAction::XdgOpen)
        );
        assert_eq!(lookup_open(Path::new("x.bin")), None);
        assert_eq!(lookup_open(Path::new("a.dat")), None);
    }

    #[test]
    fn empty_overlay_does_not_wipe_shipped_map() {
        let mut map = OpenMap::parse(SHIPPED_INI);
        map.apply_overlay("[extfs]\nlsarc = extfs/ls-archive\n[open]\n");
        assert_eq!(map.lookup(Path::new("notes.txt")), Some(OpenAction::View));
        assert_eq!(map.lookup(Path::new("a.png")), Some(OpenAction::XdgOpen));
        map.apply_overlay("");
        assert_eq!(map.lookup(Path::new("lib.rs")), Some(OpenAction::View));
    }

    #[test]
    fn overlay_extends_shipped_map() {
        let mut map = OpenMap::parse(SHIPPED_INI);
        map.apply_overlay("[open]\n.dat = view\n.txt = xdg-open\n");
        assert_eq!(map.lookup(Path::new("a.dat")), Some(OpenAction::View));
        assert_eq!(
            map.lookup(Path::new("notes.txt")),
            Some(OpenAction::XdgOpen)
        );
        assert_eq!(map.lookup(Path::new("lib.rs")), Some(OpenAction::View));
    }

    #[test]
    fn desktop_open_program_prefers_mc_open() {
        assert_eq!(resolve_desktop_open_program(None), "xdg-open".to_string());
        assert_eq!(
            resolve_desktop_open_program(Some("")),
            "xdg-open".to_string()
        );
        assert_eq!(
            resolve_desktop_open_program(Some("   ")),
            "xdg-open".to_string()
        );
        assert_eq!(
            resolve_desktop_open_program(Some(" /usr/bin/open-helper ")),
            "/usr/bin/open-helper".to_string()
        );
    }
}
