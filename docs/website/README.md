# rusty-midnight-commander website

Original Apache-2.0 project documentation for **`mcr`**. This is not a
copy of the GNU Midnight Commander website or of the GPL `mc` / `mcedit`
/ `mcview` / `mcdiff` manuals.

On GitHub these Markdown files render in the browser. The same content
is also static HTML for GitHub Pages or a local file server:

| Page | HTML | Markdown |
| --- | --- | --- |
| What `mcr` is | [index.html](index.html) | this file |
| Install and build | [install.html](install.html) | [install.md](install.md) |
| Feature map vs mc(1) | [features.html](features.html) | [features.md](features.md) |
| Manual pages | [man.html](man.html) | [man.md](man.md) |

Shipped groff manuals: [`docs/man/`](../man/). Live parity checklist:
[`docs/PARITY.md`](../PARITY.md).

## What `mcr` is

`mcr` is the binary of rusty-midnight-commander: an Apache-2.0,
clean-room Rust rewrite that aims for cell-accurate visual and
interaction parity with GNU Midnight Commander for the features it
already ships. GNU Midnight Commander remains a separate work; this
project tracks public manuals, not GPL C sources.

With no options it opens a full-screen terminal UI on the current
directory: two panels, menu bar (F9), mini-status, hint line, command
line, and F1–F10 labels. The same binary also starts the internal
viewer (`mcr --view` / F3), editor (`mcr --edit` / F4), and side-by-side
diff (`mcr --diff`).

**In this tree:** dual-pane chrome, listings and file ops (F5/F6/F7/F8
go through VFS), viewer / editor / diff / Find File, directory tree,
panelize, user menu, hotlist, skins and keymaps, user `ini` load/save,
optional PTY subshell (Ctrl-O), F1 help, local full read/write, archive
and extfs browse + extract (writes refused as read-only; not zoo), and
FTP/SFTP/fish copy-in and other panel ops when the server allows.

**Not claimed:** zoo archives (no Apache-2.0/MIT decoder; PARITY.md
stays unchecked). See [PARITY.md](../PARITY.md) for any other open
items.

## Quick start

```bash
cargo run --release -p mcr
```

Details: [install.md](install.md). Feature map: [features.md](features.md).
Manuals: [man.md](man.md).

## Local HTML preview

From the repository root:

```bash
python3 -m http.server --directory docs 8000
```

Open `http://127.0.0.1:8000/website/`. GitHub Pages can publish the
`/docs` folder; `docs/index.html` redirects here.

## License

Apache License 2.0. rusty-midnight-commander is not a derivative of GNU
Midnight Commander’s GPL C sources.
