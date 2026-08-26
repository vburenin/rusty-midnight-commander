use anyhow::Result;
use rmc_core::app::App;
use rmc_core::config::{KeyMap, Theme};
use rmc_fs::local::LocalFs;
use rmc_ui::terminal::TerminalApp;

fn main() -> Result<()> {
    // Initialize core app with two local filesystem panels starting from current dir.
    let vfs = LocalFs::new();
    let theme = Theme::default_mc();
    let keymap = KeyMap::mc_defaults();
    let mut app = App::new(Box::new(vfs), theme, keymap)?;
    TerminalApp::run(&mut app)
}
