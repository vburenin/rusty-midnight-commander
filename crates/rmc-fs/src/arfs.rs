use crate::{DirEntry, FsError, FsResult, Metadata};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn norm<P: AsRef<Path>>(p: P) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.as_ref().components() {
        match c {
            std::path::Component::CurDir => {}
            _ => out.push(c.as_os_str()),
        }
    }
    out
}

fn header_mtime(mt: i64) -> SystemTime {
    if mt <= 0 {
        UNIX_EPOCH
    } else {
        UNIX_EPOCH + Duration::from_secs(mt as u64)
    }
}

fn header_mode(mode: u32, is_dir: bool) -> u32 {
    if mode == 0 {
        if is_dir {
            0o755
        } else {
            0o644
        }
    } else {
        mode & 0o7777
    }
}

fn read_entries(archive_path: &Path) -> FsResult<Vec<(PathBuf, u64, SystemTime, u32)>> {
    let f = File::open(archive_path)?;
    let mut ar = ar::Archive::new(f);
    let mut out = Vec::new();
    while let Some(entry_res) = ar.next_entry() {
        let mut entry = entry_res.map_err(|e| FsError::Message(format!("ar entry: {e}")))?;
        let ident = entry.header().identifier();
        let mut name = String::from_utf8_lossy(ident).to_string();
        name = name.trim_matches(char::from(0)).trim().to_string();
        let p = norm(Path::new(&name));
        let size = entry.header().size();
        let mtime = header_mtime(entry.header().mtime() as i64);
        let mode = entry.header().mode();
        // consume payload
        std::io::copy(&mut entry, &mut std::io::sink()).ok();
        out.push((p, size, mtime, mode));
    }
    Ok(out)
}

pub fn list_dir(
    archive_path: &Path,
    vfs_root: &Path,
    inner: &Path,
    show_hidden: bool,
) -> FsResult<Vec<DirEntry>> {
    let entries = read_entries(archive_path)?;
    let inner_norm = norm(inner);
    let mut seen_dirs: HashSet<String> = HashSet::new();
    let mut items: HashMap<String, DirEntry> = HashMap::new();
    for (p, size, mtime, mode) in entries {
        if !p.starts_with(&inner_norm) {
            continue;
        }
        let rel = p.strip_prefix(&inner_norm).unwrap_or(&p);
        if rel.as_os_str().is_empty() {
            continue;
        }
        let mut comps = rel.components();
        if let Some(first) = comps.next() {
            let name = first.as_os_str().to_string_lossy().to_string();
            if !show_hidden && name.starts_with('.') {
                continue;
            }
            let is_dir = comps.next().is_some();
            if is_dir {
                if seen_dirs.insert(name.clone()) {
                    let p = vfs_root.join(&name);
                    items.insert(
                        name.clone(),
                        DirEntry {
                            name,
                            path: p,
                            meta: Metadata {
                                is_dir: true,
                                is_symlink: false,
                                symlink_target: None,
                                is_executable: false,
                                size: 0,
                                modified: UNIX_EPOCH,
                                permissions: 0o755,
                                owner: None,
                                group: None,
                                nlink: 1,
                                accessed: UNIX_EPOCH,
                                changed: UNIX_EPOCH,
                                inode: 0,
                            },
                        },
                    );
                }
            } else if !items.contains_key(&name) {
                let p = vfs_root.join(&name);
                items.insert(
                    name.clone(),
                    DirEntry {
                        name,
                        path: p,
                        meta: Metadata {
                            is_dir: false,
                            is_symlink: false,
                            symlink_target: None,
                            is_executable: (header_mode(mode, false) & 0o111) != 0,
                            size,
                            modified: mtime,
                            permissions: header_mode(mode, false),
                            owner: None,
                            group: None,
                            nlink: 1,
                            accessed: mtime,
                            changed: mtime,
                            inode: 0,
                        },
                    },
                );
            }
        }
    }
    let mut out: Vec<DirEntry> = Vec::new();
    if let Some(p) = vfs_root.parent() {
        out.push(DirEntry {
            name: "..".to_string(),
            path: p.to_path_buf(),
            meta: Metadata {
                is_dir: true,
                is_symlink: false,
                symlink_target: None,
                is_executable: false,
                size: 0,
                modified: UNIX_EPOCH,
                permissions: 0,
                owner: None,
                group: None,
                nlink: 1,
                accessed: UNIX_EPOCH,
                changed: UNIX_EPOCH,
                inode: 0,
            },
        });
    }
    let mut vals: Vec<DirEntry> = items.into_values().collect();
    vals.sort_by(|a, b| match (a.meta.is_dir, b.meta.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });
    out.extend(vals);
    Ok(out)
}

pub fn read_file(archive_path: &Path, inner_full: &Path) -> FsResult<Box<dyn Read + Send>> {
    let f = File::open(archive_path)?;
    let mut ar = ar::Archive::new(f);
    let target = norm(inner_full);
    while let Some(entry_res) = ar.next_entry() {
        let mut entry = entry_res.map_err(|e| FsError::Message(format!("ar entry: {e}")))?;
        let p = {
            let ident = String::from_utf8_lossy(entry.header().identifier()).to_string();
            norm(Path::new(ident.trim_matches(char::from(0)).trim()))
        };
        if p == target {
            let mut buf = Vec::new();
            std::io::copy(&mut entry, &mut buf)
                .map_err(|e| FsError::Message(format!("ar read: {e}")))?;
            return Ok(Box::new(Cursor::new(buf)));
        }
    }
    Err(FsError::Message(format!(
        "File not found in ar: {}",
        inner_full.display()
    )))
}

pub fn stat(archive_path: &Path, inner_full: &Path) -> FsResult<Metadata> {
    if inner_full.as_os_str().is_empty() {
        return Ok(Metadata {
            is_dir: true,
            is_symlink: false,
            symlink_target: None,
            is_executable: false,
            size: 0,
            modified: UNIX_EPOCH,
            permissions: 0o755,
            owner: None,
            group: None,
            nlink: 1,
            accessed: UNIX_EPOCH,
            changed: UNIX_EPOCH,
            inode: 0,
        });
    }
    let entries = read_entries(archive_path)?;
    let target = norm(inner_full);
    let mut dir_marker = false;
    for (p, size, mtime, mode) in entries {
        if p == target {
            let is_dir = false;
            return Ok(Metadata {
                is_dir,
                is_symlink: false,
                symlink_target: None,
                is_executable: (!is_dir) && (header_mode(mode, false) & 0o111 != 0),
                size,
                modified: mtime,
                permissions: header_mode(mode, is_dir),
                owner: None,
                group: None,
                nlink: 1,
                accessed: mtime,
                changed: mtime,
                inode: 0,
            });
        }
        if p.starts_with(&target) {
            dir_marker = true;
        }
    }
    if dir_marker {
        Ok(Metadata {
            is_dir: true,
            is_symlink: false,
            symlink_target: None,
            is_executable: false,
            size: 0,
            modified: UNIX_EPOCH,
            permissions: 0o755,
            owner: None,
            group: None,
            nlink: 1,
            accessed: UNIX_EPOCH,
            changed: UNIX_EPOCH,
            inode: 0,
        })
    } else {
        Err(FsError::Message(format!(
            "Path not found in ar: {}",
            inner_full.display()
        )))
    }
}

pub fn copy_out(archive_path: &Path, src_inner: &Path, dst: &Path) -> FsResult<()> {
    let f = File::open(archive_path)?;
    let mut ar = ar::Archive::new(f);
    let src_norm = norm(src_inner);
    let mut copied_exact = false;
    let mut extracted_any = false;
    while let Some(entry_res) = ar.next_entry() {
        let mut entry = entry_res.map_err(|e| FsError::Message(format!("ar entry: {e}")))?;
        let p = {
            let ident = String::from_utf8_lossy(entry.header().identifier()).to_string();
            norm(Path::new(ident.trim_matches(char::from(0)).trim()))
        };
        if p == src_norm {
            copied_exact = true;
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut out = std::fs::File::create(dst)?;
            std::io::copy(&mut entry, &mut out)?;
        } else if p.starts_with(&src_norm) {
            extracted_any = true;
            let rel = p.strip_prefix(&src_norm).unwrap();
            let target = dst.join(rel);
            if target.as_os_str().is_empty() {
                std::fs::create_dir_all(&target)?;
            } else {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let mut out = std::fs::File::create(&target)?;
                std::io::copy(&mut entry, &mut out)?;
            }
        }
    }
    if copied_exact || extracted_any {
        Ok(())
    } else {
        Err(FsError::Message(format!(
            "Source not found in ar: {}",
            src_inner.display()
        )))
    }
}
