use crate::{DirEntry, FsError, FsResult, Metadata};
use flate2::read::GzDecoder;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tar::{Archive, EntryType};
use xz2::read::XzDecoder;

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

fn header_mtime(h: &tar::Header) -> SystemTime {
    h.mtime()
        .map(|s| UNIX_EPOCH + Duration::from_secs(s))
        .unwrap_or(UNIX_EPOCH)
}

fn header_mode(h: &tar::Header, is_dir: bool) -> u32 {
    h.mode().unwrap_or(if is_dir { 0o755 } else { 0o644 })
}

fn list_tar_from_reader<R: Read>(
    reader: R,
    inner: &Path,
    vfs_root: &Path,
    show_hidden: bool,
) -> FsResult<Vec<DirEntry>> {
    let mut ar = Archive::new(reader);
    let inner_norm = norm(inner);
    let mut seen_dirs: HashSet<String> = HashSet::new();
    let mut items: HashMap<String, DirEntry> = HashMap::new();
    for entry in ar.entries()? {
        let entry = entry?;
        let path = norm(entry.path()?);
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
                            },
                        },
                    );
                }
            } else if !items.contains_key(&name) {
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
                            permissions: header_mode(entry.header(), false),
                            owner: None,
                            group: None,
                            nlink: 1,
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

fn read_tar_file_from_reader<R: Read>(
    reader: R,
    inner_full: &Path,
) -> FsResult<Box<dyn Read + Send>> {
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
        "File not found in deb tar member: {}",
        inner_full.display()
    )))
}

fn stat_tar_from_reader<R: Read>(reader: R, inner_full: &Path) -> FsResult<Metadata> {
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
        });
    }
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
                permissions: mode,
                owner: None,
                group: None,
                nlink: 1,
            });
        }
        if path.starts_with(&in_norm) {
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
        })
    } else {
        Err(FsError::Message(format!(
            "Path not found in deb tar member: {}",
            inner_full.display()
        )))
    }
}

fn copy_out_tar_from_reader<R: Read>(reader: R, src_inner: &Path, dst: &Path) -> FsResult<()> {
    let mut ar = Archive::new(reader);
    let src_norm = norm(src_inner);
    let mut copied_exact = false;
    let mut extracted_any = false;
    for entry in ar.entries()? {
        let mut entry = entry?;
        let p = norm(entry.path()?);
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
            "Source not found in deb tar member: {}",
            src_inner.display()
        )))
    }
}

enum TarCompression {
    Plain,
    Gzip,
    Xz,
}

fn detect_tar_compression(name: &str) -> TarCompression {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
        TarCompression::Gzip
    } else if lower.ends_with(".tar.xz") || lower.ends_with(".txz") {
        TarCompression::Xz
    } else {
        TarCompression::Plain
    }
}

fn read_ar_member(archive_path: &Path, member_name: &str) -> FsResult<Vec<u8>> {
    let f = File::open(archive_path)?;
    let mut ar = ar::Archive::new(f);
    while let Some(entry_res) = ar.next_entry() {
        let mut entry = entry_res.map_err(|e| FsError::Message(format!("ar entry: {e}")))?;
        let ident = String::from_utf8_lossy(entry.header().identifier()).to_string();
        let name = ident.trim_matches(char::from(0)).trim().to_string();
        if name == member_name {
            let mut buf = Vec::with_capacity(entry.header().size() as usize);
            std::io::copy(&mut entry, &mut buf)
                .map_err(|e| FsError::Message(format!("ar read: {e}")))?;
            return Ok(buf);
        }
        std::io::copy(&mut entry, &mut std::io::sink()).ok();
    }
    Err(FsError::Message(format!(
        "member not found in deb: {member_name}"
    )))
}

fn open_tar_from_deb_member(
    archive_path: &Path,
    member_name: &str,
) -> FsResult<(TarCompression, Vec<u8>)> {
    let buf = read_ar_member(archive_path, member_name)?;
    let kind = detect_tar_compression(member_name);
    Ok((kind, buf))
}

pub fn list_dir(
    archive_path: &Path,
    vfs_root: &Path,
    inner: &Path,
    show_hidden: bool,
) -> FsResult<Vec<DirEntry>> {
    if inner.as_os_str().is_empty() {
        // Root of .deb: list ar members
        return crate::arfs::list_dir(archive_path, vfs_root, inner, show_hidden);
    }
    // Entering a member inside the AR; first component is the tar member name
    let mut comps = inner.components();
    let first = comps
        .next()
        .ok_or_else(|| FsError::Message("invalid deb inner path".into()))?;
    let first_name = first.as_os_str().to_string_lossy().to_string();
    let rest: PathBuf = comps.map(|c| PathBuf::from(c.as_os_str())).collect();
    // Open tar member from AR then delegate listing
    let (kind, buf) = open_tar_from_deb_member(archive_path, &first_name)?;
    match kind {
        TarCompression::Plain => {
            list_tar_from_reader(Cursor::new(buf), &rest, vfs_root, show_hidden)
        }
        TarCompression::Gzip => {
            let dec = GzDecoder::new(Cursor::new(buf));
            list_tar_from_reader(dec, &rest, vfs_root, show_hidden)
        }
        TarCompression::Xz => {
            let dec = XzDecoder::new(Cursor::new(buf));
            list_tar_from_reader(dec, &rest, vfs_root, show_hidden)
        }
    }
}

pub fn read_file(archive_path: &Path, inner_full: &Path) -> FsResult<Box<dyn Read + Send>> {
    // If single component -> plain ar file read
    let mut comps = inner_full.components();
    let first = comps
        .next()
        .ok_or_else(|| FsError::Message("invalid deb inner path for read_file".into()))?;
    let first_name = first.as_os_str().to_string_lossy().to_string();
    let rest: PathBuf = comps.map(|c| PathBuf::from(c.as_os_str())).collect();
    if rest.as_os_str().is_empty() {
        // Read the ar member as a file
        let buf = read_ar_member(archive_path, &first_name)?;
        return Ok(Box::new(Cursor::new(buf)));
    }
    // Else treat first_name as tar member name
    let (kind, buf) = open_tar_from_deb_member(archive_path, &first_name)?;
    match kind {
        TarCompression::Plain => read_tar_file_from_reader(Cursor::new(buf), &rest),
        TarCompression::Gzip => read_tar_file_from_reader(GzDecoder::new(Cursor::new(buf)), &rest),
        TarCompression::Xz => read_tar_file_from_reader(XzDecoder::new(Cursor::new(buf)), &rest),
    }
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
        });
    }
    let mut comps = inner_full.components();
    let first = comps
        .next()
        .ok_or_else(|| FsError::Message("invalid deb inner path for stat".into()))?;
    let first_name = first.as_os_str().to_string_lossy().to_string();
    let rest: PathBuf = comps.map(|c| PathBuf::from(c.as_os_str())).collect();
    if rest.as_os_str().is_empty() {
        // Stat the ar member (file at root)
        let f = File::open(archive_path)?;
        let mut ar = ar::Archive::new(f);
        while let Some(entry_res) = ar.next_entry() {
            let entry = entry_res.map_err(|e| FsError::Message(format!("ar entry: {e}")))?;
            let ident = String::from_utf8_lossy(entry.header().identifier()).to_string();
            let name = ident.trim_matches(char::from(0)).trim().to_string();
            if name == first_name {
                let size = entry.header().size();
                let modified = UNIX_EPOCH
                    + Duration::from_secs(std::cmp::max(0, entry.header().mtime()) as u64);
                return Ok(Metadata {
                    is_dir: false,
                    is_symlink: false,
                    is_executable: (entry.header().mode() & 0o111) != 0,
                    size,
                    modified,
                    permissions: entry.header().mode() & 0o7777,
                    owner: None,
                    group: None,
                    nlink: 1,
                });
            }
        }
        return Err(FsError::Message(format!(
            "Path not found in deb: {}",
            inner_full.display()
        )));
    }
    // Stat inside tar member
    let (kind, buf) = open_tar_from_deb_member(archive_path, &first_name)?;
    match kind {
        TarCompression::Plain => stat_tar_from_reader(Cursor::new(buf), &rest),
        TarCompression::Gzip => stat_tar_from_reader(GzDecoder::new(Cursor::new(buf)), &rest),
        TarCompression::Xz => stat_tar_from_reader(XzDecoder::new(Cursor::new(buf)), &rest),
    }
}

pub fn copy_out(archive_path: &Path, src_inner: &Path, dst: &Path) -> FsResult<()> {
    let mut comps = src_inner.components();
    let first = comps
        .next()
        .ok_or_else(|| FsError::Message("invalid deb inner path for copy_out".into()))?;
    let first_name = first.as_os_str().to_string_lossy().to_string();
    let rest: PathBuf = comps.map(|c| PathBuf::from(c.as_os_str())).collect();
    if rest.as_os_str().is_empty() {
        // Copy AR member out as a file
        let f = File::open(archive_path)?;
        let mut ar = ar::Archive::new(f);
        while let Some(entry_res) = ar.next_entry() {
            let mut entry = entry_res.map_err(|e| FsError::Message(format!("ar entry: {e}")))?;
            let ident = String::from_utf8_lossy(entry.header().identifier()).to_string();
            let name = ident.trim_matches(char::from(0)).trim().to_string();
            if name == first_name {
                if let Some(parent) = dst.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let mut out = std::fs::File::create(dst)?;
                std::io::copy(&mut entry, &mut out)?;
                return Ok(());
            }
            std::io::copy(&mut entry, &mut std::io::sink()).ok();
        }
        return Err(FsError::Message(format!(
            "Source not found in deb: {}",
            src_inner.display()
        )));
    }
    // Copy from inside tar member
    let (kind, buf) = open_tar_from_deb_member(archive_path, &first_name)?;
    match kind {
        TarCompression::Plain => copy_out_tar_from_reader(Cursor::new(buf), &rest, dst),
        TarCompression::Gzip => {
            let dec = GzDecoder::new(Cursor::new(buf));
            copy_out_tar_from_reader(dec, &rest, dst)
        }
        TarCompression::Xz => {
            let dec = XzDecoder::new(Cursor::new(buf));
            copy_out_tar_from_reader(dec, &rest, dst)
        }
    }
}
