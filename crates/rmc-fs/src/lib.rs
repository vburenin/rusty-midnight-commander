use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use walkdir::WalkDir;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metadata {
    pub is_dir: bool,
    pub is_symlink: bool,
    pub is_executable: bool,
    pub size: u64,
    pub modified: SystemTime,
    pub permissions: u32,
    pub owner: Option<String>,
    pub group: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirEntry {
    pub name: String,
    pub path: PathBuf,
    pub meta: Metadata,
}

#[derive(Debug, thiserror::Error)]
pub enum FsError {
    #[error("{0}")]
    Message(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("walkdir: {0}")]
    Walkdir(#[from] walkdir::Error),
}

pub type FsResult<T> = Result<T, FsError>;

pub trait Vfs: Send {
    fn cwd(&self) -> FsResult<PathBuf>;
    fn list_dir(&self, path: &Path, show_hidden: bool) -> FsResult<Vec<DirEntry>>;
    fn mkdir(&self, path: &Path) -> FsResult<()>;
    fn remove(&self, path: &Path, recursive: bool) -> FsResult<()>;
    fn copy(&self, src: &Path, dst: &Path) -> FsResult<()>;
    fn move_path(&self, src: &Path, dst: &Path) -> FsResult<()>;
    fn read_file(&self, path: &Path) -> FsResult<Box<dyn Read + Send>>;
    fn write_file(&self, path: &Path) -> FsResult<Box<dyn Write + Send>>;
    fn stat(&self, path: &Path) -> FsResult<Metadata>;
}

pub mod local {
    use super::*;
    use std::fs::{File, OpenOptions};
    use std::os::unix::fs::PermissionsExt;

    #[derive(Debug)]
    pub struct LocalFs;

    impl LocalFs {
        pub fn new() -> Self {
            Self
        }
    }
    impl Default for LocalFs {
        fn default() -> Self {
            Self::new()
        }
    }

    fn meta_from(md: fs::Metadata) -> Metadata {
        let mode = md.permissions().mode();
        let is_exe = !md.is_dir() && (mode & 0o111 != 0);
        Metadata {
            is_dir: md.is_dir(),
            is_symlink: md.file_type().is_symlink(),
            is_executable: is_exe,
            size: md.len(),
            modified: md.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            permissions: mode,
            owner: None,
            group: None,
        }
    }

    impl Vfs for LocalFs {
        fn cwd(&self) -> FsResult<PathBuf> {
            Ok(std::env::current_dir()?)
        }

        fn list_dir(&self, path: &Path, show_hidden: bool) -> FsResult<Vec<DirEntry>> {
            let mut out = Vec::new();
            // Add parent marker except root
            if let Some(parent) = path.parent() {
                out.push(DirEntry {
                    name: "..".to_string(),
                    path: parent.to_path_buf(),
                    meta: Metadata {
                        is_dir: true,
                        is_symlink: false,
                        is_executable: false,
                        size: 0,
                        modified: SystemTime::UNIX_EPOCH,
                        permissions: 0,
                        owner: None,
                        group: None,
                    },
                });
            }
            for entry in fs::read_dir(path)? {
                let entry = entry?;
                let file_name = entry.file_name();
                let name = file_name.to_string_lossy().to_string();
                if !show_hidden && name.starts_with('.') {
                    continue;
                }
                let md = entry.metadata()?;
                out.push(DirEntry {
                    name,
                    path: entry.path(),
                    meta: meta_from(md),
                });
            }
            // Directories first, then files by name
            out.sort_by(|a, b| {
                match (a.meta.is_dir, b.meta.is_dir) {
                    (true, false) => std::cmp::Ordering::Less,
                    (false, true) => std::cmp::Ordering::Greater,
                    _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                }
            });
            Ok(out)
        }

        fn mkdir(&self, path: &Path) -> FsResult<()> {
            fs::create_dir_all(path)?;
            Ok(())
        }

        fn remove(&self, path: &Path, recursive: bool) -> FsResult<()> {
            let md = fs::symlink_metadata(path)?;
            if md.is_dir() {
                if recursive {
                    fs::remove_dir_all(path)?;
                } else {
                    fs::remove_dir(path)?;
                }
            } else {
                fs::remove_file(path)?;
            }
            Ok(())
        }

        fn copy(&self, src: &Path, dst: &Path) -> FsResult<()> {
            let md = fs::symlink_metadata(src)?;
            if md.is_dir() {
                fs::create_dir_all(dst)?;
                for e in WalkDir::new(src) {
                    let e = e?;
                    let rel = e.path().strip_prefix(src).unwrap();
                    let target = dst.join(rel);
                    if e.file_type().is_dir() {
                        fs::create_dir_all(&target)?;
                    } else {
                        fs::copy(e.path(), &target)?;
                    }
                }
            } else {
                if let Some(parent) = dst.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(src, dst)?;
            }
            Ok(())
        }

        fn move_path(&self, src: &Path, dst: &Path) -> FsResult<()> {
            if let Some(parent) = dst.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::rename(src, dst)?;
            Ok(())
        }

        fn read_file(&self, path: &Path) -> FsResult<Box<dyn Read + Send>> {
            Ok(Box::new(File::open(path)?))
        }

        fn write_file(&self, path: &Path) -> FsResult<Box<dyn Write + Send>> {
            let f = OpenOptions::new().create(true).write(true).truncate(true).open(path)?;
            Ok(Box::new(f))
        }

        fn stat(&self, path: &Path) -> FsResult<Metadata> {
            let md = fs::symlink_metadata(path)?;
            Ok(meta_from(md))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn list_and_mkdir_remove() {
        let fs = local::LocalFs::new();
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs.mkdir(&root.join("a")).unwrap();
        fs.write_file(&root.join("b.txt")).unwrap();
        let list = fs.list_dir(root, true).unwrap();
        assert!(list.iter().any(|e| e.name == "a"));
        assert!(list.iter().any(|e| e.name == "b.txt"));
        fs.remove(&root.join("a"), true).unwrap();
        assert!(!fs.list_dir(root, true).unwrap().iter().any(|e| e.name == "a"));
    }
}
