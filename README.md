# Rusty Midnight Commander (mcr)

An Apache-2.0 licensed, clean-room Rust rewrite aiming for cell-accurate visual and interaction parity with GNU Midnight Commander (mc).

This is not a derivative of Midnight Commander’s GPL C sources. Behavior is tracked from public manuals (mc(1), mcview(1), mcedit(1), mcdiff(1)).

## Project website

Original Apache-2.0 documentation (not a copy of the GNU Midnight Commander website or GPL manuals):

- [What `mcr` is](docs/website/README.md) ([HTML](docs/website/index.html))
- [Install and build](docs/website/install.md) ([HTML](docs/website/install.html))
- [Feature map vs mc(1)](docs/website/features.md) ([HTML](docs/website/features.html))
- [Manual pages](docs/website/man.md) ([HTML](docs/website/man.html)) — indexes the groff sources under [`docs/man/`](docs/man/)

The live parity checklist is [`docs/PARITY.md`](docs/PARITY.md). GitHub Pages can publish the `/docs` folder on `main`; [`docs/index.html`](docs/index.html) redirects to the site.

## Building and Running

```bash
cargo run --release -p mcr
```

This launches `mcr` in your current working directory in a full-screen terminal UI. More detail is in the [install](docs/website/install.md) page.

## Development

Workspace crates:

- `mcr`: binary entrypoint
- `rmc-core`: app state, actions, keymap, events, panels
- `rmc-fs`: VFS trait and local filesystem backend
- `rmc-ui`: exact-MC renderer, dialogs, input handling
- `rmc-view`: in-app viewer
- `rmc-edit`: in-app editor
- `rmc-diff`: in-app side-by-side diff viewer

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
