//! FTP virtual filesystem (ftpfs).
//!
//! User-visible paths follow GNU mc(1) Virtual File System / ftpfs:
//! - `/#ftp:[!][user[:pass]@]machine[:port]/[remote-dir]`
//! - `ftp://[user[:pass]@]machine[:port]/[remote-dir]`
//!
//! Browse (`list_dir`), `stat`, `read_file`, copy-out, and write ops
//! (copy-in, mkdir, delete, rename) when the server allows.
//!
//! The `ftp` crate always uses PASV; a GNU `#ftp:!` prefix is accepted and
//! ignored. Errors are plain [`FsError`] strings for the existing error dialog.

use crate::remote::{self, RemoteScheme, RemoteUrl};
use crate::{DirEntry, FsError, FsResult, Metadata};
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const CONNECT_IO_TIMEOUT: Duration = Duration::from_secs(15);

/// GNU mc-style panel path for an FTP location (`/#ftp:authority[/remote]`).
pub fn panel_path(url: &RemoteUrl) -> PathBuf {
    let root = format!("/#ftp:{}", url.authority());
    let remote = url.path.trim_end_matches('/');
    if remote.is_empty() || remote == "/" {
        PathBuf::from(root)
    } else {
        PathBuf::from(format!("{root}/{}", remote.trim_start_matches('/')))
    }
}

/// Normalize `ftp://…` to `/#ftp:…` so `Path::parent` / `..` leave ftpfs
/// the same way archive `#` paths leave an archive. Paths that already use
/// `#ftp:` (including a local prefix such as `/tmp/#ftp:host`) are kept.
pub fn canonicalize_panel_path(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(idx) = s.find("#ftp:") {
        if idx == 0 {
            return PathBuf::from(format!("/{s}"));
        }
        return path.to_path_buf();
    }
    match remote::parse_remote_url(path) {
        Ok(url) if url.scheme == RemoteScheme::Ftp => panel_path(&url),
        _ => path.to_path_buf(),
    }
}

fn is_ftp_root(url: &RemoteUrl) -> bool {
    let p = url.path.trim_end_matches('/');
    p.is_empty() || p == "/" || p == "."
}

fn parent_marker_path(url: &RemoteUrl, vfs_root: &Path) -> PathBuf {
    if is_ftp_root(url) {
        match vfs_root.parent() {
            Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
            _ => PathBuf::from("/"),
        }
    } else {
        vfs_root
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| panel_path(url))
    }
}

fn parent_marker(path: PathBuf) -> DirEntry {
    DirEntry {
        name: "..".to_string(),
        path,
        meta: dir_meta(0, SystemTime::UNIX_EPOCH),
    }
}

fn dir_meta(size: u64, modified: SystemTime) -> Metadata {
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

fn file_meta(
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

struct FtpSession {
    inner: ftp::FtpStream,
}

impl FtpSession {
    fn connect(url: &RemoteUrl) -> FsResult<Self> {
        let proxy = remote::current_ftp_proxy();
        let (addr, login_user) = remote::ftp_connect_target(url, proxy.as_deref());
        let mut ftp = ftp::FtpStream::connect(&addr)
            .map_err(|e| FsError::Message(format!("FTP connect: {e}")))?;
        let _ = ftp.get_ref().set_read_timeout(Some(CONNECT_IO_TIMEOUT));
        let _ = ftp.get_ref().set_write_timeout(Some(CONNECT_IO_TIMEOUT));
        match &url.user {
            Some(_) => {
                let pass = url.pass.as_deref().unwrap_or("anonymous@");
                ftp.login(&login_user, pass)
                    .map_err(|e| FsError::Message(format!("FTP login: {e}")))?;
            }
            None => {
                ftp.login(&login_user, "anonymous@")
                    .map_err(|e| FsError::Message(format!("FTP anonymous login: {e}")))?;
            }
        }
        let _ = ftp.transfer_type(ftp::types::FileType::Binary);
        Ok(Self { inner: ftp })
    }

    fn list_entries(&mut self, path: &str) -> FsResult<Vec<FtpListEntry>> {
        let lines = match self.inner.list(Some(path)) {
            Ok(lines) => lines,
            Err(_) => {
                self.inner
                    .cwd(path)
                    .map_err(|e| FsError::Message(format!("FTP CWD: {e}")))?;
                self.inner
                    .list(None)
                    .map_err(|e| FsError::Message(format!("FTP LIST: {e}")))?
            }
        };
        Ok(lines.iter().filter_map(|l| parse_list_line(l)).collect())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FtpListEntry {
    name: String,
    is_dir: bool,
    is_symlink: bool,
    symlink_target: Option<String>,
    size: u64,
}

/// Parse a Unix `LIST` line (GNU ftpfs user-visible listing).
///
/// Example: `drwxr-xr-x  2 user group 4096 Jan 01 00:00 dirname`
fn parse_list_line(line: &str) -> Option<FtpListEntry> {
    let line = line.trim();
    if line.is_empty() || line.starts_with("total ") {
        return None;
    }
    let kind = line.chars().next()?;
    if kind != 'd' && kind != '-' && kind != 'l' {
        return None;
    }
    let mut parts = line.split_whitespace();
    let _perms = parts.next()?;
    let _nlink = parts.next()?;
    let _owner = parts.next()?;
    let _group = parts.next()?;
    let size: u64 = parts.next()?.parse().unwrap_or(0);
    let _month = parts.next()?;
    let _day = parts.next()?;
    let _time_or_year = parts.next()?;
    let rest: Vec<&str> = parts.collect();
    if rest.is_empty() {
        return None;
    }
    let joined = rest.join(" ");
    let (name_part, target) = match joined.split_once(" -> ") {
        Some((n, t)) => (n, Some(t.to_string())),
        None => (joined.as_str(), None),
    };
    let name = name_part.trim_end_matches('/').to_string();
    if name.is_empty() || name == "." || name == ".." {
        return None;
    }
    Some(FtpListEntry {
        name,
        is_dir: kind == 'd',
        is_symlink: kind == 'l',
        symlink_target: target,
        size,
    })
}

pub fn list_dir(url: &RemoteUrl, vfs_root: &Path, show_hidden: bool) -> FsResult<Vec<DirEntry>> {
    let vfs_root = canonicalize_panel_path(vfs_root);
    let mut session = FtpSession::connect(url)?;
    let entries = session.list_entries(&url.path)?;
    let mut out = Vec::with_capacity(entries.len() + 1);
    out.push(parent_marker(parent_marker_path(url, &vfs_root)));
    for e in entries {
        if !show_hidden && e.name.starts_with('.') {
            continue;
        }
        let child = vfs_root.join(&e.name);
        out.push(DirEntry {
            name: e.name,
            path: child,
            meta: if e.is_dir {
                dir_meta(e.size, SystemTime::UNIX_EPOCH)
            } else {
                file_meta(
                    e.size,
                    SystemTime::UNIX_EPOCH,
                    e.is_symlink,
                    e.symlink_target,
                )
            },
        });
    }
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
    Ok(out)
}

pub fn stat(url: &RemoteUrl) -> FsResult<Metadata> {
    if is_ftp_root(url) {
        return Ok(dir_meta(0, SystemTime::UNIX_EPOCH));
    }
    let mut session = FtpSession::connect(url)?;
    if let Ok(Some(sz)) = session.inner.size(&url.path) {
        let modified = mdtm_or_epoch(&mut session, &url.path);
        return Ok(file_meta(sz as u64, modified, false, None));
    }
    if session.inner.cwd(&url.path).is_ok() {
        return Ok(dir_meta(0, SystemTime::UNIX_EPOCH));
    }
    // Fall back to a parent LIST (SIZE/CWD unsupported on some servers).
    let parent = Path::new(&url.path)
        .parent()
        .map(|p| {
            let s = p.to_string_lossy();
            if s.is_empty() {
                "/".to_string()
            } else {
                s.into_owned()
            }
        })
        .unwrap_or_else(|| "/".to_string());
    let name = Path::new(&url.path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| FsError::Message(format!("FTP stat: invalid path {}", url.path)))?;
    let entries = session.list_entries(&parent)?;
    let ent = entries
        .into_iter()
        .find(|e| e.name == name)
        .ok_or_else(|| FsError::Message(format!("FTP stat: not found: {}", url.path)))?;
    Ok(if ent.is_dir {
        dir_meta(ent.size, SystemTime::UNIX_EPOCH)
    } else {
        file_meta(
            ent.size,
            SystemTime::UNIX_EPOCH,
            ent.is_symlink,
            ent.symlink_target,
        )
    })
}

fn mdtm_or_epoch(session: &mut FtpSession, path: &str) -> SystemTime {
    match session.inner.mdtm(path) {
        Ok(Some(dt)) => {
            let secs = dt.timestamp();
            if secs >= 0 {
                SystemTime::UNIX_EPOCH + Duration::from_secs(secs as u64)
            } else {
                SystemTime::UNIX_EPOCH
            }
        }
        _ => SystemTime::UNIX_EPOCH,
    }
}

pub fn copy_out(url: &RemoteUrl, dst: &Path) -> FsResult<()> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut session = FtpSession::connect(url)?;
    let cursor: Cursor<Vec<u8>> = session
        .inner
        .simple_retr(&url.path)
        .map_err(|e| FsError::Message(format!("FTP RETR: {e}")))?;
    std::fs::write(dst, cursor.into_inner())?;
    Ok(())
}

pub fn read_file(url: &RemoteUrl) -> FsResult<Box<dyn Read + Send>> {
    let tmp =
        tempfile::NamedTempFile::new().map_err(|e| FsError::Message(format!("tempfile: {e}")))?;
    let p = tmp.path().to_path_buf();
    copy_out(url, &p)?;
    let f = tmp
        .reopen()
        .map_err(|e| FsError::Message(format!("temp reopen: {e}")))?;
    drop(tmp);
    Ok(Box::new(f))
}

pub fn copy_in(src: &Path, url: &RemoteUrl) -> FsResult<()> {
    let mut f = std::fs::File::open(src)?;
    let mut session = FtpSession::connect(url)?;
    session
        .inner
        .put(&url.path, &mut f)
        .map_err(|e| FsError::Message(format!("FTP STOR: {e}")))?;
    Ok(())
}

pub fn mkdir(url: &RemoteUrl) -> FsResult<()> {
    let mut session = FtpSession::connect(url)?;
    session
        .inner
        .mkdir(&url.path)
        .map_err(|e| FsError::Message(format!("FTP MKD: {e}")))?;
    Ok(())
}

pub fn remove(url: &RemoteUrl, recursive: bool) -> FsResult<()> {
    let mut session = FtpSession::connect(url)?;
    remove_at(&mut session, &url.path, recursive)
}

fn remove_at(session: &mut FtpSession, path: &str, recursive: bool) -> FsResult<()> {
    if recursive {
        if let Ok(entries) = session.list_entries(path) {
            for e in entries {
                let child = child_remote_path(path, &e.name);
                if e.is_dir {
                    remove_at(session, &child, true)?;
                } else {
                    session
                        .inner
                        .rm(&child)
                        .map_err(|e| FsError::Message(format!("FTP DELE: {e}")))?;
                }
            }
            #[allow(deprecated)]
            session
                .inner
                .rmdir(path)
                .map_err(|e| FsError::Message(format!("FTP RMDIR: {e}")))?;
            return Ok(());
        }
    }
    match session.inner.rm(path) {
        Ok(()) => Ok(()),
        Err(_) =>
        {
            #[allow(deprecated)]
            session
                .inner
                .rmdir(path)
                .map_err(|e| FsError::Message(format!("FTP DELE: {e}")))
        }
    }
}

fn child_remote_path(parent: &str, name: &str) -> String {
    if parent.ends_with('/') {
        format!("{parent}{name}")
    } else if parent == "/" {
        format!("/{name}")
    } else {
        format!("{parent}/{name}")
    }
}

pub fn rename(src: &RemoteUrl, dst: &RemoteUrl) -> FsResult<()> {
    let mut session = FtpSession::connect(src)?;
    session
        .inner
        .rename(&src.path, &dst.path)
        .map_err(|e| FsError::Message(format!("FTP RNFR/RNTO: {e}")))?;
    Ok(())
}

pub fn write_file(url: &RemoteUrl) -> FsResult<Box<dyn std::io::Write + Send>> {
    let url = url.clone();
    Ok(Box::new(crate::staging::StagingWrite::new(move |p| {
        copy_in(p, &url)
    })?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::parse_remote_url_str;

    #[test]
    fn parse_unix_list_lines() {
        let d = parse_list_line("drwxr-xr-x  2 user group 4096 Jan 01 00:00 pub").unwrap();
        assert!(d.is_dir);
        assert_eq!(d.name, "pub");
        assert_eq!(d.size, 4096);

        let f = parse_list_line("-rw-r--r--  1 user group 5 Jan 01 2020 hello.txt").unwrap();
        assert!(!f.is_dir);
        assert_eq!(f.name, "hello.txt");
        assert_eq!(f.size, 5);

        let spaced =
            parse_list_line("-rw-r--r--  1 user group 12 Jan 01 00:00 my file.txt").unwrap();
        assert_eq!(spaced.name, "my file.txt");

        let link = parse_list_line("lrwxrwxrwx  1 user group 4 Jan 01 00:00 link -> dest").unwrap();
        assert!(link.is_symlink);
        assert_eq!(link.name, "link");
        assert_eq!(link.symlink_target.as_deref(), Some("dest"));

        assert!(parse_list_line("total 12").is_none());
        assert!(parse_list_line("drwxr-xr-x  2 u g 0 Jan 01 00:00 .").is_none());
        assert!(parse_list_line("drwxr-xr-x  2 u g 0 Jan 01 00:00 ..").is_none());
    }

    #[test]
    fn panel_path_and_canonicalize() {
        let url = parse_remote_url_str("ftp://alice:s3cret@example.com:2121/pub/docs").unwrap();
        assert_eq!(
            panel_path(&url),
            PathBuf::from("/#ftp:alice:s3cret@example.com:2121/pub/docs")
        );
        let root = parse_remote_url_str("ftp://127.0.0.1:21/").unwrap();
        assert_eq!(panel_path(&root), PathBuf::from("/#ftp:127.0.0.1:21"));

        assert_eq!(
            canonicalize_panel_path(Path::new("ftp://example.com/pub")),
            PathBuf::from("/#ftp:example.com/pub")
        );
        assert_eq!(
            canonicalize_panel_path(Path::new("/tmp/#ftp:example.com/pub")),
            PathBuf::from("/tmp/#ftp:example.com/pub")
        );
        assert_eq!(
            canonicalize_panel_path(Path::new("#ftp:example.com")),
            PathBuf::from("/#ftp:example.com")
        );
        assert_eq!(
            canonicalize_panel_path(Path::new("/local/dir")),
            PathBuf::from("/local/dir")
        );
        assert_eq!(
            canonicalize_panel_path(Path::new("sftp://user@host/tmp")),
            PathBuf::from("sftp://user@host/tmp")
        );
    }

    #[test]
    fn parent_of_ftp_root_leaves_and_nested_stays() {
        let root_url = parse_remote_url_str("ftp://example.com/").unwrap();
        let root = panel_path(&root_url);
        assert_eq!(parent_marker_path(&root_url, &root), PathBuf::from("/"));
        let prefixed = PathBuf::from("/home/me/#ftp:example.com");
        assert_eq!(
            parent_marker_path(&root_url, &prefixed),
            PathBuf::from("/home/me")
        );

        let nested_url = parse_remote_url_str("ftp://example.com/pub/docs").unwrap();
        let nested = panel_path(&nested_url);
        assert_eq!(
            parent_marker_path(&nested_url, &nested),
            PathBuf::from("/#ftp:example.com/pub")
        );
        let pub_url = parse_remote_url_str("ftp://example.com/pub").unwrap();
        let pub_path = panel_path(&pub_url);
        assert_eq!(
            parent_marker_path(&pub_url, &pub_path),
            PathBuf::from("/#ftp:example.com")
        );
    }
}
