# Rusty Midnight Commander (mcr)

An Apache-2.0 licensed, clean-room Rust rewrite aiming for cell-accurate visual and interaction parity with GNU Midnight Commander (mc) for local files.

This repository ships the foundation: a working dual-pane terminal file manager with MC-fidelity chrome, basic file operations, and a modular crate layout for future features (viewer/editor/VFS/search).

## Building and Running

```bash
cargo run --release -p mcr
```

This launches `mcr` in your current working directory in a full-screen terminal UI.

## Current Feature Parity

- MC-style UI chrome: menu bar, two directory panels with frames and captions, mini-status lines, hint line, command line, and MC-like F-key bar.
- Navigation: arrows, PgUp/PgDn, Home/End, Tab, Enter, Backspace/Ctrl-PageUp for parent, Ctrl-H toggle hidden, Ctrl-U swap panels, Ctrl-R refresh.
- Sorting: by name/size/time.
- Viewer: minimal in-process viewer entry (stub; toggling hex/text is wired; content will evolve).
- File operations: APIs are present; dialogs are being built out in follow-up work.

## Roadmap

- mcedit-like editor (`rmc-edit`)
- Extended VFS backends (tar/zip/ftp/sftp)
- Find file, diff viewer, user menu/extfs
- Skins and keymaps as external data
- Subshell integration

## Development

Workspace crates:

- `mcr`: binary entrypoint
- `rmc-core`: app state, actions, keymap, events, panels
- `rmc-fs`: VFS trait and local filesystem backend
- `rmc-ui`: exact-MC renderer, dialogs, input handling
- `rmc-view`: in-app viewer plumbing
- `rmc-edit`: editor trait stub

Format, lint, and test:

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace
```

## Manual pages

Original Apache-2.0 manuals covering the shipped CLI (not copies of GNU
`mc`/`mcedit`/`mcview`/`mcdiff` GPL text):

```bash
man -l docs/man/mcr.1
man -l docs/man/mcr-edit.1   # mcedit equivalent: mcr --edit
man -l docs/man/mcr-view.1   # mcview equivalent: mcr --view
man -l docs/man/mcr-diff.1   # mcdiff equivalent: mcr --diff
```

`mcr --help` prints the same access hints. To install into the system
`man` path:

```bash
sudo install -D -m 644 docs/man/mcr.1 /usr/local/share/man/man1/mcr.1
sudo install -D -m 644 docs/man/mcr-edit.1 /usr/local/share/man/man1/mcr-edit.1
sudo install -D -m 644 docs/man/mcr-view.1 /usr/local/share/man/man1/mcr-view.1
sudo install -D -m 644 docs/man/mcr-diff.1 /usr/local/share/man/man1/mcr-diff.1
```

Then `man mcr`, `man mcr-edit`, `man mcr-view`, and `man mcr-diff`.

## License

Apache-2.0. This is not a derivative of Midnight Commander’s GPL C sources. We rely on public manuals and screenshots to match behavior and visuals.
