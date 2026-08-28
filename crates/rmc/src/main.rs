use anyhow::Result;
use rmc_core::app::App;
use rmc_core::config::KeyMap;
use rmc_fs::composite::CompositeFs;
use rmc_ui::terminal::{apply_cli_args, apply_mc_skin_env, TerminalApp};
use std::env;

fn main() -> Result<()> {
    // Initialize core app with a composite VFS (local + archives).
    let vfs = CompositeFs::new();
    let keymap = KeyMap::load_default();
    let mut app = App::new(Box::new(vfs), keymap)?;
    // GNU mc(1) skin order: ini (in App::new) → MC_SKIN → `-S`/`--skin`.
    apply_mc_skin_env(&mut app);
    let args = env::args().skip(1).collect::<Vec<_>>();
    apply_cli_args(&mut app, &args)?;
    TerminalApp::run(&mut app)
}
