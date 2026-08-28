use crate::{DirEntry, FsError, FsResult, Metadata};
use rars::{Archive, ArchiveReader};
use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

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

pub fn list_dir(
    archive_path: &Path,
    vfs_root: &Path,
    inner: &Path,
    show_hidden: bool,
) -> FsResult<Vec<DirEntry>> {
    let archive = ArchiveReader::read_path(archive_path)
        .map_err(|e| FsError::Message(format!("rar open: {e}")))?;
    let inner_norm = norm(inner);
    let mut seen_dirs: HashSet<String> = HashSet::new();
    let mut items: HashMap<String, DirEntry> = HashMap::new();
    for m in archive.members() {
        let name = match std::str::from_utf8(&m.meta.name) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let path = norm(Path::new(name));
        if !path.starts_with(&inner_norm) {
            continue;
        }
        let rel = path.strip_prefix(&inner_norm).unwrap_or(&path);
        if rel.as_os_str().is_empty() {
            continue;
        }
        let mut comps = rel.components();
        if let Some(first) = comps.next() {
            let fname = first.as_os_str().to_string_lossy().to_string();
            if !show_hidden && fname.starts_with('.') {
                continue;
            }
            let is_dir = comps.next().is_some() || m.meta.is_directory;
            if is_dir {
                if seen_dirs.insert(fname.clone()) {
                    let p = vfs_root.join(&fname);
                    items.insert(
                        fname.clone(),
                        DirEntry {
                            name: fname,
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
            } else if !items.contains_key(&fname) {
                let p = vfs_root.join(&fname);
                items.insert(
                    fname.clone(),
                    DirEntry {
                        name: fname,
                        path: p,
                        meta: Metadata {
                            is_dir: false,
                            is_symlink: false,
                            symlink_target: None,
                            is_executable: false,
                            size: m.meta.unpacked_size,
                            modified: UNIX_EPOCH,
                            permissions: 0o644,
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
    let archive = ArchiveReader::read_path(archive_path)
        .map_err(|e| FsError::Message(format!("rar open: {e}")))?;
    let name = inner_full.to_string_lossy().replace('\\', "/");
    match archive.read_member(name.as_bytes(), None) {
        Ok(Some(data)) => Ok(Box::new(Cursor::new(data))),
        Ok(None) => Err(FsError::Message(format!(
            "File not found in RAR: {}",
            inner_full.display()
        ))),
        Err(e) => Err(FsError::Message(format!("rar read: {e}"))),
    }
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
    let archive = ArchiveReader::read_path(archive_path)
        .map_err(|e| FsError::Message(format!("rar open: {e}")))?;
    let in_norm = norm(inner_full);
    let mut dir_marker = false;
    for m in archive.members() {
        let name = match std::str::from_utf8(&m.meta.name) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let p = norm(Path::new(name));
        if p == in_norm {
            return Ok(Metadata {
                is_dir: m.meta.is_directory,
                is_symlink: false,
                symlink_target: None,
                is_executable: false,
                size: m.meta.unpacked_size,
                modified: UNIX_EPOCH,
                permissions: if m.meta.is_directory { 0o755 } else { 0o644 },
                owner: None,
                group: None,
                nlink: 1,
                accessed: UNIX_EPOCH,
                changed: UNIX_EPOCH,
                inode: 0,
            });
        }
        if p.starts_with(&in_norm) {
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
            "Path not found in RAR: {}",
            inner_full.display()
        )))
    }
}

pub fn copy_out(archive_path: &Path, src_inner: &Path, dst: &Path) -> FsResult<()> {
    let archive = ArchiveReader::read_path(archive_path)
        .map_err(|e| FsError::Message(format!("rar open: {e}")))?;
    let src_norm = norm(src_inner);
    let mut copied_exact = false;
    let mut extracted_any = false;
    for m in archive.members() {
        let name = match std::str::from_utf8(&m.meta.name) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let p = norm(Path::new(name));
        if p == src_norm && !m.meta.is_directory {
            copied_exact = true;
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let data = Archive::read_member(&archive, name.as_bytes(), None)
                .map_err(|e| FsError::Message(format!("rar read: {e}")))?
                .unwrap_or_default();
            let mut out = std::fs::File::create(dst)?;
            std::io::copy(&mut Cursor::new(data), &mut out)?;
        } else if p.starts_with(&src_norm) {
            extracted_any = true;
            let rel = p.strip_prefix(&src_norm).unwrap();
            let target = dst.join(rel);
            if m.meta.is_directory || target.as_os_str().is_empty() {
                std::fs::create_dir_all(&target)?;
            } else {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let data = Archive::read_member(&archive, name.as_bytes(), None)
                    .map_err(|e| FsError::Message(format!("rar read: {e}")))?
                    .unwrap_or_default();
                let mut out = std::fs::File::create(&target)?;
                std::io::copy(&mut Cursor::new(data), &mut out)?;
            }
        }
    }
    if copied_exact || extracted_any {
        Ok(())
    } else {
        Err(FsError::Message(format!(
            "Source not found in RAR: {}",
            src_inner.display()
        )))
    }
}
