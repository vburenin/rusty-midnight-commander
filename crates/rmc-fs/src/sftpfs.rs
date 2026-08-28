//! SFTP virtual filesystem (sftpfs).
//!
//! User-visible paths follow GNU mc(1) Virtual File System / SFTP:
//! - `/#sftp:[user@]machine:[port]/[remote-dir]`
//! - `sftp://[user@]machine:[port]/[remote-dir]`
//!
//! Browse (`list_dir`), `stat`, `read_file`, and copy-out are supported.
//! Upload / write / mkdir / delete are a follow-up (CompositeFs still routes
//! those through the generic remote helpers).
//!
//! Host keys are checked against `~/.ssh/known_hosts`. Missing or mismatched
//! keys stash a [`sshconn::HostKeyPrompt`] for the GNU SFTP filesystem
//! Yes / Ignore / No dialog.

use crate::hashvfs;
use crate::remote::{RemoteScheme, RemoteUrl};
use crate::sshconn;
use crate::{DirEntry, FsError, FsResult, Metadata};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::FileAttributes;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tokio::io::AsyncReadExt;

/// GNU mc-style panel path for an SFTP location (`/#sftp:authority[/remote]`).
pub fn panel_path(url: &RemoteUrl) -> PathBuf {
    hashvfs::panel_path("sftp", &url.vfs_authority(), &url.path)
}

/// Normalize `sftp://…` to `/#sftp:…`. Existing `#sftp:` paths (including a
/// local prefix such as `/tmp/#sftp:host`) are kept.
pub fn canonicalize_panel_path(path: &Path) -> PathBuf {
    hashvfs::canonicalize_panel_path(path, "sftp", "#sftp:", RemoteScheme::Sftp)
}

struct LiveSftp {
    _ssh: russh::client::Handle<crate::sshconn::ClientHandler>,
    sftp: SftpSession,
}

async fn sftp_session(url: &RemoteUrl) -> FsResult<LiveSftp> {
    let ssh = sshconn::connect_handle(url).await?;
    let channel = ssh
        .channel_open_session()
        .await
        .map_err(|e| FsError::Message(format!("SFTP channel: {e}")))?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|e| FsError::Message(format!("SFTP subsystem: {e}")))?;
    let sftp = SftpSession::new(channel.into_stream())
        .await
        .map_err(|e| FsError::Message(format!("SFTP init: {e}")))?;
    Ok(LiveSftp { _ssh: ssh, sftp })
}

fn attrs_size(attrs: &FileAttributes) -> u64 {
    attrs.size.unwrap_or(0)
}

fn attrs_mtime(attrs: &FileAttributes) -> SystemTime {
    match attrs.mtime {
        Some(s) => SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(u64::from(s)),
        None => SystemTime::UNIX_EPOCH,
    }
}

pub fn list_dir(url: &RemoteUrl, vfs_root: &Path, show_hidden: bool) -> FsResult<Vec<DirEntry>> {
    let vfs_root = canonicalize_panel_path(vfs_root);
    sshconn::block_on(async {
        let live = sftp_session(url).await?;
        let read = live
            .sftp
            .read_dir(&url.path)
            .await
            .map_err(|e| FsError::Message(format!("SFTP readdir: {e}")))?;
        let mut out = Vec::new();
        out.push(hashvfs::parent_marker(hashvfs::parent_marker_path(
            url,
            &vfs_root,
            panel_path(url),
        )));
        for ent in read {
            let name = ent.file_name();
            if name == "." || name == ".." {
                continue;
            }
            if !show_hidden && name.starts_with('.') {
                continue;
            }
            let attrs = ent.metadata();
            let is_dir = attrs.is_dir();
            let is_link = attrs.is_symlink();
            out.push(DirEntry {
                name: name.clone(),
                path: vfs_root.join(&name),
                meta: if is_dir {
                    hashvfs::dir_meta(attrs_size(&attrs), attrs_mtime(&attrs))
                } else {
                    hashvfs::file_meta(attrs_size(&attrs), attrs_mtime(&attrs), is_link, None)
                },
            });
        }
        hashvfs::sort_entries(&mut out);
        Ok(out)
    })
}

pub fn stat(url: &RemoteUrl) -> FsResult<Metadata> {
    if hashvfs::is_remote_root(url) {
        return Ok(hashvfs::dir_meta(0, SystemTime::UNIX_EPOCH));
    }
    sshconn::block_on(async {
        let live = sftp_session(url).await?;
        let attrs = live
            .sftp
            .metadata(&url.path)
            .await
            .map_err(|e| FsError::Message(format!("SFTP stat: {e}")))?;
        if attrs.is_dir() {
            Ok(hashvfs::dir_meta(attrs_size(&attrs), attrs_mtime(&attrs)))
        } else {
            Ok(hashvfs::file_meta(
                attrs_size(&attrs),
                attrs_mtime(&attrs),
                attrs.is_symlink(),
                None,
            ))
        }
    })
}

pub fn copy_out(url: &RemoteUrl, dst: &Path) -> FsResult<()> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let bytes = sshconn::block_on(async {
        let live = sftp_session(url).await?;
        let mut file = live
            .sftp
            .open(&url.path)
            .await
            .map_err(|e| FsError::Message(format!("SFTP open: {e}")))?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)
            .await
            .map_err(|e| FsError::Message(format!("SFTP read: {e}")))?;
        Ok::<_, FsError>(buf)
    })?;
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

pub use sshconn::{
    set_host_key_action, set_known_hosts_path, take_host_key_prompt, HostKeyAction, HostKeyKind,
    HostKeyPrompt,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::parse_remote_url_str;

    #[test]
    fn panel_path_and_canonicalize() {
        let url = parse_remote_url_str("sftp://alice@example.com:2222/pub/docs").unwrap();
        assert_eq!(
            panel_path(&url),
            PathBuf::from("/#sftp:alice@example.com:2222/pub/docs")
        );
        assert_eq!(
            canonicalize_panel_path(Path::new("sftp://example.com/pub")),
            PathBuf::from("/#sftp:example.com/pub")
        );
        assert_eq!(
            canonicalize_panel_path(Path::new("/tmp/#sftp:example.com/pub")),
            PathBuf::from("/tmp/#sftp:example.com/pub")
        );
        assert_eq!(
            canonicalize_panel_path(Path::new("#sftp:example.com")),
            PathBuf::from("/#sftp:example.com")
        );
        assert_eq!(
            canonicalize_panel_path(Path::new("ftp://example.com/pub")),
            PathBuf::from("ftp://example.com/pub")
        );
    }

    #[test]
    fn parent_of_sftp_root_leaves_and_nested_stays() {
        let root_url = parse_remote_url_str("sftp://example.com/").unwrap();
        let root = panel_path(&root_url);
        assert_eq!(
            hashvfs::parent_marker_path(&root_url, &root, root.clone()),
            PathBuf::from("/")
        );
        let prefixed = PathBuf::from("/home/me/#sftp:example.com");
        assert_eq!(
            hashvfs::parent_marker_path(&root_url, &prefixed, prefixed.clone()),
            PathBuf::from("/home/me")
        );
        let nested_url = parse_remote_url_str("sftp://example.com/pub/docs").unwrap();
        let nested = panel_path(&nested_url);
        assert_eq!(
            hashvfs::parent_marker_path(&nested_url, &nested, nested.clone()),
            PathBuf::from("/#sftp:example.com/pub")
        );
    }
}
