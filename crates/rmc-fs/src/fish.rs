//! FISH / shell virtual filesystem.
//!
//! User-visible paths follow GNU mc(1) Virtual File System / FISH:
//! - `/#sh:[user@]machine[:options]/[remote-dir]`
//! - `sh://[user@]machine[:options]/[remote-dir]`
//!
//! Options (mc(1)): `C` (compression), `r` (rsh instead of ssh), and/or a
//! numeric port. `fish://` and `#fish:` are accepted aliases and canonicalize
//! to `/#sh:…`.
//!
//! Browse (`list_dir`), `stat`, `read_file`, and copy-out are supported.
//! Upload / write / mkdir / delete are a follow-up.

use crate::hashvfs;
use crate::remote::{RemoteScheme, RemoteUrl};
use crate::sshconn;
use crate::{DirEntry, FsError, FsResult, Metadata};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// GNU mc-style panel path (`/#sh:authority[/remote]`).
pub fn panel_path(url: &RemoteUrl) -> PathBuf {
    hashvfs::panel_path("sh", &url.vfs_authority(), &url.path)
}

/// Normalize `sh://` / `fish://` / `#fish:` to `/#sh:…`. Existing `#sh:`
/// paths (including a local prefix) are kept.
pub fn canonicalize_panel_path(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(idx) = s.find("#fish:") {
        let rewritten = format!("{}#sh:{}", &s[..idx], &s[idx + "#fish:".len()..]);
        return canonicalize_panel_path(Path::new(&rewritten));
    }
    hashvfs::canonicalize_panel_path(path, "sh", "#sh:", RemoteScheme::Fish)
}

fn shell_escape(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

fn list_command(path: &str) -> String {
    format!("LC_ALL=C ls -l {}", shell_escape(path))
}

fn stat_command(path: &str) -> String {
    format!("LC_ALL=C ls -ld {}", shell_escape(path))
}

fn cat_command(path: &str) -> String {
    format!("exec cat {}", shell_escape(path))
}

/// Parse a Unix `ls -l` line (FISH listing).
fn parse_list_line(line: &str) -> Option<(String, bool, bool, Option<String>, u64)> {
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
    Some((name, kind == 'd', kind == 'l', target, size))
}

fn listing_from_ls(stdout: &str, vfs_root: &Path, show_hidden: bool) -> Vec<DirEntry> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        let Some((name, is_dir, is_link, target, size)) = parse_list_line(line) else {
            continue;
        };
        if !show_hidden && name.starts_with('.') {
            continue;
        }
        out.push(DirEntry {
            name: name.clone(),
            path: vfs_root.join(&name),
            meta: if is_dir {
                hashvfs::dir_meta(size, SystemTime::UNIX_EPOCH)
            } else {
                hashvfs::file_meta(size, SystemTime::UNIX_EPOCH, is_link, target)
            },
        });
    }
    out
}

pub fn list_dir(url: &RemoteUrl, vfs_root: &Path, show_hidden: bool) -> FsResult<Vec<DirEntry>> {
    let vfs_root = canonicalize_panel_path(vfs_root);
    let stdout = sshconn::block_on(sshconn::exec_bytes(url, &list_command(&url.path)))?;
    let text = String::from_utf8_lossy(&stdout);
    let mut out = Vec::new();
    out.push(hashvfs::parent_marker(hashvfs::parent_marker_path(
        url,
        &vfs_root,
        panel_path(url),
    )));
    out.extend(listing_from_ls(&text, &vfs_root, show_hidden));
    hashvfs::sort_entries(&mut out);
    Ok(out)
}

pub fn stat(url: &RemoteUrl) -> FsResult<Metadata> {
    if hashvfs::is_remote_root(url) {
        return Ok(hashvfs::dir_meta(0, SystemTime::UNIX_EPOCH));
    }
    let stdout = sshconn::block_on(sshconn::exec_bytes(url, &stat_command(&url.path)))?;
    let text = String::from_utf8_lossy(&stdout);
    for line in text.lines() {
        if let Some((_name, is_dir, is_link, target, size)) = parse_list_line(line) {
            return Ok(if is_dir {
                hashvfs::dir_meta(size, SystemTime::UNIX_EPOCH)
            } else {
                hashvfs::file_meta(size, SystemTime::UNIX_EPOCH, is_link, target)
            });
        }
    }
    Err(FsError::Message(format!(
        "FISH stat: not found: {}",
        url.path
    )))
}

pub fn copy_out(url: &RemoteUrl, dst: &Path) -> FsResult<()> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let bytes = sshconn::block_on(sshconn::exec_bytes(url, &cat_command(&url.path)))?;
    std::fs::write(dst, bytes)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::parse_remote_url_str;

    #[test]
    fn parse_sh_urls_and_fish_options() {
        let u = parse_remote_url_str("sh://joe@somehost.ssh.edu/private").unwrap();
        assert!(matches!(u.scheme, RemoteScheme::Fish));
        assert_eq!(u.user.as_deref(), Some("joe"));
        assert_eq!(u.host, "somehost.ssh.edu");
        assert_eq!(u.path, "/private");
        assert!(!u.compression);
        assert!(!u.use_rsh);

        let c = parse_remote_url_str("sh://user@host:C/dir").unwrap();
        assert!(c.compression);
        assert!(!c.use_rsh);
        assert!(c.port.is_none());

        let r = parse_remote_url_str("sh://user@host:r/dir").unwrap();
        assert!(r.use_rsh);
        assert!(!r.compression);

        let both = parse_remote_url_str("sh://user@host:C2222/dir").unwrap();
        assert!(both.compression);
        assert_eq!(both.port, Some(2222));

        let rc = parse_remote_url_str("sh://host:rC22/").unwrap();
        assert!(rc.use_rsh);
        assert!(rc.compression);
        assert_eq!(rc.port, Some(22));
    }

    #[test]
    fn panel_path_and_canonicalize() {
        let url = parse_remote_url_str("sh://alice@example.com:C2222/pub/docs").unwrap();
        assert_eq!(
            panel_path(&url),
            PathBuf::from("/#sh:alice@example.com:C2222/pub/docs")
        );
        assert_eq!(
            canonicalize_panel_path(Path::new("sh://example.com/pub")),
            PathBuf::from("/#sh:example.com/pub")
        );
        assert_eq!(
            canonicalize_panel_path(Path::new("fish://user@host:2222/dir")),
            PathBuf::from("/#sh:user@host:2222/dir")
        );
        assert_eq!(
            canonicalize_panel_path(Path::new("/tmp/#sh:example.com/pub")),
            PathBuf::from("/tmp/#sh:example.com/pub")
        );
        assert_eq!(
            canonicalize_panel_path(Path::new("/tmp/#fish:example.com/pub")),
            PathBuf::from("/tmp/#sh:example.com/pub")
        );
        assert_eq!(
            canonicalize_panel_path(Path::new("sftp://example.com/pub")),
            PathBuf::from("sftp://example.com/pub")
        );
    }

    #[test]
    fn parse_unix_list_lines() {
        let d = parse_list_line("drwxr-xr-x 2 user group 4096 Jan 01 00:00 pub").unwrap();
        assert!(d.1);
        assert_eq!(d.0, "pub");
        let f = parse_list_line("-rw-r--r-- 1 user group 5 Jan 01 2020 hello.txt").unwrap();
        assert!(!f.1);
        assert_eq!(f.0, "hello.txt");
        assert_eq!(f.4, 5);
        let link = parse_list_line("lrwxrwxrwx 1 user group 4 Jan 01 00:00 link -> dest").unwrap();
        assert!(link.2);
        assert_eq!(link.3.as_deref(), Some("dest"));
        assert!(parse_list_line("total 12").is_none());
    }
}
