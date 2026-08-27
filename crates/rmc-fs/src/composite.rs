use crate::dir_cache::{DirListingCache, DEFAULT_DIR_CACHE_TIMEOUT_SECS};
use crate::extfs::{ExtfsPath, ExtfsRegistry};
use crate::local::LocalFs;
use crate::pathutil::{append_anchor, parse_archive_path, ArchiveKind};
use crate::remote;
use crate::{DirEntry, FsError, FsResult, Metadata, Vfs};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::Instant;

/// Composite virtual filesystem that routes operations to:
/// - Local filesystem
/// - Archive files (tar, tar.gz, zip) when the path contains an `archive#` anchor
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
            },
            Route::Extfs { xp, vfs_root } => crate::extfs::list_dir(
                &xp.helper_cmd,
                &xp.archive,
                &xp.inner,
                vfs_root,
                show_hidden,
            ),
            Route::Remote { url } => crate::remote::list_dir(&url, path, show_hidden),
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

    fn enter_path(&self, path: &Path) -> Option<PathBuf> {
        // Enter on archive files by extension; disallow when already in an archive
        if parse_archive_path(path).is_some() || self.extfs.parse_extfs_path(path).is_some() {
            return None;
        }
        // Remote URLs are enterable as-is
        if remote::is_remote_url(path) {
            return Some(path.to_path_buf());
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
            Route::Archive { .. } => Err(FsError::Message(
                "mkdir inside archive is not supported".into(),
            )),
            Route::Extfs { .. } => Err(FsError::Message(
                "mkdir inside extfs is not supported".into(),
            )),
            Route::Remote { url, .. } => crate::remote::mkdir(&url),
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
            Route::Archive { .. } => Err(FsError::Message(
                "remove inside archive is not supported".into(),
            )),
            Route::Extfs { .. } => Err(FsError::Message(
                "remove inside extfs is not supported".into(),
            )),
            Route::Remote { url, .. } => crate::remote::remove(&url, recursive),
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
            (Route::Archive { ap, .. }, Route::Local { path: d }) => match ap.kind {
                ArchiveKind::Tar | ArchiveKind::TarGz => {
                    crate::tarfs::copy_out(&ap.archive, ap.kind, &ap.inner, d)
                }
                ArchiveKind::Zip => crate::zipfs::copy_out(&ap.archive, &ap.inner, d),
                ArchiveKind::Cpio | ArchiveKind::CpioGz => {
                    crate::cpiofs::copy_out(&ap.archive, ap.kind, &ap.inner, d)
                }
                ArchiveKind::Ar => crate::arfs::copy_out(&ap.archive, &ap.inner, d),
                ArchiveKind::Deb => crate::debfs::copy_out(&ap.archive, &ap.inner, d),
                ArchiveKind::Rpm => crate::rpmfs::copy_out(&ap.archive, &ap.inner, d),
                ArchiveKind::SevenZ => crate::sevenzfs::copy_out(&ap.archive, &ap.inner, d),
                ArchiveKind::Iso => crate::isofs::copy_out(&ap.archive, &ap.inner, d),
                ArchiveKind::Rar => crate::rarfs::copy_out(&ap.archive, &ap.inner, d),
            },
            (Route::Extfs { xp, .. }, Route::Local { path: d }) => {
                crate::extfs::copy_out(&xp.helper_cmd, &xp.archive, &xp.inner, d)
            }
            (Route::Remote { url, .. }, Route::Local { path: d }) => {
                // remote -> local
                crate::remote::copy_out(&url, d)
            }
            (Route::Local { path: s }, Route::Remote { url, .. }) => {
                // local -> remote
                crate::remote::copy_in(s, &url)
            }
            (Route::Local { .. }, Route::Archive { .. }) => Err(FsError::Message(
                "copy into an archive is not supported".into(),
            )),
            (Route::Archive { .. }, Route::Archive { .. }) => Err(FsError::Message(
                "copy between archives is not supported".into(),
            )),
            (Route::Local { .. }, Route::Extfs { .. }) => {
                Err(FsError::Message("copy into extfs is not supported".into()))
            }
            (Route::Extfs { .. }, Route::Extfs { .. }) => Err(FsError::Message(
                "copy between extfs is not supported".into(),
            )),
            _ => Err(FsError::Message(
                "copy between different VFS backends is not supported".into(),
            )),
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
            _ => Err(FsError::Message(
                "move is only supported on the local filesystem in this version".into(),
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
            },
            Route::Extfs { .. } => Err(FsError::Message(
                "read_file inside extfs is not supported; use copy-out".into(),
            )),
            Route::Remote { url, .. } => {
                let f = crate::remote::read_file_to_temp(&url)?;
                Ok(Box::new(f))
            }
        }
    }

    fn write_file(&self, path: &Path) -> FsResult<Box<dyn Write + Send>> {
        let cacheable = self.is_cacheable(path);
        let result: FsResult<Box<dyn Write + Send>> = match self.route_kind(path) {
            Route::Local { path } => self.local.write_file(path),
            Route::Archive { .. } => Err(FsError::Message(
                "write into an archive is not supported".into(),
            )),
            Route::Extfs { .. } => {
                Err(FsError::Message("write into extfs is not supported".into()))
            }
            Route::Remote { url, .. } => {
                let w = crate::remote::RemoteWrite::new(url)?;
                Ok(Box::new(w))
            }
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
            },
            Route::Extfs { .. } => Err(FsError::Message(
                "stat inside extfs is not supported in this minimal implementation".into(),
            )),
            Route::Remote { .. } => {
                Err(FsError::Message("stat on remote is not implemented".into()))
            }
        }
    }
    fn chmod(&self, path: &Path, mode: u32, recursive: bool) -> FsResult<()> {
        match self.route_kind(path) {
            Route::Local { path } => self.local.chmod(path, mode, recursive),
            Route::Archive { .. } => Err(FsError::Message(
                "chmod inside archive is not supported".into(),
            )),
            Route::Extfs { .. } => Err(FsError::Message(
                "chmod inside extfs is not supported".into(),
            )),
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
            Route::Archive { .. } => Err(FsError::Message(
                "chown inside archive is not supported".into(),
            )),
            Route::Extfs { .. } => Err(FsError::Message(
                "chown inside extfs is not supported".into(),
            )),
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
            Route::Archive { .. } => Err(FsError::Message(
                "symlink inside archive is not supported".into(),
            )),
            Route::Extfs { .. } => Err(FsError::Message(
                "symlink inside extfs is not supported".into(),
            )),
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
        assert!(!vfs.is_local_fs_path(Path::new("sftp://user@host/tmp")));
    }
}
