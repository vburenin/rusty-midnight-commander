//! Listing/stat populate symlink targets; App maps them into mini-status.
use anyhow::Result;
use rmc_core::app::App;
use rmc_core::config::KeyMap;
use rmc_core::panel::{format_mini_status, panel_mini_status_line};
use rmc_fs::local::LocalFs;
use rmc_fs::{DirEntry, FsResult, Metadata, Vfs};
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use tempfile::tempdir;

struct CwdFs {
    inner: LocalFs,
    cwd: PathBuf,
}

impl Vfs for CwdFs {
    fn cwd(&self) -> FsResult<PathBuf> {
        Ok(self.cwd.clone())
    }
    fn list_dir(&self, path: &Path, show_hidden: bool) -> FsResult<Vec<DirEntry>> {
        self.inner.list_dir(path, show_hidden)
    }
    fn mkdir(&self, path: &Path) -> FsResult<()> {
        self.inner.mkdir(path)
    }
    fn remove(&self, path: &Path, recursive: bool) -> FsResult<()> {
        self.inner.remove(path, recursive)
    }
    fn copy(&self, src: &Path, dst: &Path) -> FsResult<()> {
        self.inner.copy(src, dst)
    }
    fn move_path(&self, src: &Path, dst: &Path) -> FsResult<()> {
        self.inner.move_path(src, dst)
    }
    fn read_file(&self, path: &Path) -> FsResult<Box<dyn std::io::Read + Send>> {
        self.inner.read_file(path)
    }
    fn write_file(&self, path: &Path) -> FsResult<Box<dyn std::io::Write + Send>> {
        self.inner.write_file(path)
    }
    fn stat(&self, path: &Path) -> FsResult<Metadata> {
        self.inner.stat(path)
    }
}

#[cfg(unix)]
#[test]
fn app_listing_maps_symlink_target_into_mini_status() -> Result<()> {
    let dir = tempdir()?;
    let root = dir.path();
    {
        let mut w = File::create(root.join("readme.txt"))?;
        writeln!(w, "hi")?;
    }
    std::os::unix::fs::symlink("readme.txt", root.join("thelink"))?;
    std::fs::create_dir(root.join("subdir"))?;
    std::os::unix::fs::symlink("subdir", root.join("dirlink"))?;

    let mut app = App::new(
        Box::new(CwdFs {
            inner: LocalFs::new(),
            cwd: root.to_path_buf(),
        }),
        KeyMap::mc_defaults(),
    )?;
    app.panel_opts.show_mini_status = true;

    let parent = app
        .left
        .entries
        .iter()
        .find(|e| e.name == "..")
        .expect("parent marker");
    assert_eq!(format_mini_status(parent, false), "UP--DIR");

    let regular = app
        .left
        .entries
        .iter()
        .find(|e| e.name == "readme.txt")
        .expect("regular file");
    assert!(!regular.is_symlink);
    assert_eq!(regular.symlink_target, None);
    let regular_line = format_mini_status(regular, false);
    assert!(
        regular_line.starts_with("-"),
        "regular file mini-status: {regular_line:?}"
    );
    assert!(
        !regular_line.contains("->"),
        "regular file must not show a link arrow: {regular_line:?}"
    );

    let link = app
        .left
        .entries
        .iter()
        .find(|e| e.name == "thelink")
        .expect("file symlink");
    assert!(link.is_symlink);
    assert_eq!(link.symlink_target.as_deref(), Some("readme.txt"));
    assert_eq!(format_mini_status(link, false), "-> readme.txt");
    assert_eq!(
        panel_mini_status_line(true, true, None, Some(link), false).as_deref(),
        Some("-> readme.txt")
    );

    let dir_link = app
        .left
        .entries
        .iter()
        .find(|e| e.name == "dirlink")
        .expect("directory symlink");
    assert!(dir_link.is_symlink);
    assert_eq!(dir_link.symlink_target.as_deref(), Some("subdir"));
    assert_eq!(format_mini_status(dir_link, false), "-> subdir");

    app.panel_opts.show_mini_status = false;
    assert_eq!(
        panel_mini_status_line(false, true, None, Some(link), false),
        None
    );
    Ok(())
}
