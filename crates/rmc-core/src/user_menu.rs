use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MenuEntry {
    pub hotkey: Option<char>,
    pub title: String,
    pub command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserMenu {
    pub title: String,
    pub entries: Vec<MenuEntry>,
    /// Absolute on-disk path of the loaded menu file (for debugging/tests)
    pub source_path: PathBuf,
}

/// Load only `cwd/.mc.menu` (GNU mc Auto menus). Does not walk parents and does
/// not fall back to `~/.config/mc/menu` or the shipped `data/mc.menu`.
/// Missing, unsafe, or unreadable files return `None`.
pub fn try_load_local_menu(cwd: &Path) -> Option<UserMenu> {
    load_menu_file(&cwd.join(".mc.menu"), true)
}

/// User menu file GNU mc(1) “Edit menu file” opens: `~/.config/mc/menu`.
pub fn user_menu_file_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".config").join("mc").join("menu")
}

/// Load order similar to MC:
/// 1) ./.mc.menu in the current working directory (safe-only)
/// 2) ~/.config/mc/menu (safe-only)
/// 3) data/mc.menu shipped with the binary/repo (Apache-2.0 original)
pub fn load_menu(cwd: &Path) -> Result<UserMenu> {
    let mut candidates: Vec<(PathBuf, bool)> = Vec::new();
    candidates.push((cwd.join(".mc.menu"), true));
    if std::env::var_os("HOME").is_some() {
        candidates.push((user_menu_file_path(), true));
    }
    // Two repo-relative fallbacks like keymap loader uses
    candidates.push((PathBuf::from("data/mc.menu"), false));
    candidates.push((
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/mc.menu"),
        false,
    ));

    for (p, require_safe) in candidates {
        if let Some(m) = load_menu_file(&p, require_safe) {
            return Ok(m);
        }
    }
    Err(anyhow!("No user menu file found"))
}

fn load_menu_file(path: &Path, require_safe: bool) -> Option<UserMenu> {
    if path.exists() && (!require_safe || is_safe_file(path)) {
        parse_menu_file(path).ok()
    } else {
        None
    }
}

fn is_safe_file(path: &Path) -> bool {
    // Basic safety: file owned by current uid and not group/world-writable
    // On non-Unix platforms, consider it safe.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::symlink_metadata(path)
            .ok()
            .map(|md| md.permissions().mode() & 0o022 == 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Minimal, original menu format:
/// - Lines starting with '#' are comments; empty lines separate entries.
/// - Entry begins with: "<key>: <Title>" or "Title" (no hotkey).
/// - Following indented lines (starting with one or more spaces or a tab) form the command (joined with '\n').
///   Example:
///   a: Echo file
///   echo %f
///   Show dir
///   echo %d
pub fn parse_menu_file(path: &Path) -> Result<UserMenu> {
    let f = File::open(path)?;
    let mut rdr = BufReader::new(f).lines().peekable();
    let mut entries: Vec<MenuEntry> = Vec::new();
    while let Some(line) = rdr.next() {
        let raw = line?;
        let s = raw.trim_end();
        if s.trim().is_empty() || s.trim_start().starts_with('#') {
            continue;
        }
        // Title line
        let (hotkey, title) = if let Some((a, b)) = s.split_once(':') {
            let k = a.trim();
            let t = b.trim();
            let hk = if k.len() == 1 {
                Some(k.chars().next().unwrap())
            } else {
                None
            };
            (hk, t.to_string())
        } else {
            (None, s.trim().to_string())
        };
        // Collect indented command lines
        let mut cmd_lines: Vec<String> = Vec::new();
        while let Some(Ok(peek)) = rdr.peek() {
            let is_cmd = {
                let trimmed = peek.trim_end();
                !trimmed.is_empty() && (trimmed.starts_with(' ') || trimmed.starts_with('\t'))
            };
            if !is_cmd {
                break;
            }
            // consume
            let l = rdr.next().unwrap()?;
            cmd_lines.push(l.trim_start().to_string());
        }
        if cmd_lines.is_empty() {
            // Allow empty commands; they'll just no-op
        }
        entries.push(MenuEntry {
            hotkey,
            title,
            command: cmd_lines.join("\n"),
        });
    }
    Ok(UserMenu {
        title: "User menu".to_string(),
        entries,
        source_path: path.to_path_buf(),
    })
}

/// Expand MC-like macros:
/// - %d current directory (active panel)
/// - %f selected file (cursor)
/// - %s / %t tagged files (space-separated). If none tagged, falls back to %f.
pub fn expand_macros(app: &crate::app::App, command: &str) -> String {
    let panel = app.active_panel();
    let cwd = panel.cwd.clone();
    let cur_file = panel
        .current_entry()
        .filter(|e| !e.is_dir && !e.is_parent_marker())
        .map(|e| e.path.clone());

    let tagged: Vec<PathBuf> = panel
        .selection
        .iter()
        .filter_map(|idx| panel.entries.get(idx))
        .filter(|e| !e.is_dir && !e.is_parent_marker())
        .map(|e| e.path.clone())
        .collect();
    let list_for_st = if tagged.is_empty() {
        cur_file
            .as_ref()
            .map(|p| vec![p.clone()])
            .unwrap_or_default()
    } else {
        tagged
    };

    let quoted = |p: &Path| -> String { shell_quote_path(p) };
    let st_joined = list_for_st
        .iter()
        .map(|p| quoted(p))
        .collect::<Vec<_>>()
        .join(" ");
    let f_quoted = cur_file.as_ref().map(|p| quoted(p)).unwrap_or_default();
    let d_quoted = shell_quote_path(&cwd);

    command
        .replace("%d", &d_quoted)
        .replace("%f", &f_quoted)
        .replace("%s", &st_joined)
        .replace("%t", &st_joined)
}

fn shell_quote_path(p: &Path) -> String {
    let s = p.as_os_str().to_string_lossy();
    shell_quote(&s)
}

fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    if !s.contains([' ', '\t', '\n', '\'', '"', '\\']) {
        return s.to_string();
    }
    // Single-quote, escaping single quotes with the classic '"'"'
    let mut out = String::from("'");
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\"'\"'");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// Execute a command string using the system shell in the active panel's cwd.
/// Returns Ok(()) regardless of command exit code; errors only for spawn failures.
pub fn run_menu_command(app: &crate::app::App, cmd: &str) -> Result<()> {
    use std::process::Command;
    let cwd = app.active_panel().cwd.clone();
    let status = Command::new("sh")
        .arg("-lc")
        .arg(cmd)
        .current_dir(&cwd)
        .status();
    match status {
        Ok(_s) => Ok(()),
        Err(e) => Err(anyhow!("failed to run menu command: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::config::KeyMap;
    use rmc_fs::local::LocalFs;
    use tempfile::tempdir;

    #[test]
    fn parse_simple_menu() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("menu");
        std::fs::write(&p, "a: Echo file\n  echo %f\n\nShow dir\n  echo %d\n").unwrap();
        let m = parse_menu_file(&p).unwrap();
        assert_eq!(m.entries.len(), 2);
        assert_eq!(m.entries[0].hotkey, Some('a'));
        assert_eq!(m.entries[0].title, "Echo file");
        assert!(m.entries[0].command.contains("%f"));
        assert_eq!(m.entries[1].hotkey, None);
    }

    #[test]
    fn user_menu_file_path_is_config_mc_menu() {
        let p = user_menu_file_path();
        assert_eq!(p.file_name().and_then(|n| n.to_str()), Some("menu"));
        assert!(
            p.parent()
                .and_then(|d| d.file_name())
                .and_then(|n| n.to_str())
                == Some("mc")
        );
    }

    #[test]
    fn try_load_local_menu_only_cwd_not_parent() {
        let dir = tempdir().unwrap();
        let parent = dir.path();
        let child = parent.join("sub");
        std::fs::create_dir(&child).unwrap();
        std::fs::write(parent.join(".mc.menu"), "p: Parent only\n  echo parent\n").unwrap();
        assert!(try_load_local_menu(parent).is_some());
        assert!(
            try_load_local_menu(&child).is_none(),
            "Auto menus must not walk parent directories for .mc.menu"
        );
        std::fs::write(child.join(".mc.menu"), "c: Child menu\n  echo child\n").unwrap();
        let m = try_load_local_menu(&child).expect("child .mc.menu");
        assert_eq!(m.entries[0].title, "Child menu");
        assert!(m.source_path.ends_with(".mc.menu"));
    }

    #[test]
    fn expand_macros_basic() {
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let f = root.join("a b.txt");
        std::fs::write(&f, "x").unwrap();
        std::env::set_current_dir(&root).unwrap();
        let vfs = LocalFs::new();
        let mut app = App::new(Box::new(vfs), KeyMap::mc_defaults()).unwrap();
        // Ensure the cursor selects our file (skip parent)
        let idx = app
            .active_panel()
            .entries
            .iter()
            .position(|e| e.name == "a b.txt")
            .unwrap();
        app.active_panel_mut().cursor = idx;
        let cmd = "echo %f %d %s %t";
        let exp = expand_macros(&app, cmd);
        let fq = shell_quote_path(&f);
        assert!(exp.contains(&fq));
        assert!(exp.contains(&shell_quote_path(&root)));
    }
}
