//! Options → Configuration → Auto menus (`config_opts.auto_menus`).
//! After a real `change_dir` into a different directory that has a local
//! `.mc.menu`, open the User menu. Reload / C-r / panelize / same-cwd do not.
use anyhow::Result;
use rmc_core::actions::Action;
use rmc_core::app::{App, ConfigOptions, UiMode};
use rmc_core::config::KeyMap;
use rmc_fs::local::LocalFs;
use std::fs;
use std::path::Path;
use tempfile::tempdir;

fn write_local_menu(dir: &Path, title: &str) -> Result<()> {
    fs::write(dir.join(".mc.menu"), format!("x: {title}\n  echo local\n"))?;
    Ok(())
}

fn app_in(dir: &Path) -> Result<App> {
    let vfs = LocalFs::new();
    let mut app = App::new(Box::new(vfs), KeyMap::mc_defaults())?;
    app.config_opts.auto_menus = false;
    app.change_dir(dir)?;
    app.ui_mode = UiMode::Normal;
    Ok(app)
}

fn user_menu_titles(app: &App) -> Option<Vec<String>> {
    match &app.ui_mode {
        UiMode::UserMenu { entries, .. } => Some(entries.iter().map(|e| e.title.clone()).collect()),
        _ => None,
    }
}

#[test]
fn auto_menus_defaults_false() {
    assert!(
        !ConfigOptions::default().auto_menus,
        "GNU mc Auto menus defaults to off"
    );
    let vfs = LocalFs::new();
    let app = App::new(Box::new(vfs), KeyMap::mc_defaults()).unwrap();
    assert!(!app.config_opts.auto_menus);
}

#[test]
fn auto_menus_on_change_dir_with_local_menu_opens_user_menu() -> Result<()> {
    let tmp = tempdir()?;
    let root = tmp.path();
    let child = root.join("child");
    fs::create_dir(&child)?;
    write_local_menu(&child, "Local auto menu")?;

    let mut app = app_in(root)?;
    app.config_opts.auto_menus = true;
    assert!(matches!(app.ui_mode, UiMode::Normal));

    app.change_dir(&child)?;
    let titles = user_menu_titles(&app).expect("UserMenu after change_dir");
    assert!(
        titles.iter().any(|t| t == "Local auto menu"),
        "must open the directory's local .mc.menu, got {titles:?}"
    );
    Ok(())
}

#[test]
fn auto_menus_on_change_dir_without_local_menu_stays_normal() -> Result<()> {
    let tmp = tempdir()?;
    let root = tmp.path();
    let child = root.join("child");
    fs::create_dir(&child)?;

    let mut app = app_in(root)?;
    app.config_opts.auto_menus = true;
    app.change_dir(&child)?;
    assert!(
        matches!(app.ui_mode, UiMode::Normal),
        "missing .mc.menu must not open UserMenu (no ~/.config or shipped fallback)"
    );
    Ok(())
}

#[test]
fn auto_menus_off_with_local_menu_stays_normal() -> Result<()> {
    let tmp = tempdir()?;
    let root = tmp.path();
    let child = root.join("child");
    fs::create_dir(&child)?;
    write_local_menu(&child, "Should not open")?;

    let mut app = app_in(root)?;
    assert!(!app.config_opts.auto_menus);
    app.change_dir(&child)?;
    assert!(
        matches!(app.ui_mode, UiMode::Normal),
        "Auto menus off must never auto-open UserMenu"
    );
    Ok(())
}

#[test]
fn auto_menus_reload_same_cwd_does_not_reopen() -> Result<()> {
    let tmp = tempdir()?;
    let root = tmp.path();
    let child = root.join("child");
    fs::create_dir(&child)?;
    write_local_menu(&child, "Local auto menu")?;

    let mut app = app_in(root)?;
    app.config_opts.auto_menus = true;
    app.change_dir(&child)?;
    assert!(user_menu_titles(&app).is_some());

    app.ui_mode = UiMode::Normal;
    app.reload_panels()?;
    assert!(
        matches!(app.ui_mode, UiMode::Normal),
        "reload_panels must not re-open UserMenu"
    );

    app.change_dir(&child)?;
    assert!(
        matches!(app.ui_mode, UiMode::Normal),
        "change_dir into the same cwd must not re-open UserMenu"
    );

    app.handle_action(Action::Refresh)?;
    assert!(
        matches!(app.ui_mode, UiMode::Normal),
        "C-r / Refresh must not re-open UserMenu"
    );
    Ok(())
}

#[test]
fn auto_menus_parent_mc_menu_does_not_open() -> Result<()> {
    let tmp = tempdir()?;
    let root = tmp.path();
    let child = root.join("child");
    fs::create_dir(&child)?;
    write_local_menu(root, "Parent menu")?;

    let mut app = app_in(root)?;
    app.config_opts.auto_menus = true;
    app.ui_mode = UiMode::Normal;
    app.change_dir(&child)?;
    assert!(
        matches!(app.ui_mode, UiMode::Normal),
        "only the panel cwd's .mc.menu counts, not a parent"
    );
    Ok(())
}

#[test]
fn auto_menus_unreadable_mc_menu_stays_normal() -> Result<()> {
    let tmp = tempdir()?;
    let root = tmp.path();
    let child = root.join("child");
    fs::create_dir(&child)?;
    // A directory named `.mc.menu` exists but is not a readable menu file.
    fs::create_dir(child.join(".mc.menu"))?;

    let mut app = app_in(root)?;
    app.config_opts.auto_menus = true;
    app.change_dir(&child)?;
    assert!(
        matches!(app.ui_mode, UiMode::Normal),
        "unreadable .mc.menu must stay Normal"
    );
    Ok(())
}

#[test]
fn auto_menus_panelize_does_not_open() -> Result<()> {
    let tmp = tempdir()?;
    let root = tmp.path();
    fs::write(root.join("a.txt"), b"x")?;
    write_local_menu(root, "Local auto menu")?;

    let mut app = app_in(root)?;
    app.config_opts.auto_menus = true;
    app.ui_mode = UiMode::Normal;
    let paths = vec![root.join("a.txt")];
    app.panelize_paths(&paths, Some(root))?;
    assert!(
        matches!(app.ui_mode, UiMode::Normal),
        "panelize must not auto-open UserMenu"
    );
    Ok(())
}
