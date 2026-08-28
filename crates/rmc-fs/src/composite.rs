use crate::dir_cache::{DirListingCache, DEFAULT_DIR_CACHE_TIMEOUT_SECS};
use crate::extfs::{ExtfsPath, ExtfsRegistry};
use crate::local::LocalFs;
use crate::pathutil::{append_anchor, parse_archive_path, ArchiveKind};
use crate::remote;
use crate::staging::{
    readonly_fs_error, CANNOT_CREATE_DIRECTORY, CANNOT_CREATE_TARGET_FILE, CANNOT_DELETE_FILE,
    CANNOT_MOVE_FILE,
};
use crate::{DirEntry, FsError, FsResult, Metadata, Vfs};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::Instant;

/// Composite virtual filesystem that routes operations to:
/// - Local filesystem
/// - Archive files when the path contains an `archive#` anchor
#[derive(Debug)]
pub struct CompositeFs {
    local: LocalFs,
    extfs: ExtfsRegistry,
    /// Listing cache for remote / archive / extfs only (GNU mc dir timeout).
    dir_cache: Mutex<DirListingCache>,
    dir_cache_timeout_secs: AtomicU32,
}

impl CompositeFs {
    pub fn new() -> Self {
        Self {
            local: LocalFs::new(),
            extfs: ExtfsRegistry::load_default(),
            dir_cache: Mutex::new(DirListingCache::new()),
            dir_cache_timeout_secs: AtomicU32::new(DEFAULT_DIR_CACHE_TIMEOUT_SECS),
        }
    }

    fn dir_cache_lock(&self) -> std::sync::MutexGuard<'_, DirListingCache> {
        self.dir_cache.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn is_cacheable(&self, path: &Path) -> bool {
        !matches!(self.route_kind(path), Route::Local { .. })
    }

    fn invalidate_parent_listing(&self, path: &Path) {
        let parent = path.parent().unwrap_or(path);
        self.dir_cache_lock().invalidate_path(parent);
    }

    fn list_dir_uncached(&self, path: &Path, show_hidden: bool) -> FsResult<Vec<DirEntry>> {
        match self.route_kind(path) {
            Route::Local { path } => self.local.list_dir(path, show_hidden),
            Route::Archive { ap, vfs_root } => match ap.kind {
                ArchiveKind::Tar | ArchiveKind::TarGz => {
                    crate::tarfs::list_dir(&ap.archive, ap.kind, &ap.inner, vfs_root, show_hidden)
                }
                ArchiveKind::Zip => {
                    crate::zipfs::list_dir(&ap.archive, vfs_root, &ap.inner, show_hidden)
                }
                ArchiveKind::Cpio | ArchiveKind::CpioGz => {
                    crate::cpiofs::list_dir(&ap.archive, ap.kind, &ap.inner, vfs_root, show_hidden)
                }
                ArchiveKind::Ar => {
                    crate::arfs::list_dir(&ap.archive, vfs_root, &ap.inner, show_hidden)
                }
                ArchiveKind::Deb => {
                    crate::debfs::list_dir(&ap.archive, vfs_root, &ap.inner, show_hidden)
                }
                ArchiveKind::Rpm => {
                    crate::rpmfs::list_dir(&ap.archive, vfs_root, &ap.inner, show_hidden)
                }
                ArchiveKind::SevenZ => {
                    crate::sevenzfs::list_dir(&ap.archive, vfs_root, &ap.inner, show_hidden)
                }
                ArchiveKind::Iso => {
                    crate::isofs::list_dir(&ap.archive, vfs_root, &ap.inner, show_hidden)
                }
                ArchiveKind::Rar => {
                    crate::rarfs::list_dir(&ap.archive, vfs_root, &ap.inner, show_hidden)
                }
                ArchiveKind::Lha => {
                    crate::lhafs::list_dir(&ap.archive, vfs_root, &ap.inner, show_hidden)
                }
            },
            Route::Extfs { xp, vfs_root } => crate::extfs::list_dir(
                &xp.helper_cmd,
                &xp.archive,
                &xp.inner,
                vfs_root,
                show_hidden,
            ),
            Route::Remote { url } => match url.scheme {
                crate::remote::RemoteScheme::Ftp => crate::ftpfs::list_dir(&url, path, show_hidden),
                crate::remote::RemoteScheme::Sftp => {
                    crate::sftpfs::list_dir(&url, path, show_hidden)
                }
                crate::remote::RemoteScheme::Fish => crate::fish::list_dir(&url, path, show_hidden),
                _ => crate::remote::list_dir(&url, path, show_hidden),
            },
        }
    }

    fn list_dir_cached(&self, path: &Path, show_hidden: bool) -> FsResult<Vec<DirEntry>> {
        let timeout = self.dir_cache_timeout_secs.load(Ordering::Relaxed);
        let now = Instant::now();
        if timeout > 0 {
            if let Some(hit) = self
                .dir_cache_lock()
                .lookup(path, show_hidden, timeout, now)
            {
                return Ok(hit);
            }
        }
        let entries = self.list_dir_uncached(path, show_hidden)?;
        if timeout > 0 {
            self.dir_cache_lock()
                .store(path, show_hidden, entries.clone(), now);
        }
        Ok(entries)
    }

    /// True when this path is served by local disk, not archive/extfs/remote.
    /// Same classification CompositeFs uses for `copy` / `stat` routing.
    pub fn is_local_fs_path(&self, path: &Path) -> bool {
        matches!(self.route_kind(path), Route::Local { .. })
    }

    fn is_writable_route(route: &Route<'_>) -> bool {
        matches!(route, Route::Local { .. } | Route::Remote { .. })
    }

    fn archive_copy_out(ap: &crate::pathutil::ArchivePath, dst: &Path) -> FsResult<()> {
        match ap.kind {
            ArchiveKind::Tar | ArchiveKind::TarGz => {
                crate::tarfs::copy_out(&ap.archive, ap.kind, &ap.inner, dst)
            }
            ArchiveKind::Zip => crate::zipfs::copy_out(&ap.archive, &ap.inner, dst),
            ArchiveKind::Cpio | ArchiveKind::CpioGz => {
                crate::cpiofs::copy_out(&ap.archive, ap.kind, &ap.inner, dst)
            }
            ArchiveKind::Ar => crate::arfs::copy_out(&ap.archive, &ap.inner, dst),
            ArchiveKind::Deb => crate::debfs::copy_out(&ap.archive, &ap.inner, dst),
            ArchiveKind::Rpm => crate::rpmfs::copy_out(&ap.archive, &ap.inner, dst),
            ArchiveKind::SevenZ => crate::sevenzfs::copy_out(&ap.archive, &ap.inner, dst),
            ArchiveKind::Iso => crate::isofs::copy_out(&ap.archive, &ap.inner, dst),
            ArchiveKind::Rar => crate::rarfs::copy_out(&ap.archive, &ap.inner, dst),
            ArchiveKind::Lha => crate::lhafs::copy_out(&ap.archive, &ap.inner, dst),
        }
    }

    fn remote_copy_out(url: &remote::RemoteUrl, dst: &Path) -> FsResult<()> {
        match url.scheme {
            crate::remote::RemoteScheme::Ftp => crate::ftpfs::copy_out(url, dst),
            crate::remote::RemoteScheme::Sftp => crate::sftpfs::copy_out(url, dst),
            crate::remote::RemoteScheme::Fish => crate::fish::copy_out(url, dst),
            _ => crate::remote::copy_out(url, dst),
        }
    }

    fn remote_copy_in(src: &Path, url: &remote::RemoteUrl) -> FsResult<()> {
        match url.scheme {
            crate::remote::RemoteScheme::Ftp => crate::ftpfs::copy_in(src, url),
            crate::remote::RemoteScheme::Sftp => crate::sftpfs::copy_in(src, url),
            crate::remote::RemoteScheme::Fish => crate::fish::copy_in(src, url),
            _ => crate::remote::copy_in(src, url),
        }
    }

    fn remote_mkdir(url: &remote::RemoteUrl) -> FsResult<()> {
        match url.scheme {
            crate::remote::RemoteScheme::Ftp => crate::ftpfs::mkdir(url),
            crate::remote::RemoteScheme::Sftp => crate::sftpfs::mkdir(url),
            crate::remote::RemoteScheme::Fish => crate::fish::mkdir(url),
            _ => crate::remote::mkdir(url),
        }
    }

    fn remote_remove(url: &remote::RemoteUrl, recursive: bool) -> FsResult<()> {
        match url.scheme {
            crate::remote::RemoteScheme::Ftp => crate::ftpfs::remove(url, recursive),
            crate::remote::RemoteScheme::Sftp => crate::sftpfs::remove(url, recursive),
            crate::remote::RemoteScheme::Fish => crate::fish::remove(url, recursive),
            _ => crate::remote::remove(url, recursive),
        }
    }

    fn remote_rename(src: &remote::RemoteUrl, dst: &remote::RemoteUrl) -> FsResult<()> {
        match src.scheme {
            crate::remote::RemoteScheme::Ftp => crate::ftpfs::rename(src, dst),
            crate::remote::RemoteScheme::Sftp => crate::sftpfs::rename(src, dst),
            crate::remote::RemoteScheme::Fish => crate::fish::rename(src, dst),
            _ => Err(FsError::Message(
                "rename is not supported on this remote backend".into(),
            )),
        }
    }

    fn remote_write_file(url: remote::RemoteUrl) -> FsResult<Box<dyn Write + Send>> {
        match url.scheme {
            crate::remote::RemoteScheme::Ftp => crate::ftpfs::write_file(&url),
            crate::remote::RemoteScheme::Sftp => crate::sftpfs::write_file(&url),
            crate::remote::RemoteScheme::Fish => crate::fish::write_file(&url),
            _ => {
                let w = crate::remote::RemoteWrite::new(url)?;
                Ok(Box::new(w))
            }
        }
    }

    /// Copy any readable VFS file onto a local path.
    fn materialize_to_local(&self, src: &Path, dst: &Path) -> FsResult<()> {
        if let Some(parent) = dst.parent() {
            let _ = fs::create_dir_all(parent);
        }
        match self.route_kind(src) {
            Route::Local { path } => {
                fs::copy(path, dst)?;
                Ok(())
            }
            Route::Archive { ap, .. } => Self::archive_copy_out(&ap, dst),
            Route::Extfs { xp, .. } => {
                crate::extfs::copy_out(&xp.helper_cmd, &xp.archive, &xp.inner, dst)
            }
            Route::Remote { url, .. } => Self::remote_copy_out(&url, dst),
        }
    }

    /// Copy a local file into a writable VFS destination (or local).
    fn copy_local_into(&self, src: &Path, dst: &Path) -> FsResult<()> {
        match self.route_kind(dst) {
            Route::Local { path } => {
                self.local
                    .copy_with_flags(src, path, crate::CopyFlags::default())
            }
            Route::Archive { .. } | Route::Extfs { .. } => {
                Err(readonly_fs_error(CANNOT_CREATE_TARGET_FILE))
            }
            Route::Remote { url, .. } => Self::remote_copy_in(src, &url),
        }
    }

    fn copy_via_temp(&self, src: &Path, dst: &Path) -> FsResult<()> {
        let tmp = tempfile::NamedTempFile::new()
            .map_err(|e| FsError::Message(format!("tempfile: {e}")))?;
        self.materialize_to_local(src, tmp.path())?;
        self.copy_local_into(tmp.path(), dst)
    }

    fn route_kind<'a>(&self, path: &'a Path) -> Route<'a> {
        if let Some(ap) = parse_archive_path(path) {
            let vfs_root = path; // the full composite path with '#'
            Route::Archive { ap, vfs_root }
        } else if let Some(xp) = self.extfs.parse_extfs_path(path) {
            let vfs_root = path;
            Route::Extfs { xp, vfs_root }
        } else if remote::is_remote_url(path) {
            match remote::parse_remote_url(path) {
                Ok(url) => Route::Remote { url },
                Err(_) => Route::Local { path }, // fallback: treat as local invalid path
            }
        } else {
            Route::Local { path }
        }
    }
}

impl Default for CompositeFs {
    fn default() -> Self {
        Self::new()
    }
}

enum Route<'a> {
    Local {
        path: &'a Path,
    },
    Archive {
        ap: crate::pathutil::ArchivePath,
        vfs_root: &'a Path,
    },
    Extfs {
        xp: ExtfsPath,
        vfs_root: &'a Path,
    },
    Remote {
        url: remote::RemoteUrl,
    },
}

impl Vfs for CompositeFs {
    fn cwd(&self) -> FsResult<PathBuf> {
        self.local.cwd()
    }

    fn list_dir(&self, path: &Path, show_hidden: bool) -> FsResult<Vec<DirEntry>> {
        if self.is_cacheable(path) {
            self.list_dir_cached(path, show_hidden)
        } else {
            self.local.list_dir(path, show_hidden)
        }
    }

    fn set_dir_cache_timeout_secs(&self, secs: u32) {
        self.dir_cache_timeout_secs.store(secs, Ordering::Relaxed);
        if secs == 0 {
            self.dir_cache_lock().clear();
        }
    }

    fn invalidate_dir_cache(&self, path: Option<&Path>) {
        match path {
            Some(p) => self.dir_cache_lock().invalidate_path(p),
            None => self.dir_cache_lock().clear(),
        }
    }

    fn is_local_path(&self, path: &Path) -> bool {
        self.is_local_fs_path(path)
    }

    fn is_writable(&self, path: &Path) -> bool {
        Self::is_writable_route(&self.route_kind(path))
    }

    fn canonicalize_path(&self, path: &Path) -> PathBuf {
        let p = crate::ftpfs::canonicalize_panel_path(path);
        let p = crate::sftpfs::canonicalize_panel_path(&p);
        crate::fish::canonicalize_panel_path(&p)
    }

    fn enter_path(&self, path: &Path) -> Option<PathBuf> {
        // Enter on archive files by extension; disallow when already in an archive
        if parse_archive_path(path).is_some() || self.extfs.parse_extfs_path(path).is_some() {
            return None;
        }
        // Remote URLs are enterable; GNU `#ftp:` / `#sftp:` / `#sh:` so `..` leaves.
        if remote::is_remote_url(path) {
            return Some(self.canonicalize_path(path));
        }
        if crate::pathutil::detect_archive_kind(path).is_some() {
            // Ensure the file exists and is a regular file
            if let Ok(md) = fs::symlink_metadata(path) {
                if md.is_file() {
                    return Some(append_anchor(path));
                }
            }
        } else if self.extfs.match_extension(path).is_some() {
            if let Ok(md) = fs::symlink_metadata(path) {
                if md.is_file() {
                    return Some(append_anchor(path));
                }
            }
        }
        None
    }

    fn mkdir(&self, path: &Path) -> FsResult<()> {
        let cacheable = self.is_cacheable(path);
        let result = match self.route_kind(path) {
            Route::Local { path } => self.local.mkdir(path),
            Route::Archive { .. } | Route::Extfs { .. } => {
                Err(readonly_fs_error(CANNOT_CREATE_DIRECTORY))
            }
            Route::Remote { url, .. } => Self::remote_mkdir(&url),
        };
        if result.is_ok() && cacheable {
            self.invalidate_parent_listing(path);
        }
        result
    }

    fn remove(&self, path: &Path, recursive: bool) -> FsResult<()> {
        let cacheable = self.is_cacheable(path);
        let result = match self.route_kind(path) {
            Route::Local { path } => self.local.remove(path, recursive),
            Route::Archive { .. } | Route::Extfs { .. } => {
                Err(readonly_fs_error(CANNOT_DELETE_FILE))
            }
            Route::Remote { url, .. } => Self::remote_remove(&url, recursive),
        };
        if result.is_ok() && cacheable {
            self.invalidate_parent_listing(path);
        }
        result
    }

    fn copy(&self, src: &Path, dst: &Path) -> FsResult<()> {
        let src_cacheable = self.is_cacheable(src);
        let dst_cacheable = self.is_cacheable(dst);
        let result = match (self.route_kind(src), self.route_kind(dst)) {
            (Route::Local { path: s }, Route::Local { path: d }) => {
                self.local
                    .copy_with_flags(s, d, crate::CopyFlags::default())
            }
            (Route::Archive { ap, .. }, Route::Local { path: d }) => Self::archive_copy_out(&ap, d),
            (Route::Extfs { xp, .. }, Route::Local { path: d }) => {
                crate::extfs::copy_out(&xp.helper_cmd, &xp.archive, &xp.inner, d)
            }
            (Route::Remote { url, .. }, Route::Local { path: d }) => Self::remote_copy_out(&url, d),
            (Route::Local { path: s }, Route::Remote { url, .. }) => Self::remote_copy_in(s, &url),
            (_, Route::Archive { .. }) | (_, Route::Extfs { .. }) => {
                Err(readonly_fs_error(CANNOT_CREATE_TARGET_FILE))
            }
            (Route::Archive { .. }, Route::Remote { .. })
            | (Route::Extfs { .. }, Route::Remote { .. })
            | (Route::Remote { .. }, Route::Remote { .. }) => self.copy_via_temp(src, dst),
        };
        if result.is_ok() {
            if src_cacheable {
                self.invalidate_parent_listing(src);
            }
            if dst_cacheable {
                self.invalidate_parent_listing(dst);
            }
        }
        result
    }

    fn copy_with_flags(
        &self,
        src: &Path,
        dst: &Path,
        flags: crate::copy_local::CopyFlags,
    ) -> FsResult<()> {
        match (self.route_kind(src), self.route_kind(dst)) {
            (Route::Local { path: s }, Route::Local { path: d }) => {
                let src_cacheable = self.is_cacheable(src);
                let dst_cacheable = self.is_cacheable(dst);
                let result = self.local.copy_with_flags(s, d, flags);
                if result.is_ok() {
                    if src_cacheable {
                        self.invalidate_parent_listing(src);
                    }
                    if dst_cacheable {
                        self.invalidate_parent_listing(dst);
                    }
                }
                result
            }
            // Archive/remote/extfs: Configuration clone/preallocate are no-ops.
            _ => self.copy(src, dst),
        }
    }

    fn move_path(&self, src: &Path, dst: &Path) -> FsResult<()> {
        match (self.route_kind(src), self.route_kind(dst)) {
            (Route::Local { path: s }, Route::Local { path: d }) => self.local.move_path(s, d),
            (_, Route::Archive { .. }) | (_, Route::Extfs { .. }) => {
                Err(readonly_fs_error(CANNOT_MOVE_FILE))
            }
            (Route::Remote { url: su, .. }, Route::Remote { url: du, .. })
                if su.same_identity(&du) =>
            {
                let src_cacheable = self.is_cacheable(src);
                let dst_cacheable = self.is_cacheable(dst);
                let result = Self::remote_rename(&su, &du);
                if result.is_ok() {
                    if src_cacheable {
                        self.invalidate_parent_listing(src);
                    }
                    if dst_cacheable {
                        self.invalidate_parent_listing(dst);
                    }
                }
                result
            }
            _ => Err(FsError::Message(
                "move across VFS backends uses copy then delete".into(),
            )),
        }
    }

    fn read_file(&self, path: &Path) -> FsResult<Box<dyn Read + Send>> {
        match self.route_kind(path) {
            Route::Local { path } => self.local.read_file(path),
            Route::Archive { ap, .. } => match ap.kind {
                ArchiveKind::Tar | ArchiveKind::TarGz => {
                    crate::tarfs::read_file(&ap.archive, ap.kind, &ap.inner)
                }
                ArchiveKind::Zip => crate::zipfs::read_file(&ap.archive, &ap.inner),
                ArchiveKind::Cpio | ArchiveKind::CpioGz => {
                    crate::cpiofs::read_file(&ap.archive, ap.kind, &ap.inner)
                }
                ArchiveKind::Ar => crate::arfs::read_file(&ap.archive, &ap.inner),
                ArchiveKind::Deb => crate::debfs::read_file(&ap.archive, &ap.inner),
                ArchiveKind::Rpm => crate::rpmfs::read_file(&ap.archive, &ap.inner),
                ArchiveKind::SevenZ => crate::sevenzfs::read_file(&ap.archive, &ap.inner),
                ArchiveKind::Iso => crate::isofs::read_file(&ap.archive, &ap.inner),
                ArchiveKind::Rar => crate::rarfs::read_file(&ap.archive, &ap.inner),
                ArchiveKind::Lha => crate::lhafs::read_file(&ap.archive, &ap.inner),
            },
            Route::Extfs { .. } => Err(FsError::Message(
                "read_file inside extfs is not supported; use copy-out".into(),
            )),
            Route::Remote { url, .. } => match url.scheme {
                crate::remote::RemoteScheme::Ftp => crate::ftpfs::read_file(&url),
                crate::remote::RemoteScheme::Sftp => crate::sftpfs::read_file(&url),
                crate::remote::RemoteScheme::Fish => crate::fish::read_file(&url),
                _ => {
                    let f = crate::remote::read_file_to_temp(&url)?;
                    Ok(Box::new(f))
                }
            },
        }
    }

    fn write_file(&self, path: &Path) -> FsResult<Box<dyn Write + Send>> {
        let cacheable = self.is_cacheable(path);
        let result: FsResult<Box<dyn Write + Send>> = match self.route_kind(path) {
            Route::Local { path } => self.local.write_file(path),
            Route::Archive { .. } | Route::Extfs { .. } => {
                Err(readonly_fs_error(CANNOT_CREATE_TARGET_FILE))
            }
            Route::Remote { url, .. } => Self::remote_write_file(url),
        };
        if result.is_ok() && cacheable {
            self.invalidate_parent_listing(path);
        }
        result
    }

    fn stat(&self, path: &Path) -> FsResult<Metadata> {
        match self.route_kind(path) {
            Route::Local { path } => self.local.stat(path),
            Route::Archive { ap, .. } => match ap.kind {
                ArchiveKind::Tar | ArchiveKind::TarGz => {
                    crate::tarfs::stat(&ap.archive, ap.kind, &ap.inner)
                }
                ArchiveKind::Zip => crate::zipfs::stat(&ap.archive, &ap.inner),
                ArchiveKind::Cpio | ArchiveKind::CpioGz => {
                    crate::cpiofs::stat(&ap.archive, ap.kind, &ap.inner)
                }
                ArchiveKind::Ar => crate::arfs::stat(&ap.archive, &ap.inner),
                ArchiveKind::Deb => crate::debfs::stat(&ap.archive, &ap.inner),
                ArchiveKind::Rpm => crate::rpmfs::stat(&ap.archive, &ap.inner),
                ArchiveKind::SevenZ => crate::sevenzfs::stat(&ap.archive, &ap.inner),
                ArchiveKind::Iso => crate::isofs::stat(&ap.archive, &ap.inner),
                ArchiveKind::Rar => crate::rarfs::stat(&ap.archive, &ap.inner),
                ArchiveKind::Lha => crate::lhafs::stat(&ap.archive, &ap.inner),
            },
            Route::Extfs { .. } => Err(FsError::Message(
                "stat inside extfs is not supported in this minimal implementation".into(),
            )),
            Route::Remote { url } => match url.scheme {
                crate::remote::RemoteScheme::Ftp => crate::ftpfs::stat(&url),
                crate::remote::RemoteScheme::Sftp => crate::sftpfs::stat(&url),
                crate::remote::RemoteScheme::Fish => crate::fish::stat(&url),
                _ => Err(FsError::Message("stat on remote is not implemented".into())),
            },
        }
    }
    fn chmod(&self, path: &Path, mode: u32, recursive: bool) -> FsResult<()> {
        match self.route_kind(path) {
            Route::Local { path } => self.local.chmod(path, mode, recursive),
            Route::Archive { .. } | Route::Extfs { .. } => {
                Err(readonly_fs_error(CANNOT_CREATE_TARGET_FILE))
            }
            Route::Remote { .. } => {
                Err(FsError::Message("chmod on remote is not supported".into()))
            }
        }
    }
    fn chown(
        &self,
        path: &Path,
        owner: Option<&str>,
        group: Option<&str>,
        recursive: bool,
    ) -> FsResult<()> {
        match self.route_kind(path) {
            Route::Local { path } => self.local.chown(path, owner, group, recursive),
            Route::Archive { .. } | Route::Extfs { .. } => {
                Err(readonly_fs_error(CANNOT_CREATE_TARGET_FILE))
            }
            Route::Remote { .. } => {
                Err(FsError::Message("chown on remote is not supported".into()))
            }
        }
    }
    fn link_hard(&self, src: &Path, dst: &Path) -> FsResult<()> {
        match (self.route_kind(src), self.route_kind(dst)) {
            (Route::Local { path: s }, Route::Local { path: d }) => self.local.link_hard(s, d),
            _ => Err(FsError::Message(
                "hard link is only supported on the local filesystem".into(),
            )),
        }
    }
    fn symlink(&self, target: &Path, link_path: &Path) -> FsResult<()> {
        match self.route_kind(link_path) {
            Route::Local { path } => self.local.symlink(target, path),
            Route::Archive { .. } | Route::Extfs { .. } => {
                Err(readonly_fs_error(CANNOT_CREATE_TARGET_FILE))
            }
            Route::Remote { .. } => Err(FsError::Message(
                "symlink on remote is not supported".into(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn is_local_fs_path_matches_composite_routing() {
        let vfs = CompositeFs::new();
        assert!(vfs.is_local_fs_path(Path::new("/tmp/file.txt")));
        assert!(vfs.is_local_path(Path::new("/tmp/file.txt")));
        assert!(!vfs.is_local_fs_path(Path::new("/tmp/sample.tar#")));
        assert!(!vfs.is_local_fs_path(Path::new("/tmp/sample.tar#/inner.txt")));
        assert!(!vfs.is_local_fs_path(Path::new("/tmp/a.zip#/dir/f.txt")));
        assert!(!vfs.is_local_fs_path(Path::new("ftp://host/pub")));
        assert!(!vfs.is_local_fs_path(Path::new("/#ftp:host/pub")));
        assert!(!vfs.is_local_fs_path(Path::new("sftp://user@host/tmp")));
        assert!(!vfs.is_local_fs_path(Path::new("/#sftp:user@host/tmp")));
        assert!(!vfs.is_local_fs_path(Path::new("sh://host/tmp")));
        assert!(!vfs.is_local_fs_path(Path::new("/#sh:host/tmp")));
        assert!(vfs.is_writable(Path::new("/tmp/file.txt")));
        assert!(vfs.is_writable(Path::new("ftp://host/pub")));
        assert!(vfs.is_writable(Path::new("/#sftp:user@host/tmp")));
        assert!(vfs.is_writable(Path::new("sh://host/tmp")));
        assert!(!vfs.is_writable(Path::new("/tmp/sample.tar#")));
        assert!(!vfs.is_writable(Path::new("/tmp/sample.tar#/inner.txt")));
        assert!(!vfs.is_writable(Path::new("/tmp/a.zip#/dir/f.txt")));
    }

    #[test]
    fn enter_path_normalizes_ftp_url_to_hash_ftp() {
        let vfs = CompositeFs::new();
        let entered = vfs
            .enter_path(Path::new("ftp://example.com/pub"))
            .expect("ftp enterable");
        assert_eq!(entered, PathBuf::from("/#ftp:example.com/pub"));
        assert_eq!(
            vfs.canonicalize_path(Path::new("ftp://127.0.0.1:2121/")),
            PathBuf::from("/#ftp:127.0.0.1:2121")
        );
    }

    #[test]
    fn enter_path_normalizes_sftp_and_sh_urls() {
        let vfs = CompositeFs::new();
        assert_eq!(
            vfs.enter_path(Path::new("sftp://user@host:2222/tmp"))
                .expect("sftp enterable"),
            PathBuf::from("/#sftp:user@host:2222/tmp")
        );
        assert_eq!(
            vfs.enter_path(Path::new("sh://joe@somehost.ssh.edu/private"))
                .expect("sh enterable"),
            PathBuf::from("/#sh:joe@somehost.ssh.edu/private")
        );
        assert_eq!(
            vfs.canonicalize_path(Path::new("fish://user@host:C/dir")),
            PathBuf::from("/#sh:user@host:C/dir")
        );
    }
}
