use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FsOp {
    Copy { src: PathBuf, dst: PathBuf },
    Move { src: PathBuf, dst: PathBuf },
    Delete { path: PathBuf, recursive: bool },
    Mkdir { path: PathBuf },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FsOpProgress {
    Started { total_bytes: u64, total_files: usize },
    FileStarted { path: PathBuf, size: u64 },
    FileProgress { path: PathBuf, bytes_copied: u64, size: u64 },
    FileDone { path: PathBuf },
    Completed,
    Error { message: String },
    Canceled,
}
