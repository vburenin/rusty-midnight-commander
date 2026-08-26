//! Remote VFS backends (FTP/SFTP) — documented stubs.
//!
//! These backends are defined to match the common MC VFS surface, but this
//! repository does not spin up live servers during testing. The implementations
//! therefore return an explanatory error for operations requiring a network.
//! The trait is fully wired so offline unit tests can verify resolution and
//! error reporting paths without external dependencies.
use crate::{DirEntry, FsError, FsResult, Metadata, Vfs};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Default)]
pub struct FtpFs;

#[derive(Debug, Default)]
pub struct SftpFs;

fn offline_err() -> FsError {
    FsError::Message(
        "Remote VFS (FTP/SFTP) is not available in offline tests; configure live servers for integration testing."
            .into(),
    )
}

impl Vfs for FtpFs {
    fn cwd(&self) -> FsResult<PathBuf> {
        Err(offline_err())
    }
    fn list_dir(&self, _path: &Path, _show_hidden: bool) -> FsResult<Vec<DirEntry>> {
        Err(offline_err())
    }
    fn mkdir(&self, _path: &Path) -> FsResult<()> {
        Err(offline_err())
    }
    fn remove(&self, _path: &Path, _recursive: bool) -> FsResult<()> {
        Err(offline_err())
    }
    fn copy(&self, _src: &Path, _dst: &Path) -> FsResult<()> {
        Err(offline_err())
    }
    fn move_path(&self, _src: &Path, _dst: &Path) -> FsResult<()> {
        Err(offline_err())
    }
    fn read_file(&self, _path: &Path) -> FsResult<Box<dyn Read + Send>> {
        Err(offline_err())
    }
    fn write_file(&self, _path: &Path) -> FsResult<Box<dyn Write + Send>> {
        Err(offline_err())
    }
    fn stat(&self, _path: &Path) -> FsResult<Metadata> {
        Err(offline_err())
    }
    fn enter_path(&self, _path: &Path) -> Option<PathBuf> {
        None
    }
}

impl Vfs for SftpFs {
    fn cwd(&self) -> FsResult<PathBuf> {
        Err(offline_err())
    }
    fn list_dir(&self, _path: &Path, _show_hidden: bool) -> FsResult<Vec<DirEntry>> {
        Err(offline_err())
    }
    fn mkdir(&self, _path: &Path) -> FsResult<()> {
        Err(offline_err())
    }
    fn remove(&self, _path: &Path, _recursive: bool) -> FsResult<()> {
        Err(offline_err())
    }
    fn copy(&self, _src: &Path, _dst: &Path) -> FsResult<()> {
        Err(offline_err())
    }
    fn move_path(&self, _src: &Path, _dst: &Path) -> FsResult<()> {
        Err(offline_err())
    }
    fn read_file(&self, _path: &Path) -> FsResult<Box<dyn Read + Send>> {
        Err(offline_err())
    }
    fn write_file(&self, _path: &Path) -> FsResult<Box<dyn Write + Send>> {
        Err(offline_err())
    }
    fn stat(&self, _path: &Path) -> FsResult<Metadata> {
        Err(offline_err())
    }
    fn enter_path(&self, _path: &Path) -> Option<PathBuf> {
        None
    }
}

/// Helper to recognize remote URLs; currently supports `ftp://` and `sftp://`.
pub fn is_remote_url(path: &Path) -> bool {
    let s = path.as_os_str().to_string_lossy();
    s.starts_with("ftp://") || s.starts_with("sftp://")
}

