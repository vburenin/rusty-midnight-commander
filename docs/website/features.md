# Feature map vs GNU mc(1)

High-level only. Names below are the public GNU manuals this project
tracks; they are separate GPL works and are not shipped here. Tick-level
detail lives in [`docs/PARITY.md`](../PARITY.md).

HTML twin: [features.html](features.html).

## Shipped in current `mcr`

| Area | GNU reference | What `mcr` does |
| --- | --- | --- |
| Dual-pane chrome | mc(1) overview / panels | Menu bar, two panels, mini-status (including symlink targets), hint line, command line, F-key bar, horizontal or vertical split, Ctrl-L repaint. `mcr DIR1 [DIR2]` sets the current then other panel directories |
| Listings | mc(1) listing format / sort / filter | Full, Brief, Long, user-defined fields; sort; filter; hidden files; mouse; `-d`/`--nomouse` |
| File ops | mc(1) File menu | Copy, rename/move, mkdir, delete (F5/F6/F7/F8 go through VFS), chmod/chown, links, overwrite dialog, background jobs, progress totals |
| Viewer | mcview(1) | F3 or `mcr --view`: text/hex, wrap, search, filters — not `$PAGER` for `--view` |
| Editor | mcedit(1) | F4 or `mcr --edit`: buffers, search/replace, syntax, macros, F9 menu |
| Diff | mcdiff(1) | Panel pair or `mcr --diff file1 file2`: hunks, merge, swap, in-place edit |
| Find / tree / panelize | mc(1) Find File, tree, External panelize | Find File dialog, directory-tree figure, Ctrl-X ! panelize |
| VFS (local + archives) | mc(1) Virtual File System | Enter to browse; `..` leaves; copy-out to local. tar, tar.gz/tgz, zip, cpio, ar, rpm, deb, iso, rar, 7z, lha/lzh and extfs stay browse+extract — create/delete/copy-in is refused with a GNU-class “Cannot … / Read-only file system” error |
| VFS (remote) | mc(1) ftpfs / sftpfs / fish | `ftp://` / `/#ftp:`, SFTP, and fish: browse, stat, copy-out, and copy-in / mkdir / rename / delete when the server allows |
| Subshell | mc(1) subshell | Ctrl-O PTY; `-U`/`-u`; `SHELL` selects the shell |
| Config | mc(1) Configuration | Compiled defaults, then system `mc.ini` (`/etc/mc/mc.ini` else `$MC_DATADIR` or `/usr/share/mc`), then user `~/.config/mc/ini` (`MC_PROFILE_ROOT` / `XDG_CONFIG_HOME` / `MCR_CONFIG_DIR`). Save setup and Auto save setup write the user ini without clobbering unknown keys |
| Skins / keys / menus | mc(1) Skins, keymap, user menu, hotlist | `-S` / `MC_SKIN`, `mc.keymap`, Learn keys, F2 user menu, extension file, directory hotlist |
| Help | mc(1) Help | F1 hypertext help; original man pages under [`docs/man/`](../man/) |

## Not claimed (unchecked or out of scope)

Do not treat these as done because GNU `mc` has them:

- **Zoo archives** — no Apache-2.0/MIT decoder in this tree; browse is not implemented. The PARITY.md zoo checkbox stays unchecked on purpose.

Anything still unchecked in [PARITY.md](../PARITY.md) is unfinished, even
if GNU Midnight Commander already does it. Archive write is *claimed as
refused* (read-only), not as missing: tar/zip/cpio/ar/rpm/lha/rar/deb/iso/7z
and extfs stay browse+extract.
