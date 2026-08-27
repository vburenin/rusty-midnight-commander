use crate::pathutil::ArchiveKind;
use crate::{DirEntry, FsError, FsResult, Metadata};
use flate2::read::GzDecoder;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tar::{Archive, EntryType};

fn open_tar_reader(archive_path: &Path, kind: ArchiveKind) -> FsResult<Box<dyn Read>> {
    let f = File::open(archive_path)?;
    match kind {
        ArchiveKind::Tar => Ok(Box::new(f)),
        ArchiveKind::TarGz => Ok(Box::new(GzDecoder::new(f))),
        _ => Err(FsError::Message("non-tar kind passed to tarfs".into())),
    }
}

fn header_mtime(h: &tar::Header) -> SystemTime {
    h.mtime()
        .map(|s| UNIX_EPOCH + Duration::from_secs(s))
        .unwrap_or(UNIX_EPOCH)
}

fn header_mode(h: &tar::Header, is_dir: bool) -> u32 {
    h.mode().unwrap_or(if is_dir { 0o755 } else { 0o644 })
}

pub fn list_dir(
    archive_path: &Path,
    kind: ArchiveKind,
    inner: &Path,
    vfs_root: &Path,
    show_hidden: bool,
) -> FsResult<Vec<DirEntry>> {
    let reader = open_tar_reader(archive_path, kind)?;
    let mut ar = Archive::new(reader);
    let inner_norm = norm(inner);
    let mut seen_dirs: HashSet<String> = HashSet::new();
    let mut items: HashMap<String, DirEntry> = HashMap::new();
    for entry in ar.entries()? {
        let entry = entry?;
        let path = entry.path()?;
        let path = norm(path);
        // Filter by inner prefix
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
            let is_dir =
                comps.next().is_some() || entry.header().entry_type() == EntryType::Directory;
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
                                nlink: 1,
                                accessed: UNIX_EPOCH,
                                changed: UNIX_EPOCH,
                                inode: 0,
                            },
                        },
                    );
                }
            } else if !items.contains_key(&name) {
                // File at this level
                let size = entry.header().size().unwrap_or(0);
                let modified = header_mtime(entry.header());
                let p = vfs_root.join(&name);
                items.insert(
                    name.clone(),
                    DirEntry {
                        name,
                        path: p,
                        meta: Metadata {
                            is_dir: false,
                            is_symlink: false,
                            is_executable: (header_mode(entry.header(), false) & 0o111) != 0,
                            size,
                            modified,
                            accessed: modified,
                            changed: modified,
                            permissions: header_mode(entry.header(), false),
                            owner: None,
                            group: None,
                            nlink: 1,
                            inode: 0,
                        },
                    },
                );
            }
        }
    }
    let mut out: Vec<DirEntry> = Vec::new();
    // Add parent marker when not at host FS root of archive
    // For archive root path like "...tar#", parent should be outside path (handled by CompositeFs)
    let parent = vfs_root.parent();
    if let Some(p) = parent {
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
    // Directories first, then files by name (case-insensitive)
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
    let reader = open_tar_reader(archive_path, kind)?;
    let mut ar = Archive::new(reader);
    let in_norm = norm(inner_full);
    for entry in ar.entries()? {
        let mut entry = entry?;
        let path = norm(entry.path()?);
        if path == in_norm {
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            return Ok(Box::new(Cursor::new(buf)));
        }
    }
    Err(FsError::Message(format!(
        "File not found in archive: {}",
        inner_full.display()
    )))
}

pub fn stat(archive_path: &Path, kind: ArchiveKind, inner_full: &Path) -> FsResult<Metadata> {
    if inner_full.as_os_str().is_empty() {
        // Root of the archive
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
    let reader = open_tar_reader(archive_path, kind)?;
    let mut ar = Archive::new(reader);
    let in_norm = norm(inner_full);
    let mut is_dir_marker = false;
    for entry in ar.entries()? {
        let entry = entry?;
        let path = norm(entry.path()?);
        if path == in_norm {
            let h = entry.header();
            let is_dir = h.entry_type() == EntryType::Directory;
            let size = h.size().unwrap_or(0);
            let modified = header_mtime(h);
            let mode = header_mode(h, is_dir);
            return Ok(Metadata {
                is_dir,
                is_symlink: false,
                is_executable: (!is_dir) && (mode & 0o111 != 0),
                size,
                modified,
                accessed: modified,
                changed: modified,
                permissions: mode,
                owner: None,
                group: None,
                nlink: 1,
                inode: 0,
            });
        }
        if path.starts_with(&in_norm) {
            // Any descendant implies it's a dir
            is_dir_marker = true;
        }
    }
    if is_dir_marker {
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
    // Copy one file or a directory tree out to dst.
    // If src_inner is a file, dst is the file path. If a directory, dst is the target dir.
    let reader = open_tar_reader(archive_path, kind)?;
    let mut ar = Archive::new(reader);
    let src_norm = norm(src_inner);
    let mut copied_exact = false;
    let mut extracted_any = false;
    for entry in ar.entries()? {
        let mut entry = entry?;
        let p = norm(entry.path()?);
        if p == src_norm {
            copied_exact = true;
            // exact file
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut out = std::fs::File::create(dst)?;
            std::io::copy(&mut entry, &mut out)?;
            // continue, do not early-return to avoid unused assignment warning
        } else if p.starts_with(&src_norm) {
            extracted_any = true;
            // Dir extraction
            let rel = p.strip_prefix(&src_norm).unwrap();
            let target = dst.join(rel);
            if entry.header().entry_type() == EntryType::Directory || target.as_os_str().is_empty()
            {
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
