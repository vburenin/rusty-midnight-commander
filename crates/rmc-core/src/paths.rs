//! GNU mc(1) environment paths and external editor/viewer programs.
//!
//! Public manual only (`FILES`, File menu View/Edit, Redefine hotkey bindings).
//! `MC_SKIN` is handled in `rmc-ui` (already landed).

use std::path::{Path, PathBuf};

fn nonempty(val: Option<&str>) -> Option<&str> {
    val.map(str::trim).filter(|s| !s.is_empty())
}

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().and_then(|s| {
        let t = s.trim();
        if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        }
    })
}

/// GNU mc(1) `FILES`: `MC_PROFILE_ROOT` must be an absolute path.
/// Unset, empty, or relative values fall back to `HOME`, then `"."`.
pub fn profile_root_from(mc_profile_root: Option<&str>, home: Option<&str>) -> PathBuf {
    if let Some(p) = nonempty(mc_profile_root) {
        let path = PathBuf::from(p);
        if path.is_absolute() {
            return path;
        }
    }
    match nonempty(home) {
        Some(h) => PathBuf::from(h),
        None => PathBuf::from("."),
    }
}

/// Live `MC_PROFILE_ROOT` / `HOME` profile root.
pub fn profile_root() -> PathBuf {
    profile_root_from(
        env_nonempty("MC_PROFILE_ROOT").as_deref(),
        env_nonempty("HOME").as_deref(),
    )
}

/// GNU mc(1) `FILES`: `MC_DATADIR` replaces `%pkgdatadir%` when set and non-empty.
pub fn pkg_data_dir_from(mc_datadir: Option<&str>) -> Option<PathBuf> {
    nonempty(mc_datadir).map(PathBuf::from)
}

pub fn pkg_data_dir() -> Option<PathBuf> {
    pkg_data_dir_from(env_nonempty("MC_DATADIR").as_deref())
}

/// User GNU-style config directory (`…/.config/mc`).
///
/// `MC_PROFILE_ROOT` (absolute) relocates the whole profile. Otherwise
/// `XDG_CONFIG_HOME/mc` if set, else `$HOME/.config/mc`.
pub fn user_mc_config_dir_from(
    mc_profile_root: Option<&str>,
    xdg_config_home: Option<&str>,
    home: Option<&str>,
) -> PathBuf {
    if let Some(p) = nonempty(mc_profile_root) {
        let path = PathBuf::from(p);
        if path.is_absolute() {
            return path.join(".config").join("mc");
        }
    }
    if let Some(xdg) = nonempty(xdg_config_home) {
        return PathBuf::from(xdg).join("mc");
    }
    profile_root_from(None, home).join(".config").join("mc")
}

pub fn user_mc_config_dir() -> PathBuf {
    user_mc_config_dir_from(
        env_nonempty("MC_PROFILE_ROOT").as_deref(),
        env_nonempty("XDG_CONFIG_HOME").as_deref(),
        env_nonempty("HOME").as_deref(),
    )
}

/// User setup dir for `ini` / `keymap` (GNU mc(1) `~/.config/mc`).
///
/// `$MCR_CONFIG_DIR` wins (tests / local override). Otherwise the same
/// directory as [`user_mc_config_dir_from`] (`MC_PROFILE_ROOT` / `XDG_CONFIG_HOME`
/// / `HOME/.config/mc`).
pub fn default_config_dir_from(
    mcr_config_dir: Option<&str>,
    mc_profile_root: Option<&str>,
    xdg_config_home: Option<&str>,
    home: Option<&str>,
) -> PathBuf {
    if let Some(dir) = nonempty(mcr_config_dir) {
        return PathBuf::from(dir);
    }
    user_mc_config_dir_from(mc_profile_root, xdg_config_home, home)
}

pub fn default_config_dir() -> PathBuf {
    default_config_dir_from(
        env_nonempty("MCR_CONFIG_DIR").as_deref(),
        env_nonempty("MC_PROFILE_ROOT").as_deref(),
        env_nonempty("XDG_CONFIG_HOME").as_deref(),
        env_nonempty("HOME").as_deref(),
    )
}

/// GNU mc(1) `FILES`: system-wide setup files used when present.
/// `/etc/mc/mc.ini` wins over `%pkgdatadir%/mc.ini` (`$MC_DATADIR` or `/usr/share/mc`).
pub fn system_ini_candidates_from(etc_mc: &Path, share_mc: &Path) -> Vec<PathBuf> {
    vec![etc_mc.join("mc.ini"), share_mc.join("mc.ini")]
}

pub fn system_ini_candidates() -> Vec<PathBuf> {
    let share = pkg_data_dir().unwrap_or_else(|| PathBuf::from("/usr/share/mc"));
    system_ini_candidates_from(Path::new("/etc/mc"), &share)
}

/// First existing path in `candidates`, matching GNU “if etc exists, share isn’t used”.
pub fn first_existing_file(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates.iter().find(|p| p.is_file()).cloned()
}

/// Search paths for a file under `%pkgdatadir%` (`mc.keymap`, `mc.menu`, …).
///
/// Order: `$MC_DATADIR/<name>`, `data/<name>` (cwd), then `crate_fallback`
/// (typically `$CARGO_MANIFEST_DIR/../../data/<name>`).
pub fn data_file_candidates(name: &str, crate_fallback: PathBuf) -> Vec<PathBuf> {
    data_file_candidates_from(pkg_data_dir(), name, crate_fallback)
}

pub fn data_file_candidates_from(
    datadir: Option<PathBuf>,
    name: &str,
    crate_fallback: PathBuf,
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(dir) = datadir {
        out.push(dir.join(name));
    }
    out.push(PathBuf::from("data").join(name));
    out.push(crate_fallback);
    out
}

/// GNU mc(1) keymap value: absolute path (with or without `.keymap`), or a
/// name searched in `search_dirs`. First existing file wins.
pub fn resolve_keymap_spec(spec: &str, search_dirs: &[PathBuf]) -> Option<PathBuf> {
    let spec = spec.trim();
    if spec.is_empty() {
        return None;
    }
    let path = PathBuf::from(spec);
    if path.is_absolute() {
        return existing_keymap_file(&path);
    }
    let mut names = vec![spec.to_string()];
    if !spec.ends_with(".keymap") {
        names.push(format!("{spec}.keymap"));
    }
    for dir in search_dirs {
        for n in &names {
            if let Some(found) = existing_keymap_file(&dir.join(n)) {
                return Some(found);
            }
        }
    }
    None
}

fn existing_keymap_file(path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    if path.extension().is_none() {
        let with = path.with_extension("keymap");
        if with.is_file() {
            return Some(with);
        }
    }
    None
}

/// Directories searched for a named `MC_KEYMAP` (user config, `$MC_DATADIR`, shipped `data/`).
pub fn keymap_search_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![user_mc_config_dir()];
    if let Some(d) = pkg_data_dir() {
        dirs.push(d);
    }
    dirs.push(PathBuf::from("data"));
    dirs.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data"));
    dirs
}

/// GNU mc(1) Edit: `$EDITOR` if non-empty, else `vi`.
/// `VISUAL` is kept as a Unix extra already used by this tree.
pub fn external_editor_from(editor: Option<&str>, visual: Option<&str>) -> String {
    nonempty(editor)
        .or_else(|| nonempty(visual))
        .unwrap_or("vi")
        .to_string()
}

pub fn external_editor() -> String {
    external_editor_from(
        env_nonempty("EDITOR").as_deref(),
        env_nonempty("VISUAL").as_deref(),
    )
}

/// GNU mc(1) File menu View: `$VIEWER`, else `$PAGER`, else `view`.
pub fn external_viewer_from(viewer: Option<&str>, pager: Option<&str>) -> String {
    nonempty(viewer)
        .or_else(|| nonempty(pager))
        .unwrap_or("view")
        .to_string()
}

pub fn external_viewer() -> String {
    external_viewer_from(
        env_nonempty("VIEWER").as_deref(),
        env_nonempty("PAGER").as_deref(),
    )
}

#[cfg(test)]
pub(crate) fn lock_mc_env() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_root_requires_absolute_mc_profile_root() {
        assert_eq!(
            profile_root_from(Some("/abs/profile"), Some("/home/me")),
            PathBuf::from("/abs/profile")
        );
        assert_eq!(
            profile_root_from(Some("relative"), Some("/home/me")),
            PathBuf::from("/home/me"),
            "relative MC_PROFILE_ROOT is ignored per mc(1)"
        );
        assert_eq!(
            profile_root_from(Some(""), Some("/home/me")),
            PathBuf::from("/home/me")
        );
        assert_eq!(
            profile_root_from(Some("  "), Some("/home/me")),
            PathBuf::from("/home/me")
        );
        assert_eq!(profile_root_from(None, None), PathBuf::from("."));
    }

    #[test]
    fn user_mc_config_dir_profile_root_wins_over_xdg() {
        let got = user_mc_config_dir_from(Some("/abs/profile"), Some("/xdg"), Some("/home/me"));
        assert_eq!(got, PathBuf::from("/abs/profile/.config/mc"));
        let xdg = user_mc_config_dir_from(None, Some("/xdg"), Some("/home/me"));
        assert_eq!(xdg, PathBuf::from("/xdg/mc"));
        let home = user_mc_config_dir_from(None, None, Some("/home/me"));
        assert_eq!(home, PathBuf::from("/home/me/.config/mc"));
    }

    #[test]
    fn default_config_dir_mcr_override_wins() {
        let got = default_config_dir_from(
            Some("/tmp/mcr-cfg"),
            Some("/abs/profile"),
            Some("/xdg"),
            Some("/h"),
        );
        assert_eq!(got, PathBuf::from("/tmp/mcr-cfg"));
        let profile = default_config_dir_from(None, Some("/abs/profile"), Some("/xdg"), Some("/h"));
        assert_eq!(
            profile,
            PathBuf::from("/abs/profile/.config/mc"),
            "Save setup uses GNU ~/.config/mc under MC_PROFILE_ROOT"
        );
        let xdg = default_config_dir_from(None, None, Some("/xdg"), Some("/h"));
        assert_eq!(xdg, PathBuf::from("/xdg/mc"));
        let home = default_config_dir_from(None, None, None, Some("/home/me"));
        assert_eq!(home, PathBuf::from("/home/me/.config/mc"));
    }

    #[test]
    fn system_ini_etc_before_share() {
        let c = system_ini_candidates_from(Path::new("/etc/mc"), Path::new("/usr/share/mc"));
        assert_eq!(
            c,
            vec![
                PathBuf::from("/etc/mc/mc.ini"),
                PathBuf::from("/usr/share/mc/mc.ini"),
            ]
        );
        let dir = tempfile::tempdir().unwrap();
        let etc = dir.path().join("etc");
        let share = dir.path().join("share");
        std::fs::create_dir_all(&etc).unwrap();
        std::fs::create_dir_all(&share).unwrap();
        std::fs::write(share.join("mc.ini"), "share\n").unwrap();
        let only_share = system_ini_candidates_from(&etc, &share);
        assert_eq!(
            first_existing_file(&only_share).as_deref(),
            Some(share.join("mc.ini").as_path())
        );
        std::fs::write(etc.join("mc.ini"), "etc\n").unwrap();
        assert_eq!(
            first_existing_file(&only_share).as_deref(),
            Some(etc.join("mc.ini").as_path()),
            "GNU: /etc/mc/mc.ini wins over share"
        );
    }

    #[test]
    fn pkg_data_dir_skips_empty() {
        assert!(pkg_data_dir_from(None).is_none());
        assert!(pkg_data_dir_from(Some("")).is_none());
        assert_eq!(
            pkg_data_dir_from(Some("/opt/mc-data")).as_deref(),
            Some(Path::new("/opt/mc-data"))
        );
    }

    #[test]
    fn data_file_candidates_prepend_datadir() {
        let crate_fb = PathBuf::from("/crate/data/mc.menu");
        let got = data_file_candidates_from(
            Some(PathBuf::from("/opt/data")),
            "mc.menu",
            crate_fb.clone(),
        );
        assert_eq!(
            got,
            vec![
                PathBuf::from("/opt/data/mc.menu"),
                PathBuf::from("data/mc.menu"),
                crate_fb,
            ]
        );
        let no_env = data_file_candidates_from(None, "mc.keymap", PathBuf::from("/c/mc.keymap"));
        assert_eq!(no_env[0], PathBuf::from("data/mc.keymap"));
        assert!(!no_env.iter().any(|p| p.starts_with("/opt")));
    }

    #[test]
    fn resolve_keymap_absolute_with_and_without_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("custom.keymap");
        std::fs::write(&file, "F5 = Quit\n").unwrap();
        assert_eq!(
            resolve_keymap_spec(file.to_str().unwrap(), &[]).as_deref(),
            Some(file.as_path())
        );
        let no_ext = dir.path().join("custom");
        assert_eq!(
            resolve_keymap_spec(no_ext.to_str().unwrap(), &[]).as_deref(),
            Some(file.as_path())
        );
        assert!(resolve_keymap_spec("/no/such/keymap", &[]).is_none());
    }

    #[test]
    fn resolve_keymap_name_searches_dirs_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("a");
        let second = dir.path().join("b");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        std::fs::write(first.join("mine.keymap"), "F5 = Quit\n").unwrap();
        std::fs::write(second.join("mine.keymap"), "F5 = Copy\n").unwrap();
        let found = resolve_keymap_spec("mine", &[first.clone(), second.clone()]).unwrap();
        assert_eq!(found, first.join("mine.keymap"));
        let by_ext = resolve_keymap_spec("mine.keymap", std::slice::from_ref(&second)).unwrap();
        assert_eq!(by_ext, second.join("mine.keymap"));
        assert!(resolve_keymap_spec("missing", &[first, second]).is_none());
    }

    #[test]
    fn external_editor_prefers_editor_then_visual_then_vi() {
        assert_eq!(external_editor_from(Some("emacs"), Some("nano")), "emacs");
        assert_eq!(external_editor_from(Some(""), Some("nano")), "nano");
        assert_eq!(external_editor_from(None, None), "vi");
        assert_eq!(external_editor_from(Some("  "), Some("  ")), "vi");
    }

    #[test]
    fn external_viewer_prefers_viewer_then_pager_then_view() {
        assert_eq!(external_viewer_from(Some("most"), Some("less")), "most");
        assert_eq!(external_viewer_from(Some(""), Some("less")), "less");
        assert_eq!(external_viewer_from(None, None), "view");
        assert_eq!(external_viewer_from(Some("  "), None), "view");
    }
}
