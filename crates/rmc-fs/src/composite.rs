use crate::local::LocalFs;
use crate::pathutil::{append_anchor, parse_archive_path, ArchiveKind};
use crate::{DirEntry, FsError, FsResult, Metadata, Vfs};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Composite virtual filesystem that routes operations to:
/// - Local filesystem
/// - Archive files (tar, tar.gz, zip) when the path contains an `archive#` anchor
#[derive(Debug, Default)]
pub struct CompositeFs {
    local: LocalFs,
}

impl CompositeFs {
    pub fn new() -> Self {
        Self { local: LocalFs::new() }
    }

    fn route_kind<'a>(&self, path: &'a Path) -> Route<'a> {
        if let Some(ap) = parse_archive_path(path) {
            let vfs_root = path; // the full composite path with '#'
            Route::Archive { ap, vfs_root }
        } else {
            Route::Local { path }
        }
    }
}

enum Route<'a> {
    Local { path: &'a Path },
    Archive { ap: crate::pathutil::ArchivePath, vfs_root: &'a Path },
}

impl Vfs for CompositeFs {
    fn cwd(&self) -> FsResult<PathBuf> {
        self.local.cwd()
    }

    fn list_dir(&self, path: &Path, show_hidden: bool) -> FsResult<Vec<DirEntry>> {
        match self.route_kind(path) {
            Route::Local { path } => self.local.list_dir(path, show_hidden),
            Route::Archive { ap, vfs_root } => match ap.kind {
                ArchiveKind::Tar | ArchiveKind::TarGz => {
                    crate::tarfs::list_dir(&ap.archive, ap.kind, &ap.inner, vfs_root, show_hidden)
                }
                ArchiveKind::Zip => crate::zipfs::list_dir(&ap.archive, vfs_root, &ap.inner, show_hidden),
            },
        }
    }

    fn enter_path(&self, path: &Path) -> Option<PathBuf> {
        // Enter on archive files by extension; disallow when already in an archive
        if parse_archive_path(path).is_some() {
            return None;
        }
        if crate::pathutil::detect_archive_kind(path).is_some() {
            // Ensure the file exists and is a regular file
            if let Ok(md) = fs::symlink_metadata(path) {
                if md.is_file() {
                    return Some(append_anchor(path));
                }
            }
        }
        None
    }

    fn mkdir(&self, path: &Path) -> FsResult<()> {
        match self.route_kind(path) {
            Route::Local { path } => self.local.mkdir(path),
            Route::Archive { .. } => Err(FsError::Message("mkdir inside archive is not supported".into())),
        }
    }

    fn remove(&self, path: &Path, recursive: bool) -> FsResult<()> {
        match self.route_kind(path) {
            Route::Local { path } => self.local.remove(path, recursive),
            Route::Archive { .. } => Err(FsError::Message("remove inside archive is not supported".into())),
        }
    }

    fn copy(&self, src: &Path, dst: &Path) -> FsResult<()> {
        match (self.route_kind(src), self.route_kind(dst)) {
            (Route::Local { path: s }, Route::Local { path: d }) => self.local.copy(s, d),
            (Route::Archive { ap, .. }, Route::Local { path: d }) => match ap.kind {
                ArchiveKind::Tar | ArchiveKind::TarGz => crate::tarfs::copy_out(&ap.archive, ap.kind, &ap.inner, d),
                ArchiveKind::Zip => crate::zipfs::copy_out(&ap.archive, &ap.inner, d),
            },
            (Route::Local { .. }, Route::Archive { .. }) => {
                Err(FsError::Message("copy into an archive is not supported".into()))
            }
            (Route::Archive { .. }, Route::Archive { .. }) => {
                Err(FsError::Message("copy between archives is not supported".into()))
            }
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
                ArchiveKind::Tar | ArchiveKind::TarGz => crate::tarfs::read_file(&ap.archive, ap.kind, &ap.inner),
                ArchiveKind::Zip => crate::zipfs::read_file(&ap.archive, &ap.inner),
            },
        }
    }

    fn write_file(&self, path: &Path) -> FsResult<Box<dyn Write + Send>> {
        match self.route_kind(path) {
            Route::Local { path } => self.local.write_file(path),
            Route::Archive { .. } => Err(FsError::Message("write into an archive is not supported".into())),
        }
    }

    fn stat(&self, path: &Path) -> FsResult<Metadata> {
        match self.route_kind(path) {
            Route::Local { path } => self.local.stat(path),
            Route::Archive { ap, .. } => match ap.kind {
                ArchiveKind::Tar | ArchiveKind::TarGz => crate::tarfs::stat(&ap.archive, ap.kind, &ap.inner),
                ArchiveKind::Zip => crate::zipfs::stat(&ap.archive, &ap.inner),
            },
        }
    }
}

