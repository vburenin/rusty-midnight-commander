use anyhow::Result;
use rmc_core::actions::Action;
use rmc_core::app::App;
use rmc_core::config::KeyMap;
use rmc_core::find::{search_files, FindParams, NamePattern};
use rmc_fs::{local::LocalFs, Vfs};
use tempfile::tempdir;

#[derive(Debug)]
struct FixedCwdFs {
    inner: LocalFs,
    cwd: std::path::PathBuf,
}

impl FixedCwdFs {
    fn new(cwd: std::path::PathBuf) -> Self {
        Self {
            inner: LocalFs::new(),
            cwd,
        }
    }
}

impl Vfs for FixedCwdFs {
    fn cwd(&self) -> rmc_fs::FsResult<std::path::PathBuf> {
        Ok(self.cwd.clone())
    }
    fn list_dir(
        &self,
        path: &std::path::Path,
        show_hidden: bool,
    ) -> rmc_fs::FsResult<Vec<rmc_fs::DirEntry>> {
        self.inner.list_dir(path, show_hidden)
    }
    fn enter_path(&self, path: &std::path::Path) -> Option<std::path::PathBuf> {
        self.inner.enter_path(path)
    }
    fn mkdir(&self, path: &std::path::Path) -> rmc_fs::FsResult<()> {
        self.inner.mkdir(path)
    }
    fn remove(&self, path: &std::path::Path, recursive: bool) -> rmc_fs::FsResult<()> {
        self.inner.remove(path, recursive)
    }
    fn copy(&self, src: &std::path::Path, dst: &std::path::Path) -> rmc_fs::FsResult<()> {
        self.inner.copy(src, dst)
    }
    fn move_path(&self, src: &std::path::Path, dst: &std::path::Path) -> rmc_fs::FsResult<()> {
        self.inner.move_path(src, dst)
    }
    fn read_file(&self, path: &std::path::Path) -> rmc_fs::FsResult<Box<dyn std::io::Read + Send>> {
        self.inner.read_file(path)
    }
    fn write_file(
        &self,
        path: &std::path::Path,
    ) -> rmc_fs::FsResult<Box<dyn std::io::Write + Send>> {
        self.inner.write_file(path)
    }
    fn stat(&self, path: &std::path::Path) -> rmc_fs::FsResult<rmc_fs::Metadata> {
        self.inner.stat(path)
    }
}

#[test]
fn find_and_panelize_results_restore_on_parent() -> Result<()> {
    // Prepare temp directory with files
    let dir = tempdir()?;
    let root = dir.path().to_path_buf();
    let f1 = root.join("note.txt");
    let f2 = root.join("data.log");
    std::fs::write(&f1, "hello rust")?;
    std::fs::write(&f2, "zzz")?;

    // Search: *.txt containing "hello" (case-sensitive false)
    let params = FindParams {
        start_dir: root.clone(),
        name_pattern: NamePattern::Glob("*.txt".into()),
        content_substring: Some("HELLO".into()),
        case_sensitive: false,
    };
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let hits = search_files(&params, &cancel);
    assert_eq!(hits, vec![f1.clone()]);

    // App rooted at temp dir (avoid global cwd to prevent race in parallel tests)
    let vfs = FixedCwdFs::new(root.clone());
    let mut app = App::new(Box::new(vfs), KeyMap::mc_defaults())?;
    assert_eq!(app.active_panel().cwd, root);

    // Panelize and verify entries include parent and found file
    app.panelize_paths(&hits, Some(&root))?;
    let names: Vec<String> = app
        .active_panel()
        .entries
        .iter()
        .map(|e| e.name.clone())
        .collect();
    assert!(names.iter().any(|n| n == ".."));
    assert!(names.iter().any(|n| n == "note.txt"));

    // Cursor at 0 should be ".."; Enter should restore normal listing
    app.handle_action(Action::Enter)?;
    // After restore, panel entries should correspond to directory listing (should include "data.log")
    let names_after: Vec<String> = app
        .active_panel()
        .entries
        .iter()
        .map(|e| e.name.clone())
        .collect();
    assert!(names_after.iter().any(|n| n == "data.log"));
    Ok(())
}

#[test]
fn panelize_uses_relative_names_for_disambiguation() -> Result<()> {
    let dir = tempdir()?;
    let root = dir.path().to_path_buf();
    let sub1 = root.join("sub1");
    let sub2 = root.join("sub2");
    std::fs::create_dir_all(&sub1)?;
    std::fs::create_dir_all(&sub2)?;
    let f1 = sub1.join("dup.txt");
    let f2 = sub2.join("dup.txt");
    std::fs::write(&f1, "a")?;
    std::fs::write(&f2, "b")?;
    // App rooted at temp dir (avoid global cwd to prevent race in parallel tests)
    let vfs = FixedCwdFs::new(root.clone());
    let mut app = App::new(Box::new(vfs), KeyMap::mc_defaults())?;
    app.panelize_paths(&[f1.clone(), f2.clone()], Some(&root))?;
    let names: Vec<String> = app
        .active_panel()
        .entries
        .iter()
        .map(|e| e.name.clone())
        .collect();
    assert!(names.iter().any(|n| n == "sub1/dup.txt"));
    assert!(names.iter().any(|n| n == "sub2/dup.txt"));
    Ok(())
}
