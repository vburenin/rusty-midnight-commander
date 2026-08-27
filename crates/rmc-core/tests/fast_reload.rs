//! Fast reload: reuse a local listing when the directory stamp is unchanged.
//! Remote/archive Directory cache timeout is a separate option (see dir_cache_timeout.rs).
use anyhow::Result;
use rmc_core::actions::{Action, PaneSide};
use rmc_core::app::{App, PanelOptions};
use rmc_core::config::KeyMap;
use rmc_core::panel::DirReloadStamp;
use rmc_fs::local::LocalFs;
use rmc_fs::{DirEntry, FsResult, Metadata, Vfs};
use std::fs::{self, File};
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
    names_of(&app.left)
}

fn names_of(panel: &rmc_core::panel::PanelState) -> Vec<String> {
    let mut n: Vec<String> = panel
        .entries
        .iter()
        .filter(|e| e.name != "..")
        .map(|e| e.name.clone())
        .collect();
    n.sort();
    n
}

fn cursor_name(panel: &rmc_core::panel::PanelState) -> Option<&str> {
    panel.current_entry().map(|e| e.name.as_str())
}

fn two_panel_app(
    left_dir: &Path,
    right_dir: &Path,
    lists: &Arc<Mutex<Vec<PathBuf>>>,
) -> Result<App> {
    let mut app = app_in(left_dir, lists)?;
    app.active = PaneSide::Right;
    app.change_dir(right_dir)?;
    app.active = PaneSide::Left;
    Ok(app)
}

fn seed_dir(dir: &Path) -> Result<()> {
    File::create(dir.join("alpha.txt"))?;
    Ok(())
}

/// Create `name` and advance the directory Fast-reload stamp.
/// Overlay/lazytime often keeps dir mtime unchanged after a same-second create;
/// `touch -d +1 second` forces the mtime change GNU mc Fast reload keys off.
fn create_file_changing_dir_stamp(dir: &Path, name: &str) -> Result<()> {
    let before = DirReloadStamp::from_local_dir(dir);
    File::create(dir.join(name))?;
    let status = std::process::Command::new("touch")
        .args(["-d", "+1 second"])
        .arg(dir)
        .status()?;
    anyhow::ensure!(status.success(), "touch {dir:?} failed: {status}");
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

#[test]
fn refresh_relists_only_the_active_panel() -> Result<()> {
    let tmp = tempdir()?;
    let left_dir = tmp.path().join("L");
    let right_dir = tmp.path().join("R");
    fs::create_dir(&left_dir)?;
    fs::create_dir(&right_dir)?;
    File::create(left_dir.join("left.txt"))?;
    File::create(right_dir.join("right.txt"))?;
    let lists = Arc::new(Mutex::new(Vec::new()));
    let mut app = two_panel_app(&left_dir, &right_dir, &lists)?;
    assert_eq!(names_of(&app.left), ["left.txt"]);
    assert_eq!(names_of(&app.right), ["right.txt"]);

    File::create(left_dir.join("new-left.txt"))?;
    File::create(right_dir.join("new-right.txt"))?;
    lists.lock().unwrap().clear();
    app.handle_action(Action::Refresh)?;

    assert!(
        list_count(&lists, &left_dir) > 0,
        "C-r must list_dir the active (left) panel"
    );
    assert_eq!(
        list_count(&lists, &right_dir),
        0,
        "C-r must not list_dir the inactive panel"
    );
    assert_eq!(names_of(&app.left), ["left.txt", "new-left.txt"]);
    assert_eq!(
        names_of(&app.right),
        ["right.txt"],
        "inactive panel listing stays stale until that panel is reread"
    );

    lists.lock().unwrap().clear();
    app.active = PaneSide::Right;
    app.handle_action(Action::Refresh)?;
    assert_eq!(list_count(&lists, &left_dir), 0);
    assert!(list_count(&lists, &right_dir) > 0);
    assert_eq!(names_of(&app.left), ["left.txt", "new-left.txt"]);
    assert_eq!(names_of(&app.right), ["new-right.txt", "right.txt"]);
    Ok(())
}

#[test]
fn refresh_create_and_delete_update_active_listing() -> Result<()> {
    let tmp = tempdir()?;
    let dir = tmp.path();
    seed_dir(dir)?;
    let lists = Arc::new(Mutex::new(Vec::new()));
    let mut app = app_in(dir, &lists)?;
    assert_eq!(names(&app), ["alpha.txt"]);

    File::create(dir.join("beta.txt"))?;
    assert_eq!(
        names(&app),
        ["alpha.txt"],
        "disk create is not visible until C-r"
    );
    app.handle_action(Action::Refresh)?;
    assert_eq!(names(&app), ["alpha.txt", "beta.txt"]);

    fs::remove_file(dir.join("beta.txt"))?;
    assert_eq!(
        names(&app),
        ["alpha.txt", "beta.txt"],
        "disk delete is not visible until C-r"
    );
    app.handle_action(Action::Refresh)?;
    assert_eq!(names(&app), ["alpha.txt"]);
    Ok(())
}

#[test]
fn refresh_preserves_cursor_name_and_marks() -> Result<()> {
    let tmp = tempdir()?;
    let dir = tmp.path();
    File::create(dir.join("alpha.txt"))?;
    File::create(dir.join("beta.txt"))?;
    File::create(dir.join("gamma.txt"))?;
    let lists = Arc::new(Mutex::new(Vec::new()));
    let mut app = app_in(dir, &lists)?;
    let beta = app
        .left
        .entries
        .iter()
        .position(|e| e.name == "beta.txt")
        .expect("beta.txt");
    app.left.cursor = beta;
    app.left.selection.select(beta);

    File::create(dir.join("aaa.txt"))?;
    app.handle_action(Action::Refresh)?;
    assert_eq!(cursor_name(&app.left), Some("beta.txt"));
    let marked: Vec<String> = app
        .left
        .selection
        .iter()
        .filter_map(|i| app.left.entries.get(i).map(|e| e.name.clone()))
        .collect();
    assert_eq!(marked, ["beta.txt"]);

    fs::remove_file(dir.join("beta.txt"))?;
    app.handle_action(Action::Refresh)?;
    assert_ne!(cursor_name(&app.left), Some("beta.txt"));
    assert!(
        cursor_name(&app.left).is_some(),
        "cursor must clamp to a remaining entry"
    );
    assert!(
        app.left.selection.is_empty(),
        "mark on deleted name is gone"
    );
    Ok(())
}

#[test]
fn refresh_active_only_with_horizontal_split() -> Result<()> {
    let tmp = tempdir()?;
    let left_dir = tmp.path().join("L");
    let right_dir = tmp.path().join("R");
    fs::create_dir(&left_dir)?;
    fs::create_dir(&right_dir)?;
    File::create(left_dir.join("left.txt"))?;
    File::create(right_dir.join("right.txt"))?;
    let lists = Arc::new(Mutex::new(Vec::new()));
    let mut app = two_panel_app(&left_dir, &right_dir, &lists)?;
    app.layout.horizontal_split = true;
    app.active = PaneSide::Right;

    File::create(left_dir.join("new-left.txt"))?;
    File::create(right_dir.join("new-right.txt"))?;
    lists.lock().unwrap().clear();
    app.handle_action(Action::Refresh)?;
    assert_eq!(list_count(&lists, &left_dir), 0);
    assert!(list_count(&lists, &right_dir) > 0);
    assert_eq!(names_of(&app.left), ["left.txt"]);
    assert_eq!(names_of(&app.right), ["new-right.txt", "right.txt"]);
    Ok(())
}

#[test]
fn refresh_forces_listing_when_fast_reload_skips_auto_path() -> Result<()> {
    let tmp = tempdir()?;
    let dir = tmp.path();
    seed_dir(dir)?;
    let lists = Arc::new(Mutex::new(Vec::new()));
    let mut app = app_in(dir, &lists)?;
    app.panel_opts.fast_reload = true;

    File::create(dir.join("gamma.txt"))?;
    lists.lock().unwrap().clear();
    app.reload_panels()?;
    if list_count(&lists, dir) == 0 {
        assert_eq!(
            names(&app),
            ["alpha.txt"],
            "Fast reload ON may skip when the directory stamp is unchanged"
        );
        app.handle_action(Action::Refresh)?;
        assert_eq!(names(&app), ["alpha.txt", "gamma.txt"]);
    } else {
        // Overlay/mtime did change; auto path already listed. C-r must still force another list_dir.
        lists.lock().unwrap().clear();
        app.handle_action(Action::Refresh)?;
        assert!(
            list_count(&lists, dir) > 0,
            "C-r must force list_dir even after an opportunistic reload"
        );
        assert_eq!(names(&app), ["alpha.txt", "gamma.txt"]);
    }
    Ok(())
}
