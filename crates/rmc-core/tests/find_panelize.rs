use anyhow::Result;
use rmc_core::app::App;
use rmc_core::config::KeyMap;
use rmc_core::actions::Action;
use rmc_core::find::{FindParams, NamePattern, search_files};
use rmc_fs::local::LocalFs;
use tempfile::tempdir;

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

    // App rooted at temp dir
    std::env::set_current_dir(&root)?;
    let vfs = LocalFs::new();
    let mut app = App::new(Box::new(vfs), KeyMap::mc_defaults())?;
    assert_eq!(app.active_panel().cwd, root);

    // Panelize and verify entries include parent and found file
    app.panelize_paths(&hits, Some(&root))?;
    let names: Vec<String> = app.active_panel().entries.iter().map(|e| e.name.clone()).collect();
    assert!(names.iter().any(|n| n == ".."));
    assert!(names.iter().any(|n| n == "note.txt"));

    // Cursor at 0 should be ".."; Enter should restore normal listing
    app.handle_action(Action::Enter)?;
    // After restore, panel entries should correspond to directory listing (should include "data.log")
    let names_after: Vec<String> = app.active_panel().entries.iter().map(|e| e.name.clone()).collect();
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
    std::env::set_current_dir(&root)?;
    let vfs = LocalFs::new();
    let mut app = App::new(Box::new(vfs), KeyMap::mc_defaults())?;
    app.panelize_paths(&[f1.clone(), f2.clone()], Some(&root))?;
    let names: Vec<String> = app.active_panel().entries.iter().map(|e| e.name.clone()).collect();
    assert!(names.iter().any(|n| n == "sub1/dup.txt"));
    assert!(names.iter().any(|n| n == "sub2/dup.txt"));
    Ok(())
}

