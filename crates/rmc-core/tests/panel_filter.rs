use anyhow::Result;
use rmc_core::app::App;
use rmc_core::config::KeyMap;
use rmc_fs::local::LocalFs;
use std::fs;
use tempfile::tempdir;

#[test]
fn panel_filename_filter_applies_and_keeps_parent() -> Result<()> {
    let dir = tempdir()?;
    let root = dir.path();
    // Files and a subdir
    fs::write(root.join("a.txt"), b"a")?;
    fs::write(root.join("b.c"), b"b")?;
    fs::create_dir(root.join("sub"))?;
    fs::write(root.join("abc.c"), b"c")?;

    // App using LocalFs; switch to temp dir
    let vfs = LocalFs::new();
    let mut app = App::new(Box::new(vfs), KeyMap::mc_defaults())?;
    app.change_dir(root)?;

    // Apply filter to show only *.c on the active (left) panel
    app.left.filter_glob = Some("*.c".to_string());
    app.reload_panels()?;

    let names: Vec<_> = app.left.entries.iter().map(|e| e.name.as_str()).collect();
    // Parent marker must be present
    assert!(names.first().copied() == Some(".."));
    // Only *.c must be listed besides the parent marker
    assert!(names.contains(&"b.c"));
    assert!(names.contains(&"abc.c"));
    assert!(!names.contains(&"a.txt"));
    // Default Files only off: non-matching directories drop too.
    assert!(!names.contains(&"sub"));

    Ok(())
}

#[test]
fn panel_filter_star_resets_to_all() -> Result<()> {
    let dir = tempdir()?;
    let root = dir.path();
    fs::write(root.join("x.rs"), b"x")?;
    fs::write(root.join("y.md"), b"y")?;

    let vfs = LocalFs::new();
    let mut app = App::new(Box::new(vfs), KeyMap::mc_defaults())?;
    app.change_dir(root)?;

    // Set filter to "*" (should behave as no filter)
    app.left.filter_glob = Some("*".to_string());
    app.reload_panels()?;
    let names: Vec<_> = app.left.entries.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"x.rs"));
    assert!(names.contains(&"y.md"));
    assert!(names.first().copied() == Some(".."));
    Ok(())
}

#[test]
fn panel_filter_files_only_keeps_directories() -> Result<()> {
    let dir = tempdir()?;
    let root = dir.path();
    fs::write(root.join("keep.txt"), b"a")?;
    fs::write(root.join("skip.dat"), b"b")?;
    fs::create_dir(root.join("subdir"))?;

    let vfs = LocalFs::new();
    let mut app = App::new(Box::new(vfs), KeyMap::mc_defaults())?;
    app.change_dir(root)?;

    app.left.filter_glob = Some("*.txt".to_string());
    app.left.filter_files_only = true;
    app.reload_panels()?;
    let names: Vec<_> = app.left.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names.first().copied(), Some(".."));
    assert!(names.contains(&"keep.txt"));
    assert!(names.contains(&"subdir"));
    assert!(!names.contains(&"skip.dat"));
    Ok(())
}

#[test]
fn panel_filter_case_insensitive_glob() -> Result<()> {
    let dir = tempdir()?;
    let root = dir.path();
    fs::write(root.join("KEEP.TXT"), b"a")?;
    fs::write(root.join("skip.dat"), b"b")?;

    let vfs = LocalFs::new();
    let mut app = App::new(Box::new(vfs), KeyMap::mc_defaults())?;
    app.change_dir(root)?;

    app.left.filter_glob = Some("*.txt".to_string());
    app.left.filter_case_sensitive = true;
    app.reload_panels()?;
    let names: Vec<_> = app.left.entries.iter().map(|e| e.name.as_str()).collect();
    assert!(!names.contains(&"KEEP.TXT"));

    app.left.filter_case_sensitive = false;
    app.reload_panels()?;
    let names: Vec<_> = app.left.entries.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"KEEP.TXT"));
    assert!(!names.contains(&"skip.dat"));
    Ok(())
}
