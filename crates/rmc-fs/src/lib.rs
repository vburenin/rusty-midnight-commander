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
    /// Given a filesystem path, return a virtual directory path to enter if this
    /// path is an “enterable container” (e.g., an archive or remote location).
    /// Returns None when the path cannot be entered specially (regular files).
    /// Default implementation returns None to preserve existing backends.
    fn enter_path(&self, _path: &Path) -> Option<PathBuf> {
        None
    }
    fn mkdir(&self, path: &Path) -> FsResult<()>;
    fn remove(&self, path: &Path, recursive: bool) -> FsResult<()>;
    fn copy(&self, src: &Path, dst: &Path) -> FsResult<()>;
    fn move_path(&self, src: &Path, dst: &Path) -> FsResult<()>;
    fn read_file(&self, path: &Path) -> FsResult<Box<dyn Read + Send>>;
    fn write_file(&self, path: &Path) -> FsResult<Box<dyn Write + Send>>;
    fn stat(&self, path: &Path) -> FsResult<Metadata>;
    /// Change file permissions (octal mode). When `recursive` and `path` is a directory,
    /// apply to all entries under it (do not follow symlinks).
    fn chmod(&self, _path: &Path, _mode: u32, _recursive: bool) -> FsResult<()> {
        Err(FsError::Message(
            "chmod is not supported by this VFS backend".into(),
        ))
    }
    /// Change owner and/or group by name. `None` leaves field unchanged.
    /// When `recursive` and `path` is a directory, apply to all entries under it.
    fn chown(
        &self,
        _path: &Path,
        _owner: Option<&str>,
        _group: Option<&str>,
        _recursive: bool,
    ) -> FsResult<()> {
        Err(FsError::Message(
            "chown is not supported by this VFS backend".into(),
        ))
    }
    /// Create a hard link at `dst` pointing to `src`.
    fn link_hard(&self, _src: &Path, _dst: &Path) -> FsResult<()> {
        Err(FsError::Message(
            "hard link is not supported by this VFS backend".into(),
        ))
    }
    /// Create a symbolic link at `link_path` pointing to `target` (string path stored as-is).
    fn symlink(&self, _target: &Path, _link_path: &Path) -> FsResult<()> {
        Err(FsError::Message(
            "symlink is not supported by this VFS backend".into(),
        ))
    }
}

pub mod composite;
pub mod extfs;
pub mod pathutil;
pub mod remote;
pub mod tarfs;
pub mod zipfs;

pub mod local {
    use super::*;
    use std::fs::{File, OpenOptions};
    #[cfg(unix)]
    use std::os::unix::fs as unixfs;
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
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        #[cfg(unix)]
        let (owner, group) = {
            let uid = md.uid();
            let gid = md.gid();
            let uname =
                users::get_user_by_uid(uid).map(|u| u.name().to_string_lossy().into_owned());
            let gname =
                users::get_group_by_gid(gid).map(|g| g.name().to_string_lossy().into_owned());
            (uname, gname)
        };
        #[cfg(not(unix))]
        let (owner, group) = (None, None);
        Metadata {
            is_dir: md.is_dir(),
            is_symlink: md.file_type().is_symlink(),
            is_executable: is_exe,
            size: md.len(),
            modified: md.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            permissions: mode,
            owner,
            group,
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
            out.sort_by(|a, b| match (a.meta.is_dir, b.meta.is_dir) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
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
            let f = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(path)?;
            Ok(Box::new(f))
        }

        fn stat(&self, path: &Path) -> FsResult<Metadata> {
            let md = fs::symlink_metadata(path)?;
            Ok(meta_from(md))
        }
        fn chmod(&self, path: &Path, mode: u32, recursive: bool) -> FsResult<()> {
            #[cfg(unix)]
            fn chmod_one(path: &Path, mode: u32) -> FsResult<()> {
                let p = fs::symlink_metadata(path)?;
                if p.file_type().is_symlink() {
                    // Cannot chmod a symlink on Unix; skip silently
                    return Ok(());
                }
                let mut perms = p.permissions();
                perms.set_mode(mode);
                fs::set_permissions(path, perms)?;
                Ok(())
            }
            #[cfg(not(unix))]
            fn chmod_one(_path: &Path, _mode: u32) -> FsResult<()> {
                Err(FsError::Message(
                    "chmod not supported on this platform".into(),
                ))
            }
            let meta = fs::symlink_metadata(path)?;
            if meta.is_dir() && recursive {
                for e in WalkDir::new(path).into_iter() {
                    let e = e?;
                    let p = e.path();
                    chmod_one(p, mode)?;
                }
                Ok(())
            } else {
                chmod_one(path, mode)
            }
        }
        fn chown(
            &self,
            path: &Path,
            owner: Option<&str>,
            group: Option<&str>,
            recursive: bool,
        ) -> FsResult<()> {
            #[cfg(unix)]
            fn chown_one(path: &Path, uid: Option<u32>, gid: Option<u32>) -> FsResult<()> {
                use std::ffi::CString;
                use std::os::unix::ffi::OsStrExt;
                let md = fs::symlink_metadata(path)?;
                let uid_t = uid
                    .map(|u| u as libc::uid_t)
                    .unwrap_or(!0u32 as libc::uid_t);
                let gid_t = gid
                    .map(|g| g as libc::gid_t)
                    .unwrap_or(!0u32 as libc::gid_t);
                // Use lchown for symlinks to avoid following
                let c_path = CString::new(path.as_os_str().as_bytes())
                    .map_err(|e| FsError::Message(format!("invalid path: {e}")))?;
                let ret = if md.file_type().is_symlink() {
                    unsafe { libc::lchown(c_path.as_ptr(), uid_t, gid_t) }
                } else {
                    unsafe { libc::chown(c_path.as_ptr(), uid_t, gid_t) }
                };
                if ret == 0 {
                    Ok(())
                } else {
                    Err(FsError::Io(std::io::Error::last_os_error()))
                }
            }
            #[cfg(not(unix))]
            fn chown_one(_path: &Path, _uid: Option<u32>, _gid: Option<u32>) -> FsResult<()> {
                Err(FsError::Message(
                    "chown not supported on this platform".into(),
                ))
            }
            #[cfg(unix)]
            fn lookup_uid_gid(
                owner: Option<&str>,
                group: Option<&str>,
            ) -> FsResult<(Option<u32>, Option<u32>)> {
                if let Some(name) = owner {
                    if users::get_user_by_name(name).is_none() {
                        return Err(FsError::Message(format!("unknown user: {name}")));
                    }
                }
                if let Some(name) = group {
                    if users::get_group_by_name(name).is_none() {
                        return Err(FsError::Message(format!("unknown group: {name}")));
                    }
                }
                let uid = owner.and_then(|name| users::get_user_by_name(name).map(|u| u.uid()));
                let gid = group.and_then(|name| users::get_group_by_name(name).map(|g| g.gid()));
                Ok((uid, gid))
            }
            #[cfg(not(unix))]
            fn lookup_uid_gid(
                _owner: Option<&str>,
                _group: Option<&str>,
            ) -> FsResult<(Option<u32>, Option<u32>)> {
                Ok((None, None))
            }
            let (uid, gid) = lookup_uid_gid(owner, group)?;
            let meta = fs::symlink_metadata(path)?;
            if meta.is_dir() && recursive {
                for e in WalkDir::new(path).into_iter() {
                    let e = e?;
                    let p = e.path();
                    chown_one(p, uid, gid)?;
                }
                Ok(())
            } else {
                chown_one(path, uid, gid)
            }
        }
        fn link_hard(&self, src: &Path, dst: &Path) -> FsResult<()> {
            #[cfg(unix)]
            {
                if let Some(parent) = dst.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::hard_link(src, dst)?;
                Ok(())
            }
            #[cfg(not(unix))]
            {
                Err(FsError::Message(
                    "hard links not supported on this platform".into(),
                ))
            }
        }
        fn symlink(&self, target: &Path, link_path: &Path) -> FsResult<()> {
            #[cfg(unix)]
            {
                if let Some(parent) = link_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                // Select symlink_* based on whether target is a directory if it is absolute and exists.
                // For portability, create a generic symlink with unixfs::symlink which stores target path as-is.
                unixfs::symlink(target, link_path)?;
                Ok(())
            }
            #[cfg(not(unix))]
            {
                Err(FsError::Message(
                    "symlinks not supported on this platform".into(),
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
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
        assert!(!fs
            .list_dir(root, true)
            .unwrap()
            .iter()
            .any(|e| e.name == "a"));
    }

    #[test]
    fn hidden_files_filtering() {
        let fs = local::LocalFs::new();
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs.write_file(&root.join(".hidden")).unwrap();
        fs.write_file(&root.join("visible")).unwrap();
        // When show_hidden=false, dotfiles are omitted
        let list = fs.list_dir(root, false).unwrap();
        assert!(list.iter().any(|e| e.name == ".."));
        assert!(list.iter().any(|e| e.name == "visible"));
        assert!(!list.iter().any(|e| e.name == ".hidden"));
        // When show_hidden=true, include
        let list = fs.list_dir(root, true).unwrap();
        assert!(list.iter().any(|e| e.name == ".hidden"));
    }

    #[test]
    fn chmod_recursive_applies_modes() {
        let fs = local::LocalFs::new();
        let dir = tempdir().unwrap();
        let root = dir.path();
        let subdir = root.join("d");
        let file = root.join("f.txt");
        fs.mkdir(&subdir).unwrap();
        {
            let mut w = fs.write_file(&file).unwrap();
            use std::io::Write;
            writeln!(w, "data").unwrap();
        }
        // Change permissions recursively on root
        fs.chmod(root, 0o755, true).unwrap();
        let md_root = std::fs::symlink_metadata(root).unwrap();
        let md_sub = std::fs::symlink_metadata(&subdir).unwrap();
        let md_file = std::fs::symlink_metadata(&file).unwrap();
        #[cfg(unix)]
        {
            assert_eq!(md_root.permissions().mode() & 0o7777, 0o755);
            assert_eq!(md_sub.permissions().mode() & 0o7777, 0o755);
            assert_eq!(md_file.permissions().mode() & 0o7777, 0o755);
        }
    }

    #[test]
    fn chown_nochange_ok_on_tempfile() {
        let fs = local::LocalFs::new();
        let dir = tempdir().unwrap();
        let file = dir.path().join("x.txt");
        {
            let mut w = fs.write_file(&file).unwrap();
            use std::io::Write;
            writeln!(w, "data").unwrap();
        }
        // Call chown with None/None (no change) to verify it succeeds
        fs.chown(&file, None, None, false).unwrap();
        // And also on the directory recursively
        fs.chown(dir.path(), None, None, true).unwrap();
    }
}
