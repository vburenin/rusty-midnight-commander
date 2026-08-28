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

struct LhaMember {
    path: PathBuf,
    size: u64,
    is_dir: bool,
}

fn open_reader(archive_path: &Path) -> FsResult<delharc::LhaDecodeReader<std::fs::File>> {
    delharc::parse_file(archive_path).map_err(|e| FsError::Message(format!("lha open: {e}")))
}

fn seek_next(reader: &mut delharc::LhaDecodeReader<std::fs::File>) -> FsResult<bool> {
    reader
        .seek_next_file()
        .map_err(|e| FsError::Message(format!("lha next: {e}")))
}

fn list_members(archive_path: &Path) -> FsResult<Vec<LhaMember>> {
    let mut reader = open_reader(archive_path)?;
    let mut out = Vec::new();
    loop {
        let header = reader.header();
        let path = norm(header.parse_pathname());
        if !path.as_os_str().is_empty() {
            out.push(LhaMember {
                path,
                size: header.original_size,
                is_dir: header.is_directory(),
            });
        }
        if !seek_next(&mut reader)? {
            break;
        }
    }
    Ok(out)
}

fn decode_current(
    reader: &mut delharc::LhaDecodeReader<std::fs::File>,
    name: &Path,
) -> FsResult<Vec<u8>> {
    if !reader.is_decoder_supported() {
        return Err(FsError::Message(format!(
            "lha: unsupported compression in {}",
            name.display()
        )));
    }
    let mut buf = Vec::new();
    std::io::copy(reader, &mut buf).map_err(|e| FsError::Message(format!("lha read: {e}")))?;
    reader
        .crc_check()
        .map_err(|e| FsError::Message(format!("lha crc: {e}")))?;
    Ok(buf)
}

fn parent_dotdot(vfs_root: &Path) -> Option<DirEntry> {
    vfs_root.parent().map(|p| DirEntry {
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
    })
}

fn dir_meta() -> Metadata {
    Metadata {
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
    }
}

fn file_meta(size: u64) -> Metadata {
    Metadata {
        is_dir: false,
        is_symlink: false,
        symlink_target: None,
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
    }
}

pub fn list_dir(
    archive_path: &Path,
    vfs_root: &Path,
    inner: &Path,
    show_hidden: bool,
) -> FsResult<Vec<DirEntry>> {
    let members = list_members(archive_path)?;
    let inner_norm = norm(inner);
    let mut seen_dirs: HashSet<String> = HashSet::new();
    let mut items: HashMap<String, DirEntry> = HashMap::new();
    for m in members {
        if !m.path.starts_with(&inner_norm) {
            continue;
        }
        let rel = m.path.strip_prefix(&inner_norm).unwrap_or(&m.path);
        if rel.as_os_str().is_empty() {
            continue;
        }
        let mut comps = rel.components();
        if let Some(first) = comps.next() {
            let name = first.as_os_str().to_string_lossy().to_string();
            if !show_hidden && name.starts_with('.') {
                continue;
            }
            let is_dir = comps.next().is_some() || m.is_dir;
            if is_dir {
                if seen_dirs.insert(name.clone()) {
                    let p = vfs_root.join(&name);
                    items.insert(
                        name.clone(),
                        DirEntry {
                            name,
                            path: p,
                            meta: dir_meta(),
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
                        meta: file_meta(m.size),
                    },
                );
            }
        }
    }
    let mut out: Vec<DirEntry> = Vec::new();
    if let Some(dd) = parent_dotdot(vfs_root) {
        out.push(dd);
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
    let mut reader = open_reader(archive_path)?;
    let target = norm(inner_full);
    loop {
        let header = reader.header();
        let p = norm(header.parse_pathname());
        let is_dir = header.is_directory();
        if p == target && !is_dir {
            let buf = decode_current(&mut reader, &p)?;
            return Ok(Box::new(Cursor::new(buf)));
        }
        if !seek_next(&mut reader)? {
            break;
        }
    }
    Err(FsError::Message(format!(
        "File not found in lha: {}",
        inner_full.display()
    )))
}

pub fn stat(archive_path: &Path, inner_full: &Path) -> FsResult<Metadata> {
    if inner_full.as_os_str().is_empty() {
        return Ok(dir_meta());
    }
    let members = list_members(archive_path)?;
    let target = norm(inner_full);
    let mut dir_marker = false;
    for m in members {
        if m.path == target {
            return Ok(if m.is_dir {
                dir_meta()
            } else {
                file_meta(m.size)
            });
        }
        if m.path.starts_with(&target) {
            dir_marker = true;
        }
    }
    if dir_marker {
        Ok(dir_meta())
    } else {
        Err(FsError::Message(format!(
            "Path not found in lha: {}",
            inner_full.display()
        )))
    }
}

pub fn copy_out(archive_path: &Path, src_inner: &Path, dst: &Path) -> FsResult<()> {
    let mut reader = open_reader(archive_path)?;
    let src_norm = norm(src_inner);
    let mut copied_exact = false;
    let mut extracted_any = false;
    loop {
        let header = reader.header();
        let p = norm(header.parse_pathname());
        let is_dir = header.is_directory();
        if p.as_os_str().is_empty() {
            if !seek_next(&mut reader)? {
                break;
            }
            continue;
        }
        if p == src_norm && !is_dir {
            copied_exact = true;
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let data = decode_current(&mut reader, &p)?;
            let mut out = std::fs::File::create(dst)?;
            std::io::copy(&mut Cursor::new(data), &mut out)?;
        } else if p.starts_with(&src_norm) {
            extracted_any = true;
            let rel = p.strip_prefix(&src_norm).unwrap();
            let target = dst.join(rel);
            if is_dir || target.as_os_str().is_empty() {
                std::fs::create_dir_all(&target)?;
                if !seek_next(&mut reader)? {
                    break;
                }
                continue;
            }
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let data = decode_current(&mut reader, &p)?;
            let mut out = std::fs::File::create(&target)?;
            std::io::copy(&mut Cursor::new(data), &mut out)?;
        } else if !seek_next(&mut reader)? {
            break;
        } else {
            continue;
        }
        if !seek_next(&mut reader)? {
            break;
        }
    }
    if copied_exact || extracted_any {
        Ok(())
    } else {
        Err(FsError::Message(format!(
            "Source not found in lha: {}",
            src_inner.display()
        )))
    }
}
