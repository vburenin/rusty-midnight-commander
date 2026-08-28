# Feature map vs GNU mc(1)

High-level only. Names below are the public GNU manuals this project
tracks; they are separate GPL works and are not shipped here. Tick-level
detail lives in [`docs/PARITY.md`](../PARITY.md).

HTML twin: [features.html](features.html).

## Shipped in current `mcr`

| Area | GNU reference | What `mcr` does |
| --- | --- | --- |
| Dual-pane chrome | mc(1) overview / panels | Menu bar, two panels, mini-status (including symlink targets), hint line, command line, F-key bar, horizontal or vertical split, Ctrl-L repaint |
| Listings | mc(1) listing format / sort / filter | Full, Brief, Long, user-defined fields; sort; filter; hidden files; mouse; `-d`/`--nomouse` |
| File ops | mc(1) File menu | Copy, rename/move, mkdir, delete, chmod/chown, links, overwrite dialog, background jobs, progress totals |
| Viewer | mcview(1) | F3 or `mcr --view`: text/hex, wrap, search, filters — not `$PAGER` for `--view` |
| Editor | mcedit(1) | F4 or `mcr --edit`: buffers, search/replace, syntax, macros, F9 menu |
| Diff | mcdiff(1) | Panel pair or `mcr --diff file1 file2`: hunks, merge, swap, in-place edit |
| Find / tree / panelize | mc(1) Find File, tree, External panelize | Find File dialog, directory-tree figure, Ctrl-X ! panelize |
| VFS (local + archives) | mc(1) Virtual File System | Enter to browse; `..` leaves; copy-out to local. tar, tar.gz/tgz, zip, cpio, ar, rpm, deb, iso, rar, 7z, lha/lzh. extfs helpers for list + copy-out |
| VFS (remote) | mc(1) ftpfs / sftpfs / fish | Browse, stat, and copy-out for `ftp://`, SFTP, and fish. Upload/write is not shipped |
| Subshell | mc(1) subshell | Ctrl-O PTY; `-U`/`-u`; `SHELL` selects the shell |
| Skins / keys / menus | mc(1) Skins, keymap, user menu, hotlist | `-S` / `MC_SKIN`, `mc.keymap`, Learn keys, F2 user menu, extension file, directory hotlist |
| Help | mc(1) Help | F1 hypertext help; original man pages under [`docs/man/`](../man/) |

## Not claimed (unchecked or out of scope)

Do not treat these as done because GNU `mc` has them:

- **Zoo archives** — no Apache-2.0/MIT decoder in this tree; browse is not implemented
- **Writing into archives or remote VFS** — copy/create/delete *into* an archive, and FTP/SFTP/fish upload, are follow-ups. Panel operations on non-local backends are not “as if local”
- **Full config auto-save** — system defaults vs user `~/.config/mc/ini` auto-save remains an open parity item (section 11)

Anything still unchecked in [PARITY.md](../PARITY.md) is unfinished, even
if GNU Midnight Commander already does it.
