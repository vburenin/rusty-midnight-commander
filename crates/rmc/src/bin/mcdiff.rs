use anyhow::Result;
use rmc_core::app::{App, DiffState, UiMode};
use rmc_core::config::KeyMap;
use rmc_fs::composite::CompositeFs;
use rmc_ui::terminal::TerminalApp;
use std::env;
use std::path::PathBuf;

fn main() -> Result<()> {
    // Initialize as in main: keep CompositeFs::new and KeyMap::load_default present in repo
    let vfs = CompositeFs::new();
    let keymap = KeyMap::load_default();
    let mut app = App::new(Box::new(vfs), keymap)?;
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args.len() >= 2 {
        let left = PathBuf::from(&args[0]);
        let right = PathBuf::from(&args[1]);
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
        app.ui_mode = UiMode::Diff(DiffState {
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
            left_scroll: 0,
            right_scroll: 0,
            panel_ratio: 0.5,
            tab_width: 4,
            merge_target_right: true,
        });
    } else {
        // Fallback: just run normal app
    }
    TerminalApp::run(&mut app)
}
