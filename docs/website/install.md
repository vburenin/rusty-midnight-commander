# Install and build

`mcr` is a Cargo workspace binary. There is no separate installer in
this repository; build from a git checkout.

HTML twin: [install.html](install.html).

## Requirements

- A Unix-like terminal (the UI is full-screen; it is not a web app)
- Rust and Cargo matching [`rust-toolchain.toml`](../../rust-toolchain.toml)
  (currently 1.98.0, with `rustfmt` and `clippy`)
- Git, to clone the tree

## Build and run

```bash
git clone https://github.com/vburenin/rusty-midnight-commander.git
cd rusty-midnight-commander
cargo run --release -p mcr
```

That launches `mcr` in the current working directory. The release binary
is `target/release/mcr` after `cargo build --release -p mcr`.

```bash
mcr --help
mcr -h
```

## Useful flags

Unknown flags are ignored so a later `--edit`, `--view`, or `--diff` is
still honored. Last of `-U`/`-u` wins.

| Flag | Meaning |
| --- | --- |
| `-d`, `--nomouse` | Do not capture the mouse for this run |
| `-u`, `--nosubshell` | Do not attach a PTY subshell |
| `-U`, `--subshell` | Enable the PTY subshell (Ctrl-O) |
| `-S name`, `--skin=name` | Skin name for this process (overrides `MC_SKIN` and user ini) |
| `-v file`, `--view=file` | Start the internal viewer (not `$PAGER`) |
| `-e [file…]`, `--edit` | Start the internal editor; no files opens one untitled buffer |
| `--diff file1 file2` | Start the internal side-by-side diff viewer |

Environment variables honored for paths and helpers include `SHELL`,
`MC_SKIN`, `MC_KEYMAP`, `MC_COLOR_TABLE`, `MC_DATADIR`,
`MC_PROFILE_ROOT`, `EDITOR`, and `VIEWER`.

## Configuration files

Setup is loaded in order: compiled-in defaults, then the first existing
system file (`/etc/mc/mc.ini`, else `$MC_DATADIR/mc.ini` or
`/usr/share/mc/mc.ini`), then the user file `~/.config/mc/ini`.
`MC_PROFILE_ROOT` (absolute) relocates the profile; otherwise
`$XDG_CONFIG_HOME/mc` or `~/.config/mc`. `MCR_CONFIG_DIR` overrides the
setup directory for tests.

Options → Save setup writes the user ini (creating `~/.config/mc/` as
needed). Quit does the same when Options → Panels → Auto save setup is
on. Unknown keys and sections are preserved, including Left/Right panel
modes and appearance.

## Manual pages

From a source checkout:

```bash
man -l docs/man/mcr.1
man -l docs/man/mcr-edit.1
man -l docs/man/mcr-view.1
man -l docs/man/mcr-diff.1
```

To install into the system `man` path:

```bash
sudo install -D -m 644 docs/man/mcr.1 /usr/local/share/man/man1/mcr.1
sudo install -D -m 644 docs/man/mcr-edit.1 /usr/local/share/man/man1/mcr-edit.1
sudo install -D -m 644 docs/man/mcr-view.1 /usr/local/share/man/man1/mcr-view.1
sudo install -D -m 644 docs/man/mcr-diff.1 /usr/local/share/man/man1/mcr-diff.1
```

Index: [man.md](man.md).

## Checks used in CI

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

## This website locally

```bash
python3 -m http.server --directory docs 8000
```

Then open `http://127.0.0.1:8000/` (redirects to `/website/`) or
`http://127.0.0.1:8000/website/`.

GitHub Pages can publish the `/docs` folder on `main`. `docs/index.html`
redirects here; `docs/.nojekyll` keeps the tree static (no Jekyll).
