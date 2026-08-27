use anyhow::Result;
use rmc_core::actions::Action;
use rmc_core::app::{App, ConfigOptions, UiMode};
use rmc_core::config::KeyMap;
use rmc_fs::local::LocalFs;
use std::fs;
use tempfile::tempdir;

fn app_in_dir(root: &std::path::Path) -> Result<App> {
    let vfs = LocalFs::new();
    let mut app = App::new(Box::new(vfs), KeyMap::mc_defaults())?;
    app.change_dir(root)?;
    Ok(app)
}

fn listed_names(app: &App) -> Vec<String> {
    app.active_panel()
        .entries
        .iter()
        .map(|e| e.name.clone())
        .collect()
}

fn selected_names(app: &App) -> Vec<String> {
    let p = app.active_panel();
    p.selection
        .iter()
        .filter_map(|i| p.entries.get(i).map(|e| e.name.clone()))
        .collect()
}

#[test]
fn shell_patterns_defaults_true() -> Result<()> {
    assert!(ConfigOptions::default().shell_patterns);
    let vfs = LocalFs::new();
    let app = App::new(Box::new(vfs), KeyMap::mc_defaults())?;
    assert!(app.config_opts.shell_patterns);
    Ok(())
}

#[test]
fn filter_shell_glob_on_matches_txt_not_rs() -> Result<()> {
    let dir = tempdir()?;
    let root = dir.path();
    fs::write(root.join("foo.txt"), b"a")?;
    fs::write(root.join("foo.rs"), b"b")?;

    let mut app = app_in_dir(root)?;
    assert!(app.config_opts.shell_patterns);
    app.left.filter_glob = Some("*.txt".to_string());
    app.reload_panels()?;

    let names = listed_names(&app);
    assert!(names.first().map(String::as_str) == Some(".."));
    assert!(names.iter().any(|n| n == "foo.txt"));
    assert!(!names.iter().any(|n| n == "foo.rs"));
    Ok(())
}

#[test]
fn filter_regex_off_matches_txt() -> Result<()> {
    let dir = tempdir()?;
    let root = dir.path();
    fs::write(root.join("foo.txt"), b"a")?;
    fs::write(root.join("foo.rs"), b"b")?;

    let mut app = app_in_dir(root)?;
    app.config_opts.shell_patterns = false;
    app.left.filter_glob = Some(r".*\.txt$".to_string());
    app.left.filter_regex = true;
    app.reload_panels()?;

    let names = listed_names(&app);
    assert!(names.iter().any(|n| n == "foo.txt"));
    assert!(!names.iter().any(|n| n == "foo.rs"));
    Ok(())
}

#[test]
fn filter_star_and_empty_show_all_in_both_modes() -> Result<()> {
    let dir = tempdir()?;
    let root = dir.path();
    fs::write(root.join("foo.txt"), b"a")?;
    fs::write(root.join("foo.rs"), b"b")?;

    let mut app = app_in_dir(root)?;
    for regex in [false, true] {
        app.left.filter_regex = regex;
        app.left.filter_glob = Some("*".to_string());
        app.reload_panels()?;
        let names = listed_names(&app);
        assert!(names.iter().any(|n| n == "foo.txt"), "star regex={regex}");
        assert!(names.iter().any(|n| n == "foo.rs"), "star regex={regex}");

        app.left.filter_glob = Some(String::new());
        app.reload_panels()?;
        let names = listed_names(&app);
        assert!(names.iter().any(|n| n == "foo.txt"), "empty regex={regex}");
        assert!(names.iter().any(|n| n == "foo.rs"), "empty regex={regex}");
    }
    Ok(())
}

#[test]
fn select_unselect_group_honor_shell_patterns() -> Result<()> {
    let dir = tempdir()?;
    let root = dir.path();
    fs::write(root.join("foo.txt"), b"a")?;
    fs::write(root.join("foo.rs"), b"b")?;

    let mut app = app_in_dir(root)?;
    assert!(app.config_opts.shell_patterns);

    app.handle_action(Action::SelectGroup)?;
    match &app.ui_mode {
        UiMode::SelectGroupDialog {
            select: true,
            regular_expression,
            ..
        } => assert!(
            !*regular_expression,
            "first Select open seeds Regular expression from shell_patterns"
        ),
        _ => panic!("SelectGroup must open Select dialog"),
    }
    app.ui_mode = UiMode::Normal;
    app.apply_group_pattern("*.txt", true, false, true, false);
    let sel = selected_names(&app);
    assert!(sel.iter().any(|n| n == "foo.txt"));
    assert!(!sel.iter().any(|n| n == "foo.rs"));

    app.handle_action(Action::UnselectGroup)?;
    match &app.ui_mode {
        UiMode::SelectGroupDialog { select: false, .. } => {}
        _ => panic!("UnselectGroup must open Unselect dialog"),
    }
    app.ui_mode = UiMode::Normal;
    app.apply_group_pattern("*.txt", false, false, true, false);
    assert!(selected_names(&app).is_empty());

    app.apply_group_pattern(r".*\.txt$", true, false, true, true);
    let sel = selected_names(&app);
    assert!(sel.iter().any(|n| n == "foo.txt"));
    assert!(!sel.iter().any(|n| n == "foo.rs"));

    app.apply_group_pattern(r".*\.txt$", false, false, true, true);
    assert!(selected_names(&app).is_empty());
    Ok(())
}
