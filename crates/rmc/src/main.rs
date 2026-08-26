use anyhow::Result;
use rmc_core::app::App;
use rmc_core::config::KeyMap;
use rmc_fs::composite::CompositeFs;
use rmc_ui::terminal::TerminalApp;

fn main() -> Result<()> {
    // Initialize core app with a composite VFS (local + archives).
    let vfs = CompositeFs::new();
    let keymap = KeyMap::load_default();
    let mut app = App::new(Box::new(vfs), keymap)?;
    TerminalApp::run(&mut app)
}
