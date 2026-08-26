use anyhow::Result;
use rmc_core::app::App;
use rmc_core::config::KeyMap;
use rmc_fs::local::LocalFs;
use rmc_ui::terminal::TerminalApp;

fn main() -> Result<()> {
    // Initialize core app with two local filesystem panels starting from current dir.
    let vfs = LocalFs::new();
    let keymap = KeyMap::mc_defaults();
    let mut app = App::new(Box::new(vfs), keymap)?;
    TerminalApp::run(&mut app)
}
