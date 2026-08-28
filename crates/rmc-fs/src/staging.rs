//! Temp-file writer that uploads on drop (remote `write_file`).
use crate::{FsError, FsResult};
use std::io::{self, Write};
use std::path::Path;
use tempfile::NamedTempFile;

/// GNU mc(1) Error dialog class for writes into a read-only VFS
/// (tar/zip/cpio/… browse+extract; extfs list+copy-out). Public manuals
/// describe tarfs as read-only; the second line matches EROFS wording.
pub fn readonly_fs_error(cannot: &str) -> FsError {
    FsError::Message(format!("{cannot}\nRead-only file system"))
}

pub const CANNOT_CREATE_TARGET_FILE: &str = "Cannot create target file";
pub const CANNOT_CREATE_DIRECTORY: &str = "Cannot create directory";
pub const CANNOT_DELETE_FILE: &str = "Cannot delete file";
pub const CANNOT_MOVE_FILE: &str = "Cannot move file";

type UploadFn = Box<dyn FnOnce(&Path) -> FsResult<()> + Send>;

pub struct StagingWrite {
    tmp: NamedTempFile,
    upload: Option<UploadFn>,
}

impl StagingWrite {
    pub fn new(upload: impl FnOnce(&Path) -> FsResult<()> + Send + 'static) -> FsResult<Self> {
        let tmp = NamedTempFile::new().map_err(|e| FsError::Message(format!("tempfile: {e}")))?;
        Ok(Self {
            tmp,
            upload: Some(Box::new(upload)),
        })
    }
}

impl Write for StagingWrite {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.tmp.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.tmp.flush()
    }
}

impl Drop for StagingWrite {
    fn drop(&mut self) {
        let _ = self.tmp.flush();
        if let Some(upload) = self.upload.take() {
            let _ = upload(self.tmp.path());
        }
    }
}
