//! Fast reload: reuse a local listing when the directory stamp is unchanged.
//! Remote/archive Directory cache timeout is a separate option (see dir_cache_timeout.rs).
use anyhow::Result;
use rmc_core::actions::Action;
use rmc_core::app::{App, PanelOptions};
use rmc_core::config::KeyMap;
use rmc_core::panel::DirReloadStamp;
use rmc_fs::local::LocalFs;
use rmc_fs::{DirEntry, FsResult, Metadata, Vfs};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tempfile::tempdir;

struct CountingFs {
    inner: LocalFs,
    cwd: PathBuf,
    lists: Arc<Mutex<Vec<PathBuf>>>,
}

impl Vfs for CountingFs {
    fn cwd(&self) -> FsResult<PathBuf> {
        Ok(self.cwd.clone())
    }
    fn list_dir(&self, path: &Path, show_hidden: bool) -> FsResult<Vec<DirEntry>> {
        self.lists.lock().unwrap().push(path.to_path_buf());
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

fn app_in(dir: &Path, lists: &Arc<Mutex<Vec<PathBuf>>>) -> Result<App> {
    let vfs = CountingFs {
        inner: LocalFs::new(),
        cwd: dir.to_path_buf(),
        lists: lists.clone(),
    };
    App::new(Box::new(vfs), KeyMap::mc_defaults())
}

fn list_count(lists: &Arc<Mutex<Vec<PathBuf>>>, dir: &Path) -> usize {
    lists
        .lock()
        .unwrap()
        .iter()
        .filter(|p| p.as_path() == dir)
        .count()
}

fn names(app: &App) -> Vec<String> {
    let mut n: Vec<String> = app
        .left
        .entries
        .iter()
        .filter(|e| e.name != "..")
        .map(|e| e.name.clone())
        .collect();
    n.sort();
    n
}

fn seed_dir(dir: &Path) -> Result<()> {
    File::create(dir.join("alpha.txt"))?;
    Ok(())
}

/// Create `name` and ensure the directory Fast-reload stamp changes.
/// Overlay/lazytime often keeps dir mtime/ctime/nlink/size the same after a
/// same-second create; GNU mc Fast reload keys off that stamp, so bump mtime
/// the way a conventional local disk would.
fn create_file_changing_dir_stamp(dir: &Path, name: &str) -> Result<()> {
    let before = DirReloadStamp::from_local_dir(dir);
    File::create(dir.join(name))?;
    if DirReloadStamp::from_local_dir(dir) == before {
        let status = std::process::Command::new("touch").arg(dir).status()?;
        anyhow::ensure!(status.success(), "touch {dir:?} failed: {status}");
    }
    anyhow::ensure!(
        DirReloadStamp::from_local_dir(dir) != before,
        "directory stamp must change after creating {name}"
    );
    Ok(())
}

#[test]
fn default_fast_reload_is_off() {
    assert!(
        !PanelOptions::default().fast_reload,
        "GNU mc Fast reload defaults to off"
    );
}

#[test]
fn fast_reload_on_skips_when_mtime_unchanged_and_relists_after_create() -> Result<()> {
    let tmp = tempdir()?;
    let dir = tmp.path();
    seed_dir(dir)?;
    let lists = Arc::new(Mutex::new(Vec::new()));
    let mut app = app_in(dir, &lists)?;
    app.panel_opts.fast_reload = true;
    assert_eq!(names(&app), ["alpha.txt"]);

    lists.lock().unwrap().clear();
    app.reload_panels()?;
    assert_eq!(
        list_count(&lists, dir),
        0,
        "Fast reload ON must skip list_dir when the directory stamp is unchanged"
    );
    assert_eq!(names(&app), ["alpha.txt"]);

    create_file_changing_dir_stamp(dir, "beta.txt")?;
    lists.lock().unwrap().clear();
    app.reload_panels()?;
    assert!(
        list_count(&lists, dir) > 0,
        "creating a file changes dir mtime, so Fast reload must re-list"
    );
    assert_eq!(names(&app), ["alpha.txt", "beta.txt"]);
    Ok(())
}

#[test]
fn fast_reload_off_always_relists() -> Result<()> {
    let tmp = tempdir()?;
    let dir = tmp.path();
    seed_dir(dir)?;
    let lists = Arc::new(Mutex::new(Vec::new()));
    let mut app = app_in(dir, &lists)?;
    app.panel_opts.fast_reload = false;

    lists.lock().unwrap().clear();
    app.reload_panels()?;
    assert!(
        list_count(&lists, dir) > 0,
        "Fast reload OFF must re-list even when the directory is unchanged"
    );
    Ok(())
}

#[test]
fn refresh_always_relists_when_fast_reload_on() -> Result<()> {
    let tmp = tempdir()?;
    let dir = tmp.path();
    seed_dir(dir)?;
    let lists = Arc::new(Mutex::new(Vec::new()));
    let mut app = app_in(dir, &lists)?;
    app.panel_opts.fast_reload = true;

    lists.lock().unwrap().clear();
    app.handle_action(Action::Refresh)?;
    assert!(
        list_count(&lists, dir) > 0,
        "C-r / Refresh must force-relist even when Fast reload is ON"
    );
    assert_eq!(names(&app), ["alpha.txt"]);
    Ok(())
}
