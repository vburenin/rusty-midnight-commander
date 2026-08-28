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
    /// Stored `readlink` text (not canonicalized). `None` if this is not a symlink
    /// or if reading the target failed. Listing and `stat` populate this for local files.
    #[serde(default)]
    pub symlink_target: Option<String>,
    pub is_executable: bool,
    pub size: u64,
    pub modified: SystemTime,
    /// Last access time (`st_atime`). Archives, remote, extfs, and `..` copy `modified`.
    pub accessed: SystemTime,
    /// Inode status-change time (`st_ctime`). Archives, remote, extfs, and `..` copy `modified`.
    pub changed: SystemTime,
    pub permissions: u32,
    pub owner: Option<String>,
    pub group: Option<String>,
    /// Hard-link count (`st_nlink`). Archives, remote VFS, and `..` markers use 1.
    pub nlink: u64,
    /// Filesystem inode (`st_ino`). Archives, remote, extfs, and `..` use 0.
    pub inode: u64,
}

/// Stored `readlink` path as text, or `None` if `path` is not a readable symlink.
pub fn read_symlink_target(path: &Path) -> Option<String> {
    fs::read_link(path)
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

/// Unix `st_nlink`, or 1 when the OS does not expose a link count.
pub(crate) fn nlink_from_std(md: &fs::Metadata) -> u64 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        md.nlink()
    }
    #[cfg(not(unix))]
    {
        let _ = md;
        1
    }
}

/// `(accessed, changed, inode)` from local `stat()`. Non-Unix: atime=ctime=`modified`, inode=0.
pub(crate) fn extra_stat_from_std(
    md: &fs::Metadata,
    modified: SystemTime,
) -> (SystemTime, SystemTime, u64) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let accessed = md.accessed().unwrap_or(modified);
        let changed = unix_ctime_to_system_time(md.ctime(), md.ctime_nsec()).unwrap_or(modified);
        (accessed, changed, md.ino())
    }
    #[cfg(not(unix))]
    {
        let _ = md;
        (modified, modified, 0)
    }
}

#[cfg(unix)]
fn unix_ctime_to_system_time(sec: i64, nsec: i64) -> Option<SystemTime> {
    use std::time::{Duration, UNIX_EPOCH};
    let nsec = u32::try_from(nsec).ok().filter(|n| *n < 1_000_000_000)?;
    if sec >= 0 {
        Some(UNIX_EPOCH + Duration::new(sec as u64, nsec))
    } else {
        UNIX_EPOCH.checked_sub(Duration::new(sec.unsigned_abs(), nsec))
    }
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
    /// Directory listing cache timeout in seconds (`0` = do not cache).
    /// Default is a no-op; [`composite::CompositeFs`] honors this for
    /// remote/archive/extfs listings (not local disk).
    fn set_dir_cache_timeout_secs(&self, _secs: u32) {}
    /// Drop cached directory listings. `None` clears the whole cache;
    /// `Some(path)` drops that directory (both hidden and non-hidden variants).
    /// Callers use this for C-r (Reload). Re-entering a directory within the
    /// timeout reuses the cache and does not invalidate.
    fn invalidate_dir_cache(&self, _path: Option<&Path>) {}
    /// Given a filesystem path, return a virtual directory path to enter if this
    /// path is an “enterable container” (e.g., an archive or remote location).
    /// Returns None when the path cannot be entered specially (regular files).
    /// Default implementation returns None to preserve existing backends.
    fn enter_path(&self, _path: &Path) -> Option<PathBuf> {
        None
    }
    /// Whether this backend treats `path` as local disk (`LocalFs`).
    ///
    /// [`composite::CompositeFs`] returns `false` for archive `#` paths,
    /// extfs anchors, and remote URLs — the same routing as `copy` / `stat`.
    /// Default is `true` (local-only backends and test doubles).
    fn is_local_path(&self, _path: &Path) -> bool {
        true
    }
    fn mkdir(&self, path: &Path) -> FsResult<()>;
    fn remove(&self, path: &Path, recursive: bool) -> FsResult<()>;
    fn copy(&self, src: &Path, dst: &Path) -> FsResult<()>;
    /// Local → local copy honoring GNU mc Preallocate space / Use COW file cloning.
    /// Default ignores flags and calls [`Vfs::copy`] (remote/archive no-ops).
    fn copy_with_flags(&self, src: &Path, dst: &Path, _flags: crate::CopyFlags) -> FsResult<()> {
        self.copy(src, dst)
    }
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

pub mod arfs;
pub mod composite;
pub mod copy_local;
pub use copy_local::{apply_dive_into_subdir, CopyFlags};
pub mod cpiofs;
pub mod debfs;
pub mod dir_cache;
pub mod extfs;
pub mod isofs;
pub mod pathutil;
pub mod rarfs;
pub mod remote;
pub mod rpmfs;
pub mod sevenzfs;
pub mod tarfs;
pub mod zipfs;

#[cfg(test)]
mod tests_archives;

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

    fn meta_from(path: &Path, md: fs::Metadata) -> Metadata {
        let mode = md.permissions().mode();
        let is_exe = !md.is_dir() && (mode & 0o111 != 0);
        let is_symlink = md.file_type().is_symlink();
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
        let modified = md.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let (accessed, changed, inode) = super::extra_stat_from_std(&md, modified);
        Metadata {
            is_dir: md.is_dir(),
            is_symlink,
            symlink_target: if is_symlink {
                super::read_symlink_target(path)
            } else {
                None
            },
            is_executable: is_exe,
            size: md.len(),
            modified,
            accessed,
            changed,
            permissions: mode,
            owner,
            group,
            nlink: super::nlink_from_std(&md),
            inode,
        }
    }

    /// `..` uses the parent directory's real `stat()` (mtime, inode size).
    fn parent_marker(parent: &Path) -> DirEntry {
        let meta = match fs::metadata(parent) {
            Ok(md) => {
                let mut m = meta_from(parent, md);
                m.is_dir = true;
                m.is_symlink = false;
                m.symlink_target = None;
                m.is_executable = false;
                m
            }
            Err(_) => Metadata {
                is_dir: true,
                is_symlink: false,
                symlink_target: None,
                is_executable: false,
                size: 0,
                modified: SystemTime::UNIX_EPOCH,
                accessed: SystemTime::UNIX_EPOCH,
                changed: SystemTime::UNIX_EPOCH,
                permissions: 0,
                owner: None,
                group: None,
                nlink: 1,
                inode: 0,
            },
        };
        DirEntry {
            name: "..".to_string(),
            path: parent.to_path_buf(),
            meta,
        }
    }

    /// `lstat` so type marks see symlinks; follow to treat symlink-to-dir as a directory.
    fn listing_meta(entry: &fs::DirEntry) -> FsResult<Metadata> {
        let path = entry.path();
        let lmd = fs::symlink_metadata(&path)?;
        let is_symlink = lmd.file_type().is_symlink();
        let mut meta = meta_from(&path, lmd);
        if is_symlink {
            match fs::metadata(&path) {
                Ok(target) => {
                    meta.is_dir = target.is_dir();
                    meta.is_symlink = true;
                }
                Err(_) => {
                    meta.is_symlink = true;
                    meta.is_dir = false;
                }
            }
        }
        Ok(meta)
    }

    impl Vfs for LocalFs {
        fn cwd(&self) -> FsResult<PathBuf> {
            Ok(std::env::current_dir()?)
        }

        fn list_dir(&self, path: &Path, show_hidden: bool) -> FsResult<Vec<DirEntry>> {
            let mut out = Vec::new();
            // Parent `..` carries the real parent-directory mtime/size (GNU Full listing).
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    out.push(parent_marker(parent));
                }
            }
            for entry in fs::read_dir(path)? {
                let entry = entry?;
                let file_name = entry.file_name();
                let name = file_name.to_string_lossy().to_string();
                if !show_hidden && name.starts_with('.') {
                    continue;
                }
                out.push(DirEntry {
                    name,
                    path: entry.path(),
                    meta: listing_meta(&entry)?,
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
            self.copy_with_flags(src, dst, crate::copy_local::CopyFlags::default())
        }

        fn copy_with_flags(
            &self,
            src: &Path,
            dst: &Path,
            flags: crate::copy_local::CopyFlags,
        ) -> FsResult<()> {
            crate::copy_local::copy_path_with_flags(src, dst, flags)?;
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
            Ok(meta_from(path, md))
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

    #[test]
    fn nlink_regular_file_is_one() {
        let fs = local::LocalFs::new();
        let dir = tempdir().unwrap();
        let file = dir.path().join("plain.txt");
        {
            let mut w = fs.write_file(&file).unwrap();
            use std::io::Write;
            writeln!(w, "data").unwrap();
        }
        assert_eq!(fs.stat(&file).unwrap().nlink, 1);
        let list = fs.list_dir(dir.path(), true).unwrap();
        let ent = list.iter().find(|e| e.name == "plain.txt").unwrap();
        assert_eq!(ent.meta.nlink, 1);
        let parent = list.iter().find(|e| e.name == "..").unwrap();
        assert!(
            parent.meta.nlink >= 1,
            "parent `..` uses the real parent directory nlink"
        );
        assert_ne!(
            parent.meta.modified,
            SystemTime::UNIX_EPOCH,
            "parent `..` mtime is the real parent directory, not epoch"
        );
        let expected = std::fs::metadata(dir.path().parent().expect("tempdir parent"))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(parent.meta.modified, expected);
    }

    #[cfg(unix)]
    #[test]
    fn nlink_hard_link_pair_is_two() {
        let fs = local::LocalFs::new();
        let dir = tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        {
            let mut w = fs.write_file(&a).unwrap();
            use std::io::Write;
            writeln!(w, "data").unwrap();
        }
        fs.link_hard(&a, &b).unwrap();
        assert_eq!(fs.stat(&a).unwrap().nlink, 2);
        assert_eq!(fs.stat(&b).unwrap().nlink, 2);
        let list = fs.list_dir(dir.path(), true).unwrap();
        for name in ["a.txt", "b.txt"] {
            let ent = list.iter().find(|e| e.name == name).unwrap();
            assert_eq!(ent.meta.nlink, 2, "{name}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn list_and_stat_populate_symlink_target() {
        let fs = local::LocalFs::new();
        let dir = tempdir().unwrap();
        let root = dir.path();
        let target_file = root.join("target.txt");
        {
            let mut w = fs.write_file(&target_file).unwrap();
            use std::io::Write;
            writeln!(w, "data").unwrap();
        }
        let file_link = root.join("filelink");
        std::os::unix::fs::symlink("target.txt", &file_link).unwrap();
        fs.mkdir(&root.join("subdir")).unwrap();
        let dir_link = root.join("dirlink");
        std::os::unix::fs::symlink("subdir", &dir_link).unwrap();
        let abs_link = root.join("abslink");
        std::os::unix::fs::symlink(&target_file, &abs_link).unwrap();
        let dangling = root.join("dangling");
        std::os::unix::fs::symlink("missing-target", &dangling).unwrap();

        let list = fs.list_dir(root, true).unwrap();
        let parent = list.iter().find(|e| e.name == "..").unwrap();
        assert!(!parent.meta.is_symlink);
        assert_eq!(parent.meta.symlink_target, None);

        let regular = list.iter().find(|e| e.name == "target.txt").unwrap();
        assert!(!regular.meta.is_symlink);
        assert_eq!(regular.meta.symlink_target, None);

        let fl = list.iter().find(|e| e.name == "filelink").unwrap();
        assert!(fl.meta.is_symlink, "file symlink");
        assert_eq!(fl.meta.symlink_target.as_deref(), Some("target.txt"));
        assert_eq!(
            fs.stat(&file_link).unwrap().symlink_target.as_deref(),
            Some("target.txt")
        );

        let dl = list.iter().find(|e| e.name == "dirlink").unwrap();
        assert!(dl.meta.is_symlink, "directory symlink");
        assert_eq!(dl.meta.symlink_target.as_deref(), Some("subdir"));
        assert_eq!(
            fs.stat(&dir_link).unwrap().symlink_target.as_deref(),
            Some("subdir")
        );

        let al = list.iter().find(|e| e.name == "abslink").unwrap();
        assert_eq!(
            al.meta.symlink_target.as_deref(),
            Some(target_file.to_str().unwrap())
        );

        let dang = list.iter().find(|e| e.name == "dangling").unwrap();
        assert!(dang.meta.is_symlink);
        assert_eq!(dang.meta.symlink_target.as_deref(), Some("missing-target"));
    }
}
