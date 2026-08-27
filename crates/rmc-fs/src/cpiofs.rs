use crate::pathutil::ArchiveKind;
use crate::{DirEntry, FsError, FsResult, Metadata};
use flate2::read::GzDecoder;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

fn load_cpio_bytes(path: &Path, kind: ArchiveKind) -> FsResult<Vec<u8>> {
    let f = File::open(path)?;
    match kind {
        ArchiveKind::Cpio => {
            let mut buf = Vec::new();
            let mut r = std::io::BufReader::new(f);
            r.read_to_end(&mut buf)?;
            Ok(buf)
        }
        ArchiveKind::CpioGz => {
            let mut dec = GzDecoder::new(f);
            let mut buf = Vec::new();
            dec.read_to_end(&mut buf)?;
            Ok(buf)
        }
        _ => Err(FsError::Message("invalid ArchiveKind for cpiofs".into())),
    }
}

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

/// Helpers that operate on in-memory cpio bytes. Used by rpmfs to reuse cpio logic.
pub fn list_dir_from_bytes(
    data: &[u8],
    inner: &Path,
    vfs_root: &Path,
    show_hidden: bool,
) -> FsResult<Vec<DirEntry>> {
    let inner_norm = norm(inner);
    let mut seen_dirs: HashSet<String> = HashSet::new();
    let mut items: HashMap<String, DirEntry> = HashMap::new();
    for entry in cpio_reader::iter_files(data) {
        let name = entry.name();
        if name.is_empty() || name == "TRAILER!!!" {
            continue;
        }
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
            let first_name = first.as_os_str().to_string_lossy().to_string();
            if !show_hidden && first_name.starts_with('.') {
                continue;
            }
            let is_dir = comps.next().is_some();
            if is_dir {
                if seen_dirs.insert(first_name.clone()) {
                    let p = vfs_root.join(&first_name);
                    items.insert(
                        first_name.clone(),
                        DirEntry {
                            name: first_name,
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
                                nlink: 1,
                                accessed: UNIX_EPOCH,
                                changed: UNIX_EPOCH,
                                inode: 0,
                            },
                        },
                    );
                }
            } else if !items.contains_key(&first_name) {
                let p = vfs_root.join(&first_name);
                let size = entry.file().len() as u64;
                items.insert(
                    first_name.clone(),
                    DirEntry {
                        name: first_name,
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

pub fn read_file_from_bytes(data: &[u8], inner_full: &Path) -> FsResult<Box<dyn Read + Send>> {
    let in_norm = norm(inner_full);
    for entry in cpio_reader::iter_files(data) {
        let name = entry.name();
        if name.is_empty() || name == "TRAILER!!!" {
            continue;
        }
        if norm(Path::new(name)) == in_norm {
            return Ok(Box::new(Cursor::new(entry.file().to_vec())));
        }
    }
    Err(FsError::Message(format!(
        "File not found in archive: {}",
        inner_full.display()
    )))
}

pub fn stat_from_bytes(data: &[u8], inner_full: &Path) -> FsResult<Metadata> {
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
            nlink: 1,
            accessed: UNIX_EPOCH,
            changed: UNIX_EPOCH,
            inode: 0,
        });
    }
    let in_norm = norm(inner_full);
    let mut dir_marker = false;
    for entry in cpio_reader::iter_files(data) {
        let name = entry.name();
        if name.is_empty() || name == "TRAILER!!!" {
            continue;
        }
        let p = norm(Path::new(name));
        if p == in_norm {
            return Ok(Metadata {
                is_dir: false,
                is_symlink: false,
                is_executable: false,
                size: entry.file().len() as u64,
                modified: UNIX_EPOCH,
                permissions: 0o644,
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
            "Path not found in archive: {}",
            inner_full.display()
        )))
    }
}

pub fn copy_out_from_bytes(data: &[u8], src_inner: &Path, dst: &Path) -> FsResult<()> {
    let src_norm = norm(src_inner);
    let mut copied_exact = false;
    let mut extracted_any = false;
    for entry in cpio_reader::iter_files(data) {
        let name = entry.name();
        if name.is_empty() || name == "TRAILER!!!" {
            continue;
        }
        let p = norm(Path::new(name));
        if p == src_norm {
            copied_exact = true;
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut out = std::fs::File::create(dst)?;
            std::io::copy(&mut Cursor::new(entry.file()), &mut out)?;
        } else if p.starts_with(&src_norm) {
            extracted_any = true;
            let rel = p.strip_prefix(&src_norm).unwrap();
            let target = dst.join(rel);
            if target.as_os_str().is_empty() || rel.as_os_str().is_empty() {
                std::fs::create_dir_all(&target)?;
            } else {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let mut out = std::fs::File::create(&target)?;
                std::io::copy(&mut Cursor::new(entry.file()), &mut out)?;
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

pub fn list_dir(
    archive_path: &Path,
    kind: ArchiveKind,
    inner: &Path,
    vfs_root: &Path,
    show_hidden: bool,
) -> FsResult<Vec<DirEntry>> {
    let data = load_cpio_bytes(archive_path, kind)?;
    let inner_norm = norm(inner);
    let mut seen_dirs: HashSet<String> = HashSet::new();
    let mut items: HashMap<String, DirEntry> = HashMap::new();
    for entry in cpio_reader::iter_files(&data) {
        let name = entry.name();
        // Skip trailer or empty
        if name.is_empty() || name == "TRAILER!!!" {
            continue;
        }
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
            let first_name = first.as_os_str().to_string_lossy().to_string();
            if !show_hidden && first_name.starts_with('.') {
                continue;
            }
            let is_dir = comps.next().is_some();
            if is_dir {
                if seen_dirs.insert(first_name.clone()) {
                    let p = vfs_root.join(&first_name);
                    items.insert(
                        first_name.clone(),
                        DirEntry {
                            name: first_name,
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
                                nlink: 1,
                                accessed: UNIX_EPOCH,
                                changed: UNIX_EPOCH,
                                inode: 0,
                            },
                        },
                    );
                }
            } else if !items.contains_key(&first_name) {
                let p = vfs_root.join(&first_name);
                // Use entry file length as size if this is the exact file at this level
                let size = entry.file().len() as u64;
                items.insert(
                    first_name.clone(),
                    DirEntry {
                        name: first_name,
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

pub fn read_file(
    archive_path: &Path,
    kind: ArchiveKind,
    inner_full: &Path,
) -> FsResult<Box<dyn Read + Send>> {
    let data = load_cpio_bytes(archive_path, kind)?;
    let in_norm = norm(inner_full);
    for entry in cpio_reader::iter_files(&data) {
        let name = entry.name();
        if name.is_empty() || name == "TRAILER!!!" {
            continue;
        }
        if norm(Path::new(name)) == in_norm {
            return Ok(Box::new(Cursor::new(entry.file().to_vec())));
        }
    }
    Err(FsError::Message(format!(
        "File not found in archive: {}",
        inner_full.display()
    )))
}

pub fn stat(archive_path: &Path, kind: ArchiveKind, inner_full: &Path) -> FsResult<Metadata> {
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
            nlink: 1,
            accessed: UNIX_EPOCH,
            changed: UNIX_EPOCH,
            inode: 0,
        });
    }
    let data = load_cpio_bytes(archive_path, kind)?;
    let in_norm = norm(inner_full);
    let mut dir_marker = false;
    for entry in cpio_reader::iter_files(&data) {
        let name = entry.name();
        if name.is_empty() || name == "TRAILER!!!" {
            continue;
        }
        let p = norm(Path::new(name));
        if p == in_norm {
            return Ok(Metadata {
                is_dir: false,
                is_symlink: false,
                is_executable: false,
                size: entry.file().len() as u64,
                modified: UNIX_EPOCH,
                permissions: 0o644,
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
            "Path not found in archive: {}",
            inner_full.display()
        )))
    }
}

pub fn copy_out(
    archive_path: &Path,
    kind: ArchiveKind,
    src_inner: &Path,
    dst: &Path,
) -> FsResult<()> {
    let data = load_cpio_bytes(archive_path, kind)?;
    let src_norm = norm(src_inner);
    let mut copied_exact = false;
    let mut extracted_any = false;
    for entry in cpio_reader::iter_files(&data) {
        let name = entry.name();
        if name.is_empty() || name == "TRAILER!!!" {
            continue;
        }
        let p = norm(Path::new(name));
        if p == src_norm {
            copied_exact = true;
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut out = std::fs::File::create(dst)?;
            std::io::copy(&mut Cursor::new(entry.file()), &mut out)?;
        } else if p.starts_with(&src_norm) {
            extracted_any = true;
            let rel = p.strip_prefix(&src_norm).unwrap();
            let target = dst.join(rel);
            if target.as_os_str().is_empty() || rel.as_os_str().is_empty() {
                std::fs::create_dir_all(&target)?;
            } else {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let mut out = std::fs::File::create(&target)?;
                std::io::copy(&mut Cursor::new(entry.file()), &mut out)?;
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
