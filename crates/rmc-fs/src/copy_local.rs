//! Local file-body copy honoring GNU mc Options → Configuration
//! (**Preallocate space**, **Use COW file cloning**) and Copy/Move dialog
//! flags (**Follow links**, **Preserve attributes**, **Stable symlinks**).
//!
//! Remote/archive VFS backends ignore these flags (ordinary copy). Local
//! copies try clone / fallocate and soft-fail to a normal byte copy.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use walkdir::WalkDir;

/// GNU mc flags that affect local → local Copy/Move.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CopyFlags {
    /// Preallocate the whole destination size before writing. GNU default **false**.
    pub preallocate_space: bool,
    /// Attempt copy-on-write clone (`FICLONE` / `copy_file_range`). GNU default **true**.
    pub use_cow_file_cloning: bool,
    /// Copy/Move dialog: Follow links. GNU default **false** (`cp -P`-like).
    pub follow_links: bool,
    /// Copy/Move dialog: Preserve attributes. GNU default **true**.
    pub preserve_attrs: bool,
    /// Copy/Move dialog: Dive into subdir if exists. GNU default **false**.
    pub dive_into_subdir: bool,
    /// Copy/Move dialog: Stable symlinks. GNU default **false**.
    pub stable_symlinks: bool,
}

impl Default for CopyFlags {
    fn default() -> Self {
        Self {
            preallocate_space: false,
            use_cow_file_cloning: true,
            follow_links: false,
            preserve_attrs: true,
            dive_into_subdir: false,
            stable_symlinks: false,
        }
    }
}

/// GNU mc(1) “Dive into subdir if exists” dest adjustment after source-mask
/// resolution.
///
/// When the source is a directory and the typed destination already exists as
/// a directory, off copies the source *contents* into that directory; on
/// copies the source directory *itself* into it (`/foo` → existing `/bla/foo`
/// becomes `/bla/foo/foo/...`).
pub fn apply_dive_into_subdir(
    src: &Path,
    mask_dst: PathBuf,
    dest_typed: &Path,
    dest_typed_is_dir: bool,
    src_is_dir: bool,
    mask_dst_is_dir: bool,
    dive: bool,
) -> PathBuf {
    if !src_is_dir {
        return mask_dst;
    }
    let src_name = src.file_name();
    let dest_same_name = dest_typed.file_name() == src_name;
    if dive {
        if dest_typed_is_dir && dest_same_name {
            mask_dst
        } else if mask_dst_is_dir {
            match src_name {
                Some(n) => mask_dst.join(n),
                None => mask_dst,
            }
        } else {
            mask_dst
        }
    } else if dest_typed_is_dir && dest_same_name {
        dest_typed.to_path_buf()
    } else {
        mask_dst
    }
}

/// Copy a file or directory tree, applying [`CopyFlags`] to each entry.
pub fn copy_path_with_flags(src: &Path, dst: &Path, flags: CopyFlags) -> io::Result<()> {
    let mut hardlinks = HashMap::new();
    copy_one(src, dst, flags, &mut hardlinks)
}

fn copy_one(
    src: &Path,
    dst: &Path,
    flags: CopyFlags,
    hardlinks: &mut HashMap<(u64, u64), PathBuf>,
) -> io::Result<()> {
    let lmd = fs::symlink_metadata(src)?;
    if lmd.file_type().is_symlink() && !flags.follow_links {
        copy_symlink(src, dst, flags.stable_symlinks)?;
        if flags.preserve_attrs {
            preserve_attrs(src, dst)?;
        }
        return Ok(());
    }
    let md = if flags.follow_links {
        fs::metadata(src).unwrap_or(lmd)
    } else {
        lmd
    };
    if md.is_dir() {
        copy_dir(src, dst, flags, hardlinks)
    } else {
        maybe_hardlink_or_copy_file(src, dst, flags, hardlinks, &md)
    }
}

fn copy_dir(
    src: &Path,
    dst: &Path,
    flags: CopyFlags,
    hardlinks: &mut HashMap<(u64, u64), PathBuf>,
) -> io::Result<()> {
    fs::create_dir_all(dst)?;
    let mut walker = WalkDir::new(src);
    if flags.follow_links {
        walker = walker.follow_links(true);
    }
    let mut dirs = Vec::new();
    for e in walker {
        let e = e?;
        let rel = e.path().strip_prefix(src).unwrap();
        if rel.as_os_str().is_empty() {
            dirs.push((src.to_path_buf(), dst.to_path_buf()));
            continue;
        }
        let target = dst.join(rel);
        if e.file_type().is_symlink() {
            copy_symlink(e.path(), &target, flags.stable_symlinks)?;
            if flags.preserve_attrs {
                preserve_attrs(e.path(), &target)?;
            }
        } else if e.file_type().is_dir() {
            fs::create_dir_all(&target)?;
            dirs.push((e.path().to_path_buf(), target));
        } else {
            let md = e.metadata()?;
            maybe_hardlink_or_copy_file(e.path(), &target, flags, hardlinks, &md)?;
        }
    }
    if flags.preserve_attrs {
        for (s, d) in dirs.into_iter().rev() {
            preserve_attrs(&s, &d)?;
        }
    }
    Ok(())
}

fn maybe_hardlink_or_copy_file(
    src: &Path,
    dst: &Path,
    flags: CopyFlags,
    hardlinks: &mut HashMap<(u64, u64), PathBuf>,
    md: &fs::Metadata,
) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if !flags.follow_links && md.nlink() > 1 {
            let key = (md.dev(), md.ino());
            if let Some(prev) = hardlinks.get(&key) {
                if let Some(parent) = dst.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::hard_link(prev, dst)?;
                return Ok(());
            }
            copy_regular_file(src, dst, flags)?;
            hardlinks.insert(key, dst.to_path_buf());
            return Ok(());
        }
    }
    let _ = md;
    copy_regular_file(src, dst, flags)
}

/// Recreate `src` as a symlink at `dst`. With `stable`, rewrite a relative
/// target so it still resolves to the original referent.
pub fn copy_symlink(src: &Path, dst: &Path, stable: bool) -> io::Result<()> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    let stored = fs::read_link(src)?;
    let target = if stable {
        stable_symlink_target(src, dst, &stored)
    } else {
        stored
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let _ = fs::remove_file(dst);
        symlink(target, dst)
    }
    #[cfg(not(unix))]
    {
        let _ = target;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "symlink copy is not supported on this platform",
        ))
    }
}

/// Recompute a relative symlink target so `dst_link` points at the same
/// location `src_link` did. Absolute targets are left unchanged.
pub fn stable_symlink_target(src_link: &Path, dst_link: &Path, stored: &Path) -> PathBuf {
    if stored.is_absolute() {
        return stored.to_path_buf();
    }
    let src_dir = src_link.parent().unwrap_or(Path::new("."));
    let referent = normalize_path(&src_dir.join(stored));
    let dst_dir = dst_link.parent().unwrap_or(Path::new("."));
    shortest_relative(dst_dir, &referent)
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

fn shortest_relative(from_dir: &Path, to: &Path) -> PathBuf {
    let from = normalize_path(from_dir);
    let to = normalize_path(to);
    let from_c: Vec<_> = from.components().collect();
    let to_c: Vec<_> = to.components().collect();
    let mut i = 0usize;
    while i < from_c.len() && i < to_c.len() && from_c[i] == to_c[i] {
        i += 1;
    }
    let mut out = PathBuf::new();
    for _ in i..from_c.len() {
        out.push("..");
    }
    for c in &to_c[i..] {
        out.push(c.as_os_str());
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

/// After a same-filesystem rename of `original_src` to `new_dst`, rewrite
/// relative symlinks under `new_dst` as GNU Stable symlinks would.
pub fn rewrite_stable_symlinks_after_move(original_src: &Path, new_dst: &Path) -> io::Result<()> {
    let md = fs::symlink_metadata(new_dst)?;
    if md.file_type().is_symlink() {
        let stored = fs::read_link(new_dst)?;
        let target = stable_symlink_target(original_src, new_dst, &stored);
        if target != stored {
            let _ = fs::remove_file(new_dst);
            #[cfg(unix)]
            {
                use std::os::unix::fs::symlink;
                symlink(target, new_dst)?;
            }
        }
        return Ok(());
    }
    if !md.is_dir() {
        return Ok(());
    }
    for e in WalkDir::new(new_dst) {
        let e = e?;
        if !e.file_type().is_symlink() {
            continue;
        }
        let rel = e.path().strip_prefix(new_dst).unwrap();
        let original_link = original_src.join(rel);
        let stored = fs::read_link(e.path())?;
        let target = stable_symlink_target(&original_link, e.path(), &stored);
        if target != stored {
            let _ = fs::remove_file(e.path());
            #[cfg(unix)]
            {
                use std::os::unix::fs::symlink;
                symlink(target, e.path())?;
            }
        }
    }
    Ok(())
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
        if flags.preserve_attrs {
            preserve_attrs(src, dst)?;
        }
        return Ok(());
    }

    if flags.preallocate_space {
        try_preallocate(&dst_f, total);
    }

    if flags.use_cow_file_cloning && try_copy_file_range(&src_f, &dst_f, total) {
        dst_f.flush()?;
        drop(dst_f);
        if flags.preserve_attrs {
            preserve_attrs(src, dst)?;
        }
        return Ok(());
    }

    io::copy(&mut src_f, &mut dst_f)?;
    dst_f.flush()?;
    drop(dst_f);
    if flags.preserve_attrs {
        preserve_attrs(src, dst)?;
    }
    Ok(())
}

/// Preserve mode, timestamps, and (when permitted) ownership from `src` onto `dst`.
pub fn preserve_attrs(src: &Path, dst: &Path) -> io::Result<()> {
    let src_md = fs::symlink_metadata(src)?;
    if src_md.file_type().is_symlink() {
        preserve_symlink_times(src, dst, &src_md);
        preserve_ownership(dst, &src_md);
        return Ok(());
    }
    fs::set_permissions(dst, src_md.permissions())?;
    set_path_times(dst, &src_md, false)?;
    preserve_ownership(dst, &src_md);
    Ok(())
}

fn preserve_symlink_times(src: &Path, dst: &Path, src_md: &fs::Metadata) {
    let _ = (src, set_path_times(dst, src_md, true));
}

fn preserve_ownership(dst: &Path, src_md: &fs::Metadata) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{lchown, MetadataExt};
        let _ = lchown(dst, Some(src_md.uid()), Some(src_md.gid()));
    }
    #[cfg(not(unix))]
    {
        let _ = (dst, src_md);
    }
}

fn set_path_times(path: &Path, src_md: &fs::Metadata, nofollow: bool) -> io::Result<()> {
    let atime = src_md.accessed().ok();
    let mtime = src_md.modified().ok();
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let Some(times) = timespec_pair(atime, mtime) else {
            return Ok(());
        };
        let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "path contains interior NUL")
        })?;
        let flags = if nofollow {
            libc::AT_SYMLINK_NOFOLLOW
        } else {
            0
        };
        let rc = unsafe { libc::utimensat(libc::AT_FDCWD, c_path.as_ptr(), times.as_ptr(), flags) };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (path, atime, nofollow);
        if let Some(mtime) = mtime {
            OpenOptions::new()
                .write(true)
                .open(path)?
                .set_modified(mtime)?;
        }
        Ok(())
    }
}

#[cfg(unix)]
fn timespec_pair(
    atime: Option<std::time::SystemTime>,
    mtime: Option<std::time::SystemTime>,
) -> Option<[libc::timespec; 2]> {
    Some([to_timespec(atime)?, to_timespec(mtime)?])
}

#[cfg(unix)]
fn to_timespec(t: Option<std::time::SystemTime>) -> Option<libc::timespec> {
    use std::time::UNIX_EPOCH;
    let t = t?;
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => Some(libc::timespec {
            tv_sec: d.as_secs() as libc::time_t,
            tv_nsec: d.subsec_nanos() as libc::c_long,
        }),
        Err(e) => {
            let d = e.duration();
            Some(libc::timespec {
                tv_sec: -(d.as_secs() as libc::time_t),
                tv_nsec: d.subsec_nanos() as libc::c_long,
            })
        }
    }
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
        assert!(!d.follow_links, "GNU mc Follow links defaults to false");
        assert!(
            d.preserve_attrs,
            "GNU mc Preserve attributes defaults to true"
        );
        assert!(
            !d.dive_into_subdir,
            "GNU mc Dive into subdir if exists defaults to false"
        );
        assert!(
            !d.stable_symlinks,
            "GNU mc Stable symlinks defaults to false"
        );
    }

    fn flags_no_cow() -> CopyFlags {
        CopyFlags {
            use_cow_file_cloning: false,
            ..CopyFlags::default()
        }
    }

    #[test]
    fn copy_with_cow_off_writes_bytes() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src.bin");
        let dst = dir.path().join("dst.bin");
        let data = b"cow-off-payload-0123456789";
        fs::write(&src, data).unwrap();
        copy_path_with_flags(&src, &dst, flags_no_cow()).unwrap();
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
                ..CopyFlags::default()
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

    #[cfg(unix)]
    #[test]
    fn follow_links_off_copies_symlink_as_symlink() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("target.txt");
        let src = dir.path().join("link");
        let dst = dir.path().join("copied");
        fs::write(&target, b"payload").unwrap();
        std::os::unix::fs::symlink(&target, &src).unwrap();
        copy_path_with_flags(&src, &dst, flags_no_cow()).unwrap();
        assert!(fs::symlink_metadata(&dst).unwrap().file_type().is_symlink());
        assert_eq!(fs::read_link(&dst).unwrap(), target);
        assert_eq!(fs::read(&dst).unwrap(), b"payload");
    }

    #[cfg(unix)]
    #[test]
    fn follow_links_on_copies_referent_content() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("target.txt");
        let src = dir.path().join("link");
        let dst = dir.path().join("copied");
        fs::write(&target, b"payload").unwrap();
        std::os::unix::fs::symlink(&target, &src).unwrap();
        copy_path_with_flags(
            &src,
            &dst,
            CopyFlags {
                follow_links: true,
                use_cow_file_cloning: false,
                ..CopyFlags::default()
            },
        )
        .unwrap();
        assert!(!fs::symlink_metadata(&dst).unwrap().file_type().is_symlink());
        assert_eq!(fs::read(&dst).unwrap(), b"payload");
    }

    #[cfg(unix)]
    #[test]
    fn follow_links_tree_mixed_links() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("tree");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("real.txt"), b"real").unwrap();
        std::os::unix::fs::symlink("real.txt", src.join("rel")).unwrap();
        let outside = dir.path().join("outside.txt");
        fs::write(&outside, b"out").unwrap();
        std::os::unix::fs::symlink(&outside, src.join("abs")).unwrap();

        let dst_off = dir.path().join("off");
        copy_path_with_flags(&src, &dst_off, flags_no_cow()).unwrap();
        assert!(fs::symlink_metadata(dst_off.join("rel"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::read_link(dst_off.join("rel")).unwrap(),
            Path::new("real.txt")
        );

        let dst_on = dir.path().join("on");
        copy_path_with_flags(
            &src,
            &dst_on,
            CopyFlags {
                follow_links: true,
                use_cow_file_cloning: false,
                ..CopyFlags::default()
            },
        )
        .unwrap();
        assert!(!fs::symlink_metadata(dst_on.join("rel"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read(dst_on.join("rel")).unwrap(), b"real");
        assert_eq!(fs::read(dst_on.join("abs")).unwrap(), b"out");
    }

    #[cfg(unix)]
    #[test]
    fn preserve_attrs_on_keeps_mode_and_mtime() {
        use std::os::unix::fs::PermissionsExt;
        use std::time::{Duration, UNIX_EPOCH};
        let dir = tempdir().unwrap();
        let src = dir.path().join("src.bin");
        let dst = dir.path().join("dst.bin");
        fs::write(&src, b"x").unwrap();
        fs::set_permissions(&src, fs::Permissions::from_mode(0o707)).unwrap();
        let old = UNIX_EPOCH + Duration::from_secs(1_600_000_000);
        File::open(&src).unwrap().set_modified(old).unwrap();
        copy_path_with_flags(&src, &dst, flags_no_cow()).unwrap();
        let md = fs::metadata(&dst).unwrap();
        assert_eq!(md.permissions().mode() & 0o777, 0o707);
        let mtime = md.modified().unwrap();
        let delta = mtime.duration_since(old).unwrap_or_else(|e| e.duration());
        assert!(
            delta < Duration::from_secs(2),
            "mtime preserved, delta={delta:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn preserve_attrs_off_respects_umask_not_source_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let src = dir.path().join("src.bin");
        let dst = dir.path().join("dst.bin");
        fs::write(&src, b"x").unwrap();
        fs::set_permissions(&src, fs::Permissions::from_mode(0o707)).unwrap();
        copy_path_with_flags(
            &src,
            &dst,
            CopyFlags {
                preserve_attrs: false,
                use_cow_file_cloning: false,
                ..CopyFlags::default()
            },
        )
        .unwrap();
        let mode = fs::metadata(&dst).unwrap().permissions().mode() & 0o777;
        assert_ne!(
            mode, 0o707,
            "umask dest must not force source mode {mode:#o}"
        );
    }

    #[test]
    fn dive_off_same_name_existing_dir_merges_contents() {
        let src = PathBuf::from("/foo");
        let dest = PathBuf::from("/bla/foo");
        let mask_dst = dest.join("foo");
        let out = apply_dive_into_subdir(&src, mask_dst, &dest, true, true, true, false);
        assert_eq!(out, PathBuf::from("/bla/foo"));
    }

    #[test]
    fn dive_on_same_name_existing_dir_nests_source() {
        let src = PathBuf::from("/foo");
        let dest = PathBuf::from("/bla/foo");
        let mask_dst = dest.join("foo");
        let out = apply_dive_into_subdir(&src, mask_dst, &dest, true, true, true, true);
        assert_eq!(out, PathBuf::from("/bla/foo/foo"));
    }

    #[test]
    fn dive_on_parent_dest_when_resolved_dir_exists_nests() {
        let src = PathBuf::from("/foo");
        let dest = PathBuf::from("/bla");
        let mask_dst = dest.join("foo");
        let out = apply_dive_into_subdir(&src, mask_dst, &dest, true, true, true, true);
        assert_eq!(out, PathBuf::from("/bla/foo/foo"));
    }

    #[test]
    fn dive_off_parent_dest_keeps_resolved_dir() {
        let src = PathBuf::from("/foo");
        let dest = PathBuf::from("/bla");
        let mask_dst = dest.join("foo");
        let out = apply_dive_into_subdir(&src, mask_dst, &dest, true, true, true, false);
        assert_eq!(out, PathBuf::from("/bla/foo"));
    }

    #[cfg(unix)]
    #[test]
    fn stable_symlinks_rewrites_relative_target() {
        let dir = tempdir().unwrap();
        let src_dir = dir.path().join("src").join("sub");
        fs::create_dir_all(&src_dir).unwrap();
        let referent = dir.path().join("src").join("file.txt");
        fs::write(&referent, b"hi").unwrap();
        let src_link = src_dir.join("link");
        std::os::unix::fs::symlink(Path::new("../file.txt"), &src_link).unwrap();

        let dst_link = dir.path().join("dst").join("deeper").join("link");
        copy_symlink(&src_link, &dst_link, true).unwrap();
        let stored = fs::read_link(&dst_link).unwrap();
        assert!(
            !stored.is_absolute(),
            "stable rewrite stays relative: {stored:?}"
        );
        let resolved = fs::canonicalize(&dst_link).unwrap();
        assert_eq!(resolved, fs::canonicalize(&referent).unwrap());
        assert_ne!(stored, Path::new("../file.txt"));
    }

    #[cfg(unix)]
    #[test]
    fn stable_symlinks_off_keeps_relative_text() {
        let dir = tempdir().unwrap();
        let src_dir = dir.path().join("src").join("sub");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(dir.path().join("src").join("file.txt"), b"hi").unwrap();
        let src_link = src_dir.join("link");
        std::os::unix::fs::symlink(Path::new("../file.txt"), &src_link).unwrap();
        let dst_link = dir.path().join("dst").join("deeper").join("link");
        copy_symlink(&src_link, &dst_link, false).unwrap();
        assert_eq!(fs::read_link(&dst_link).unwrap(), Path::new("../file.txt"));
    }

    #[cfg(unix)]
    #[test]
    fn stable_symlinks_leaves_absolute_unchanged() {
        let dir = tempdir().unwrap();
        let abs = dir.path().join("target.txt");
        fs::write(&abs, b"x").unwrap();
        let src = dir.path().join("link");
        std::os::unix::fs::symlink(&abs, &src).unwrap();
        let dst = dir.path().join("other").join("link");
        copy_symlink(&src, &dst, true).unwrap();
        assert_eq!(fs::read_link(&dst).unwrap(), abs);
    }
}
