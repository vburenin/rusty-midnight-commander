# Architecture

This project is a Cargo workspace with clear separation of concerns and an extension surface for future features.

## Crates

- `rmc` — binary. Very small `main.rs` that wires `rmc-core` and `rmc-ui`.
- `rmc-core` — core application state and logic:
  - `App` holds two `PanelState`s (left/right), active side, and global toggles.
  - `actions` defines semantic actions (navigation, file ops, dialogs).
  - `config` exposes theme and keymap as data providers.
  - `panel` and `sorting` implement listings, cursor movement, selection.
  - No terminal I/O here; pure state manipulation enables testing.
- `rmc-fs` — virtual filesystem trait `Vfs` and the local backend:
  - Operations: `list_dir`, `stat`, `copy`, `move_path`, `mkdir`, `remove`, `read_file`, `write_file`.
  - Designed so archives/remote backends can be added later.
- `rmc-ui` — terminal renderer and event loop:
  - Paints cells with `crossterm` and custom routines to match GNU MC chrome exactly.
  - Avoids generic widget abstractions where they harm fidelity (e.g., F-key bar halves).
  - Handles resize, menu bar, dialogs, and mouse.
- `rmc-view` — placeholder for the in-process viewer. The UI invokes it for F3.
- `rmc-edit` — placeholder trait for a future in-process editor (F4).

## Rendering

Rendering is done by writing colored cells directly using `crossterm` to match the default MC skin:

- Single-line frames with centered path captions.
- Headers “Name / Size / Modify time”.
- Selection/marking colors per `misc/skins/default.ini`.
- Mini-status line inside each panel’s footer.
- Disk gauge line, hint line, command line, and the split-color F-key bar.

## Testing

- Unit tests in `rmc-fs` and `rmc-core` cover directory ops and panel sorting/selection/keymap.
- An integration-style test constructs `App` and drives navigation without a TTY.

## Extensibility

- New VFS backends implement `Vfs`.
- Skins and keymaps will become external files (TOML/INI) in a future PR.
- Viewer/editor extend through separate crates with traits/APIs.
