//! App wiring: live VFS timeout, C-r invalidates, change_dir reuses cache.
use anyhow::Result;
use rmc_core::actions::Action;
use rmc_core::app::App;
use rmc_core::config::KeyMap;
use rmc_fs::composite::CompositeFs;
use rmc_fs::local::LocalFs;
use rmc_fs::{DirEntry, FsResult, Metadata, Vfs};
use std::fs::File;
use std::io::Write;
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

fn write_zip(path: &Path, files: &[(&str, &[u8])]) {
    let f = File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(f);
    let options = zip::write::FileOptions::default();
    for (name, data) in files {
        zip.start_file(*name, options).unwrap();
        zip.write_all(data).unwrap();
    }
    zip.finish().unwrap();
}

fn zip_anchor(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_string_lossy().to_string();
    s.push('#');
    PathBuf::from(s)
}

fn panel_names(app: &App) -> Vec<String> {
    let mut names: Vec<String> = app
        .active_panel()
        .entries
        .iter()
        .filter(|e| e.name != "..")
        .map(|e| e.name.clone())
        .collect();
    names.sort();
    names
}

#[test]
fn live_timeout_is_pushed_refresh_invalidates_chdir_does_not() -> Result<()> {
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
        !v.iter().any(|p| p.as_ref() == Some(&dest)),
        "change_dir must not invalidate the target (re-entry reuses cache): {v:?}"
    );
    Ok(())
}

#[test]
fn change_dir_reuses_archive_listing_until_refresh() -> Result<()> {
    let tmp = tempdir()?;
    let zip_path = tmp.path().join("sample.zip");
    write_zip(&zip_path, &[("a.txt", b"a")]);
    let root = zip_anchor(&zip_path);

    let mut app = App::new(Box::new(CompositeFs::new()), KeyMap::mc_defaults())?;
    app.vfs_opts.dir_cache_timeout_secs = 900;
    app.change_dir(&root)?;
    assert_eq!(panel_names(&app), ["a.txt"]);

    write_zip(&zip_path, &[("a.txt", b"a"), ("b.txt", b"b")]);
    app.change_dir(&root)?;
    assert_eq!(
        panel_names(&app),
        ["a.txt"],
        "re-entering within timeout must reuse the cached archive listing"
    );

    app.handle_action(Action::Refresh)?;
    assert_eq!(
        panel_names(&app),
        ["a.txt", "b.txt"],
        "Refresh must re-list the rewritten archive"
    );
    Ok(())
}
