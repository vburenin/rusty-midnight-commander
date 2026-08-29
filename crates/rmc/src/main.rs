use anyhow::Result;
use rmc_core::app::App;
use rmc_core::config::KeyMap;
use rmc_fs::composite::CompositeFs;
use rmc_ui::terminal::{apply_cli_args, apply_mc_skin_env, TerminalApp};
use std::env;

const USAGE: &str = "\
mcr - dual-pane terminal file manager (Apache-2.0)

Usage:
  mcr [OPTION]... [DIR1 [DIR2]]

  DIR1 [DIR2]              Current panel directory; optional other panel directory
                           (default left-current panel: DIR1 is left, DIR2 is right)

Options:
  -d, --nomouse            Disable mouse capture for this run
  -u, --nosubshell         Do not start a PTY subshell
  -U, --subshell           Enable the PTY subshell (last of -U/-u wins)
  -S, --skin=NAME          Skin name for this process
  -v, --view FILE          Internal viewer (see mcr-view(1))
  -e, --edit [FILE...]     Internal editor (see mcr-edit(1))
      --diff FILE1 FILE2   Internal diff viewer (see mcr-diff(1))
  -h, --help               Show this help

Manuals (original Apache-2.0 text, not GNU GPL man pages):
  man mcr
  man mcr-edit
  man mcr-view
  man mcr-diff
From a source checkout:  man -l docs/man/mcr.1
Website (HTML, not the GNU mc site):  docs/website/index.html
";

fn wants_help(args: &[String]) -> bool {
    args.iter().any(|a| a == "-h" || a == "--help")
}

fn main() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    if wants_help(&args) {
        print!("{USAGE}");
        return Ok(());
    }
    // Initialize core app with a composite VFS (local + archives).
    let vfs = CompositeFs::new();
    let keymap = KeyMap::load_default();
    let mut app = App::new(Box::new(vfs), keymap)?;
    // GNU mc(1) skin order: ini (in App::new) → MC_SKIN → `-S`/`--skin`.
    apply_mc_skin_env(&mut app);
    apply_cli_args(&mut app, &args)?;
    TerminalApp::run(&mut app)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_flag_is_recognized() {
        assert!(wants_help(&["--help".into()]));
        assert!(wants_help(&["-h".into()]));
        assert!(wants_help(&["-d".into(), "--help".into()]));
        assert!(!wants_help(&["-d".into(), "--view".into(), "a".into()]));
        assert!(!wants_help(&["-hd".into()]));
    }

    #[test]
    fn usage_points_at_man_pages() {
        assert!(USAGE.contains("man mcr-edit"));
        assert!(USAGE.contains("man mcr-view"));
        assert!(USAGE.contains("man mcr-diff"));
        assert!(USAGE.contains("man -l docs/man/mcr.1"));
        assert!(USAGE.contains("docs/website/index.html"));
        assert!(
            USAGE.contains("[DIR1 [DIR2]]"),
            "help must document GNU positional panel directories"
        );
    }
}
