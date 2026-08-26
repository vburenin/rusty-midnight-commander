use anyhow::Result;
use rmc_core::app::App;
use rmc_core::config::KeyMap;
use rmc_fs::composite::CompositeFs;
use rmc_ui::terminal::TerminalApp;
use std::env;

fn main() -> Result<()> {
    // Initialize core app with a composite VFS (local + archives).
    let vfs = CompositeFs::new();
    let keymap = KeyMap::load_default();
    let mut app = App::new(Box::new(vfs), keymap)?;
    // CLI: rmc --diff file1 file2
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args.len() >= 3 && args[0] == "--diff" {
        let left = std::path::PathBuf::from(&args[1]);
        let right = std::path::PathBuf::from(&args[2]);
        // Load contents using VFS
        let mut ltxt = String::new();
        let mut rtxt = String::new();
        {
            let mut r = app
                .vfs
                .read_file(&left)
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
            use std::io::Read;
            let _ = r.read_to_string(&mut ltxt);
        }
        {
            let mut r = app
                .vfs
                .read_file(&right)
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
            use std::io::Read;
            let _ = r.read_to_string(&mut rtxt);
        }
        let left_lines = rmc_diff::split_lines(&ltxt);
        let right_lines = rmc_diff::split_lines(&rtxt);
        let dr = rmc_diff::compute_diff(&ltxt, &rtxt);
        app.ui_mode = rmc_core::app::UiMode::Diff(rmc_core::app::DiffState {
            left_path: left,
            right_path: right,
            left_lines,
            right_lines,
            hunks: dr.hunks,
            current_hunk: 0,
            left_modified: false,
            right_modified: false,
            show_line_numbers: false,
            show_hunk_status: true,
            search: None,
            search_prompt: None,
            goto_prompt: None,
            confirm_exit: None,
            left_scroll: 0,
            right_scroll: 0,
            panel_ratio: 0.5,
            tab_width: 4,
            merge_target_right: true,
        });
    }
    TerminalApp::run(&mut app)
}
