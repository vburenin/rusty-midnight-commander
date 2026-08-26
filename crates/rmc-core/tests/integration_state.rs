use anyhow::Result;
use rmc_core::actions::Action;
use rmc_core::app::App;
use rmc_core::config::KeyMap;
use rmc_fs::local::LocalFs;

#[test]
fn app_navigates_directories() -> Result<()> {
    let vfs = LocalFs::new();
    let mut app = App::new(Box::new(vfs), KeyMap::mc_defaults())?;
    let start = app.active_panel().cwd.clone();
    app.handle_action(Action::ParentDir)?;
    // Either changed to parent or stayed if root
    let now = app.active_panel().cwd.clone();
    if let Some(parent) = start.parent() {
        assert_eq!(now, parent);
    }
    Ok(())
}
