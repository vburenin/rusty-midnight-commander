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

fn submit_prompt(app: &mut App, pattern: &str) -> Result<()> {
    let mode = std::mem::replace(&mut app.ui_mode, UiMode::Normal);
    match mode {
        UiMode::PromptInput { on_submit, .. } => on_submit(app, pattern.to_string()),
        other => {
            app.ui_mode = other;
            anyhow::bail!("expected PromptInput for Select/Unselect group");
        }
    }
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
    for shell in [true, false] {
        app.config_opts.shell_patterns = shell;
        app.left.filter_glob = Some("*".to_string());
        app.reload_panels()?;
        let names = listed_names(&app);
        assert!(names.iter().any(|n| n == "foo.txt"), "star shell={shell}");
        assert!(names.iter().any(|n| n == "foo.rs"), "star shell={shell}");

        app.left.filter_glob = Some(String::new());
        app.reload_panels()?;
        let names = listed_names(&app);
        assert!(names.iter().any(|n| n == "foo.txt"), "empty shell={shell}");
        assert!(names.iter().any(|n| n == "foo.rs"), "empty shell={shell}");
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

    // Gray+ / + with glob
    app.handle_action(Action::SelectGroup)?;
    submit_prompt(&mut app, "*.txt")?;
    let sel = selected_names(&app);
    assert!(sel.iter().any(|n| n == "foo.txt"));
    assert!(!sel.iter().any(|n| n == "foo.rs"));

    // Gray- with glob: unselect the .txt, leave none
    app.handle_action(Action::UnselectGroup)?;
    submit_prompt(&mut app, "*.txt")?;
    assert!(selected_names(&app).is_empty());

    // Regex mode: Gray+ with .*\.txt$
    app.config_opts.shell_patterns = false;
    app.handle_action(Action::SelectGroup)?;
    submit_prompt(&mut app, r".*\.txt$")?;
    let sel = selected_names(&app);
    assert!(sel.iter().any(|n| n == "foo.txt"));
    assert!(!sel.iter().any(|n| n == "foo.rs"));

    // Gray- with regex unselects the same names
    app.handle_action(Action::UnselectGroup)?;
    submit_prompt(&mut app, r".*\.txt$")?;
    assert!(selected_names(&app).is_empty());
    Ok(())
}
