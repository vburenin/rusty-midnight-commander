# Rusty Midnight Commander (rmc)

An Apache-2.0 licensed, clean-room Rust rewrite aiming for cell-accurate visual and interaction parity with GNU Midnight Commander (mc) for local files.

This repository ships the foundation: a working dual-pane terminal file manager with MC-fidelity chrome, basic file operations, and a modular crate layout for future features (viewer/editor/VFS/search).

## Building and Running

```bash
cargo run --release --bin mcr
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

## License

Apache-2.0. This is not a derivative of Midnight Commander’s GPL C sources. We rely on public manuals and screenshots to match behavior and visuals.
