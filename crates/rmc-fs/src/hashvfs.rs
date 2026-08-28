//! Shared GNU `#scheme:` panel-path helpers for remote VFS backends.
//!
//! `ftp://` / `sftp://` / `sh://` normalize to `/#ftp:` / `/#sftp:` / `/#sh:`
//! so `Path::parent` / Enter on `..` leave the remote VFS the same way archive
//! `#` paths leave a tar/zip. A local prefix such as `/tmp/#sftp:host` is kept
//! so `..` at the remote root returns there.

use crate::remote::{RemoteScheme, RemoteUrl};
use crate::{DirEntry, Metadata};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// GNU mc-style panel path (`/#tag:authority[/remote]`).
pub fn panel_path(tag: &str, authority: &str, remote_path: &str) -> PathBuf {
    let root = format!("/#{tag}:{authority}");
    let remote = remote_path.trim_end_matches('/');
    if remote.is_empty() || remote == "/" {
        PathBuf::from(root)
    } else {
        PathBuf::from(format!("{root}/{}", remote.trim_start_matches('/')))
    }
}

/// Keep an existing `#tag:` path (add a leading `/` when the marker is at 0).
pub fn keep_hash_path(path: &Path, marker: &str) -> Option<PathBuf> {
    let s = path.to_string_lossy();
    let idx = s.find(marker)?;
    if idx == 0 {
        Some(PathBuf::from(format!("/{s}")))
    } else {
        Some(path.to_path_buf())
    }
}

/// Normalize a URI of `scheme` to `/#tag:…`. Other paths are unchanged.
pub fn canonicalize_panel_path(
    path: &Path,
    tag: &str,
    marker: &str,
    scheme: RemoteScheme,
) -> PathBuf {
    if let Some(kept) = keep_hash_path(path, marker) {
        return kept;
    }
    match crate::remote::parse_remote_url(path) {
        Ok(url) if url.scheme == scheme => panel_path(tag, &url.vfs_authority(), &url.path),
        _ => path.to_path_buf(),
    }
}

pub fn is_remote_root(url: &RemoteUrl) -> bool {
    let p = url.path.trim_end_matches('/');
    p.is_empty() || p == "/" || p == "."
}

pub fn parent_marker_path(url: &RemoteUrl, vfs_root: &Path, panel: PathBuf) -> PathBuf {
    if is_remote_root(url) {
        match vfs_root.parent() {
            Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
            _ => PathBuf::from("/"),
        }
    } else {
        vfs_root
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or(panel)
    }
}

pub fn parent_marker(path: PathBuf) -> DirEntry {
    DirEntry {
        name: "..".to_string(),
        path,
        meta: dir_meta(0, SystemTime::UNIX_EPOCH),
    }
}

pub fn dir_meta(size: u64, modified: SystemTime) -> Metadata {
    Metadata {
        is_dir: true,
        is_symlink: false,
        symlink_target: None,
        is_executable: false,
        size,
        modified,
        accessed: modified,
        changed: modified,
        permissions: 0o755,
        owner: None,
        group: None,
        nlink: 1,
        inode: 0,
    }
}

pub fn file_meta(
    size: u64,
    modified: SystemTime,
    is_symlink: bool,
    target: Option<String>,
) -> Metadata {
    Metadata {
        is_dir: false,
        is_symlink,
        symlink_target: target,
        is_executable: false,
        size,
        modified,
        accessed: modified,
        changed: modified,
        permissions: 0o644,
        owner: None,
        group: None,
        nlink: 1,
        inode: 0,
    }
}

pub fn sort_entries(out: &mut [DirEntry]) {
    out.sort_by(
        |a, b| match (a.name.as_str() == "..", b.name.as_str() == "..") {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => match (a.meta.is_dir, b.meta.is_dir) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            },
        },
    );
}
