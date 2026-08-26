use anyhow::Result;
use rmc_core::user_menu::{load_menu, expand_macros, run_menu_command};
use rmc_core::{app::App, config::KeyMap};
use rmc_fs::local::LocalFs;
use tempfile::tempdir;

#[test]
fn load_default_menu_and_expand_and_run() -> Result<()> {
    // Temp cwd with a sample file
    let dir = tempdir()?;
    let root = dir.path().to_path_buf();
    std::fs::write(root.join("file name.txt"), "x")?;
    std::env::set_current_dir(&root)?;

    // Build app
    let vfs = LocalFs::new();
    let mut app = App::new(Box::new(vfs), KeyMap::mc_defaults())?;
    // Select our file
    let idx = app
        .active_panel()
        .entries
        .iter()
        .position(|e| e.name == "file name.txt")
        .unwrap();
    app.active_panel_mut().cursor = idx;

    // Load default shipped menu (no .mc.menu in cwd)
    let menu = load_menu(&root)?;
    assert!(menu.source_path.display().to_string().contains("data/mc.menu"));
    assert!(!menu.entries.is_empty());

    // Macro expansion for %f should quote spaces
    let s = expand_macros(&app, "echo %f");
    assert!(s.contains("'file name.txt'"));

    // Run an echo-like entry: create a file via the shell
    run_menu_command(&app, "echo hello > out.txt")?;
    let txt = std::fs::read_to_string(root.join("out.txt"))?;
    assert_eq!(txt.trim(), "hello");
    Ok(())
}

