## Rusty Midnight Commander Parity Checklist (Acceptance Criteria)

This document is a clean-room, user-visible behavior checklist for achieving parity with GNU Midnight Commander (MC). It is sourced exclusively from public MC manuals and docs (mc(1), mcview(1), mcedit(1), mcdiff(1), midnight-commander.org, and the MC GitHub README). No GPL source code is included.

Status legend for each section: Foundation (in scope for the initial dual-pane local file manager with chrome and basic ops), Later (post-foundation), Not started (no work yet). All boxes are intentionally unchecked.

Primary sources to cross-check behavior:
- mc(1): latest manual (online dev version) — https://source.midnight-commander.org/man/mc.html
- mcview(1): internal viewer — https://source.midnight-commander.org/man/mcview.html
- mcedit(1): internal editor — e.g. Debian/Ubuntu manpages (same content as upstream)
- mcdiff(1): internal diff viewer — https://source.midnight-commander.org/man/mcdiff.html
- Project site — https://midnight-commander.org/
- Project README/docs — https://github.com/MidnightCommander/mc and https://github.com/MidnightCommander/website
- Default skin reference (visual spec) — https://github.com/MidnightCommander/mc/blob/master/misc/skins/default.ini

Note: Keys are shown using MC conventions: C- for Ctrl, Alt- for Meta/Alt, S- for Shift. Some key chords depend on terminal capability; MC supports “Learn keys” and Esc-number emulation for F-keys. Function keys F13–F20 typically map to Shift+F3…Shift+F10.

---

## 1) Screen chrome and layout — Status: Foundation

Overall structure and visible elements from mc(1) “Overview”, “Menu Bar”, “Directory Panels”, and function key labels:
- [ ] Menu bar top row, toggled with F9; menus: Left, File, Command, Options, Right
- [ ] Dual directory panels (current/other), selection bar indicates active panel
- [ ] Mini-status line (per-panel) showing selected file info and symlink targets
- [ ] Hint line (context/help hints when enabled)
- [ ] Shell command line (second line from bottom)
- [ ] Bottom F-key label bar (F1…F10)
- [ ] Default F-bar labels: Help, Menu, View, Edit, Copy, RenMov, Mkdir, Delete, PullDn, Quit
- [ ] Horizontal and vertical panel split layouts (Left/Right; Above/Below nomenclature)
- [ ] Screen repaint: C-l

Visual spec: Default skin colors (summarized from misc/skins/default.ini; names are foreground;background)
- [ ] Core: default lightgray;blue, selected black;cyan, marked yellow;blue, mark+selected yellow;cyan, header yellow;blue, frame lightgray;blue
- [ ] Dialogs: default black;lightgray, focus black;cyan, title blue;lightgray
- [ ] Menus: default white;cyan, selected white;black, hotkey yellow;cyan, hotkey+selected yellow;black
- [ ] Errors: default white;red (focus black;lightgray)
- [ ] Editor: default lightgray;blue (e.g., editbold yellow;green, editmarked black;cyan)
- [ ] Viewer: default lightgray;blue, selected yellow;cyan
Reference only; implement an equivalent Apache-2.0 skin description without copying MC skin files.

---

## 2) Panel listing modes, sort, filter, visibility — Status: Later

From mc(1) “Listing Format…”, “Panel modes”, “Sort Order…”, “Filter…”, “Panel options”, “Show hidden files”:
- [x] Listing formats: Full, Brief (1–9 cols), Long (ls -l–like), User-defined (field spec)
- [ ] User-defined fields: name, size/bsize, type marks (* / @ = - + | ~ !), mark, mtime/atime/ctime, perm, mode, nlink, owner/group (name/num), inode, etc.
- [x] Panel modes: Quick view, Info, Tree, Panelize (results)
  - [x] Quick view
  - [x] Info
  - [x] Tree
  - [x] Panelize (results)
  - [x] Quick view
  - [x] Info
- [x] Sort: name (alpha/natural), ext, mtime/atime/ctime, size, inode, unsorted; reverse toggle
- [x] Options: directories first vs “Mix all files”
- [x] Filter dialog: glob/regex, files only toggle, case sensitivity
- [x] Show hidden files toggle (dotfiles)
- [ ] Fast directory reload; manual rescan C-r
- [ ] Mouse: select (left), mark (right), double-click execute/open; Shift for terminal selection

---

## 3) Default keybindings worth shipping — Status: Later

Function keys (File menu shortcuts; Esc-1..0 emulates F1..F10):
- [ ] F1 Help; F2 User Menu; F3 View; F4 Edit; F5 Copy; F6 RenMov; F7 Mkdir; F8 Delete; F9 PullDn (menu bar); F10 Quit
- [ ] Shift-F variants (F13..F20) where relevant (e.g., F13 view-raw, F16 move to selected panel)

Directory panels (selection, navigation, layout):
- [ ] Tab / C-i / Left/Right: switch active panel
- [ ] Up/Down, PageUp/PageDown, Home/End movement
- [ ] Insert / C-t: toggle mark; “Mark moves down” option
- [ ] Alt-g / Alt-r / Alt-j: jump to top/middle/bottom item
- [ ] Alt-t: cycle listing format (Brief/Long/User/Full)
- [x] C-\\ (Ctrl-Backslash): Directory hotlist dialog
- [ ] ‘+’ Select group; ‘\\’ Unselect group; ‘*’ Invert selection
- [ ] Quick search: C-s / Alt-s starts; C-s repeats; wildcards * ?
- [ ] Refresh/rescan directory: C-r (when enabled)

Command line integration (filename helpers, history, completion):
- [x] Alt-Enter / C-Enter: copy selected filename to command line
- [x] C-S-Enter: copy full path
- [x] Alt-Tab: completion (var/user/host/cmd/filename), popup list as configured
- [x] Alt-p / Alt-n: command history prev/next; Alt-h: history list
- [x] C-x t / C-x C-t: copy tagged/selected filenames to command line (from current/other panel)
- [x] C-x p / C-x C-p: copy current/other panel path
- [x] C-q: quote next char literally into command line
- [x] Quick cd: Alt-c

Misc panel actions:
- [ ] C-l: full repaint
- [ ] C-x c: chmod dialog; C-x o: chown dialog
- [ ] C-x l: hardlink; C-x s: absolute symlink; C-x v: relative symlink
- [x] Swap panels (Command menu; Action::SwapPanels; honors Options → Panels → Simple swap)
- [x] Equalize panels (menu actions; include keys if assigned)

Subshell/screen toggle:
- [x] C-o: toggle panels vs subshell/output screen (when subshell enabled)

Tree/directory navigation (from tree and general movement keys):
- [ ] b/C-b/C-h/Backspace/Delete: page up; Space: page down; u/d: half-page; g/G: begin/end

Note: Keymaps customizable via mc.keymap; terminal “Learn keys” dialog required for some terms.

---

## 4) File operations and dialogs — Status: Foundation (basic ops), Later (the rest)

Dialogs and behavior per mc(1) “File Menu”, “File operations”, and replace/confirm dialogs:
- [ ] Copy (F5): source mask, destination (defaults to other panel), background option, “Preallocate space”, “Use COW file cloning”
- [ ] Rename/Move (F6): analogous to Copy dialog/options
- [ ] Delete (F8): confirmation; safe-delete option flips default to No
- [ ] Mkdir (F7): input with optional auto-name
- [ ] Replace/Overwrite dialog: Yes/No/All/Older/None/Smaller/Size differs/Append/Reget; “Don’t overwrite with zero length file”
- [ ] Background jobs manager: stop/restart/kill for copy/move
- [ ] Chmod (C-x c): recursive, perm bits UI
- [ ] Chown (C-x o): owner/group changes
- [ ] Links: hardlink (C-x l), absolute symlink (C-x s), relative symlink (C-x v)
- [ ] Compute totals, classic progress bar direction (appearance preference)

Mark each sub-feature internally with Foundation (Copy/Move/Delete/Mkdir) vs Later (chmod/chown/links/advanced options).

---

## 5) Internal viewer (mcview) — Status: Later

Per mcview(1) and mc(1) “Internal File Viewer”:
- [x] Modes: text, hex, wrap toggle, raw/parsed, format/unformat
- [x] Navigation: line/page/home/end, goto
- [x] Search: F7 Search dialog (Enter search string; Case sensitive / Backwards / Whole words / Regular expression), /, n/F17 next
- [x] View compressed via filters (e.g., gzip) per extension rules
- [x] Display options: show line numbers, underline/bold formatting, show CR as ^M
- [x] Selection and keybindings parity (F-keys and movement)

---

## 6) Internal editor (mcedit) — Status: Later

From mcedit(1):
- [x] Basics: open multiple files (up to size limits), binary-safe
- [x] Editing: block copy/move/delete/cut/paste; undo; insert/overwrite; autoindent; tab width
- [x] Syntax highlighting for common types
- [x] Macros; external filters (pipe regions to commands)
- [x] Search/replace with regex
- [x] Menu via F9
- [x] F2 save; F10 quit (and Esc Esc)

---

## 7) Virtual File System (VFS) — Status: Later

From README and mc(1):
- [x] Local filesystem
- [x] Archives (initial): tar, tar.gz/tgz, zip — browse; copy/extract out
- [ ] Archives (others): cpio, ar, rpm, lha, rar, zoo, deb (via extfs), etc.
- [ ] Remote: ftpfs (FTP), sftpfs (SFTP), fish (SSH-based)
- [x] extfs framework: minimal helper-driven VFS (list + copy-out)
- [x] Transparent enter-to-open for supported archives; “..” leaves the archive
- [ ] Panel operations on other VFS backends as if local (within VFS limits)
- [ ] Read-only vs read-write semantics per backend

---

## 8) Find file, content search, external panelize, directory tree — Status: Later

Per mc(1) “Find File”, “External panelize”, and “Directory Tree”:
- [x] Find File dialog: start dir (tree picker), filename pattern (glob/regex), content search string, case sensitivity, whole words
- [x] Exclude/ignore directories list (colon-separated), follow symlinks options
- [ ] Buttons: OK, Stop/Start, Again, Chdir, Panelize, Quit
- [x] Panelize results into panel; return to normal listing with “..” or switching mode
- [x] External Panelize: Ctrl-x !; run arbitrary shell command producing path list; save named commands; re-panelize results
- [x] Directory tree (Command menu figure): dedicated dialog (not panel Tree / not Find File); Enter chdirs current panel; Esc/F10 quit; C-r/F2 rescan; F3 Forget; F4 Static/Dynamic

---

## 9) Diff viewer (mcdiff) — Status: Later

From mcdiff(1) and mc(1) “Internal Diff Viewer”:
- [x] Side-by-side diff of two files via panels and CLI `mcr --diff file1 file2`
- [x] Navigate hunks: next/prev; goto line; search (n continues)
- [x] Merge current hunk (F5); swap sides (C-u); refresh (C-r)
- [x] Show hunk status; toggle line numbers; adjust panel widths
- [x] Edit in place: open left/right in editor; diffs update dynamically

---

## 10) Subshell / command line — Status: Later

From mc(1) “Shell Command Line”, “The subshell support”:
- [ ] Embedded subshell (bash, zsh, tcsh, fish, etc. where enabled)
- [x] Toggle panels vs subshell/output: C-o; suspend/return behavior
- [ ] SHELL override on invocation; -U/--subshell or -u/--nosubshell flags
- [ ] Command line editing keys (Emacs-like) and filename helpers (see Section 3)

---

## 11) Configuration, skins, keymap, user menu, extension file, hotlist — Status: Later

Per mc(1) “Configuration”, “Skins”, “Redefine hotkey bindings”, “User menu”, “Edit extension file”, “Hotlist”:
- [ ] Config files: system defaults vs user `~/.config/mc/ini` and related; auto-save setup
 - [ ] Skins: selectable appearance; support MC_SKIN; ship Apache-2.0-compatible default resembling MC default
 - [ ] Keymap: overridable via `mc.keymap` (search order), multiple bindings per action; “Learn keys” support
- [x] User menu (F2): `.mc.menu` (cwd) or `~/.config/mc/menu`, else system menu; minimal safety
- [x] Extension rules: `mc.ext.ini` to open helper-defined VFS (minimal)
- [x] Directory hotlist: C-\\ opens; C-x h adds current; manage labels and jump
- [ ] Environment variables respected (e.g., MC_SKIN, MC_KEYMAP, MC_COLOR_TABLE, MC_DATADIR, MC_PROFILE_ROOT, EDITOR, VIEWER)

---

## 12) Help — Status: Later

From mc(1):
- [x] Built-in hypertext help (F1) context-sensitive
- [ ] Online manpages: mc, mcedit, mcview, mcdiff
- [ ] Project website docs

---

## Non-goals and compliance notes

- This file is documentation only; no GPL source or MC configuration files are copied here. Color names and feature lists are factual excerpts from manuals and public docs, summarized for parity tracking.
- “Foundation” scope is strictly dual-pane local files, screen chrome, and basic file operations (F5/F6/F7/F8 and essential UI wiring). All other features are tagged Later.

