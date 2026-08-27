//! Local file-body copy honoring GNU mc Options → Configuration:
//! **Preallocate space** and **Use COW file cloning**.
//!
//! Remote/archive VFS backends ignore these flags (ordinary copy). Local
//! copies try clone / fallocate and soft-fail to a normal byte copy.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::Path;
use walkdir::WalkDir;

/// GNU mc Configuration flags that affect local → local file copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CopyFlags {
    /// Preallocate the whole destination size before writing. GNU default **false**.
    pub preallocate_space: bool,
    /// Attempt copy-on-write clone (`FICLONE` / `copy_file_range`). GNU default **true**.
    pub use_cow_file_cloning: bool,
}

impl Default for CopyFlags {
    fn default() -> Self {
        Self {
            preallocate_space: false,
            use_cow_file_cloning: true,
        }
    }
}

/// Copy a file or directory tree, applying [`CopyFlags`] to each regular file.
pub fn copy_path_with_flags(src: &Path, dst: &Path, flags: CopyFlags) -> io::Result<()> {
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
                copy_regular_file(e.path(), &target, flags)?;
            }
        }
        Ok(())
    } else {
        copy_regular_file(src, dst, flags)
    }
}

/// Copy one regular file. Clone and preallocate are best-effort (never a hard error).
pub fn copy_regular_file(src: &Path, dst: &Path, flags: CopyFlags) -> io::Result<()> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut src_f = File::open(src)?;
    let mut dst_f = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(dst)?;
    let total = src_f.metadata()?.len();

    if flags.use_cow_file_cloning && try_cow_clone(&src_f, &dst_f) {
        drop(dst_f);
        copy_permissions(src, dst)?;
        return Ok(());
    }

    if flags.preallocate_space {
        try_preallocate(&dst_f, total);
    }

    if flags.use_cow_file_cloning && try_copy_file_range(&src_f, &dst_f, total) {
        dst_f.flush()?;
        drop(dst_f);
        copy_permissions(src, dst)?;
        return Ok(());
    }

    io::copy(&mut src_f, &mut dst_f)?;
    dst_f.flush()?;
    drop(dst_f);
    copy_permissions(src, dst)?;
    Ok(())
}

fn copy_permissions(src: &Path, dst: &Path) -> io::Result<()> {
    let perms = fs::metadata(src)?.permissions();
    fs::set_permissions(dst, perms)
}

/// Linux `FICLONE` reflink. Returns `true` only when the destination is a clone.
/// Unsupported filesystems and all errors soft-fail (`false`).
pub fn try_cow_clone(src: &File, dst: &File) -> bool {
    try_ficlone(src, dst)
}

/// Preallocate `size` bytes on `dst` (`posix_fallocate` / `fallocate`).
/// Unsupported filesystems and all errors are ignored.
pub fn try_preallocate(dst: &File, size: u64) {
    if size == 0 {
        return;
    }
    try_posix_fallocate(dst, size);
}

#[cfg(target_os = "linux")]
fn try_ficlone(src: &File, dst: &File) -> bool {
    use std::os::unix::io::AsRawFd;
    // `FICLONE` = `_IOW(0x94, 9, int)` — clone `src` into empty `dst`.
    const FICLONE: libc::c_ulong = 0x4004_9409;
    let rc = unsafe { libc::ioctl(dst.as_raw_fd(), FICLONE, src.as_raw_fd()) };
    rc == 0
}

#[cfg(not(target_os = "linux"))]
fn try_ficlone(_src: &File, _dst: &File) -> bool {
    false
}

/// Kernel `copy_file_range` (may clone on btrfs/xfs). Soft-fail unless every byte
/// is transferred. Used only when COW cloning is enabled.
#[cfg(target_os = "linux")]
fn try_copy_file_range(src: &File, dst: &File, len: u64) -> bool {
    use std::os::unix::io::AsRawFd;
    if len == 0 {
        return true;
    }
    let mut off_in: i64 = 0;
    let mut off_out: i64 = 0;
    let mut remaining = len;
    while remaining > 0 {
        let chunk = usize::try_from(remaining).unwrap_or(usize::MAX);
        let n = unsafe {
            libc::copy_file_range(
                src.as_raw_fd(),
                &mut off_in,
                dst.as_raw_fd(),
                &mut off_out,
                chunk,
                0,
            )
        };
        if n <= 0 {
            return false;
        }
        remaining = remaining.saturating_sub(n as u64);
    }
    true
}

#[cfg(not(target_os = "linux"))]
fn try_copy_file_range(_src: &File, _dst: &File, _len: u64) -> bool {
    false
}

#[cfg(unix)]
fn try_posix_fallocate(dst: &File, size: u64) {
    use std::os::unix::io::AsRawFd;
    if size > libc::off_t::MAX as u64 {
        return;
    }
    let rc = unsafe { libc::posix_fallocate(dst.as_raw_fd(), 0, size as libc::off_t) };
    if rc == 0 {
        return;
    }
    // `posix_fallocate` is not implemented everywhere; try `fallocate(2)`.
    #[cfg(target_os = "linux")]
    {
        let rc = unsafe { libc::fallocate(dst.as_raw_fd(), 0, 0, size as libc::off_t) };
        let _ = rc;
    }
}

#[cfg(not(unix))]
fn try_posix_fallocate(_dst: &File, _size: u64) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn copy_flags_gnu_defaults() {
        let d = CopyFlags::default();
        assert!(
            !d.preallocate_space,
            "GNU mc Preallocate space defaults to false"
        );
        assert!(
            d.use_cow_file_cloning,
            "GNU mc Use COW file cloning defaults to true"
        );
    }

    #[test]
    fn copy_with_cow_off_writes_bytes() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src.bin");
        let dst = dir.path().join("dst.bin");
        let data = b"cow-off-payload-0123456789";
        fs::write(&src, data).unwrap();
        copy_path_with_flags(
            &src,
            &dst,
            CopyFlags {
                preallocate_space: false,
                use_cow_file_cloning: false,
            },
        )
        .unwrap();
        assert_eq!(fs::read(&dst).unwrap(), data);
    }

    #[test]
    fn copy_with_preallocate_on_writes_bytes() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src.bin");
        let dst = dir.path().join("dst.bin");
        let data = vec![0x5Au8; 4096];
        fs::write(&src, &data).unwrap();
        copy_path_with_flags(
            &src,
            &dst,
            CopyFlags {
                preallocate_space: true,
                use_cow_file_cloning: false,
            },
        )
        .unwrap();
        assert_eq!(fs::read(&dst).unwrap(), data);
    }

    #[test]
    fn copy_with_cow_on_still_writes_when_clone_unsupported() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src.bin");
        let dst = dir.path().join("dst.bin");
        fs::write(&src, b"clone-or-copy").unwrap();
        copy_path_with_flags(&src, &dst, CopyFlags::default()).unwrap();
        assert_eq!(fs::read(&dst).unwrap(), b"clone-or-copy");
    }

    #[test]
    fn try_cow_clone_soft_fails_without_panic() {
        let dir = tempdir().unwrap();
        let src_p = dir.path().join("s.bin");
        let dst_p = dir.path().join("d.bin");
        let mut f = File::create(&src_p).unwrap();
        f.write_all(b"abc").unwrap();
        drop(f);
        let src = File::open(&src_p).unwrap();
        let dst = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&dst_p)
            .unwrap();
        let _ = try_cow_clone(&src, &dst);
    }

    #[test]
    fn try_preallocate_soft_fails_without_panic() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("pre.bin");
        let f = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&p)
            .unwrap();
        try_preallocate(&f, 1024);
    }
}
