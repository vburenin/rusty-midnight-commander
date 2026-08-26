use crate::{DirEntry, FsError, FsResult, Metadata};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

#[derive(Debug, Clone)]
pub struct ExtfsRegistry {
    /// Map from short name -> helper command path
    helpers: HashMap<String, String>,
    /// Map from extension (lowercase, includes dot) -> short name
    ext_map: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct ExtfsPath {
    pub archive: PathBuf,   // physical file on disk
    pub inner: PathBuf,     // path inside (may be empty)
    pub helper: String,     // helper short name
    pub helper_cmd: String, // resolved helper command path
}

impl ExtfsRegistry {
    pub fn load_default() -> Self {
        // Minimal INI-like parsing: recognize two sections: [extfs] and [extensions]
        // Accept files in:
        //   data/mc.ext.ini
        //   crates/*/../../data/mc.ext.ini
        let candidates = [
            PathBuf::from("data/mc.ext.ini"),
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/mc.ext.ini"),
        ];
        for p in candidates {
            if let Ok(s) = std::fs::read_to_string(&p) {
                if let Ok((mut helpers, ext_map)) = parse_simple_ini(&s) {
                    // Resolve helper paths relative to the ini file location when not absolute
                    if let Some(base) = p.parent() {
                        for (_name, cmd) in helpers.iter_mut() {
                            let cpath = PathBuf::from(&*cmd);
                            if !cpath.is_absolute() {
                                *cmd = base.join(&cpath).to_string_lossy().into_owned();
                            }
                        }
                    }
                    return Self { helpers, ext_map };
                }
            }
        }
        Self {
            helpers: HashMap::new(),
            ext_map: HashMap::new(),
        }
    }

    pub fn match_extension(&self, path: &Path) -> Option<(String, String)> {
        let name = path.file_name()?.to_string_lossy().to_lowercase();
        for (ext, short) in &self.ext_map {
            if name.ends_with(ext) {
                if let Some(cmd) = self.helpers.get(short) {
                    return Some((short.clone(), cmd.clone()));
                }
            }
        }
        None
    }

    pub fn parse_extfs_path(&self, path: &Path) -> Option<ExtfsPath> {
        // Look for a '#' anchor
        let mut comps = Vec::<std::path::Component<'_>>::new();
        for c in path.components() {
            comps.push(c);
        }
        let mut anchor_index: Option<usize> = None;
        for (i, c) in comps.iter().enumerate() {
            let s = c.as_os_str().to_string_lossy();
            if s.ends_with('#') {
                anchor_index = Some(i);
                break;
            }
        }
        let idx = anchor_index?;
        // Build the physical path (strip '#')
        let mut archive = PathBuf::new();
        for c in &comps[..=idx] {
            let mut s = c.as_os_str().to_string_lossy().to_string();
            if s.ends_with('#') {
                s.pop();
            }
            archive.push(s);
        }
        // Determine mapping
        let (helper, helper_cmd) = self.match_extension(&archive)?;
        // Build inner path
        let mut inner = PathBuf::new();
        for c in &comps[idx + 1..] {
            inner.push(c.as_os_str());
        }
        Some(ExtfsPath {
            archive,
            inner,
            helper,
            helper_cmd,
        })
    }
}

#[allow(clippy::type_complexity)]
fn parse_simple_ini(s: &str) -> Result<(HashMap<String, String>, HashMap<String, String>), ()> {
    let mut section = String::new();
    let mut helpers: HashMap<String, String> = HashMap::new();
    let mut ext_map: HashMap<String, String> = HashMap::new();
    for raw in s.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].trim().to_ascii_lowercase();
            continue;
        }
        let (k, v) = match line.split_once('=') {
            Some((a, b)) => (a.trim(), b.trim()),
            None => continue,
        };
        match section.as_str() {
            "extfs" => {
                helpers.insert(k.to_string(), v.to_string());
            }
            "extensions" => {
                // normalize extension to include leading dot, lowercase
                let ext = if k.starts_with('.') {
                    k.to_ascii_lowercase()
                } else {
                    format!(".{}", k.to_ascii_lowercase())
                };
                ext_map.insert(ext, v.to_string());
            }
            _ => {}
        }
    }
    Ok((helpers, ext_map))
}

/// Invoke helper to list virtual directory contents.
/// Listing format (original, minimal):
///   Each line: "F <display-name> <real-absolute-path>"
pub fn list_dir(
    helper_cmd: &str,
    archive: &Path,
    inner: &Path,
    vfs_root: &Path,
    show_hidden: bool,
) -> FsResult<Vec<DirEntry>> {
    if !inner.as_os_str().is_empty() {
        // Minimal extfs example has flat listing only; no nested directories.
        // Return just a parent marker that points to archive root.
        return Ok(vec![parent_marker(vfs_root.to_path_buf())]);
    }
    let output = Command::new(helper_cmd)
        .arg("list")
        .arg(archive)
        .output()
        .map_err(|e| FsError::Message(format!("extfs list failed: {e}")))?;
    if !output.status.success() {
        return Err(FsError::Message(format!(
            "extfs helper exited with {}",
            output.status
        )));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut out: Vec<DirEntry> = Vec::new();
    // Parent marker: from archive root, '..' points to the directory containing the archive
    if let Some(parent) = archive.parent() {
        out.push(DirEntry {
            name: "..".to_string(),
            path: parent.to_path_buf(),
            meta: Metadata {
                is_dir: true,
                is_symlink: false,
                is_executable: false,
                size: 0,
                modified: SystemTime::UNIX_EPOCH,
                permissions: 0,
                owner: None,
                group: None,
            },
        });
    }
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(3, ' ');
        let kind = parts.next().unwrap_or("");
        if kind != "F" {
            continue;
        }
        let display = parts.next().unwrap_or("").to_string();
        let real = parts.next().unwrap_or("");
        if display.starts_with('.') && !show_hidden {
            continue;
        }
        let real_path = PathBuf::from(real);
        let meta = match std::fs::symlink_metadata(&real_path) {
            Ok(md) => to_meta(md),
            Err(_) => Metadata {
                is_dir: false,
                is_symlink: false,
                is_executable: false,
                size: 0,
                modified: SystemTime::UNIX_EPOCH,
                permissions: 0,
                owner: None,
                group: None,
            },
        };
        // Inside-VFS paths: vfs_root/<display>
        let vpath = vfs_root.join(&display);
        out.push(DirEntry {
            name: display,
            path: vpath,
            meta: Metadata {
                is_dir: false,
                ..meta
            },
        });
    }
    Ok(out)
}

/// Copy-out using helper extract: helper extract <archive> <inner> <dst>
pub fn copy_out(helper_cmd: &str, archive: &Path, inner: &Path, dst: &Path) -> FsResult<()> {
    let status = Command::new(helper_cmd)
        .arg("extract")
        .arg(archive)
        .arg(inner)
        .arg(dst)
        .status()
        .map_err(|e| FsError::Message(format!("extfs extract failed: {e}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(FsError::Message(format!(
            "extfs helper extract exited with {status}"
        )))
    }
}

fn parent_marker(parent: PathBuf) -> DirEntry {
    DirEntry {
        name: "..".to_string(),
        path: parent,
        meta: Metadata {
            is_dir: true,
            is_symlink: false,
            is_executable: false,
            size: 0,
            modified: SystemTime::UNIX_EPOCH,
            permissions: 0,
            owner: None,
            group: None,
        },
    }
}

fn to_meta(md: std::fs::Metadata) -> Metadata {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    #[cfg(unix)]
    let mode = md.permissions().mode();
    #[cfg(not(unix))]
    let mode = 0u32;
    Metadata {
        is_dir: md.is_dir(),
        is_symlink: md.file_type().is_symlink(),
        is_executable: !md.is_dir() && (mode & 0o111 != 0),
        size: md.len(),
        modified: md.modified().unwrap_or(SystemTime::UNIX_EPOCH),
        permissions: mode,
        owner: None,
        group: None,
    }
}
