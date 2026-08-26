use crate::{DirEntry, FsError, FsResult, Metadata};
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

fn empty_password() -> sevenz_rust2::Password {
    sevenz_rust2::Password::from("")
}

pub fn list_dir(
    archive_path: &Path,
    vfs_root: &Path,
    inner: &Path,
    show_hidden: bool,
) -> FsResult<Vec<DirEntry>> {
    let archive = sevenz_rust2::Archive::open(archive_path)
        .map_err(|e| FsError::Message(format!("7z open: {e}")))?;
    let inner_norm = norm(inner);
    let mut seen_dirs: HashSet<String> = HashSet::new();
    let mut items: HashMap<String, DirEntry> = HashMap::new();
    for entry in &archive.files {
        let path = norm(Path::new(entry.name()));
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
            let is_dir = comps.next().is_some() || entry.is_directory();
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
                let size = entry.size();
                items.insert(
                    name.clone(),
                    DirEntry {
                        name,
                        path: p,
                        meta: Metadata {
                            is_dir: false,
                            is_symlink: false,
                            is_executable: false,
                            size,
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
    vals.sort_by(|a, b| match (a.meta.is_dir, b.meta.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });
    out.extend(vals);
    Ok(out)
}

pub fn read_file(archive_path: &Path, inner_full: &Path) -> FsResult<Box<dyn Read + Send>> {
    let mut reader = sevenz_rust2::ArchiveReader::open(archive_path, empty_password())
        .map_err(|e| FsError::Message(format!("7z reader open: {e}")))?;
    let name_str = inner_full.to_string_lossy().replace('\\', "/");
    let data = reader
        .read_file(&name_str)
        .map_err(|e| FsError::Message(format!("7z read: {e}")))?;
    Ok(Box::new(Cursor::new(data)))
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
    let archive = sevenz_rust2::Archive::open(archive_path)
        .map_err(|e| FsError::Message(format!("7z open: {e}")))?;
    let in_norm = norm(inner_full);
    let mut dir_marker = false;
    for entry in &archive.files {
        let p = norm(Path::new(entry.name()));
        if p == in_norm {
            return Ok(Metadata {
                is_dir: entry.is_directory(),
                is_symlink: false,
                is_executable: false,
                size: entry.size(),
                modified: UNIX_EPOCH,
                permissions: if entry.is_directory() { 0o755 } else { 0o644 },
                owner: None,
                group: None,
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
    let meta = sevenz_rust2::Archive::open(archive_path)
        .map_err(|e| FsError::Message(format!("7z open: {e}")))?;
    let mut reader = sevenz_rust2::ArchiveReader::open(archive_path, empty_password())
        .map_err(|e| FsError::Message(format!("7z reader open: {e}")))?;
    let src_norm = norm(src_inner);
    let mut copied_exact = false;
    let mut extracted_any = false;
    for f in meta.files.iter() {
        let p = norm(Path::new(f.name()));
        if p == src_norm && !f.is_directory() {
            copied_exact = true;
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let data = reader
                .read_file(f.name())
                .map_err(|e| FsError::Message(format!("7z read: {e}")))?;
            let mut out = std::fs::File::create(dst)?;
            std::io::copy(&mut Cursor::new(data), &mut out)?;
        } else if p.starts_with(&src_norm) {
            extracted_any = true;
            let rel = p.strip_prefix(&src_norm).unwrap();
            let target = dst.join(rel);
            if f.is_directory() || target.as_os_str().is_empty() {
                std::fs::create_dir_all(&target)?;
            } else {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let data = reader
                    .read_file(f.name())
                    .map_err(|e| FsError::Message(format!("7z read: {e}")))?;
                let mut out = std::fs::File::create(&target)?;
                std::io::copy(&mut Cursor::new(data), &mut out)?;
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
