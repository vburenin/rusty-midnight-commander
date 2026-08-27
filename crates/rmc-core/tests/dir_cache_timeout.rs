//! App wiring: live VFS timeout, cwd change and C-r invalidate the listing cache.
use anyhow::Result;
use rmc_core::actions::Action;
use rmc_core::app::App;
use rmc_core::config::KeyMap;
use rmc_fs::local::LocalFs;
use rmc_fs::{DirEntry, FsResult, Metadata, Vfs};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use tempfile::tempdir;

struct ProbeFs {
    inner: LocalFs,
    timeout: Arc<AtomicU32>,
    invalidates: Arc<Mutex<Vec<Option<PathBuf>>>>,
}

impl Vfs for ProbeFs {
    fn cwd(&self) -> FsResult<PathBuf> {
        self.inner.cwd()
    }
    fn list_dir(&self, path: &Path, show_hidden: bool) -> FsResult<Vec<DirEntry>> {
        self.inner.list_dir(path, show_hidden)
    }
    fn set_dir_cache_timeout_secs(&self, secs: u32) {
        self.timeout.store(secs, Ordering::SeqCst);
    }
    fn invalidate_dir_cache(&self, path: Option<&Path>) {
        self.invalidates
            .lock()
            .unwrap()
            .push(path.map(Path::to_path_buf));
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

#[test]
fn live_timeout_is_pushed_and_refresh_and_chdir_invalidate() -> Result<()> {
    let timeout = Arc::new(AtomicU32::new(0));
    let invalidates = Arc::new(Mutex::new(Vec::new()));
    let vfs = ProbeFs {
        inner: LocalFs::new(),
        timeout: timeout.clone(),
        invalidates: invalidates.clone(),
    };
    let mut app = App::new(Box::new(vfs), KeyMap::mc_defaults())?;
    assert_eq!(
        timeout.load(Ordering::SeqCst),
        app.vfs_opts.dir_cache_timeout_secs,
        "App::new reload must apply the live timeout"
    );

    app.vfs_opts.dir_cache_timeout_secs = 42;
    app.reload_panels()?;
    assert_eq!(timeout.load(Ordering::SeqCst), 42);

    let left = app.left.cwd.clone();
    let right = app.right.cwd.clone();
    invalidates.lock().unwrap().clear();
    app.handle_action(Action::Refresh)?;
    {
        let v = invalidates.lock().unwrap();
        assert!(
            v.iter().any(|p| p.as_ref() == Some(&left)),
            "Refresh must invalidate the left panel cwd: {v:?}"
        );
        assert!(
            v.iter().any(|p| p.as_ref() == Some(&right)),
            "Refresh must invalidate the right panel cwd: {v:?}"
        );
    }

    let dir = tempdir()?;
    let dest = dir.path().to_path_buf();
    invalidates.lock().unwrap().clear();
    app.change_dir(&dest)?;
    let v = invalidates.lock().unwrap().clone();
    assert!(
        v.iter().any(|p| p.as_ref() == Some(&dest)),
        "change_dir must invalidate the target path: {v:?}"
    );
    Ok(())
}
