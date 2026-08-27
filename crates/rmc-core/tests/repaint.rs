//! GNU mc(1) C-l full screen repaint is not C-r Refresh.
use anyhow::Result;
use rmc_core::actions::Action;
use rmc_core::app::App;
use rmc_core::config::KeyMap;
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
    invalidates: Arc<Mutex<Vec<Option<PathBuf>>>>,
}

impl Vfs for CountingFs {
    fn cwd(&self) -> FsResult<PathBuf> {
        Ok(self.cwd.clone())
    }
    fn list_dir(&self, path: &Path, show_hidden: bool) -> FsResult<Vec<DirEntry>> {
        self.lists.lock().unwrap().push(path.to_path_buf());
        self.inner.list_dir(path, show_hidden)
    }
    fn invalidate_dir_cache(&self, path: Option<&Path>) {
        self.invalidates
            .lock()
            .unwrap()
            .push(path.map(Path::to_path_buf));
        self.inner.invalidate_dir_cache(path);
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

fn app_in(
    dir: &Path,
    lists: &Arc<Mutex<Vec<PathBuf>>>,
    invalidates: &Arc<Mutex<Vec<Option<PathBuf>>>>,
) -> Result<App> {
    let vfs = CountingFs {
        inner: LocalFs::new(),
        cwd: dir.to_path_buf(),
        lists: lists.clone(),
        invalidates: invalidates.clone(),
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

fn marked(app: &App) -> Vec<usize> {
    app.active_panel().selection.iter().collect()
}

fn seed_dir(dir: &Path) -> Result<()> {
    File::create(dir.join("alpha.txt"))?;
    File::create(dir.join("beta.txt"))?;
    Ok(())
}

#[test]
fn repaint_does_not_relist_or_invalidate_and_preserves_panel_state() -> Result<()> {
    let tmp = tempdir()?;
    let dir = tmp.path();
    seed_dir(dir)?;
    let lists = Arc::new(Mutex::new(Vec::new()));
    let invalidates = Arc::new(Mutex::new(Vec::new()));
    let mut app = app_in(dir, &lists, &invalidates)?;
    app.panel_opts.fast_reload = true;

    // Mark a file and move the cursor so we can assert both survive C-l.
    let mark_idx = app
        .left
        .entries
        .iter()
        .position(|e| e.name == "alpha.txt")
        .expect("alpha.txt");
    app.left.cursor = mark_idx;
    app.left.selection.toggle(mark_idx);
    let cursor_before = app.left.cursor;
    let cwd_before = app.left.cwd.clone();
    let entries_before: Vec<String> = app.left.entries.iter().map(|e| e.name.clone()).collect();
    let marked_before = marked(&app);
    let stamp_left = app.left.dir_reload_stamp;
    let stamp_right = app.right.dir_reload_stamp;

    lists.lock().unwrap().clear();
    invalidates.lock().unwrap().clear();
    assert!(!app.needs_full_clear);

    app.handle_action(Action::Repaint)?;

    assert!(
        app.needs_full_clear,
        "C-l / Repaint must request a full terminal clear+redraw"
    );
    assert!(
        app.take_needs_full_clear(),
        "event loop consumes needs_full_clear before draw"
    );
    assert!(
        !app.needs_full_clear,
        "take_needs_full_clear must clear the flag"
    );
    assert_eq!(
        list_count(&lists, dir),
        0,
        "C-l must not call list_dir (that is C-r Refresh)"
    );
    assert!(
        invalidates.lock().unwrap().is_empty(),
        "C-l must not invalidate the VFS directory cache"
    );
    assert_eq!(app.left.cwd, cwd_before);
    assert_eq!(app.left.cursor, cursor_before);
    assert_eq!(
        app.left
            .entries
            .iter()
            .map(|e| e.name.clone())
            .collect::<Vec<_>>(),
        entries_before
    );
    assert_eq!(marked(&app), marked_before);
    assert_eq!(app.left.dir_reload_stamp, stamp_left);
    assert_eq!(app.right.dir_reload_stamp, stamp_right);
    assert_eq!(names(&app), ["alpha.txt", "beta.txt"]);
    assert!(matches!(app.ui_mode, rmc_core::app::UiMode::Normal));
    Ok(())
}

#[test]
fn refresh_still_force_relists_when_fast_reload_on() -> Result<()> {
    let tmp = tempdir()?;
    let dir = tmp.path();
    seed_dir(dir)?;
    let lists = Arc::new(Mutex::new(Vec::new()));
    let invalidates = Arc::new(Mutex::new(Vec::new()));
    let mut app = app_in(dir, &lists, &invalidates)?;
    app.panel_opts.fast_reload = true;

    lists.lock().unwrap().clear();
    invalidates.lock().unwrap().clear();
    app.handle_action(Action::Refresh)?;
    assert!(
        list_count(&lists, dir) > 0,
        "C-r / Refresh must force-relist even when Fast reload is ON"
    );
    assert!(
        !invalidates.lock().unwrap().is_empty(),
        "C-r / Refresh must invalidate the VFS directory cache"
    );
    assert!(
        !app.needs_full_clear,
        "Refresh must not set the C-l repaint flag"
    );
    assert_eq!(names(&app), ["alpha.txt", "beta.txt"]);
    Ok(())
}
