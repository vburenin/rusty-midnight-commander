use crate::{DirEntry, FsError, FsResult, Metadata};
use gpt_disk_io::BlockIoAdapter;
use gpt_disk_types::BlockSize;
use iso9660;
use iso9660::directory::iterator::DirectoryIterator;
use std::collections::{HashMap, HashSet};
use std::fs::File;
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

fn to_iso_path(inner: &Path) -> String {
    let s = inner.to_string_lossy().replace('\\', "/");
    if s.is_empty() {
        "/".to_string()
    } else if s.starts_with('/') {
        s
    } else {
        format!("/{s}")
    }
}

pub fn list_dir(
    archive_path: &Path,
    vfs_root: &Path,
    inner: &Path,
    show_hidden: bool,
) -> FsResult<Vec<DirEntry>> {
    let mut f = File::open(archive_path)?;
    let mut bio = BlockIoAdapter::new(&mut f, BlockSize::new(2048).unwrap());
    let vol =
        iso9660::mount(&mut bio, 0).map_err(|e| FsError::Message(format!("iso mount: {e}")))?;
    let mut out_map: HashMap<String, DirEntry> = HashMap::new();
    let inner_norm = norm(inner);
    // Find directory extent
    let (dir_lba, dir_len): (u32, u32) = if inner_norm.as_os_str().is_empty() {
        (vol.root_extent_lba, vol.root_extent_len)
    } else {
        let path = to_iso_path(&inner_norm);
        match iso9660::find_file(&mut bio, &vol, &path) {
            Ok(fe) => {
                if !fe.flags.directory {
                    // listing a file path: return only parent marker
                    if let Some(p) = vfs_root.parent() {
                        return Ok(vec![DirEntry {
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
                        }]);
                    }
                    return Ok(Vec::new());
                }
                (fe.extent_lba, fe.data_length)
            }
            Err(_) => return Err(FsError::Message("path not found in ISO".into())),
        }
    };
    let mut seen_dirs: HashSet<String> = HashSet::new();
    let it = DirectoryIterator::new(&mut bio, dir_lba, dir_len);
    for item in it {
        let entry = item.map_err(|e| FsError::Message(format!("iso dir: {e}")))?;
        let name = entry.name();
        if name == "." || name == ".." {
            continue;
        }
        let first = name.to_string();
        if !show_hidden && first.starts_with('.') {
            continue;
        }
        if entry.flags.directory {
            if seen_dirs.insert(first.clone()) {
                let p = vfs_root.join(&first);
                out_map.insert(
                    first.clone(),
                    DirEntry {
                        name: first,
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
        } else {
            let p = vfs_root.join(&first);
            out_map.entry(first.clone()).or_insert(DirEntry {
                name: first.clone(),
                path: p,
                meta: Metadata {
                    is_dir: false,
                    is_symlink: false,
                    is_executable: false,
                    size: entry.size,
                    modified: UNIX_EPOCH,
                    permissions: 0o644,
                    owner: None,
                    group: None,
                    nlink: 1,
                    accessed: UNIX_EPOCH,
                    changed: UNIX_EPOCH,
                    inode: 0,
                },
            });
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
    let mut vals: Vec<DirEntry> = out_map.into_values().collect();
    vals.sort_by(|a, b| match (a.meta.is_dir, b.meta.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });
    out.extend(vals);
    Ok(out)
}

pub fn read_file(archive_path: &Path, inner_full: &Path) -> FsResult<Box<dyn Read + Send>> {
    let mut f = File::open(archive_path)?;
    let mut bio = BlockIoAdapter::new(&mut f, BlockSize::new(2048).unwrap());
    let vol =
        iso9660::mount(&mut bio, 0).map_err(|e| FsError::Message(format!("iso mount: {e}")))?;
    let path = to_iso_path(inner_full);
    let fe = iso9660::find_file(&mut bio, &vol, &path)
        .map_err(|e| FsError::Message(format!("iso find_file: {e}")))?;
    if fe.flags.directory {
        return Err(FsError::Message("cannot read a directory".into()));
    }
    let data = iso9660::read_file_vec(&mut bio, &fe)
        .map_err(|e| FsError::Message(format!("iso read_file_vec: {e}")))?;
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
            nlink: 1,
            accessed: UNIX_EPOCH,
            changed: UNIX_EPOCH,
            inode: 0,
        });
    }
    let mut f = File::open(archive_path)?;
    let mut bio = BlockIoAdapter::new(&mut f, BlockSize::new(2048).unwrap());
    let vol =
        iso9660::mount(&mut bio, 0).map_err(|e| FsError::Message(format!("iso mount: {e}")))?;
    let path = to_iso_path(inner_full);
    match iso9660::find_file(&mut bio, &vol, &path) {
        Ok(fe) => Ok(Metadata {
            is_dir: fe.flags.directory,
            is_symlink: false,
            is_executable: false,
            size: fe.size,
            modified: UNIX_EPOCH,
            permissions: if fe.flags.directory { 0o755 } else { 0o644 },
            owner: None,
            group: None,
            nlink: 1,
            accessed: UNIX_EPOCH,
            changed: UNIX_EPOCH,
            inode: 0,
        }),
        Err(_) => Err(FsError::Message(format!(
            "Path not found in ISO: {}",
            inner_full.display()
        ))),
    }
}

pub fn copy_out(archive_path: &Path, src_inner: &Path, dst: &Path) -> FsResult<()> {
    let mut f = File::open(archive_path)?;
    let mut bio = BlockIoAdapter::new(&mut f, BlockSize::new(2048).unwrap());
    let vol =
        iso9660::mount(&mut bio, 0).map_err(|e| FsError::Message(format!("iso mount: {e}")))?;
    let src_path = to_iso_path(src_inner);
    // Try exact file first
    if let Ok(fe) = iso9660::find_file(&mut bio, &vol, &src_path) {
        if !fe.flags.directory {
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let data = iso9660::read_file_vec(&mut bio, &fe)
                .map_err(|e| FsError::Message(format!("iso read_file_vec: {e}")))?;
            let mut out = std::fs::File::create(dst)?;
            std::io::copy(&mut Cursor::new(data), &mut out)?;
            return Ok(());
        }
        // Directory: walk recursively
        extract_dir(&mut bio, &vol, &src_path, dst)
    } else {
        // Fallback: treat as directory prefix and walk
        extract_dir(&mut bio, &vol, &src_path, dst)
    }
}

fn extract_dir(
    bio: &mut BlockIoAdapter<&mut File>,
    vol: &iso9660::types::VolumeInfo,
    src_prefix: &str,
    dst: &Path,
) -> FsResult<()> {
    // Iterative traversal using a stack
    let mut stack: Vec<(u32, u32, String)> =
        vec![(vol.root_extent_lba, vol.root_extent_len, "/".to_string())];
    let mut any = false;
    let mut to_extract: Vec<(String, iso9660::types::FileEntry)> = Vec::new();
    while let Some((lba, len, cur_path)) = stack.pop() {
        let it = DirectoryIterator::new(bio, lba, len);
        for item in it {
            let e = item.map_err(|er| FsError::Message(format!("iso walk: {er}")))?;
            let name = e.name();
            if name == "." || name == ".." {
                continue;
            }
            let full = if cur_path == "/" {
                format!("/{}", name)
            } else {
                format!("{}/{}", cur_path, name)
            };
            if e.flags.directory {
                stack.push((e.extent_lba, e.data_length, full));
            } else if full.starts_with(src_prefix) {
                any = true;
                let rel = full
                    .trim_start_matches(src_prefix)
                    .trim_start_matches('/')
                    .to_string();
                to_extract.push((rel, e.clone()));
            }
        }
    }
    // Perform extraction after traversal (no active iterator borrowing bio)
    for (rel, fe) in to_extract {
        let target = if rel.is_empty() {
            dst.to_path_buf()
        } else {
            dst.join(&rel)
        };
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let data = iso9660::read_file_vec(bio, &fe)
            .map_err(|er| FsError::Message(format!("iso extract: {er}")))?;
        let mut out = std::fs::File::create(&target)?;
        std::io::copy(&mut Cursor::new(data), &mut out)?;
    }
    if any {
        Ok(())
    } else {
        Err(FsError::Message(format!(
            "Source not found in ISO: {}",
            src_prefix
        )))
    }
}
