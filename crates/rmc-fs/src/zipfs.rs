use crate::{DirEntry, FsError, FsResult, Metadata};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::time::{UNIX_EPOCH};
use zip::read::ZipArchive;

pub fn list_dir(archive_path: &Path, vfs_root: &Path, inner: &Path, show_hidden: bool) -> FsResult<Vec<DirEntry>> {
    let f = File::open(archive_path)?;
    let mut ar = ZipArchive::new(f).map_err(|e| FsError::Message(format!("zip open: {e}")))?;
    let inner_norm = norm(inner);
    let mut seen_dirs: HashSet<String> = HashSet::new();
    let mut items: HashMap<String, DirEntry> = HashMap::new();
    for i in 0..ar.len() {
        let file = ar.by_index(i).map_err(|e| FsError::Message(format!("zip entry: {e}")))?;
        let path = norm(Path::new(file.name()));
        if !path.starts_with(&inner_norm) {
            continue;
        }
        let rel = path.strip_prefix(&inner_norm).unwrap_or(&path);
        if rel.as_os_str().is_empty() {
            continue;
        }
        let mut comps = rel.components();
        if let Some(first) = comps.next() {
            let name = first.as_os_str().to_string_lossy().to_string();
            if !show_hidden && name.starts_with('.') {
                continue;
            }
            let is_dir = comps.next().is_some() || file.is_dir();
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
                                is_executable: false,
                                size: 0,
                                modified: UNIX_EPOCH,
                                permissions: 0o755,
                                owner: None,
                                group: None,
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
                            is_executable: false,
                            size: file.size(),
                            modified: UNIX_EPOCH,
                            permissions: 0o644,
                            owner: None,
                            group: None,
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
                is_executable: false,
                size: 0,
                modified: UNIX_EPOCH,
                permissions: 0,
                owner: None,
                group: None,
            },
        });
    }
    let mut vals: Vec<DirEntry> = items.into_values().collect();
    vals.sort_by(|a, b| {
        match (a.meta.is_dir, b.meta.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        }
    });
    out.extend(vals);
    Ok(out)
}

pub fn read_file(archive_path: &Path, inner_full: &Path) -> FsResult<Box<dyn Read + Send>> {
    let f = File::open(archive_path)?;
    let mut ar = ZipArchive::new(f).map_err(|e| FsError::Message(format!("zip open: {e}")))?;
    let in_norm = norm(inner_full);
    for i in 0..ar.len() {
        let mut file = ar.by_index(i).map_err(|e| FsError::Message(format!("zip entry: {e}")))?;
        let path = norm(Path::new(file.name()));
        if path == in_norm && !file.is_dir() {
            let mut buf = Vec::new();
            std::io::copy(&mut file, &mut buf)?;
            return Ok(Box::new(Cursor::new(buf)));
        }
    }
    Err(FsError::Message(format!(
        "File not found in archive: {}",
        inner_full.display()
    )))
}

pub fn stat(archive_path: &Path, inner_full: &Path) -> FsResult<Metadata> {
    if inner_full.as_os_str().is_empty() {
        return Ok(Metadata {
            is_dir: true,
            is_symlink: false,
            is_executable: false,
            size: 0,
            modified: UNIX_EPOCH,
            permissions: 0o755,
            owner: None,
            group: None,
        });
    }
    let f = File::open(archive_path)?;
    let mut ar = ZipArchive::new(f).map_err(|e| FsError::Message(format!("zip open: {e}")))?;
    let in_norm = norm(inner_full);
    let mut dir_marker = false;
    for i in 0..ar.len() {
        let file = ar.by_index(i).map_err(|e| FsError::Message(format!("zip entry: {e}")))?;
        let path = norm(Path::new(file.name()));
        if path == in_norm {
            return Ok(Metadata {
                is_dir: file.is_dir(),
                is_symlink: false,
                is_executable: false,
                size: file.size(),
                modified: UNIX_EPOCH,
                permissions: if file.is_dir() { 0o755 } else { 0o644 },
                owner: None,
                group: None,
            });
        }
        if path.starts_with(&in_norm) {
            dir_marker = true;
        }
    }
    if dir_marker {
        Ok(Metadata {
            is_dir: true,
            is_symlink: false,
            is_executable: false,
            size: 0,
            modified: UNIX_EPOCH,
            permissions: 0o755,
            owner: None,
            group: None,
        })
    } else {
        Err(FsError::Message(format!(
            "Path not found in archive: {}",
            inner_full.display()
        )))
    }
}

pub fn copy_out(archive_path: &Path, src_inner: &Path, dst: &Path) -> FsResult<()> {
    let f = File::open(archive_path)?;
    let mut ar = ZipArchive::new(f).map_err(|e| FsError::Message(format!("zip open: {e}")))?;
    let src_norm = norm(src_inner);
    let mut copied_exact = false;
    let mut extracted_any = false;
    for i in 0..ar.len() {
        let mut file = ar.by_index(i).map_err(|e| FsError::Message(format!("zip entry: {e}")))?;
        let p = norm(Path::new(file.name()));
        if p == src_norm && !file.is_dir() {
            copied_exact = true;
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut out = std::fs::File::create(dst)?;
            std::io::copy(&mut file, &mut out)?;
            // continue extracting others not needed
        } else if p.starts_with(&src_norm) {
            extracted_any = true;
            let rel = p.strip_prefix(&src_norm).unwrap();
            let target = dst.join(rel);
            if file.is_dir() || target.as_os_str().is_empty() {
                std::fs::create_dir_all(&target)?;
            } else {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let mut out = std::fs::File::create(&target)?;
                std::io::copy(&mut file, &mut out)?;
            }
        }
    }
    if copied_exact || extracted_any {
        Ok(())
    } else {
        Err(FsError::Message(format!(
            "Source not found in archive: {}",
            src_inner.display()
        )))
    }
}

fn norm<P: AsRef<Path>>(p: P) -> PathBuf {
    // Normalize by removing redundant '.' and leading './'
    let mut out = PathBuf::new();
    for c in p.as_ref().components() {
        match c {
            std::path::Component::CurDir => {}
            _ => out.push(c.as_os_str()),
        }
    }
    out
}

