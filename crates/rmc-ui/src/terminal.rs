use crate::help::{global_index, HelpItem};
use crate::render::{viewer_menu_from_x, Renderer};
use crate::skin::load_default_palette;
use anyhow::Result;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use rmc_core::actions::{Action, PaneSide};
use rmc_core::app::{
    App, EditorGotoDialog, EditorGotoFocus, EditorMenu, EditorPipeDialog, EditorPipeFocus,
    EditorReplaceDialog, EditorReplaceFocus, EditorSaveAsDialog, EditorSaveAsFocus,
    EditorSearchDialog, EditorSearchFocus, HistoryDialogFocus, LayoutFocus, UiMode,
    ViewerDisplayDialog, ViewerDisplayFocus, ViewerMenu, ViewerSearchDialog, ViewerSearchFocus,
};
use rmc_core::find::{
    find_dialog_height, find_dialog_list_rows, search_files_streaming, CancelHandle,
    FindDialogFocus as FF, FindDialogState,
};
use rmc_core::hotlist::HotlistDialogFocus as HDF;
use rmc_core::layout::compute_chrome_geom;
use std::io::stdout;
use std::time::{Duration, Instant};

pub struct TerminalApp;

// Keep the current viewer's effective bytes alive across renders and filtering.
// Stores the original display path and the current ViewData (may reference a temp file).
pub(crate) struct ViewerState {
    pub display_path: std::path::PathBuf,
    pub view: rmc_view::ViewData,
}

use once_cell::sync::Lazy;
use std::sync::Mutex;
pub(crate) static VIEWER_STATE: Lazy<Mutex<Option<ViewerState>>> = Lazy::new(|| Mutex::new(None));

// Persistent PTY-backed subshell session for C-o console.
// Lives in the UI crate to avoid widening App's trait bounds or Debug/Clone surface.
pub(crate) static SUBSHELL_PTY: Lazy<Mutex<Option<rmc_core::subshell::PtySession>>> =
    Lazy::new(|| Mutex::new(None));

fn replace_opts(dlg: &EditorReplaceDialog) -> rmc_edit::SearchOptions {
    rmc_edit::SearchOptions {
        case_sensitive: dlg.case_sensitive,
        backwards: dlg.backwards,
        whole_words: dlg.whole_words,
        regexp: dlg.regular_expression,
    }
}

/// Replace the next match from the cursor, honoring dialog options. Empty
/// needle is a no-op (does not wipe the buffer). On success the cursor
/// advances past the replacement so another Replace finds the following match
/// (backwards: cursor stays at the replacement so the next search continues
/// toward the start). Dialog stays open.
fn editor_replace_next(
    buf: &mut rmc_edit::EditorBuffer,
    dlg: &mut EditorReplaceDialog,
) -> Option<String> {
    if dlg.search.is_empty() {
        return None;
    }
    match buf.replace_next_with_options(
        dlg.search.as_bytes(),
        dlg.replacement.as_bytes(),
        replace_opts(dlg),
        true,
    ) {
        Some(_) => {
            dlg.on_match = false;
            Some("Replaced".into())
        }
        None => {
            dlg.on_match = false;
            Some("Not found".into())
        }
    }
}

/// Replace remaining matches from the cursor. Empty needle is a no-op and
/// returns None so the caller can leave the dialog open without changing the
/// buffer.
fn editor_replace_all(
    buf: &mut rmc_edit::EditorBuffer,
    dlg: &EditorReplaceDialog,
) -> Option<usize> {
    if dlg.search.is_empty() {
        return None;
    }
    Some(buf.replace_all_with_options(
        dlg.search.as_bytes(),
        dlg.replacement.as_bytes(),
        replace_opts(dlg),
    ))
}

/// Skip this match: move to the next match without replacing. Empty needle is
/// a no-op. If none, status is `Not found` and the dialog stays open.
fn editor_replace_skip(
    buf: &mut rmc_edit::EditorBuffer,
    dlg: &mut EditorReplaceDialog,
) -> Option<String> {
    if dlg.search.is_empty() {
        return None;
    }
    let found = if dlg.on_match {
        let _ = buf.search_with_options(
            dlg.search.as_bytes(),
            dlg.case_sensitive,
            dlg.backwards,
            dlg.whole_words,
            dlg.regular_expression,
            false,
        );
        buf.search_next_opts(false)
    } else {
        buf.search_with_options(
            dlg.search.as_bytes(),
            dlg.case_sensitive,
            dlg.backwards,
            dlg.whole_words,
            dlg.regular_expression,
            false,
        )
    };
    match found {
        Some(_) => {
            dlg.on_match = true;
            Some("Found".into())
        }
        None => {
            dlg.on_match = false;
            Some("Not found".into())
        }
    }
}

/// Pipe selection (or whole buffer) through `cmd`. Empty command is a no-op
/// (does not wipe the file). On failure `pipe_selection` leaves the buffer
/// unchanged and the error text is returned for `status_msg`.
fn editor_pipe_run(buf: &mut rmc_edit::EditorBuffer, cmd: &str) -> Option<String> {
    let cmd = cmd.trim();
    if cmd.is_empty() {
        return None;
    }
    match buf.pipe_selection(cmd) {
        Ok(()) => None,
        Err(e) => Some(format!("{e}")),
    }
}

/// Run a Search-dialog query. Empty needle is a no-op (no cursor change).
/// On success/failure returns the GNU status line (`Found` / `Not found`).
fn editor_search_run(buf: &mut rmc_edit::EditorBuffer, dlg: &EditorSearchDialog) -> Option<String> {
    if dlg.search.is_empty() {
        return None;
    }
    match buf.search_with_options(
        dlg.search.as_bytes(),
        dlg.case_sensitive,
        dlg.backwards,
        dlg.whole_words,
        dlg.regular_expression,
        true,
    ) {
        Some(_) => Some("Found".into()),
        None => Some("Not found".into()),
    }
}

fn viewer_search_opts(
    case_sensitive: bool,
    backwards: bool,
    whole_words: bool,
    regexp: bool,
) -> rmc_view::SearchOptions {
    rmc_view::SearchOptions {
        case_sensitive,
        backwards,
        whole_words,
        regexp,
    }
}

/// Run a viewer Search-dialog query. Empty needle is a no-op (no offset change).
/// On success/failure returns the GNU status line (`Found` / `Not found`).
#[allow(clippy::too_many_arguments)]
fn viewer_search_run(
    path: &std::path::Path,
    offset: &mut u64,
    search: &mut Option<String>,
    case_sensitive: &mut bool,
    backwards: &mut bool,
    whole_words: &mut bool,
    regexp: &mut bool,
    dlg: &ViewerSearchDialog,
) -> anyhow::Result<Option<String>> {
    if dlg.search.is_empty() {
        return Ok(None);
    }
    *search = Some(dlg.search.clone());
    *case_sensitive = dlg.case_sensitive;
    *backwards = dlg.backwards;
    *whole_words = dlg.whole_words;
    *regexp = dlg.regular_expression;
    let cpath = crate::terminal::viewer_ensure_view_for(path);
    let opts = viewer_search_opts(
        dlg.case_sensitive,
        dlg.backwards,
        dlg.whole_words,
        dlg.regular_expression,
    );
    match rmc_view::search_with_options(&cpath, *offset, &dlg.search, opts, true)? {
        Some(pos) => {
            *offset = pos;
            Ok(Some("Found".into()))
        }
        None => Ok(Some("Not found".into())),
    }
}

fn viewer_search_next(
    path: &std::path::Path,
    offset: &mut u64,
    needle: &str,
    opts: rmc_view::SearchOptions,
) -> anyhow::Result<Option<String>> {
    if needle.is_empty() {
        return Ok(None);
    }
    let cpath = crate::terminal::viewer_ensure_view_for(path);
    match rmc_view::search_next_with_options(&cpath, *offset, needle, opts, true)? {
        Some(pos) => {
            *offset = pos;
            Ok(Some("Found".into()))
        }
        None => Ok(Some("Not found".into())),
    }
}

/// Write the editor buffer to `path` and update `buf.path`. Errors are
/// returned as text for `status_msg` — never panic.
fn editor_save_to_path(
    vfs: &dyn rmc_fs::Vfs,
    buf: &mut rmc_edit::EditorBuffer,
    path: &std::path::Path,
) -> Result<(), String> {
    let mut w = vfs.write_file(path).map_err(|e| e.to_string())?;
    use std::io::Write;
    w.write_all(&buf.to_bytes()).map_err(|e| e.to_string())?;
    buf.path = Some(path.to_path_buf());
    buf.dirty = false;
    Ok(())
}

fn editor_ui_mode(buf: rmc_edit::EditorBuffer, return_to: Option<Box<UiMode>>) -> UiMode {
    UiMode::Editor {
        buf,
        show_menu: None,
        status_msg: None,
        search_input: None,
        save_as_dialog: None,
        search_dialog: None,
        replace_dialog: None,
        pipe_dialog: None,
        goto_dialog: None,
        pending_quit: false,
        confirm_exit: None,
        return_to,
    }
}

/// GNU mc(1) Internal Diff Viewer: F4 edits the left file; F14 (Shift-F4) the right.
/// Returns `Some(true)` for the right pane, `Some(false)` for the left.
fn diff_edit_right_side(key: &KeyEvent) -> Option<bool> {
    match key.code {
        KeyCode::F(14) => Some(true),
        KeyCode::F(4) if key.modifiers.contains(KeyModifiers::SHIFT) => Some(true),
        KeyCode::F(4) if key.modifiers.is_empty() => Some(false),
        _ => None,
    }
}

fn read_vfs_bytes(app: &App, path: &std::path::Path) -> Vec<u8> {
    let mut data = Vec::new();
    if let Ok(mut r) = app.vfs.read_file(path) {
        use std::io::Read;
        let _ = r.read_to_end(&mut data);
    }
    data
}

fn read_vfs_text(app: &App, path: &std::path::Path) -> String {
    String::from_utf8_lossy(&read_vfs_bytes(app, path)).into_owned()
}

fn write_diff_file(app: &mut App, path: &std::path::Path, lines: &[String]) -> Result<()> {
    let mut w = app
        .vfs
        .write_file(path)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    use std::io::Write;
    let _ = w.write_all(rmc_diff::join_lines(lines).as_bytes());
    Ok(())
}

fn flush_diff_modified(app: &mut App, state: &mut rmc_core::app::DiffState) -> Result<()> {
    if state.left_modified {
        write_diff_file(app, &state.left_path, &state.left_lines)?;
        state.left_modified = false;
    }
    if state.right_modified {
        write_diff_file(app, &state.right_path, &state.right_lines)?;
        state.right_modified = false;
    }
    Ok(())
}

/// Re-read both sides from disk and rebuild hunks. Cursor hunk is clamped
/// unless `reset_cursor` (C-r style).
fn reload_diff_from_disk(app: &mut App, reset_cursor: bool) {
    let (left_path, right_path) = match &app.ui_mode {
        UiMode::Diff(s) => (s.left_path.clone(), s.right_path.clone()),
        _ => return,
    };
    let ltxt = read_vfs_text(app, &left_path);
    let rtxt = read_vfs_text(app, &right_path);
    let UiMode::Diff(state) = &mut app.ui_mode else {
        return;
    };
    state.left_lines = rmc_diff::split_lines(&ltxt);
    state.right_lines = rmc_diff::split_lines(&rtxt);
    state.hunks = rmc_diff::compute_diff(&ltxt, &rtxt).hunks;
    state.left_modified = false;
    state.right_modified = false;
    if reset_cursor {
        state.current_hunk = 0;
        state.left_scroll = 0;
        state.right_scroll = 0;
    } else if state.hunks.is_empty() {
        state.current_hunk = 0;
    } else {
        state.current_hunk = state.current_hunk.min(state.hunks.len() - 1);
        TerminalApp::ensure_hunk_visible(state);
    }
}

fn leave_editor(app: &mut App) {
    let prev = match &mut app.ui_mode {
        UiMode::Editor { return_to, .. } => return_to.take(),
        _ => None,
    };
    match prev {
        Some(boxed) => {
            app.ui_mode = *boxed;
            if matches!(app.ui_mode, UiMode::Diff(_)) {
                reload_diff_from_disk(app, false);
            }
        }
        None => app.ui_mode = UiMode::Normal,
    }
}

fn open_diff_side_in_editor(app: &mut App, right: bool) -> Result<()> {
    let UiMode::Diff(mut state) = std::mem::replace(&mut app.ui_mode, UiMode::Normal) else {
        return Ok(());
    };
    if let Err(e) = flush_diff_modified(app, &mut state) {
        app.ui_mode = UiMode::Diff(state);
        return Err(e);
    }
    let path = if right {
        state.right_path.clone()
    } else {
        state.left_path.clone()
    };
    let data = read_vfs_bytes(app, &path);
    let buf = rmc_edit::EditorBuffer::from_bytes(&data, Some(path));
    app.ui_mode = editor_ui_mode(buf, Some(Box::new(UiMode::Diff(state))));
    Ok(())
}

/// GNU mcedit Save as is F12 (and Shift-F2, which terminals often send as F12).
fn is_editor_save_as_key(key: &KeyEvent) -> bool {
    matches!(key.code, KeyCode::F(12))
        || (matches!(key.code, KeyCode::F(2)) && key.modifiers.contains(KeyModifiers::SHIFT))
}

/// Open the Save as dialog, clearing other editor overlays. No-op if already open.
#[allow(clippy::too_many_arguments)]
fn editor_open_save_as_dialog(
    buf: &rmc_edit::EditorBuffer,
    search_input: &mut Option<String>,
    save_as_dialog: &mut Option<Box<EditorSaveAsDialog>>,
    search_dialog: &mut Option<Box<EditorSearchDialog>>,
    replace_dialog: &mut Option<Box<EditorReplaceDialog>>,
    pipe_dialog: &mut Option<EditorPipeDialog>,
    goto_dialog: &mut Option<Box<EditorGotoDialog>>,
    status_msg: &mut Option<String>,
    show_menu: &mut Option<EditorMenu>,
) {
    if save_as_dialog.is_some() {
        return;
    }
    *search_input = None;
    *search_dialog = None;
    *replace_dialog = None;
    *pipe_dialog = None;
    *goto_dialog = None;
    *status_msg = None;
    *show_menu = None;
    *save_as_dialog = Some(Box::new(EditorSaveAsDialog::from_buffer_path(
        buf.path.as_deref(),
    )));
}

/// Commit Save as OK. Empty path: no-op close. Existing dest: GNU overwrite
/// confirm when `confirm_overwrite` is on (unless dest is the current path).
/// Write errors leave the dialog open with an error status (never panic).
/// Returns true when the dialog should close.
fn editor_save_as_commit(
    vfs: &dyn rmc_fs::Vfs,
    confirm_overwrite: bool,
    buf: &mut rmc_edit::EditorBuffer,
    dlg: &mut EditorSaveAsDialog,
    status_msg: &mut Option<String>,
) -> bool {
    let dest = dlg.filename.trim();
    if dest.is_empty() {
        return true;
    }
    let path = std::path::PathBuf::from(dest);
    let same_as_current = buf.path.as_deref() == Some(path.as_path());
    if !same_as_current {
        match vfs.stat(&path) {
            Ok(meta) if meta.is_dir => {
                *status_msg = Some("Cannot save: destination is not a regular file".into());
                return false;
            }
            Ok(_) if confirm_overwrite => {
                dlg.overwrite = Some(rmc_core::app::YncDialog {
                    title: "Warning".into(),
                    message: "A file already exists with this name".into(),
                    focus: rmc_core::app::YncFocus::Yes,
                });
                return false;
            }
            Ok(_) | Err(_) => {}
        }
    }
    match editor_save_to_path(vfs, buf, &path) {
        Ok(()) => {
            *status_msg = Some("Saved".into());
            true
        }
        Err(e) => {
            *status_msg = Some(e);
            false
        }
    }
}

/// Apply a 1-based decimal line number. Empty or non-numeric is a no-op.
/// `0` and negatives clamp to line 1; past-EOF is clamped by `goto_line`.
fn editor_goto_apply(buf: &mut rmc_edit::EditorBuffer, line: &str) {
    let t = line.trim();
    if t.is_empty() {
        return;
    }
    let Ok(n) = t.parse::<i64>() else {
        return;
    };
    if n <= 0 {
        buf.goto_line(1);
    } else if let Ok(n) = usize::try_from(n) {
        buf.goto_line(n);
    } else {
        buf.goto_line(usize::MAX);
    }
}

// Encode key events into bytes suitable for a typical xterm-compatible PTY.
fn encode_key_for_pty(key: &KeyEvent) -> Option<Vec<u8>> {
    use crossterm::event::{KeyCode, KeyModifiers};
    match key.code {
        KeyCode::Enter => Some(vec![b'\r']),
        KeyCode::Backspace => Some(vec![0x7f]),
        KeyCode::Tab => Some(vec![b'\t']),
        KeyCode::Esc => Some(vec![0x1b]),
        KeyCode::Left => Some(b"\x1b[D".to_vec()),
        KeyCode::Right => Some(b"\x1b[C".to_vec()),
        KeyCode::Up => Some(b"\x1b[A".to_vec()),
        KeyCode::Down => Some(b"\x1b[B".to_vec()),
        KeyCode::Home => Some(b"\x1b[H".to_vec()),
        KeyCode::End => Some(b"\x1b[F".to_vec()),
        KeyCode::Delete => Some(b"\x1b[3~".to_vec()),
        KeyCode::Insert => Some(b"\x1b[2~".to_vec()),
        KeyCode::PageUp => Some(b"\x1b[5~".to_vec()),
        KeyCode::PageDown => Some(b"\x1b[6~".to_vec()),
        KeyCode::Char(c) => {
            // Control-modified ASCII letters: map to ^A..^Z
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                let uc = c.to_ascii_uppercase() as u8;
                if (b'@'..=b'_').contains(&uc) || uc.is_ascii_uppercase() {
                    // Common terminals map Ctrl-@..Ctrl-_ to 0x00..0x1f
                    let b = uc & 0x1f;
                    return Some(vec![b]);
                }
            }
            // Alt-modified: send ESC prefix + char
            if key.modifiers.contains(KeyModifiers::ALT) {
                let mut v = vec![0x1b];
                v.extend(c.to_string().as_bytes());
                return Some(v);
            }
            // Plain Unicode char => UTF-8 bytes
            Some(c.to_string().into_bytes())
        }
        _ => None,
    }
}

pub(crate) fn viewer_clear_state() {
    if let Ok(mut g) = VIEWER_STATE.lock() {
        *g = None;
    }
}

pub(crate) fn viewer_ensure_view_for(display_path: &std::path::Path) -> std::path::PathBuf {
    let mut g = VIEWER_STATE.lock().expect("viewer state mutex poisoned");
    let need_new = g
        .as_ref()
        .map(|s| s.display_path != display_path)
        .unwrap_or(true);
    if need_new {
        match rmc_view::ViewData::open_view(display_path) {
            Ok(view) => {
                let p = view.path().to_path_buf();
                *g = Some(ViewerState {
                    display_path: display_path.to_path_buf(),
                    view,
                });
                return p;
            }
            Err(_) => {
                // Filter failed: never stash compressed bytes as if they were text.
                // view_current_file refuses to enter Viewer in this case.
                if rmc_view::guess_filter_for_path(display_path).is_none() {
                    let view = rmc_view::ViewData::from_path(display_path.to_path_buf());
                    let p = view.path().to_path_buf();
                    *g = Some(ViewerState {
                        display_path: display_path.to_path_buf(),
                        view,
                    });
                    return p;
                }
                return display_path.to_path_buf();
            }
        }
    }
    g.as_ref()
        .map(|s| s.view.path().to_path_buf())
        .unwrap_or_else(|| display_path.to_path_buf())
}

pub(crate) fn viewer_apply_filter_to_current(cmd: &str) -> anyhow::Result<()> {
    let mut g = VIEWER_STATE.lock().expect("viewer state mutex poisoned");
    if let Some(state) = g.as_mut() {
        let filter = rmc_view::ExternalFilter::new("sh")
            .with_args(["-c".to_string(), cmd.to_string()])
            .with_input(rmc_view::FilterInput::Stdin);
        let new_view = rmc_view::ViewData::from_filter(state.view.path(), &filter)?;
        state.view = new_view;
        Ok(())
    } else {
        anyhow::bail!("No active viewer");
    }
}

/// Reload viewer bytes as GNU Raw (on-disk) or Parsed (`mc.ext` `[view]` filter).
pub(crate) fn viewer_reload_parsed(
    display_path: &std::path::Path,
    parsed: bool,
) -> anyhow::Result<()> {
    let view = if parsed {
        rmc_view::ViewData::open_view(display_path)?
    } else {
        rmc_view::ViewData::from_path(display_path.to_path_buf())
    };
    let mut g = VIEWER_STATE.lock().expect("viewer state mutex poisoned");
    *g = Some(ViewerState {
        display_path: display_path.to_path_buf(),
        view,
    });
    Ok(())
}

fn viewer_term_size(page_rows: usize) -> (u16, u16) {
    crossterm::terminal::size().unwrap_or((80, page_rows.max(1) as u16 + 3))
}

fn viewer_page_down(
    path: &std::path::Path,
    hex: bool,
    wrap: bool,
    offset: u64,
    page_rows: usize,
) -> anyhow::Result<u64> {
    let (cols, rows) = viewer_term_size(page_rows);
    let content_rows = rows.saturating_sub(3);
    if hex {
        Ok(offset.saturating_add(16u64 * (content_rows as u64)))
    } else {
        let cpath = crate::terminal::viewer_ensure_view_for(path);
        Ok(rmc_view::nav_page_down(
            &cpath,
            offset,
            cols.saturating_sub(2),
            content_rows,
            wrap,
        )?)
    }
}

fn viewer_page_up(
    path: &std::path::Path,
    hex: bool,
    wrap: bool,
    offset: u64,
    page_rows: usize,
) -> anyhow::Result<u64> {
    let (cols, rows) = viewer_term_size(page_rows);
    let content_rows = rows.saturating_sub(3);
    if hex {
        Ok(offset.saturating_sub(16u64 * (content_rows as u64)))
    } else {
        let cpath = crate::terminal::viewer_ensure_view_for(path);
        Ok(rmc_view::nav_page_up(
            &cpath,
            offset,
            cols.saturating_sub(2),
            content_rows,
            wrap,
        )?)
    }
}

fn viewer_goto_end(
    path: &std::path::Path,
    hex: bool,
    wrap: bool,
    page_rows: usize,
) -> anyhow::Result<u64> {
    let (cols, rows) = viewer_term_size(page_rows);
    let content_rows = rows.saturating_sub(3);
    let cpath = crate::terminal::viewer_ensure_view_for(path);
    if hex {
        let len = rmc_view::file_len(&cpath)?;
        Ok(len.saturating_sub(16u64 * (content_rows as u64)))
    } else {
        Ok(rmc_view::nav_end(
            &cpath,
            cols.saturating_sub(2),
            content_rows,
            wrap,
        )?)
    }
}

fn viewer_line_down(path: &std::path::Path, hex: bool, offset: u64) -> anyhow::Result<u64> {
    if hex {
        Ok(offset.saturating_add(16))
    } else {
        let cpath = crate::terminal::viewer_ensure_view_for(path);
        Ok(rmc_view::nav_line_down(&cpath, offset)?)
    }
}

fn viewer_line_up(path: &std::path::Path, hex: bool, offset: u64) -> anyhow::Result<u64> {
    if hex {
        Ok(offset.saturating_sub(16))
    } else {
        let cpath = crate::terminal::viewer_ensure_view_for(path);
        Ok(rmc_view::nav_line_up(&cpath, offset)?)
    }
}

fn viewer_sel_clear(anchor: &mut Option<u64>) {
    *anchor = None;
}

fn viewer_sel_extend(anchor: &mut Option<u64>, cursor: u64) {
    if anchor.is_none() {
        *anchor = Some(cursor);
    }
}

fn viewer_open_search_dialog(app: &mut App, backwards: bool) {
    if let UiMode::Viewer {
        search,
        search_dialog,
        search_prompt,
        goto_prompt,
        status_msg,
        viewer_menu,
        ..
    } = &mut app.ui_mode
    {
        *search_prompt = None;
        *goto_prompt = None;
        *status_msg = None;
        *viewer_menu = None;
        let mut dlg =
            ViewerSearchDialog::from_last_search(search.as_deref().unwrap_or("").as_bytes());
        dlg.backwards = backwards;
        *search_dialog = Some(Box::new(dlg));
    }
}

fn viewer_open_display_options(app: &mut App) {
    if let UiMode::Viewer {
        hex,
        wrap,
        show_line_numbers,
        show_cr,
        search_dialog: None,
        display_dialog,
        search_prompt,
        goto_prompt,
        viewer_menu,
        ..
    } = &mut app.ui_mode
    {
        let ln = *show_line_numbers;
        let cr = *show_cr;
        let wrap = *wrap;
        let hex = *hex;
        *search_prompt = None;
        *goto_prompt = None;
        *viewer_menu = None;
        *display_dialog = Some(Box::new(ViewerDisplayDialog::from_viewer(
            ln, cr, wrap, hex,
        )));
    }
}

fn viewer_move_vertical(app: &mut App, dir: i8, extend: bool) -> anyhow::Result<()> {
    if let UiMode::Viewer {
        path,
        hex,
        offset,
        sel_anchor,
        sel_cursor,
        ..
    } = &mut app.ui_mode
    {
        let from = *sel_cursor;
        if extend {
            viewer_sel_extend(sel_anchor, from);
        } else {
            viewer_sel_clear(sel_anchor);
        }
        let next = if dir < 0 {
            viewer_line_up(path, *hex, from)?
        } else {
            viewer_line_down(path, *hex, from)?
        };
        *sel_cursor = next;
        *offset = next;
    }
    Ok(())
}

fn viewer_move_horizontal(app: &mut App, dir: i8, extend: bool) -> anyhow::Result<()> {
    if let UiMode::Viewer {
        sel_anchor,
        sel_cursor,
        ..
    } = &mut app.ui_mode
    {
        let from = *sel_cursor;
        if extend {
            viewer_sel_extend(sel_anchor, from);
        } else {
            viewer_sel_clear(sel_anchor);
        }
        *sel_cursor = if dir < 0 {
            from.saturating_sub(1)
        } else {
            from.saturating_add(1)
        };
    }
    Ok(())
}

fn viewer_move_page(
    app: &mut App,
    down: bool,
    page_rows: usize,
    extend: bool,
) -> anyhow::Result<()> {
    if let UiMode::Viewer {
        path,
        hex,
        wrap,
        offset,
        sel_anchor,
        sel_cursor,
        ..
    } = &mut app.ui_mode
    {
        let from = *offset;
        if extend {
            viewer_sel_extend(sel_anchor, from);
        } else {
            viewer_sel_clear(sel_anchor);
        }
        let next = if down {
            viewer_page_down(path, *hex, *wrap, from, page_rows)?
        } else {
            viewer_page_up(path, *hex, *wrap, from, page_rows)?
        };
        *offset = next;
        *sel_cursor = next;
    }
    Ok(())
}

fn viewer_move_home(app: &mut App, extend: bool) -> anyhow::Result<()> {
    if let UiMode::Viewer {
        offset,
        sel_anchor,
        sel_cursor,
        ..
    } = &mut app.ui_mode
    {
        if extend {
            viewer_sel_extend(sel_anchor, *sel_cursor);
        } else {
            viewer_sel_clear(sel_anchor);
        }
        *offset = rmc_view::nav_home();
        *sel_cursor = *offset;
    }
    Ok(())
}

fn viewer_move_end(app: &mut App, page_rows: usize, extend: bool) -> anyhow::Result<()> {
    if let UiMode::Viewer {
        path,
        hex,
        wrap,
        offset,
        sel_anchor,
        sel_cursor,
        ..
    } = &mut app.ui_mode
    {
        if extend {
            viewer_sel_extend(sel_anchor, *sel_cursor);
        } else {
            viewer_sel_clear(sel_anchor);
        }
        let next = viewer_goto_end(path, *hex, *wrap, page_rows)?;
        *offset = next;
        *sel_cursor = next;
    }
    Ok(())
}

pub(crate) fn open_compare_dirs_dialog(app: &mut App) {
    app.ui_mode = UiMode::CompareDirsDialog {
        mode: rmc_core::app::CompareDirsMode::Quick,
        focus: rmc_core::app::CompareDirsFocus::RadioQuick,
    };
}

fn files_differ_contents(
    app: &App,
    p1: &std::path::Path,
    p2: &std::path::Path,
) -> anyhow::Result<bool> {
    use std::io::Read;
    let mut r1 = match app.vfs.read_file(p1) {
        Ok(r) => r,
        Err(_) => return Ok(true), // unreadable -> treat as different
    };
    let mut r2 = match app.vfs.read_file(p2) {
        Ok(r) => r,
        Err(_) => return Ok(true),
    };
    let mut b1 = [0u8; 64 * 1024];
    let mut b2 = [0u8; 64 * 1024];
    loop {
        let n1 = r1.read(&mut b1).unwrap_or(0);
        let n2 = r2.read(&mut b2).unwrap_or(0);
        if n1 != n2 {
            return Ok(true);
        }
        if n1 == 0 {
            break;
        }
        if b1[..n1] != b2[..n1] {
            return Ok(true);
        }
    }
    Ok(false)
}

fn run_compare_dirs(app: &mut App, mode: rmc_core::app::CompareDirsMode) -> anyhow::Result<()> {
    use std::collections::HashMap;
    // Clear selections on both panels first
    app.left.selection.clear();
    app.right.selection.clear();
    // Build name -> index maps (skip parent marker)
    let mut left_map: HashMap<String, usize> = HashMap::new();
    for (idx, ent) in app.left.entries.iter().enumerate() {
        if ent.is_parent_marker() {
            continue;
        }
        left_map.insert(ent.name.clone(), idx);
    }
    let mut right_map: HashMap<String, usize> = HashMap::new();
    for (idx, ent) in app.right.entries.iter().enumerate() {
        if ent.is_parent_marker() {
            continue;
        }
        right_map.insert(ent.name.clone(), idx);
    }
    // Union of names
    let mut names: Vec<String> = left_map.keys().chain(right_map.keys()).cloned().collect();
    names.sort();
    names.dedup();
    for name in names {
        let li = left_map.get(&name).copied();
        let ri = right_map.get(&name).copied();
        match (li, ri) {
            (Some(l), None) => {
                app.left.selection.select(l);
            }
            (None, Some(r)) => {
                app.right.selection.select(r);
            }
            (Some(l), Some(r)) => {
                // Both present
                let le = &app.left.entries[l];
                let re = &app.right.entries[r];
                // If one is dir and the other file -> mark both
                if le.is_dir != re.is_dir {
                    app.left.selection.select(l);
                    app.right.selection.select(r);
                    continue;
                }
                // When both directories: presence only; do not mark
                if le.is_dir && re.is_dir {
                    continue;
                }
                // Both files: compare per mode
                let differ = match mode {
                    rmc_core::app::CompareDirsMode::Quick => {
                        le.size != re.size || le.modified != re.modified
                    }
                    rmc_core::app::CompareDirsMode::SizeOnly => le.size != re.size,
                    rmc_core::app::CompareDirsMode::Thorough => {
                        if le.size != re.size {
                            true
                        } else {
                            files_differ_contents(app, &le.path, &re.path)?
                        }
                    }
                };
                if differ {
                    app.left.selection.select(l);
                    app.right.selection.select(r);
                }
            }
            (None, None) => { /* unreachable */ }
        }
    }
    Ok(())
}

// Hit-test helpers mirroring render.rs packing logic
fn menu_top_index_from_x(x: u16) -> Option<usize> {
    // Labels: " Left ", " File ", " Command ", " Options ", " Right "
    // Placed sequentially starting at x=0
    let items = [" Left ", " File ", " Command ", " Options ", " Right "];
    let mut cur = 0u16;
    for (i, it) in items.iter().enumerate() {
        let start = cur;
        let end = cur + it.len() as u16; // exclusive
        if x >= start && x < end {
            return Some(i);
        }
        cur = end;
    }
    None
}

/// Virtual FS "Always use ftp proxy": honor a non-empty host when the flag is on.
fn ftp_proxy_for_vfs_opts(always: bool, proxy_host: &str) -> Option<&str> {
    let host = proxy_host.trim();
    if always && !host.is_empty() {
        Some(host)
    } else {
        None
    }
}

// Minimal ~/.netrc parser: searches for `machine <host> login <user> password <pass>`
fn netrc_lookup(host: &str) -> Option<(String, String)> {
    let home = std::env::var("HOME").ok()?;
    let path = std::path::Path::new(&home).join(".netrc");
    let data = std::fs::read_to_string(path).ok()?;
    // Tokenize by whitespace; ignore comments starting with '#'
    let mut tokens = Vec::new();
    for line in data.lines() {
        let l = line.split('#').next().unwrap_or("").trim();
        if l.is_empty() {
            continue;
        }
        for t in l.split_whitespace() {
            tokens.push(t.to_string());
        }
    }
    let mut i = 0usize;
    while i < tokens.len() {
        if tokens[i].eq_ignore_ascii_case("machine") && i + 1 < tokens.len() {
            let mch = tokens[i + 1].clone();
            i += 2;
            let mut login: Option<String> = None;
            let mut pass: Option<String> = None;
            while i < tokens.len() {
                match tokens[i].as_str() {
                    "machine" => break, // next block
                    "login" if i + 1 < tokens.len() => {
                        login = Some(tokens[i + 1].clone());
                        i += 2;
                    }
                    "password" if i + 1 < tokens.len() => {
                        pass = Some(tokens[i + 1].clone());
                        i += 2;
                    }
                    _ => i += 1,
                }
            }
            if mch == host {
                if let (Some(u), Some(p)) = (login, pass) {
                    return Some((u, p));
                }
            }
        } else {
            i += 1;
        }
    }
    None
}
fn fbar_function_from_xy(app: &App, x: u16, y: u16, cols: u16, rows: u16) -> Option<u8> {
    // Hit only when keybar is visible and y matches computed fbar row
    let geom = compute_chrome_geom(cols, rows, &app.layout);
    let fbar_y = geom.fbar_row?;
    if y != fbar_y {
        return None;
    }
    // Packing from render::draw_fbar: number, label, space
    let labels = [
        "Help", "Menu", "View", "Edit", "Copy", "RenMov", "Mkdir", "Delete", "PullDn", "Quit",
    ];
    let mut cur_x = 0u16;
    for (i, lab) in labels.iter().enumerate() {
        let num_str = if i == 9 { "10" } else { &(i + 1).to_string() };
        let num_len = num_str.len() as u16;
        let lab_len = lab.len() as u16;
        let seg_start = cur_x;
        let seg_end = {
            let mut e = cur_x + num_len + lab_len;
            if e < cols {
                e += 1; // trailing space if fits
            }
            e.min(cols)
        };
        if x >= seg_start && x < seg_end {
            return Some(if i == 9 { 10 } else { (i + 1) as u8 });
        }
        cur_x = seg_end;
        if cur_x >= cols {
            break;
        }
    }
    None
}

impl TerminalApp {
    fn apply_sort_dialog(
        app: &mut App,
        side: rmc_core::actions::PaneSide,
        by: rmc_core::panel::SortBy,
        reverse: bool,
        dirs_first: bool,
    ) -> Result<()> {
        let p = if matches!(side, rmc_core::actions::PaneSide::Left) {
            &mut app.left
        } else {
            &mut app.right
        };
        p.sort_by = by;
        p.sort_dir = if reverse {
            rmc_core::sorting::SortDir::Desc
        } else {
            rmc_core::sorting::SortDir::Asc
        };
        p.dirs_first = dirs_first;
        p.apply_sort();
        Ok(())
    }
    pub fn run(app: &mut App) -> Result<()> {
        let mut out = stdout();
        enable_raw_mode()?;
        execute!(out, EnterAlternateScreen, EnableMouseCapture)?;
        let palette = load_default_palette();
        let mut renderer = Renderer::new(palette);
        let mut current_skin = app.skin_name.clone();
        let mut last_draw = Instant::now();
        // pending_ctrl_x lives on App; no local flag here
        // Double-click detection for listing rows
        let mut last_click_time: Option<Instant> = None;
        let mut last_click_target: Option<(PaneSide, usize)> = None;

        loop {
            // Compute content rows for page/scroll visibility (shared geometry)
            let (cols, rows) = crossterm::terminal::size()?;
            let geom = compute_chrome_geom(cols, rows, &app.layout);
            let panel_top = geom.panel_top;
            let content_bottom = geom.content_bottom;
            let panel_h = content_bottom - panel_top;
            let qs_active = app.quick_search.is_some();
            let left_rows = rmc_core::panel::panel_listing_content_rows(
                panel_h,
                rmc_core::panel::reserve_panel_mini_status(
                    app.panel_opts.show_mini_status,
                    matches!(app.active, PaneSide::Left),
                    qs_active,
                ),
            ) as usize;
            let right_rows = rmc_core::panel::panel_listing_content_rows(
                panel_h,
                rmc_core::panel::reserve_panel_mini_status(
                    app.panel_opts.show_mini_status,
                    matches!(app.active, PaneSide::Right),
                    qs_active,
                ),
            ) as usize;
            // Compute per-panel visible capacity (rows or 2*rows for Brief two-column)
            let mid = cols / 2;
            let left_w = mid;
            let right_w = cols - mid;
            let left_two_cols =
                matches!(app.left.listing, rmc_core::panel::ListingFormat::Brief) && left_w >= 30;
            let right_two_cols =
                matches!(app.right.listing, rmc_core::panel::ListingFormat::Brief) && right_w >= 30;
            let left_capacity = left_rows * if left_two_cols { 2 } else { 1 };
            let right_capacity = right_rows * if right_two_cols { 2 } else { 1 };
            // Ensure cursor visibility based on last-known height
            {
                let left = &mut app.left;
                left.ensure_visible(left_capacity);
                let right = &mut app.right;
                right.ensure_visible(right_capacity);
            }
            // While subshell full-screen is shown and PTY is alive, drain and append output.
            if app.subshell.show_output_screen {
                if let Ok(mut guard) = SUBSHELL_PTY.lock() {
                    if let Some(sess) = guard.as_mut() {
                        if sess.is_alive() {
                            let bytes = sess.drain_output();
                            if !bytes.is_empty() {
                                // Convert to text, strip simple CRs, and split into lines.
                                let text = String::from_utf8_lossy(&bytes).replace('\r', "");
                                for line in text.split_inclusive('\n') {
                                    let ln = if let Some(stripped) = line.strip_suffix('\n') {
                                        stripped.to_string()
                                    } else {
                                        line.to_string()
                                    };
                                    app.subshell.output_lines.push(ln);
                                }
                                // Cap buffer to a reasonable size (match Subshell::new default).
                                const CAP: usize = 10_000;
                                if app.subshell.output_lines.len() > CAP {
                                    let overflow = app.subshell.output_lines.len() - CAP;
                                    app.subshell.output_lines.drain(0..overflow);
                                }
                            }
                        }
                    }
                }
            }
            // Draw at least at 30 FPS-equivalent idle to react to resize
            // Also check for background find results to update UI even without keypresses
            if let UiMode::FindDialog(state) = &mut app.ui_mode {
                if state.running {
                    if let Some(rx) = &state.results_rx {
                        loop {
                            match rx.try_recv() {
                                Ok(p) => {
                                    state.results.paths.push(p);
                                    if state.selected_index >= state.results.paths.len() {
                                        state.selected_index =
                                            state.results.paths.len().saturating_sub(1);
                                    }
                                }
                                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                                    state.running = false;
                                    state.cancel = None;
                                    state.results_rx = None;
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            // Apply pending skin change by reloading palette
            if app.skin_name != current_skin {
                if let Some(path) = crate::skin::find_skin_path_by_name(&app.skin_name) {
                    if let Ok(pal) = crate::skin::load_from_file(&path) {
                        renderer.set_palette(pal);
                        current_skin = app.skin_name.clone();
                    } else {
                        // Fallback: default palette
                        let pal = crate::skin::load_default_palette();
                        renderer.set_palette(pal);
                        current_skin = app.skin_name.clone();
                    }
                } else {
                    // "default" or unknown -> default loader (handles MC_SKIN first)
                    let pal = crate::skin::load_default_palette();
                    renderer.set_palette(pal);
                    current_skin = app.skin_name.clone();
                }
            }
            // Poll the jobs worker without blocking the copy. Redraw the GNU mc
            // progress dialog on every tick so File/Total bars fill, and redraw
            // immediately when Abort or completion returns to Normal.
            let file_op = matches!(app.ui_mode, UiMode::FileOpProgress { .. });
            app.drive_pending_file_op()?;
            if last_draw.elapsed() > Duration::from_millis(33)
                || file_op
                || matches!(app.ui_mode, UiMode::FileOpProgress { .. })
            {
                renderer.draw(app)?;
                last_draw = Instant::now();
            }

            if event::poll(Duration::from_millis(50))? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        // Effective page size depends on active panel format/width
                        let active_capacity = match app.active {
                            rmc_core::actions::PaneSide::Left => left_capacity,
                            rmc_core::actions::PaneSide::Right => right_capacity,
                        };
                        Self::handle_key(app, key, active_capacity)?;
                    }
                    Event::Mouse(mev) => {
                        // Ignore mouse while subshell full-screen
                        if app.subshell.show_output_screen {
                            continue;
                        }
                        if matches!(app.ui_mode, UiMode::Viewer { .. }) {
                            let active_capacity = match app.active {
                                rmc_core::actions::PaneSide::Left => left_capacity,
                                rmc_core::actions::PaneSide::Right => right_capacity,
                            };
                            let _ = Self::handle_mouse(app, mev, active_capacity);
                            continue;
                        }
                        if !matches!(app.ui_mode, UiMode::Normal) {
                            continue;
                        }
                        // Coordinates
                        let mx = mev.column;
                        let my = mev.row;
                        // Top menu bar click: open the corresponding top menu
                        if app.layout.menubar_visible && my == 0 {
                            if let Some(top_idx) = menu_top_index_from_x(mx) {
                                if matches!(mev.kind, MouseEventKind::Down(MouseButton::Left)) {
                                    app.ui_mode = UiMode::Menu {
                                        top_index: top_idx,
                                        selected_index: 0,
                                        dropped: true,
                                    };
                                }
                            }
                            continue;
                        }
                        // Bottom function bar: dispatch F1..F10
                        if let Some(n) = fbar_function_from_xy(app, mx, my, cols, rows) {
                            if matches!(mev.kind, MouseEventKind::Down(MouseButton::Left)) {
                                let key = KeyEvent::new(KeyCode::F(n), KeyModifiers::NONE);
                                // Page size based on active panel for any actions that need it
                                let active_capacity = match app.active {
                                    PaneSide::Left => left_capacity,
                                    PaneSide::Right => right_capacity,
                                };
                                let _ = Self::handle_key(app, key, active_capacity);
                            }
                            continue;
                        }
                        // Panel rectangles
                        let mid = cols / 2;
                        let left_rect = (0u16, panel_top, left_w, (content_bottom - panel_top));
                        let right_rect = (mid, panel_top, right_w, (content_bottom - panel_top));
                        let in_rect =
                            |x: u16, y: u16, rx: u16, ry: u16, rw: u16, rh: u16| -> bool {
                                x >= rx
                                    && x < rx.saturating_add(rw)
                                    && y >= ry
                                    && y < ry.saturating_add(rh)
                            };
                        let mut target_side: Option<PaneSide> = None;
                        if in_rect(mx, my, left_rect.0, left_rect.1, left_rect.2, left_rect.3) {
                            target_side = Some(PaneSide::Left);
                        } else if in_rect(
                            mx,
                            my,
                            right_rect.0,
                            right_rect.1,
                            right_rect.2,
                            right_rect.3,
                        ) {
                            target_side = Some(PaneSide::Right);
                        }
                        // Scroll wheel over a panel: move cursor and activate that panel
                        if matches!(
                            mev.kind,
                            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                        ) {
                            if let Some(side) = target_side {
                                app.active = side;
                                let up = matches!(mev.kind, MouseEventKind::ScrollUp);
                                if up {
                                    app.handle_action(Action::MoveUp)?;
                                } else {
                                    app.handle_action(Action::MoveDown)?;
                                }
                                // Best-effort ensure visible now (renderer will also fix up)
                                if matches!(side, PaneSide::Left) {
                                    app.left.ensure_visible(left_capacity);
                                } else {
                                    app.right.ensure_visible(right_capacity);
                                }
                            }
                            continue;
                        }
                        // Left/right click inside listing content: hit test row/column
                        if let Some(side) = target_side {
                            // Activate the clicked panel
                            app.active = side;
                            // Panel geometry
                            let (px, _py, pw, ph) = if matches!(side, PaneSide::Left) {
                                left_rect
                            } else {
                                right_rect
                            };
                            // Listing content area
                            let content_top = _py + 2;
                            let content_h = rmc_core::panel::panel_listing_content_rows(
                                ph,
                                rmc_core::panel::reserve_panel_mini_status(
                                    app.panel_opts.show_mini_status,
                                    true,
                                    app.quick_search.is_some(),
                                ),
                            );
                            // Only rows within listing body move the cursor / toggle / enter
                            if my >= content_top && my < content_top.saturating_add(content_h) {
                                let row_i = (my - content_top) as usize;
                                // Decide column for Brief two-column format
                                let (two_cols, per_col_width) = {
                                    let w = pw;
                                    if w >= 30
                                        && matches!(
                                            if matches!(side, PaneSide::Left) {
                                                app.left.listing
                                            } else {
                                                app.right.listing
                                            },
                                            rmc_core::panel::ListingFormat::Brief
                                        )
                                    {
                                        (true, (w - 3) / 2)
                                    } else {
                                        (false, w - 2)
                                    }
                                };
                                // Compute target index
                                let (scroll_top, entries_len) = if matches!(side, PaneSide::Left) {
                                    (app.left.scroll_top, app.left.entries.len())
                                } else {
                                    (app.right.scroll_top, app.right.entries.len())
                                };
                                let mut idx: Option<usize> = None;
                                if two_cols {
                                    let inner_x = mx.saturating_sub(px);
                                    let right_col_start = 2 + per_col_width;
                                    let use_right = inner_x >= right_col_start; // >= x+2+per_col_width
                                    let base = if use_right {
                                        scroll_top + row_i + content_h as usize
                                    } else {
                                        scroll_top + row_i
                                    };
                                    if base < entries_len {
                                        idx = Some(base);
                                    }
                                } else {
                                    let base = scroll_top + row_i;
                                    if base < entries_len {
                                        idx = Some(base);
                                    }
                                }
                                // Apply left/right button semantics
                                if let Some(sel_idx) = idx {
                                    // Move cursor to clicked row
                                    if matches!(side, PaneSide::Left) {
                                        app.left.cursor = sel_idx;
                                        app.left.ensure_visible(left_capacity);
                                    } else {
                                        app.right.cursor = sel_idx;
                                        app.right.ensure_visible(right_capacity);
                                    }
                                    match mev.kind {
                                        MouseEventKind::Down(MouseButton::Left) => {
                                            let now = Instant::now();
                                            let is_double = if let (Some(t0), Some((s0, i0))) =
                                                (last_click_time, last_click_target)
                                            {
                                                now.duration_since(t0) <= Duration::from_millis(400)
                                                    && s0 == side
                                                    && i0 == sel_idx
                                            } else {
                                                false
                                            };
                                            last_click_time = Some(now);
                                            last_click_target = Some((side, sel_idx));
                                            if is_double {
                                                let key = KeyEvent::new(
                                                    KeyCode::Enter,
                                                    KeyModifiers::NONE,
                                                );
                                                let page = if matches!(side, PaneSide::Left) {
                                                    left_capacity
                                                } else {
                                                    right_capacity
                                                };
                                                let _ = Self::handle_key(app, key, page);
                                            }
                                        }
                                        MouseEventKind::Down(MouseButton::Right) => {
                                            // Toggle mark (Insert) without entering
                                            let key =
                                                KeyEvent::new(KeyCode::Insert, KeyModifiers::NONE);
                                            let page = if matches!(side, PaneSide::Left) {
                                                left_capacity
                                            } else {
                                                right_capacity
                                            };
                                            let _ = Self::handle_key(app, key, page);
                                        }
                                        _ => {}
                                    }
                                }
                                continue;
                            } else {
                                // Click within panel but outside listing body: just activate
                                continue;
                            }
                        }
                        // Otherwise ignore
                        // nothing
                    }
                    Event::Resize(c, r) => {
                        // Resize PTY if alive; redraw next loop
                        if let Ok(mut guard) = SUBSHELL_PTY.lock() {
                            if let Some(sess) = guard.as_mut() {
                                let _ = sess.resize(r, c);
                            }
                        }
                    }
                    _ => {}
                }
            }

            if app.quit {
                break;
            }
        }

        // Best-effort: kill PTY session on quit.
        if let Ok(mut guard) = SUBSHELL_PTY.lock() {
            if let Some(sess) = guard.as_mut() {
                let _ = sess.kill();
            }
            *guard = None;
        }
        disable_raw_mode()?;
        execute!(out, LeaveAlternateScreen, DisableMouseCapture)?;
        Ok(())
    }

    fn file_op_progress_mode(
        vfs: &dyn rmc_fs::Vfs,
        jobs: &rmc_core::jobs::JobQueue,
        opts: &rmc_core::app::ConfigOptions,
        op: rmc_core::app::CopyMoveOp,
        src: std::path::PathBuf,
        dst: std::path::PathBuf,
    ) -> UiMode {
        match rmc_core::fileop::FileOpProgressState::prepare(vfs, op, &src, &dst, opts) {
            Ok(state) => {
                let job_id = match op {
                    rmc_core::app::CopyMoveOp::Copy => jobs.spawn_copy(&src, &dst),
                    rmc_core::app::CopyMoveOp::Move => jobs.spawn_move(&src, &dst),
                };
                UiMode::FileOpProgress {
                    op,
                    src,
                    dst,
                    state,
                    started: true,
                    job_id,
                }
            }
            Err(err) => UiMode::DialogConfirm {
                title: "Error".into(),
                message: format!("{err}"),
                on_ok: Box::new(|_| Ok(())),
            },
        }
    }

    fn handle_mouse(app: &mut App, mev: MouseEvent, page_rows: usize) -> Result<()> {
        if !matches!(mev.kind, MouseEventKind::Down(MouseButton::Left)) {
            return Ok(());
        }
        if !matches!(
            app.ui_mode,
            UiMode::Viewer {
                search_dialog: None,
                display_dialog: None,
                goto_prompt: None,
                ..
            }
        ) {
            return Ok(());
        }
        // GNU mc(1): menu bar appears if you click the topmost line.
        if mev.row == 0 {
            if let UiMode::Viewer { viewer_menu, .. } = &mut app.ui_mode {
                *viewer_menu = Some(viewer_menu_from_x(mev.column));
            }
            return Ok(());
        }
        let (_cols, rows) = viewer_term_size(page_rows);
        if mev.row == rows.saturating_sub(1) {
            // F-bar click: packed number+label+space like draw_viewer_fbar.
            if let UiMode::Viewer {
                wrap,
                hex,
                parsed,
                format_nroff,
                ..
            } = &app.ui_mode
            {
                let labels = crate::render::viewer_fbar_labels(*wrap, *hex, *parsed, *format_nroff);
                let mut x = 0u16;
                let mut hit = None;
                for (i, lab) in labels.iter().enumerate() {
                    let num = if i == 9 { "10" } else { &(i + 1).to_string() };
                    let w = num.len() as u16 + lab.len() as u16 + 1;
                    if mev.column >= x && mev.column < x.saturating_add(w) {
                        hit = Some((i + 1) as u8);
                        break;
                    }
                    x = x.saturating_add(w);
                }
                if let Some(n) = hit {
                    return Self::handle_key(
                        app,
                        KeyEvent::new(KeyCode::F(n), KeyModifiers::NONE),
                        page_rows,
                    );
                }
            }
        }
        Ok(())
    }

    fn handle_key(app: &mut App, key: KeyEvent, page_rows: usize) -> Result<()> {
        let active_cwd = app.active_panel().cwd.clone();
        // Subshell full-screen mode: C-o toggles back; support PgUp/PgDn/Up/Down scrolling.
        if app.subshell.show_output_screen {
            match key.code {
                KeyCode::Char('o')
                    if key
                        .modifiers
                        .contains(crossterm::event::KeyModifiers::CONTROL) =>
                {
                    app.handle_action(Action::ToggleSubshell)?;
                }
                KeyCode::PageUp => {
                    let (_c, r) = crossterm::terminal::size()?;
                    app.subshell.scroll_page_up(r as usize);
                }
                KeyCode::PageDown => {
                    let (_c, r) = crossterm::terminal::size()?;
                    app.subshell.scroll_page_down(r as usize);
                }
                KeyCode::Up => app.subshell.scroll_page_up(1),
                KeyCode::Down => app.subshell.scroll_page_down(1),
                _ => {
                    // Forward other keys to the live PTY session, if any.
                    if let Ok(mut guard) = SUBSHELL_PTY.lock() {
                        if let Some(sess) = guard.as_mut() {
                            if sess.is_alive() {
                                if let Some(bytes) = encode_key_for_pty(&key) {
                                    let _ = sess.write(&bytes);
                                }
                            }
                        }
                    }
                }
            }
            return Ok(());
        }
        // After a waited external: any key dismisses so panels can redraw.
        if matches!(app.ui_mode, UiMode::PauseAfterRun) {
            app.ui_mode = UiMode::Normal;
            return Ok(());
        }
        // GNU mc Abort: honor Esc/F10/A/[ Abort ] while the transfer is in-flight.
        // Handled before the ui_mode match so we can call App::abort_file_op.
        if matches!(app.ui_mode, UiMode::FileOpProgress { .. }) {
            match key.code {
                KeyCode::Esc
                | KeyCode::F(10)
                | KeyCode::Enter
                | KeyCode::Char('a')
                | KeyCode::Char('A') => {
                    app.abort_file_op()?;
                }
                _ => {}
            }
            return Ok(());
        }
        // GNU mcdiff F4 / F14: open the existing editor on that side, then
        // return here and rebuild hunks from disk. Skip while Diff overlays
        // (save-confirm / search / goto) are open so those keep their keys.
        if let UiMode::Diff(state) = &app.ui_mode {
            let overlays_idle = state.confirm_exit.is_none()
                && state.search_prompt.is_none()
                && state.goto_prompt.is_none();
            if overlays_idle {
                if let Some(right) = diff_edit_right_side(&key) {
                    return open_diff_side_in_editor(app, right);
                }
            }
        }
        // Dialog handling first
        match &mut app.ui_mode {
            UiMode::LearnKeysDialog {
                draft,
                selected,
                capturing,
                focus_ok,
            } => {
                let rows_len = draft.len();
                // When capturing, take next key except Esc; F10 should not quit.
                if *capturing {
                    match key.code {
                        KeyCode::Esc => {
                            *capturing = false;
                        }
                        _ => {
                            if *selected < rows_len {
                                // Update binding for this row in draft
                                draft[*selected].1 = key;
                            }
                            *capturing = false;
                        }
                    }
                    return Ok(());
                }
                // Not capturing: navigate and accept/cancel
                match key.code {
                    KeyCode::Esc | KeyCode::F(10) => {
                        app.ui_mode = UiMode::Normal;
                    }
                    KeyCode::Up => {
                        if *selected == rows_len {
                            // Move from buttons to last row
                            if rows_len > 0 {
                                *selected = rows_len - 1;
                            }
                        } else if *selected > 0 {
                            *selected -= 1;
                        }
                    }
                    KeyCode::Down => {
                        if *selected < rows_len.saturating_sub(1) {
                            *selected += 1;
                        } else {
                            // Move focus to buttons
                            *selected = rows_len;
                        }
                    }
                    KeyCode::Tab => {
                        if *selected < rows_len {
                            *selected = rows_len; // move to buttons
                            *focus_ok = true;
                        } else {
                            *focus_ok = !*focus_ok;
                        }
                    }
                    KeyCode::Left => {
                        if *selected == rows_len {
                            *focus_ok = true;
                        }
                    }
                    KeyCode::Right => {
                        if *selected == rows_len {
                            *focus_ok = false;
                        }
                    }
                    KeyCode::Enter | KeyCode::Char(' ') => {
                        if *selected < rows_len {
                            // Start capturing next key
                            *capturing = true;
                        } else {
                            if *focus_ok {
                                // Apply: for each action, unbind previous keys then set the new one.
                                let pairs: Vec<(
                                    rmc_core::actions::Action,
                                    crossterm::event::KeyEvent,
                                )> = draft.clone();
                                // Close the dialog first.
                                app.ui_mode = UiMode::Normal;
                                for (act, keyev) in pairs {
                                    app.keymap.remove_action_bindings(&act);
                                    app.keymap.set_binding(keyev, act);
                                }
                            } else {
                                // Cancel
                                app.ui_mode = UiMode::Normal;
                            }
                        }
                    }
                    _ => {}
                }
                return Ok(());
            }
            UiMode::Editor {
                buf,
                show_menu,
                status_msg,
                search_input,
                save_as_dialog,
                search_dialog,
                replace_dialog,
                pipe_dialog,
                goto_dialog,
                pending_quit: _,
                confirm_exit,
                return_to: _,
            } => {
                // Confirm-exit overlay (F10/q on dirty)
                if let Some(c) = confirm_exit {
                    use rmc_core::app::YncFocus as F;
                    match key.code {
                        KeyCode::Esc | KeyCode::F(10) => {
                            // Close overlay only
                            *confirm_exit = None;
                        }
                        KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
                            c.focus = match c.focus {
                                F::Yes => F::No,
                                F::No => F::Cancel,
                                F::Cancel => F::Yes,
                            };
                        }
                        KeyCode::Enter => {
                            match c.focus {
                                F::Yes => {
                                    // Save, then restore Diff (or panels).
                                    if let Some(path) = &buf.path {
                                        let mut w = app
                                            .vfs
                                            .write_file(path)
                                            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
                                        use std::io::Write;
                                        let _ = w.write_all(&buf.to_bytes());
                                    }
                                    leave_editor(app);
                                }
                                F::No => {
                                    leave_editor(app);
                                }
                                F::Cancel => {
                                    *confirm_exit = None;
                                }
                            }
                        }
                        _ => {}
                    }
                    return Ok(());
                }
                // F12 / Shift-F2 opens Save as even when another overlay is up,
                // clearing Search / Replace / Pipe / Goto / leftover Find.
                // No-op when Save as is already open (do not nest).
                if is_editor_save_as_key(&key) {
                    if save_as_dialog.is_none() {
                        editor_open_save_as_dialog(
                            buf,
                            search_input,
                            save_as_dialog,
                            search_dialog,
                            replace_dialog,
                            pipe_dialog,
                            goto_dialog,
                            status_msg,
                            show_menu,
                        );
                    }
                    return Ok(());
                }
                // GNU mcedit F4 Replace dialog (Replace / All / Skip / Cancel +
                // four Search checkboxes). Replace next / Skip keep the dialog
                // open; All closes after reporting how many replacements were made.
                if let Some(dlg) = replace_dialog {
                    use EditorReplaceFocus as F;
                    let order = [
                        F::Search,
                        F::Replacement,
                        F::CaseSensitive,
                        F::Backwards,
                        F::WholeWords,
                        F::RegularExpression,
                        F::Replace,
                        F::All,
                        F::Skip,
                        F::Cancel,
                    ];
                    let mut idx = order.iter().position(|f0| *f0 == dlg.focus).unwrap_or(0);
                    match key.code {
                        KeyCode::Esc | KeyCode::F(10) => {
                            *replace_dialog = None;
                        }
                        KeyCode::Tab | KeyCode::Down => {
                            idx = (idx + 1) % order.len();
                            dlg.focus = order[idx];
                        }
                        KeyCode::BackTab | KeyCode::Up => {
                            idx = (idx + order.len() - 1) % order.len();
                            dlg.focus = order[idx];
                        }
                        KeyCode::Left | KeyCode::Right if dlg.focus.is_button() => {
                            let buttons = [F::Replace, F::All, F::Skip, F::Cancel];
                            let bidx = buttons.iter().position(|f0| *f0 == dlg.focus).unwrap_or(0);
                            let next = if matches!(key.code, KeyCode::Right) {
                                (bidx + 1) % buttons.len()
                            } else {
                                (bidx + buttons.len() - 1) % buttons.len()
                            };
                            dlg.focus = buttons[next];
                        }
                        KeyCode::Backspace => match dlg.focus {
                            F::Search => {
                                dlg.search.pop();
                                dlg.on_match = false;
                            }
                            F::Replacement => {
                                dlg.replacement.pop();
                            }
                            _ => {}
                        },
                        // Space toggles checkboxes before generic Char so typing
                        // still inserts a space into the search/replacement fields.
                        KeyCode::Char(' ')
                            if key.modifiers.is_empty() && dlg.focus.is_checkbox() =>
                        {
                            let _ = dlg.toggle_focused_checkbox();
                        }
                        KeyCode::Enter if dlg.focus.is_checkbox() => {
                            let _ = dlg.toggle_focused_checkbox();
                        }
                        KeyCode::Enter | KeyCode::Char(' ')
                            if dlg.focus.is_button() || matches!(key.code, KeyCode::Enter) =>
                        {
                            match dlg.focus {
                                F::Cancel => {
                                    *replace_dialog = None;
                                }
                                F::All => {
                                    if let Some(n) = editor_replace_all(buf, dlg) {
                                        *status_msg = Some(format!("Replaced all: {n}"));
                                        *replace_dialog = None;
                                    }
                                }
                                F::Skip => {
                                    if let Some(msg) = editor_replace_skip(buf, dlg) {
                                        *status_msg = Some(msg);
                                    }
                                }
                                F::Search | F::Replacement | F::Replace => {
                                    if let Some(msg) = editor_replace_next(buf, dlg) {
                                        *status_msg = Some(msg);
                                    }
                                }
                                _ => {}
                            }
                        }
                        KeyCode::Char(c)
                            if key.modifiers.is_empty()
                                && matches!(dlg.focus, F::Search | F::Replacement) =>
                        {
                            match dlg.focus {
                                F::Search => {
                                    dlg.search.push(c);
                                    dlg.on_match = false;
                                }
                                F::Replacement => dlg.replacement.push(c),
                                _ => {}
                            }
                        }
                        _ => {}
                    }
                    return Ok(());
                }
                // GNU mcedit `|` Pipe dialog (filter selection / whole buffer).
                if let Some(dlg) = pipe_dialog {
                    use EditorPipeFocus as F;
                    let order = [F::Command, F::Ok, F::Cancel];
                    let mut idx = order.iter().position(|f0| *f0 == dlg.focus).unwrap_or(0);
                    match key.code {
                        KeyCode::Esc | KeyCode::F(10) => {
                            *pipe_dialog = None;
                        }
                        KeyCode::Tab | KeyCode::Down => {
                            idx = (idx + 1) % order.len();
                            dlg.focus = order[idx];
                        }
                        KeyCode::BackTab | KeyCode::Up => {
                            idx = (idx + order.len() - 1) % order.len();
                            dlg.focus = order[idx];
                        }
                        KeyCode::Left | KeyCode::Right
                            if matches!(dlg.focus, F::Ok | F::Cancel) =>
                        {
                            dlg.focus = if matches!(dlg.focus, F::Ok) {
                                F::Cancel
                            } else {
                                F::Ok
                            };
                        }
                        KeyCode::Backspace if matches!(dlg.focus, F::Command) => {
                            dlg.command.pop();
                        }
                        KeyCode::Enter | KeyCode::Char(' ')
                            if matches!(dlg.focus, F::Ok | F::Cancel)
                                || matches!(key.code, KeyCode::Enter) =>
                        {
                            match dlg.focus {
                                F::Cancel => {
                                    *pipe_dialog = None;
                                }
                                F::Command | F::Ok => {
                                    let cmd = dlg.command.clone();
                                    *pipe_dialog = None;
                                    if let Some(msg) = editor_pipe_run(buf, &cmd) {
                                        *status_msg = Some(msg);
                                    }
                                }
                            }
                        }
                        KeyCode::Char(c)
                            if !key
                                .modifiers
                                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                                && matches!(dlg.focus, F::Command) =>
                        {
                            dlg.command.push(c);
                        }
                        _ => {}
                    }
                    return Ok(());
                }
                // GNU mcedit Alt-l Goto line dialog (OK / Cancel).
                if let Some(dlg) = goto_dialog {
                    use EditorGotoFocus as F;
                    let order = [F::Line, F::Ok, F::Cancel];
                    let mut idx = order.iter().position(|f0| *f0 == dlg.focus).unwrap_or(0);
                    match key.code {
                        KeyCode::Esc | KeyCode::F(10) => {
                            *goto_dialog = None;
                        }
                        KeyCode::Tab | KeyCode::Down => {
                            idx = (idx + 1) % order.len();
                            dlg.focus = order[idx];
                        }
                        KeyCode::BackTab | KeyCode::Up => {
                            idx = (idx + order.len() - 1) % order.len();
                            dlg.focus = order[idx];
                        }
                        KeyCode::Left | KeyCode::Right
                            if matches!(dlg.focus, F::Ok | F::Cancel) =>
                        {
                            dlg.focus = if matches!(dlg.focus, F::Ok) {
                                F::Cancel
                            } else {
                                F::Ok
                            };
                        }
                        KeyCode::Backspace if matches!(dlg.focus, F::Line) => {
                            dlg.line.pop();
                        }
                        KeyCode::Enter | KeyCode::Char(' ')
                            if matches!(dlg.focus, F::Ok | F::Cancel)
                                || matches!(key.code, KeyCode::Enter) =>
                        {
                            match dlg.focus {
                                F::Cancel => {
                                    *goto_dialog = None;
                                }
                                F::Line | F::Ok => {
                                    let line = dlg.line.clone();
                                    *goto_dialog = None;
                                    editor_goto_apply(buf, &line);
                                }
                            }
                        }
                        KeyCode::Char(c)
                            if !key
                                .modifiers
                                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                                && matches!(dlg.focus, F::Line) =>
                        {
                            dlg.line.push(c);
                        }
                        _ => {}
                    }
                    return Ok(());
                }
                // GNU mcedit F7 Search dialog (OK / Cancel + four checkboxes).
                if let Some(dlg) = search_dialog {
                    use EditorSearchFocus as F;
                    let order = [
                        F::Search,
                        F::CaseSensitive,
                        F::Backwards,
                        F::WholeWords,
                        F::RegularExpression,
                        F::Ok,
                        F::Cancel,
                    ];
                    let mut idx = order.iter().position(|f0| *f0 == dlg.focus).unwrap_or(0);
                    match key.code {
                        KeyCode::Esc | KeyCode::F(10) => {
                            *search_dialog = None;
                        }
                        KeyCode::F(7) => {
                            // Already open: do not nest.
                        }
                        KeyCode::Tab | KeyCode::Down => {
                            idx = (idx + 1) % order.len();
                            dlg.focus = order[idx];
                        }
                        KeyCode::BackTab | KeyCode::Up => {
                            idx = (idx + order.len() - 1) % order.len();
                            dlg.focus = order[idx];
                        }
                        KeyCode::Left | KeyCode::Right
                            if matches!(dlg.focus, F::Ok | F::Cancel) =>
                        {
                            dlg.focus = if matches!(dlg.focus, F::Ok) {
                                F::Cancel
                            } else {
                                F::Ok
                            };
                        }
                        KeyCode::Backspace if matches!(dlg.focus, F::Search) => {
                            dlg.search.pop();
                        }
                        // Space toggles checkboxes before generic Char so typing
                        // still inserts a space into the search field.
                        KeyCode::Char(' ')
                            if key.modifiers.is_empty() && dlg.focus.is_checkbox() =>
                        {
                            let _ = dlg.toggle_focused_checkbox();
                        }
                        KeyCode::Enter if dlg.focus.is_checkbox() => {
                            let _ = dlg.toggle_focused_checkbox();
                        }
                        KeyCode::Enter | KeyCode::Char(' ')
                            if matches!(dlg.focus, F::Ok | F::Cancel)
                                || matches!(key.code, KeyCode::Enter) =>
                        {
                            match dlg.focus {
                                F::Cancel => {
                                    *search_dialog = None;
                                }
                                F::Search | F::Ok => {
                                    if let Some(msg) = editor_search_run(buf, dlg) {
                                        *status_msg = Some(msg);
                                    }
                                    *search_dialog = None;
                                }
                                _ => {}
                            }
                        }
                        KeyCode::Char(c)
                            if !key
                                .modifiers
                                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                                && matches!(dlg.focus, F::Search) =>
                        {
                            dlg.search.push(c);
                        }
                        _ => {}
                    }
                    return Ok(());
                }
                // Inline "Find:" overlay
                if let Some(q) = search_input {
                    match key.code {
                        KeyCode::Esc | KeyCode::F(10) => {
                            // Close overlay only
                            *search_input = None;
                        }
                        KeyCode::Enter => {
                            let needle = q.clone();
                            if !needle.is_empty() {
                                let _ = buf.search_forward_opts(needle.as_bytes(), false, true);
                                *status_msg = if buf.last_search.is_empty() {
                                    Some("Not found".into())
                                } else {
                                    Some("Found".into())
                                };
                            }
                            *search_input = None;
                        }
                        KeyCode::Backspace => {
                            q.pop();
                        }
                        KeyCode::Char(c) if key.modifiers.is_empty() => {
                            q.push(c);
                        }
                        _ => {}
                    }
                    return Ok(());
                }
                // GNU mcedit Save as dialog (F12 / Shift-F2).
                if let Some(dlg) = save_as_dialog {
                    if dlg.overwrite.is_some() {
                        use rmc_core::app::YncFocus as YF;
                        match key.code {
                            KeyCode::Esc | KeyCode::F(10) => {
                                dlg.overwrite = None;
                            }
                            KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
                                if let Some(c) = dlg.overwrite.as_mut() {
                                    c.focus = match c.focus {
                                        YF::Yes => YF::No,
                                        YF::No => YF::Cancel,
                                        YF::Cancel => YF::Yes,
                                    };
                                }
                            }
                            KeyCode::Enter | KeyCode::Char(' ') => {
                                let focus = dlg.overwrite.as_ref().map(|c| c.focus);
                                match focus {
                                    Some(YF::Yes) => {
                                        let path = std::path::PathBuf::from(dlg.filename.trim());
                                        match editor_save_to_path(app.vfs.as_ref(), buf, &path) {
                                            Ok(()) => {
                                                *status_msg = Some("Saved".into());
                                                *save_as_dialog = None;
                                            }
                                            Err(e) => {
                                                dlg.overwrite = None;
                                                *status_msg = Some(e);
                                            }
                                        }
                                    }
                                    Some(YF::No) | Some(YF::Cancel) => {
                                        dlg.overwrite = None;
                                    }
                                    None => {}
                                }
                            }
                            _ => {}
                        }
                        return Ok(());
                    }
                    use EditorSaveAsFocus as F;
                    let order = [F::Filename, F::Ok, F::Cancel];
                    let mut idx = order.iter().position(|f0| *f0 == dlg.focus).unwrap_or(0);
                    match key.code {
                        KeyCode::Esc | KeyCode::F(10) => {
                            *save_as_dialog = None;
                        }
                        KeyCode::F(12) => {
                            // Already open: do not nest.
                        }
                        KeyCode::F(2) if key.modifiers.contains(KeyModifiers::SHIFT) => {
                            // Already open: do not nest.
                        }
                        KeyCode::Tab | KeyCode::Down => {
                            idx = (idx + 1) % order.len();
                            dlg.focus = order[idx];
                        }
                        KeyCode::BackTab | KeyCode::Up => {
                            idx = (idx + order.len() - 1) % order.len();
                            dlg.focus = order[idx];
                        }
                        KeyCode::Left | KeyCode::Right
                            if matches!(dlg.focus, F::Ok | F::Cancel) =>
                        {
                            dlg.focus = if matches!(dlg.focus, F::Ok) {
                                F::Cancel
                            } else {
                                F::Ok
                            };
                        }
                        KeyCode::Backspace if matches!(dlg.focus, F::Filename) => {
                            dlg.filename.pop();
                        }
                        KeyCode::Enter | KeyCode::Char(' ')
                            if matches!(dlg.focus, F::Ok | F::Cancel)
                                || matches!(key.code, KeyCode::Enter) =>
                        {
                            match dlg.focus {
                                F::Cancel => {
                                    *save_as_dialog = None;
                                }
                                F::Filename | F::Ok => {
                                    let confirm_overwrite = app.confirm.overwrite;
                                    if editor_save_as_commit(
                                        app.vfs.as_ref(),
                                        confirm_overwrite,
                                        buf,
                                        dlg,
                                        status_msg,
                                    ) {
                                        *save_as_dialog = None;
                                    }
                                }
                            }
                        }
                        KeyCode::Char(c)
                            if !key
                                .modifiers
                                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                                && matches!(dlg.focus, F::Filename) =>
                        {
                            dlg.filename.push(c);
                        }
                        _ => {}
                    }
                    return Ok(());
                }
                // GNU mcedit F9 pull-down. Search/Replace/Pipe/Goto/Save as overlays
                // above take keys first so the menu cannot steal those dialogs.
                if let Some(menu) = *show_menu {
                    match key.code {
                        KeyCode::Esc | KeyCode::F(9) | KeyCode::F(10) => {
                            *show_menu = None;
                        }
                        KeyCode::Left => {
                            *show_menu = Some(menu.left());
                        }
                        KeyCode::Right => {
                            *show_menu = Some(menu.right());
                        }
                        KeyCode::Up => {
                            *show_menu = Some(menu.up());
                        }
                        KeyCode::Down => {
                            *show_menu = Some(menu.down());
                        }
                        KeyCode::Enter | KeyCode::Char(' ') => {
                            let item = menu.current_item();
                            *show_menu = None;
                            match item {
                                Some("Save") => {
                                    if let Some(path) = buf.path.clone() {
                                        match editor_save_to_path(app.vfs.as_ref(), buf, &path) {
                                            Ok(()) => *status_msg = Some("Saved".into()),
                                            Err(e) => *status_msg = Some(e),
                                        }
                                    } else {
                                        editor_open_save_as_dialog(
                                            buf,
                                            search_input,
                                            save_as_dialog,
                                            search_dialog,
                                            replace_dialog,
                                            pipe_dialog,
                                            goto_dialog,
                                            status_msg,
                                            show_menu,
                                        );
                                    }
                                }
                                Some("Save as") => {
                                    editor_open_save_as_dialog(
                                        buf,
                                        search_input,
                                        save_as_dialog,
                                        search_dialog,
                                        replace_dialog,
                                        pipe_dialog,
                                        goto_dialog,
                                        status_msg,
                                        show_menu,
                                    );
                                }
                                Some("Quit") => {
                                    if buf.dirty {
                                        *confirm_exit = Some(rmc_core::app::YncDialog {
                                            title: "Save modified file?".into(),
                                            message: "The file has unsaved changes. Save before leaving the editor?"
                                                .into(),
                                            focus: rmc_core::app::YncFocus::Yes,
                                        });
                                    } else {
                                        leave_editor(app);
                                    }
                                }
                                Some("Undo") => {
                                    if buf.undo() {
                                        *status_msg = Some("Undo".into());
                                    }
                                }
                                Some("Copy") => {
                                    if buf.copy_block_here() {
                                        *status_msg = Some("Copied block".into());
                                    }
                                }
                                Some("Move") => {
                                    if buf.move_block_here() {
                                        *status_msg = Some("Moved block".into());
                                    }
                                }
                                Some("Delete") => {
                                    if buf.delete_selection() {
                                        *status_msg = Some("Deleted block".into());
                                    }
                                }
                                Some("Mark") => {
                                    if buf.selection_bounds().is_none() {
                                        buf.mark_start();
                                        *status_msg = Some("Mark start".into());
                                    } else if buf.selection_bounds().is_some()
                                        && buf.last_search.is_empty()
                                    {
                                        buf.mark_end();
                                        *status_msg = Some("Mark end".into());
                                    } else {
                                        buf.clear_selection();
                                        *status_msg = Some("Unmark".into());
                                    }
                                }
                                Some("Search") => {
                                    *search_input = None;
                                    *save_as_dialog = None;
                                    *replace_dialog = None;
                                    *pipe_dialog = None;
                                    *goto_dialog = None;
                                    *status_msg = None;
                                    *search_dialog = Some(Box::new(
                                        EditorSearchDialog::from_last_search(&buf.last_search),
                                    ));
                                }
                                Some("Replace") => {
                                    *search_input = None;
                                    *save_as_dialog = None;
                                    *search_dialog = None;
                                    *pipe_dialog = None;
                                    *goto_dialog = None;
                                    *status_msg = None;
                                    *replace_dialog = Some(Box::new(
                                        EditorReplaceDialog::from_last_search(&buf.last_search),
                                    ));
                                }
                                Some("Go to line") => {
                                    *search_input = None;
                                    *save_as_dialog = None;
                                    *search_dialog = None;
                                    *replace_dialog = None;
                                    *pipe_dialog = None;
                                    *status_msg = None;
                                    *goto_dialog =
                                        Some(Box::new(EditorGotoDialog::from_cursor_row(buf.row)));
                                }
                                Some("Pipe") => {
                                    *search_input = None;
                                    *save_as_dialog = None;
                                    *search_dialog = None;
                                    *replace_dialog = None;
                                    *goto_dialog = None;
                                    *status_msg = None;
                                    *pipe_dialog = Some(EditorPipeDialog::default());
                                }
                                _ => {}
                            }
                        }
                        _ => {}
                    }
                    return Ok(());
                }
                // Base editor keys
                match key.code {
                    // GNU mcedit F9: drop File / Edit / Search / Command / Options
                    KeyCode::F(9) => {
                        *show_menu = Some(EditorMenu::default_open());
                    }
                    // MC: F7 search dialog
                    KeyCode::F(7) => {
                        *search_input = None;
                        *save_as_dialog = None;
                        *replace_dialog = None;
                        *pipe_dialog = None;
                        *goto_dialog = None;
                        *status_msg = None;
                        *search_dialog = Some(Box::new(EditorSearchDialog::from_last_search(
                            &buf.last_search,
                        )));
                    }
                    // GNU mcedit: Ctrl-R start/stop macro recording (toggle)
                    KeyCode::Char('r')
                        if key
                            .modifiers
                            .contains(crossterm::event::KeyModifiers::CONTROL) =>
                    {
                        if buf.start_macro_record() {
                            *status_msg = Some("Macro recording".into());
                        } else {
                            let n = buf.stop_macro_record();
                            *status_msg = Some(match n {
                                Some(cnt) => format!("Macro recorded ({cnt} events)"),
                                None => "Macro recording stopped".into(),
                            });
                        }
                    }
                    // GNU mcedit: Ctrl-A execute last macro
                    KeyCode::Char('a')
                        if key
                            .modifiers
                            .contains(crossterm::event::KeyModifiers::CONTROL) =>
                    {
                        if !buf.replay_macro() {
                            *status_msg = Some("No macro available".into());
                        }
                    }
                    // Shift-F7 (F19) or 'n': next match with wrap
                    KeyCode::F(19) | KeyCode::Char('n') => {
                        let _ = buf.search_next_opts(true);
                    }
                    // GNU mcedit: F4 opens Replace (F7 is Search). Prefill from last Search.
                    KeyCode::F(4) => {
                        *search_input = None;
                        *save_as_dialog = None;
                        *search_dialog = None;
                        *pipe_dialog = None;
                        *goto_dialog = None;
                        *status_msg = None;
                        *replace_dialog = Some(Box::new(EditorReplaceDialog::from_last_search(
                            &buf.last_search,
                        )));
                    }
                    // Pipe selection (or whole buffer) through external command (GNU mcedit).
                    KeyCode::Char('|') => {
                        *search_input = None;
                        *save_as_dialog = None;
                        *search_dialog = None;
                        *replace_dialog = None;
                        *goto_dialog = None;
                        *status_msg = None;
                        *pipe_dialog = Some(EditorPipeDialog::default());
                    }
                    // GNU mcedit: Alt-l / Alt-L Goto line (stay in Editor).
                    KeyCode::Char('l' | 'L') if key.modifiers.contains(KeyModifiers::ALT) => {
                        *search_input = None;
                        *save_as_dialog = None;
                        *search_dialog = None;
                        *replace_dialog = None;
                        *pipe_dialog = None;
                        *status_msg = None;
                        *goto_dialog = Some(Box::new(EditorGotoDialog::from_cursor_row(buf.row)));
                    }
                    // Block ops
                    KeyCode::F(3) => {
                        if buf.selection_bounds().is_none() {
                            buf.mark_start();
                            *status_msg = Some("Mark start".into());
                        } else if buf.selection_bounds().is_some() && buf.last_search.is_empty() {
                            // No-op hint; allow second press to clear
                            buf.mark_end();
                            *status_msg = Some("Mark end".into());
                        } else {
                            buf.clear_selection();
                            *status_msg = Some("Unmark".into());
                        }
                    }
                    KeyCode::F(5) => {
                        if buf.copy_block_here() {
                            *status_msg = Some("Copied block".into());
                        }
                    }
                    KeyCode::F(6) => {
                        if buf.move_block_here() {
                            *status_msg = Some("Moved block".into());
                        }
                    }
                    KeyCode::F(8) => {
                        if buf.delete_selection() {
                            *status_msg = Some("Deleted block".into());
                        }
                    }
                    // Save / Quit. F2 writes the current path (no dialog).
                    KeyCode::F(2) => {
                        if let Some(path) = buf.path.clone() {
                            match editor_save_to_path(app.vfs.as_ref(), buf, &path) {
                                Ok(()) => *status_msg = Some("Saved".into()),
                                Err(e) => *status_msg = Some(e),
                            }
                        } else {
                            editor_open_save_as_dialog(
                                buf,
                                search_input,
                                save_as_dialog,
                                search_dialog,
                                replace_dialog,
                                pipe_dialog,
                                goto_dialog,
                                status_msg,
                                show_menu,
                            );
                        }
                    }
                    KeyCode::F(10) | KeyCode::Char('q') => {
                        if buf.dirty {
                            *confirm_exit = Some(rmc_core::app::YncDialog {
                                title: "Save modified file?".into(),
                                message:
                                    "The file has unsaved changes. Save before leaving the editor?"
                                        .into(),
                                focus: rmc_core::app::YncFocus::Yes,
                            });
                        } else {
                            leave_editor(app);
                        }
                    }
                    // Basic cursor/editing
                    KeyCode::Left => buf.move_left(),
                    KeyCode::Right => buf.move_right(),
                    KeyCode::Up => buf.move_up(),
                    KeyCode::Down => buf.move_down(),
                    KeyCode::Backspace => buf.backspace(),
                    KeyCode::Delete => buf.delete(),
                    KeyCode::Enter => buf.insert_newline(),
                    KeyCode::Insert => buf.toggle_overwrite(),
                    KeyCode::Char(c) if key.modifiers.is_empty() && !c.is_control() => {
                        buf.insert_char(c)
                    }
                    _ => {}
                }
                return Ok(());
            }
            UiMode::Help { state, prev } => {
                // Navigation within help
                match key.code {
                    KeyCode::Esc | KeyCode::F(10) => {
                        let prior = std::mem::replace(&mut **prev, UiMode::Normal);
                        app.ui_mode = prior;
                    }
                    KeyCode::F(2) => {
                        // Index (Contents)
                        state.topic = global_index().contents_name();
                        state.cursor = 0;
                        state.scroll_top = 0;
                    }
                    KeyCode::F(3) => {
                        // Prev (history back)
                        if let Some(prev_topic) = state.history.pop() {
                            state.topic = prev_topic;
                            state.cursor = 0;
                            state.scroll_top = 0;
                        }
                    }
                    KeyCode::F(4) => {
                        // Next (follow selected link)
                        if let Some(node) = global_index().get(&state.topic) {
                            let mut cur = 0usize;
                            for it in &node.items {
                                if let HelpItem::Link { target, .. } = it {
                                    if cur == state.cursor {
                                        state.history.push(state.topic.clone());
                                        state.topic = target.clone();
                                        state.cursor = 0;
                                        state.scroll_top = 0;
                                        break;
                                    }
                                    cur += 1;
                                }
                            }
                        }
                    }
                    KeyCode::Tab | KeyCode::Down | KeyCode::Right => {
                        if let Some(node) = global_index().get(&state.topic) {
                            let links = node
                                .items
                                .iter()
                                .filter(|it| matches!(it, HelpItem::Link { .. }))
                                .count();
                            if links > 0 {
                                state.cursor = (state.cursor + 1) % links;
                            }
                        }
                    }
                    KeyCode::Up | KeyCode::Left => {
                        if let Some(node) = global_index().get(&state.topic) {
                            let links = node
                                .items
                                .iter()
                                .filter(|it| matches!(it, HelpItem::Link { .. }))
                                .count();
                            if links > 0 {
                                let prev_idx = state.cursor as isize - 1;
                                state.cursor = if prev_idx < 0 {
                                    links.saturating_sub(1)
                                } else {
                                    prev_idx as usize
                                };
                            }
                        }
                    }
                    KeyCode::PageDown => {
                        state.scroll_top = state.scroll_top.saturating_add(page_rows);
                    }
                    KeyCode::PageUp => {
                        state.scroll_top = state.scroll_top.saturating_sub(page_rows);
                    }
                    KeyCode::Home => state.scroll_top = 0,
                    KeyCode::End => {
                        state.scroll_top = state.scroll_top.saturating_add(10 * page_rows)
                    }
                    KeyCode::Enter | KeyCode::Char('\n') => {
                        if let Some(node) = global_index().get(&state.topic) {
                            let mut cur = 0usize;
                            for it in &node.items {
                                if let HelpItem::Link { target, .. } = it {
                                    if cur == state.cursor {
                                        state.history.push(state.topic.clone());
                                        state.topic = target.clone();
                                        state.cursor = 0;
                                        state.scroll_top = 0;
                                        break;
                                    }
                                    cur += 1;
                                }
                            }
                        }
                    }
                    KeyCode::Backspace | KeyCode::Char('l') => {
                        if let Some(prev_topic) = state.history.pop() {
                            state.topic = prev_topic;
                            state.cursor = 0;
                            state.scroll_top = 0;
                        }
                    }
                    KeyCode::F(1) => {
                        state.topic = global_index().contents_name();
                        state.cursor = 0;
                        state.scroll_top = 0;
                    }
                    _ => {}
                }
                return Ok(());
            }
            UiMode::ShellInput => {
                // Command line editing and execution
                match key.code {
                    KeyCode::Esc => {
                        app.ui_mode = UiMode::Normal;
                    }
                    KeyCode::Enter if key.modifiers.is_empty() => {
                        if app.subshell.cmdline.trim().is_empty() {
                            // Empty command: use panel Enter behavior
                            app.ui_mode = UiMode::Normal;
                            handle_panel_enter(app)?;
                        } else {
                            // Prefer executing inside a live PTY session when available.
                            let outcome = {
                                if let Ok(mut guard) = SUBSHELL_PTY.lock() {
                                    let pty_opt = guard.as_mut();
                                    app.subshell.execute_in_pty(&active_cwd, pty_opt)?
                                } else {
                                    app.subshell.execute_current(&active_cwd)?
                                }
                            };
                            let _ = outcome;
                            // Always rescan panels after a command (local VFS only here).
                            app.reload_panels()?;
                            app.subshell.clear_cmdline();
                            app.ui_mode = UiMode::Normal;
                        }
                    }
                    KeyCode::Backspace => {
                        app.subshell.cmdline.pop();
                        app.subshell.clear_history_nav();
                    }
                    KeyCode::Up => {
                        if let Some(s) = app.subshell.history_prev() {
                            app.subshell.cmdline = s;
                        }
                    }
                    KeyCode::Down => {
                        if let Some(s) = app.subshell.history_next() {
                            app.subshell.cmdline = s;
                        }
                    }
                    KeyCode::Char('p')
                        if key.modifiers.contains(crossterm::event::KeyModifiers::ALT) =>
                    {
                        if let Some(s) = app.subshell.history_prev() {
                            app.subshell.cmdline = s;
                        }
                    }
                    KeyCode::Char('n')
                        if key.modifiers.contains(crossterm::event::KeyModifiers::ALT) =>
                    {
                        if let Some(s) = app.subshell.history_next() {
                            app.subshell.cmdline = s;
                        }
                    }
                    KeyCode::Char('h') | KeyCode::Char('H')
                        if key.modifiers.contains(crossterm::event::KeyModifiers::ALT) =>
                    {
                        open_command_history(app);
                    }
                    KeyCode::Enter
                        if key.modifiers.contains(crossterm::event::KeyModifiers::ALT) =>
                    {
                        // Alt-Enter: copy current filename into command line
                        if let Some(ent) = app.active_panel().current_entry() {
                            let name = ent.name.clone();
                            app.subshell.append_filename(&name);
                        }
                    }
                    KeyCode::Char(c) if key.modifiers.is_empty() => {
                        app.subshell.cmdline.push(c);
                        app.subshell.clear_history_nav();
                    }
                    _ => {}
                }
                return Ok(());
            }
            UiMode::HistoryDialog {
                selected_index,
                scroll_top,
                focus,
                confirm_clean,
            } => {
                use HistoryDialogFocus as HF;
                if *confirm_clean {
                    match key.code {
                        KeyCode::Esc | KeyCode::F(10) | KeyCode::Char('n') | KeyCode::Char('N') => {
                            *confirm_clean = false;
                        }
                        KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                            app.subshell.clear_history();
                            *selected_index = 0;
                            *scroll_top = 0;
                            *confirm_clean = false;
                            *focus = HF::List;
                        }
                        _ => {}
                    }
                    return Ok(());
                }
                let list_rows = history_list_rows();
                match key.code {
                    KeyCode::Esc | KeyCode::F(10) => {
                        app.ui_mode = UiMode::ShellInput;
                    }
                    KeyCode::Tab => {
                        *focus = match *focus {
                            HF::List => HF::Ok,
                            HF::Ok => HF::Cancel,
                            HF::Cancel => HF::Clear,
                            HF::Clear => HF::List,
                        };
                    }
                    KeyCode::BackTab => {
                        *focus = match *focus {
                            HF::List => HF::Clear,
                            HF::Ok => HF::List,
                            HF::Cancel => HF::Ok,
                            HF::Clear => HF::Cancel,
                        };
                    }
                    KeyCode::Left | KeyCode::Right
                        if matches!(*focus, HF::Ok | HF::Cancel | HF::Clear) =>
                    {
                        *focus = match (*focus, key.code) {
                            (HF::Ok, KeyCode::Right) => HF::Cancel,
                            (HF::Cancel, KeyCode::Right) => HF::Clear,
                            (HF::Clear, KeyCode::Right) => HF::Ok,
                            (HF::Ok, KeyCode::Left) => HF::Clear,
                            (HF::Cancel, KeyCode::Left) => HF::Ok,
                            (HF::Clear, KeyCode::Left) => HF::Cancel,
                            (f, _) => f,
                        };
                    }
                    KeyCode::Up if matches!(*focus, HF::List) => {
                        if *selected_index > 0 {
                            *selected_index -= 1;
                        }
                        if *selected_index < *scroll_top {
                            *scroll_top = *selected_index;
                        }
                    }
                    KeyCode::Down if matches!(*focus, HF::List) => {
                        if *selected_index + 1 < app.subshell.history_len() {
                            *selected_index += 1;
                        }
                        if *selected_index >= *scroll_top + list_rows {
                            *scroll_top =
                                selected_index.saturating_sub(list_rows.saturating_sub(1));
                        }
                    }
                    KeyCode::Home if matches!(*focus, HF::List) => {
                        *selected_index = 0;
                        *scroll_top = 0;
                    }
                    KeyCode::End if matches!(*focus, HF::List) => {
                        if app.subshell.history_len() > 0 {
                            *selected_index = app.subshell.history_len() - 1;
                            *scroll_top =
                                selected_index.saturating_sub(list_rows.saturating_sub(1));
                        }
                    }
                    KeyCode::F(8) => {
                        let cleanup = app.confirm.history_cleanup;
                        request_history_clear(
                            &mut app.subshell,
                            cleanup,
                            selected_index,
                            scroll_top,
                            focus,
                            confirm_clean,
                        );
                    }
                    KeyCode::Enter => match *focus {
                        HF::List | HF::Ok => {
                            let idx = *selected_index;
                            if let Some(s) = app.subshell.history().get(idx).cloned() {
                                app.subshell.cmdline = s;
                                app.subshell.clear_history_nav();
                                app.ui_mode = UiMode::ShellInput;
                            }
                        }
                        HF::Cancel => {
                            app.ui_mode = UiMode::ShellInput;
                        }
                        HF::Clear => {
                            let cleanup = app.confirm.history_cleanup;
                            request_history_clear(
                                &mut app.subshell,
                                cleanup,
                                selected_index,
                                scroll_top,
                                focus,
                                confirm_clean,
                            );
                        }
                    },
                    _ => {}
                }
                return Ok(());
            }
            UiMode::HotlistDialog(state) => {
                // Estimate list rows based on current terminal size
                let (_cols, rows) = crossterm::terminal::size()?;
                let list_rows = rows.saturating_sub(4).clamp(12, 20).saturating_sub(4) as usize;
                match key.code {
                    KeyCode::Esc | KeyCode::F(10) => {
                        app.ui_mode = UiMode::Normal;
                    }
                    KeyCode::Tab => {
                        state.focus = match state.focus {
                            HDF::List => HDF::ButtonGoto,
                            HDF::ButtonGoto => HDF::ButtonAdd,
                            HDF::ButtonAdd => HDF::ButtonRemove,
                            HDF::ButtonRemove => HDF::ButtonCancel,
                            HDF::ButtonCancel => HDF::List,
                        };
                    }
                    KeyCode::Up if matches!(state.focus, HDF::List) => {
                        if state.selected_index > 0 {
                            state.selected_index -= 1;
                        }
                        if state.selected_index < state.scroll_top {
                            state.scroll_top = state.selected_index;
                        }
                    }
                    KeyCode::Down if matches!(state.focus, HDF::List) => {
                        if state.selected_index + 1 < state.entries.len() {
                            state.selected_index += 1;
                        }
                        if state.selected_index >= state.scroll_top + list_rows {
                            state.scroll_top = state
                                .selected_index
                                .saturating_sub(list_rows.saturating_sub(1));
                        }
                    }
                    KeyCode::Home if matches!(state.focus, HDF::List) => {
                        state.selected_index = 0;
                        state.scroll_top = 0;
                    }
                    KeyCode::End if matches!(state.focus, HDF::List) => {
                        if !state.entries.is_empty() {
                            state.selected_index = state.entries.len() - 1;
                            state.scroll_top = state
                                .selected_index
                                .saturating_sub(list_rows.saturating_sub(1));
                        }
                    }
                    KeyCode::Enter => match state.focus {
                        HDF::List | HDF::ButtonGoto => {
                            if let Some(entry) = state.entries.get(state.selected_index).cloned() {
                                let _ = app.change_dir(&entry.path);
                                app.ui_mode = UiMode::Normal;
                            }
                        }
                        HDF::ButtonAdd => {
                            let cwd = app.active_panel().cwd.clone();
                            let suggested = cwd
                                .file_name()
                                .and_then(|s| s.to_str())
                                .unwrap_or("")
                                .to_string();
                            app.ui_mode = UiMode::PromptInput {
                                title: "Add to hotlist: Label".into(),
                                value: suggested,
                                on_submit: Box::new(move |app, label| {
                                    if !label.trim().is_empty() {
                                        app.hotlist
                                            .add_or_replace(label.trim().to_string(), cwd)?;
                                        app.hotlist.save_to_default_path()?;
                                    }
                                    // Reopen dialog with updated entries
                                    let st = rmc_core::hotlist::HotlistDialogState::new(
                                        app.hotlist.entries.clone(),
                                    );
                                    app.ui_mode = UiMode::HotlistDialog(st);
                                    Ok(())
                                }),
                            };
                        }
                        HDF::ButtonRemove => {
                            if state.selected_index < state.entries.len() {
                                let idx = state.selected_index;
                                if app.confirm.directory_hotlist {
                                    let label = state.entries[idx].label.clone();
                                    app.ui_mode = UiMode::DialogConfirm {
                                        title: "Confirmation".into(),
                                        message: format!("Remove \"{label}\" from hotlist?"),
                                        on_ok: Box::new(move |app| {
                                            app.hotlist.remove_at(idx);
                                            app.hotlist.save_to_default_path()?;
                                            let mut st = rmc_core::hotlist::HotlistDialogState::new(
                                                app.hotlist.entries.clone(),
                                            );
                                            if !st.entries.is_empty() {
                                                st.selected_index = idx.min(st.entries.len() - 1);
                                            }
                                            app.ui_mode = UiMode::HotlistDialog(st);
                                            Ok(())
                                        }),
                                    };
                                } else {
                                    app.hotlist.remove_at(idx);
                                    app.hotlist.save_to_default_path()?;
                                    // refresh dialog state
                                    state.entries = app.hotlist.entries.clone();
                                    if state.selected_index >= state.entries.len()
                                        && !state.entries.is_empty()
                                    {
                                        state.selected_index = state.entries.len() - 1;
                                    }
                                }
                            }
                        }
                        HDF::ButtonCancel => {
                            app.ui_mode = UiMode::Normal;
                        }
                    },
                    _ => {}
                }
                return Ok(());
            }
            UiMode::PromptInput {
                title: _,
                value,
                on_submit,
            } => {
                match key.code {
                    KeyCode::Esc => {
                        app.ui_mode = UiMode::Normal;
                    }
                    KeyCode::Enter => {
                        let cb = std::mem::replace(on_submit, Box::new(|_, _| Ok(())));
                        let val = value.clone();
                        app.ui_mode = UiMode::Normal;
                        cb(app, val)?;
                        app.reload_panels()?;
                    }
                    KeyCode::Backspace => {
                        value.pop();
                    }
                    KeyCode::Char(c) if key.modifiers.is_empty() => {
                        value.push(c);
                    }
                    KeyCode::Char(_) => {}
                    _ => {}
                }
                return Ok(());
            }
            UiMode::InputDialog {
                title: _,
                prompt: _,
                value,
                on_submit,
                focus_ok,
            } => {
                match key.code {
                    KeyCode::Esc | KeyCode::F(10) => {
                        app.ui_mode = UiMode::Normal;
                    }
                    KeyCode::Tab => {
                        *focus_ok = !*focus_ok;
                    }
                    KeyCode::Enter => {
                        if *focus_ok {
                            let cb = std::mem::replace(on_submit, Box::new(|_, _| Ok(())));
                            let val = value.clone();
                            app.ui_mode = UiMode::Normal;
                            cb(app, val)?;
                            app.reload_panels()?;
                        } else {
                            // stay, user can toggle to OK
                        }
                    }
                    KeyCode::Backspace => {
                        value.pop();
                    }
                    KeyCode::Char(c) if key.modifiers.is_empty() => {
                        value.push(c);
                    }
                    _ => {}
                }
                return Ok(());
            }
            UiMode::FtpConnectDialog {
                scheme,
                host,
                port,
                user,
                password,
                directory,
                anonymous,
                focus_index,
                focus_ok,
            } => {
                match key.code {
                    KeyCode::Esc | KeyCode::F(10) => app.ui_mode = UiMode::Normal,
                    KeyCode::Tab => {
                        if *focus_ok {
                            *focus_ok = false;
                            *focus_index = 0;
                        } else if *focus_index < 5 {
                            *focus_index += 1;
                        } else {
                            *focus_ok = true;
                        }
                    }
                    KeyCode::BackTab => {
                        if *focus_ok {
                            *focus_ok = false;
                            *focus_index = 5;
                        } else if *focus_index > 0 {
                            *focus_index -= 1;
                        } else {
                            *focus_ok = true;
                        }
                    }
                    KeyCode::Backspace if !*focus_ok => match *focus_index {
                        0 => {
                            host.pop();
                        }
                        1 => {
                            port.pop();
                        }
                        2 => {
                            user.pop();
                        }
                        3 => {
                            password.pop();
                        }
                        4 => {
                            directory.pop();
                        }
                        _ => {}
                    },
                    KeyCode::Char(' ') if !*focus_ok && *focus_index == 5 => {
                        *anonymous = !*anonymous;
                    }
                    KeyCode::Char(c) if key.modifiers.is_empty() && !*focus_ok => {
                        match *focus_index {
                            0 => host.push(c),
                            1 => port.push(c),
                            2 => user.push(c),
                            3 => password.push(c),
                            4 => directory.push(c),
                            _ => {}
                        }
                    }
                    KeyCode::Enter if *focus_ok => {
                        // Build URL and attempt to change_dir; empty host => no-op.
                        let host_val = host.trim().to_string();
                        if host_val.is_empty() {
                            return Ok(());
                        }
                        let port_val = port.trim();
                        let mut user_val = user.trim().to_string();
                        let mut pass_val = password.clone(); // allow empty

                        // Apply ~/.netrc if enabled and no user provided (FTP only)
                        if scheme == "ftp" && app.vfs_opts.use_netrc && user_val.is_empty() {
                            if let Some((u, p)) = netrc_lookup(&host_val) {
                                user_val = u;
                                pass_val = p;
                            }
                        }
                        // If FTP anonymous with no provided password, use configured default
                        if scheme == "ftp"
                            && *anonymous
                            && user_val.is_empty()
                            && pass_val.is_empty()
                        {
                            let anon_pass = app.vfs_opts.ftp_anon_password.trim().to_string();
                            if !anon_pass.is_empty() {
                                pass_val = anon_pass;
                            }
                        }
                        let dir_val = {
                            let d = directory.trim();
                            if d.is_empty() {
                                "/".to_string()
                            } else if d.starts_with('/') {
                                d.to_string()
                            } else {
                                format!("/{d}")
                            }
                        };
                        let user_part = {
                            // When anonymous and user empty, default to "anonymous"
                            let u = if *anonymous && user_val.is_empty() {
                                "anonymous".to_string()
                            } else {
                                user_val.to_string()
                            };
                            if u.is_empty() {
                                String::new()
                            } else if pass_val.is_empty() {
                                format!("{u}@")
                            } else {
                                format!("{u}:{pass_val}@")
                            }
                        };
                        let host_part = if port_val.is_empty() {
                            host_val
                        } else {
                            format!("{host_val}:{port_val}")
                        };
                        let url = format!("{scheme}://{user_part}{host_part}{dir_val}");
                        if scheme == "ftp" {
                            rmc_fs::remote::set_ftp_proxy(ftp_proxy_for_vfs_opts(
                                app.vfs_opts.always_use_ftp_proxy,
                                &app.vfs_opts.ftp_proxy_host,
                            ));
                        }
                        app.ui_mode = UiMode::Normal;
                        let path = std::path::PathBuf::from(url);
                        match app.change_dir(&path) {
                            Ok(()) => {
                                app.reload_panels()?;
                            }
                            Err(err) => {
                                app.ui_mode = UiMode::DialogConfirm {
                                    title: "Error".into(),
                                    message: format!("{err}"),
                                    on_ok: Box::new(|_| Ok(())),
                                };
                            }
                        }
                    }
                    _ => {}
                }
                return Ok(());
            }
            UiMode::DialogConfirm {
                title: _,
                message: _,
                on_ok,
            } => {
                match key.code {
                    KeyCode::Esc | KeyCode::F(10) => {
                        app.ui_mode = UiMode::Normal;
                    }
                    KeyCode::Enter => {
                        let cb = std::mem::replace(on_ok, Box::new(|_| Ok(())));
                        app.ui_mode = UiMode::Normal;
                        cb(app)?;
                        app.reload_panels()?;
                    }
                    _ => {}
                }
                return Ok(());
            }
            UiMode::FindDialog(state) => {
                // Update from background search if completed
                if state.running {
                    if let Some(rx) = &state.results_rx {
                        loop {
                            match rx.try_recv() {
                                Ok(p) => state.results.paths.push(p),
                                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                                    state.running = false;
                                    state.cancel = None;
                                    state.results_rx = None;
                                    break;
                                }
                            }
                        }
                    }
                }
                match key.code {
                    KeyCode::Esc => {
                        app.ui_mode = UiMode::Normal;
                    }
                    KeyCode::Tab => {
                        state.focus = state.focus.next();
                    }
                    KeyCode::BackTab => {
                        state.focus = state.focus.prev();
                    }
                    KeyCode::Up => {
                        if state.focus.is_checkbox()
                            || matches!(state.focus, FF::StartDir | FF::NamePattern | FF::Content)
                        {
                            if !matches!(state.focus, FF::StartDir) {
                                state.focus = state.focus.prev();
                            }
                        } else if state.selected_index > 0 {
                            state.selected_index -= 1;
                            let (_c, r) = crossterm::terminal::size()?;
                            let h = find_dialog_height(r);
                            let list_rows = find_dialog_list_rows(h);
                            if state.selected_index < state.scroll_top {
                                state.scroll_top = state.selected_index;
                            } else if state.selected_index >= state.scroll_top + list_rows {
                                state.scroll_top = state
                                    .selected_index
                                    .saturating_sub(list_rows.saturating_sub(1));
                            }
                        }
                    }
                    KeyCode::Down => {
                        if state.focus.is_checkbox()
                            || matches!(state.focus, FF::StartDir | FF::NamePattern | FF::Content)
                        {
                            let n = state.focus.next();
                            // Do not wrap from the last form widget back to Start at.
                            if !matches!(n, FF::StartDir) {
                                state.focus = n;
                            }
                        } else if state.selected_index + 1 < state.results.paths.len() {
                            state.selected_index += 1;
                            let (_c, r) = crossterm::terminal::size()?;
                            let h = find_dialog_height(r);
                            let list_rows = find_dialog_list_rows(h);
                            if state.selected_index < state.scroll_top {
                                state.scroll_top = state.selected_index;
                            } else if state.selected_index >= state.scroll_top + list_rows {
                                state.scroll_top = state
                                    .selected_index
                                    .saturating_sub(list_rows.saturating_sub(1));
                            }
                        }
                    }
                    KeyCode::Home => {
                        state.selected_index = 0;
                        state.scroll_top = 0;
                    }
                    KeyCode::End => {
                        if !state.results.paths.is_empty() {
                            state.selected_index = state.results.paths.len() - 1;
                            let (_c, r) = crossterm::terminal::size()?;
                            let h = find_dialog_height(r);
                            let list_rows = find_dialog_list_rows(h);
                            state.scroll_top = state
                                .selected_index
                                .saturating_sub(list_rows.saturating_sub(1));
                        }
                    }
                    KeyCode::Backspace => match state.focus {
                        FF::StartDir => {
                            state.start_dir_edit.pop();
                        }
                        FF::NamePattern => match &mut state.params.name_pattern {
                            rmc_core::find::NamePattern::Glob(s) => {
                                s.pop();
                            }
                        },
                        FF::Content => {
                            if let Some(s) = &mut state.params.content_substring {
                                s.pop();
                                if s.is_empty() {
                                    state.params.content_substring = None;
                                }
                            }
                        }
                        _ => {}
                    },
                    // Space toggles checkboxes before generic Char so typing still
                    // inserts spaces into Start at / Filename / Content.
                    KeyCode::Char(' ') if key.modifiers.is_empty() && state.focus.is_checkbox() => {
                        let _ = state.toggle_focused_checkbox();
                    }
                    KeyCode::Char(c) => {
                        if key.modifiers.is_empty() {
                            match state.focus {
                                FF::StartDir => {
                                    if !c.is_control() {
                                        state.start_dir_edit.push(c);
                                    }
                                }
                                FF::NamePattern => match &mut state.params.name_pattern {
                                    rmc_core::find::NamePattern::Glob(s) => {
                                        s.push(c);
                                    }
                                },
                                FF::Content => {
                                    if c == '\n' {
                                        // ignore
                                    } else if let Some(ref mut s) = state.params.content_substring {
                                        s.push(c);
                                    } else {
                                        state.params.content_substring = Some(c.to_string());
                                    }
                                }
                                _ => {}
                            }
                        } else if key
                            .modifiers
                            .contains(crossterm::event::KeyModifiers::CONTROL)
                            && c == 'c'
                        {
                            // Allow Ctrl-C to stop search when focused anywhere
                            if let Some(ch) = &state.cancel {
                                ch.cancel();
                            }
                        }
                    }
                    KeyCode::Enter => {
                        if state.toggle_focused_checkbox() {
                            return Ok(());
                        }
                        match state.focus {
                            FF::ButtonStart | FF::ButtonAgain => {
                                if !state.running {
                                    // Prepare params (apply Start at edit)
                                    let start_str = state.start_dir_edit.trim().to_string();
                                    let start_dir = if start_str.is_empty() {
                                        active_cwd.clone()
                                    } else {
                                        std::path::PathBuf::from(start_str)
                                    };
                                    state.params.start_dir = start_dir;
                                    // Reset results and selection
                                    state.results.paths.clear();
                                    state.selected_index = 0;
                                    state.scroll_top = 0;
                                    // Kick off background search (streaming)
                                    let params = state.params.clone();
                                    let cancel = CancelHandle::new();
                                    let flag = cancel.flag();
                                    let (tx, rx) = std::sync::mpsc::channel();
                                    state.cancel = Some(cancel);
                                    state.results_rx = Some(rx);
                                    state.running = true;
                                    std::thread::spawn(move || {
                                        search_files_streaming(&params, &flag, |p| {
                                            let _ = tx.send(p);
                                        });
                                        // dropping tx signals completion
                                    });
                                }
                            }
                            FF::ButtonStop => {
                                if let Some(ch) = &state.cancel {
                                    ch.cancel();
                                }
                            }
                            FF::ButtonChdir => {
                                if let Some(p) =
                                    state.results.paths.get(state.selected_index).cloned()
                                {
                                    if let Ok(md) = app.vfs.stat(&p) {
                                        let dest = if md.is_dir {
                                            p
                                        } else {
                                            p.parent()
                                                .map(|x| x.to_path_buf())
                                                .unwrap_or(active_cwd.clone())
                                        };
                                        let _ = app.change_dir(&dest);
                                        app.ui_mode = UiMode::Normal;
                                    }
                                }
                            }
                            FF::ButtonPanelize => {
                                if !state.running && !state.results.paths.is_empty() {
                                    let paths = state.results.paths.clone();
                                    let base = state.params.start_dir.clone();
                                    app.panelize_paths(&paths, Some(&base))?;
                                    app.ui_mode = UiMode::Normal;
                                }
                            }
                            FF::ButtonQuit => {
                                app.ui_mode = UiMode::Normal;
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
                return Ok(());
            }
            UiMode::SortDialog {
                side,
                focus_index,
                by,
                reverse,
                dirs_first,
            } => {
                // Focus order: 0..3 radios; 4 Reverse; 5 Dirs-first; 6 OK; 7 Cancel
                let mut apply: Option<(
                    rmc_core::actions::PaneSide,
                    rmc_core::panel::SortBy,
                    bool,
                    bool,
                )> = None;
                let mut close_dialog = false;
                match key.code {
                    KeyCode::Esc | KeyCode::F(10) => {
                        close_dialog = true;
                    }
                    KeyCode::Tab => {
                        *focus_index = (*focus_index + 1) % 8;
                    }
                    KeyCode::BackTab => {
                        *focus_index = (*focus_index + 8 - 1) % 8;
                    }
                    KeyCode::Up => {
                        if *focus_index > 0 {
                            *focus_index -= 1;
                        }
                    }
                    KeyCode::Down => {
                        *focus_index = (*focus_index + 1).min(7);
                    }
                    KeyCode::Char(' ') => {
                        if *focus_index <= 3 {
                            *by = match *focus_index {
                                0 => rmc_core::panel::SortBy::Name,
                                1 => rmc_core::panel::SortBy::Ext,
                                2 => rmc_core::panel::SortBy::Time,
                                3 => rmc_core::panel::SortBy::Size,
                                _ => *by,
                            };
                        } else if *focus_index == 4 {
                            *reverse = !*reverse;
                        } else if *focus_index == 5 {
                            *dirs_first = !*dirs_first;
                        } else if *focus_index == 6 {
                            // OK via space
                            apply = Some((*side, *by, *reverse, *dirs_first));
                            close_dialog = true;
                        } else if *focus_index == 7 {
                            // Cancel via space
                            close_dialog = true;
                        }
                    }
                    KeyCode::Enter => {
                        match *focus_index {
                            0..=3 => {
                                // select radio
                                *by = match *focus_index {
                                    0 => rmc_core::panel::SortBy::Name,
                                    1 => rmc_core::panel::SortBy::Ext,
                                    2 => rmc_core::panel::SortBy::Time,
                                    3 => rmc_core::panel::SortBy::Size,
                                    _ => *by,
                                };
                            }
                            4 => *reverse = !*reverse,
                            5 => *dirs_first = !*dirs_first,
                            6 => {
                                apply = Some((*side, *by, *reverse, *dirs_first));
                                close_dialog = true;
                            }
                            7 => {
                                close_dialog = true;
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
                let _ = side;
                let _ = focus_index;
                let _ = by;
                let _ = reverse;
                let _ = dirs_first;
                if let Some((s, b, r, d)) = apply {
                    Self::apply_sort_dialog(app, s, b, r, d)?;
                }
                if close_dialog {
                    app.ui_mode = UiMode::Normal;
                }
                return Ok(());
            }
            UiMode::ListingModeDialog {
                side,
                listing,
                user_format,
                focus,
            } => {
                use rmc_core::app::ListingModeFocus as F;
                let order = [
                    F::RadioFull,
                    F::RadioBrief,
                    F::RadioLong,
                    F::RadioUser,
                    F::Input,
                    F::Ok,
                    F::Cancel,
                ];
                let mut idx = order.iter().position(|f0| f0 == focus).unwrap_or(0);
                let mut apply = false;
                match key.code {
                    KeyCode::Esc | KeyCode::F(10) => {
                        app.ui_mode = UiMode::Normal;
                        return Ok(());
                    }
                    KeyCode::Tab => {
                        idx = (idx + 1) % order.len();
                        *focus = order[idx];
                    }
                    KeyCode::BackTab => {
                        idx = (idx + order.len() - 1) % order.len();
                        *focus = order[idx];
                    }
                    KeyCode::Up => {
                        if idx > 0 {
                            idx -= 1;
                            *focus = order[idx];
                        }
                    }
                    KeyCode::Down => {
                        if idx + 1 < order.len() {
                            idx += 1;
                            *focus = order[idx];
                        }
                    }
                    KeyCode::Backspace => {
                        if matches!(*focus, F::Input) && !user_format.is_empty() {
                            user_format.pop();
                        }
                    }
                    // Handle Enter on radios/buttons before generic Char handler
                    KeyCode::Enter => match *focus {
                        F::RadioFull => *listing = rmc_core::panel::ListingFormat::Full,
                        F::RadioBrief => *listing = rmc_core::panel::ListingFormat::Brief,
                        F::RadioLong => *listing = rmc_core::panel::ListingFormat::Long,
                        F::RadioUser => *listing = rmc_core::panel::ListingFormat::User,
                        F::Ok => apply = true,
                        F::Cancel => {
                            app.ui_mode = UiMode::Normal;
                            return Ok(());
                        }
                        F::Input => { /* no-op on Enter while editing */ }
                    },
                    // Space picks a radio/OK/Cancel; when editing Input, let it fall through to push(' ')
                    KeyCode::Char(' ') if !matches!(*focus, F::Input) => match *focus {
                        F::RadioFull => *listing = rmc_core::panel::ListingFormat::Full,
                        F::RadioBrief => *listing = rmc_core::panel::ListingFormat::Brief,
                        F::RadioLong => *listing = rmc_core::panel::ListingFormat::Long,
                        F::RadioUser => *listing = rmc_core::panel::ListingFormat::User,
                        F::Ok => apply = true,
                        F::Cancel => {
                            app.ui_mode = UiMode::Normal;
                            return Ok(());
                        }
                        F::Input => { /* unreachable due to guard */ }
                    },
                    // Generic character input only when the one-line format field is focused
                    KeyCode::Char(c) if key.modifiers.is_empty() && matches!(*focus, F::Input) => {
                        user_format.push(c);
                    }
                    _ => {}
                }
                if apply {
                    // Apply to the selected side's panel
                    let p = if matches!(*side, rmc_core::actions::PaneSide::Left) {
                        &mut app.left
                    } else {
                        &mut app.right
                    };
                    p.listing = *listing;
                    p.user_format = user_format.clone();
                    app.ui_mode = UiMode::Normal;
                }
                return Ok(());
            }
            UiMode::CompareDirsDialog { mode, focus } => {
                use rmc_core::app::{CompareDirsFocus as F, CompareDirsMode as M};
                let order = [
                    F::RadioQuick,
                    F::RadioSizeOnly,
                    F::RadioThorough,
                    F::Ok,
                    F::Cancel,
                ];
                let mut idx = order.iter().position(|f0| f0 == focus).unwrap_or(0);
                let mut apply = false;
                match key.code {
                    KeyCode::Esc | KeyCode::F(10) => {
                        app.ui_mode = UiMode::Normal;
                        return Ok(());
                    }
                    KeyCode::Tab => {
                        idx = (idx + 1) % order.len();
                        *focus = order[idx];
                    }
                    KeyCode::BackTab => {
                        idx = (idx + order.len() - 1) % order.len();
                        *focus = order[idx];
                    }
                    KeyCode::Up => {
                        if idx > 0 {
                            idx -= 1;
                            *focus = order[idx];
                        }
                    }
                    KeyCode::Down => {
                        if idx + 1 < order.len() {
                            idx += 1;
                            *focus = order[idx];
                        }
                    }
                    KeyCode::Left | KeyCode::Right => {
                        // Only toggle between buttons when on a button
                        if matches!(*focus, F::Ok | F::Cancel) {
                            *focus = if matches!(*focus, F::Ok) {
                                F::Cancel
                            } else {
                                F::Ok
                            };
                        }
                    }
                    KeyCode::Char(' ') => match *focus {
                        F::RadioQuick => *mode = M::Quick,
                        F::RadioSizeOnly => *mode = M::SizeOnly,
                        F::RadioThorough => *mode = M::Thorough,
                        F::Ok => apply = true,
                        F::Cancel => {
                            app.ui_mode = UiMode::Normal;
                            return Ok(());
                        }
                    },
                    KeyCode::Enter => match *focus {
                        F::RadioQuick => *mode = M::Quick,
                        F::RadioSizeOnly => *mode = M::SizeOnly,
                        F::RadioThorough => *mode = M::Thorough,
                        F::Ok => apply = true,
                        F::Cancel => {
                            app.ui_mode = UiMode::Normal;
                            return Ok(());
                        }
                    },
                    _ => {}
                }
                if apply {
                    let m = *mode;
                    // Close dialog first, then apply to avoid borrow issues
                    app.ui_mode = UiMode::Normal;
                    let _ = run_compare_dirs(app, m);
                }
                return Ok(());
            }
            UiMode::LayoutDialog { draft, focus } => {
                use LayoutFocus as F;
                // Focus order: checkboxes then buttons
                let order = [
                    F::MenuBar,
                    F::CommandPrompt,
                    F::KeyBar,
                    F::HintBar,
                    F::XtermTitle,
                    F::ShowFreeSpace,
                    F::Ok,
                    F::Cancel,
                ];
                let mut idx = order.iter().position(|f0| f0 == focus).unwrap_or(0);
                match key.code {
                    KeyCode::Esc | KeyCode::F(10) => {
                        app.ui_mode = UiMode::Normal;
                    }
                    KeyCode::Tab => {
                        idx = (idx + 1) % order.len();
                        *focus = order[idx];
                    }
                    KeyCode::BackTab => {
                        idx = (idx + order.len() - 1) % order.len();
                        *focus = order[idx];
                    }
                    KeyCode::Up => {
                        if idx > 0 {
                            idx -= 1;
                            *focus = order[idx];
                        }
                    }
                    KeyCode::Down => {
                        if idx + 1 < order.len() {
                            idx += 1;
                            *focus = order[idx];
                        }
                    }
                    KeyCode::Left | KeyCode::Right => {
                        // Only toggle between buttons when the focus is on a button
                        if matches!(*focus, F::Ok | F::Cancel) {
                            *focus = if matches!(*focus, F::Ok) {
                                F::Cancel
                            } else {
                                F::Ok
                            };
                        }
                    }
                    KeyCode::Char(' ') => match *focus {
                        F::MenuBar => draft.menubar_visible = !draft.menubar_visible,
                        F::CommandPrompt => draft.command_prompt = !draft.command_prompt,
                        F::KeyBar => draft.keybar_visible = !draft.keybar_visible,
                        F::HintBar => draft.hintbar_visible = !draft.hintbar_visible,
                        F::XtermTitle => draft.xterm_title = !draft.xterm_title,
                        F::ShowFreeSpace => draft.show_free_space = !draft.show_free_space,
                        _ => {}
                    },
                    KeyCode::Enter => match *focus {
                        F::MenuBar => draft.menubar_visible = !draft.menubar_visible,
                        F::CommandPrompt => draft.command_prompt = !draft.command_prompt,
                        F::KeyBar => draft.keybar_visible = !draft.keybar_visible,
                        F::HintBar => draft.hintbar_visible = !draft.hintbar_visible,
                        F::XtermTitle => draft.xterm_title = !draft.xterm_title,
                        F::ShowFreeSpace => draft.show_free_space = !draft.show_free_space,
                        F::Ok => {
                            app.layout = *draft;
                            app.ui_mode = UiMode::Normal;
                        }
                        F::Cancel => {
                            app.ui_mode = UiMode::Normal;
                        }
                    },
                    _ => {}
                }
                return Ok(());
            }
            UiMode::ConfirmationsDialog { draft, focus } => {
                use rmc_core::app::ConfirmationsFocus as F;
                // Focus order: checkboxes then buttons
                let order = [
                    F::Delete,
                    F::Overwrite,
                    F::Execute,
                    F::Exit,
                    F::DirectoryHotlist,
                    F::HistoryCleanup,
                    F::Ok,
                    F::Cancel,
                ];
                let mut idx = order.iter().position(|f0| f0 == focus).unwrap_or(0);
                match key.code {
                    KeyCode::Esc | KeyCode::F(10) => {
                        app.ui_mode = UiMode::Normal;
                    }
                    KeyCode::Tab => {
                        idx = (idx + 1) % order.len();
                        *focus = order[idx];
                    }
                    KeyCode::BackTab => {
                        idx = (idx + order.len() - 1) % order.len();
                        *focus = order[idx];
                    }
                    KeyCode::Up => {
                        if idx > 0 {
                            idx -= 1;
                            *focus = order[idx];
                        }
                    }
                    KeyCode::Down => {
                        if idx + 1 < order.len() {
                            idx += 1;
                            *focus = order[idx];
                        }
                    }
                    KeyCode::Left | KeyCode::Right => {
                        // Only swap buttons when a button is focused
                        if matches!(*focus, F::Ok | F::Cancel) {
                            *focus = if matches!(*focus, F::Ok) {
                                F::Cancel
                            } else {
                                F::Ok
                            };
                        }
                    }
                    KeyCode::Char(' ') => match *focus {
                        F::Delete => draft.delete = !draft.delete,
                        F::Overwrite => draft.overwrite = !draft.overwrite,
                        F::Execute => draft.execute = !draft.execute,
                        F::Exit => draft.exit = !draft.exit,
                        F::DirectoryHotlist => draft.directory_hotlist = !draft.directory_hotlist,
                        F::HistoryCleanup => draft.history_cleanup = !draft.history_cleanup,
                        _ => {}
                    },
                    KeyCode::Enter => match *focus {
                        F::Delete => draft.delete = !draft.delete,
                        F::Overwrite => draft.overwrite = !draft.overwrite,
                        F::Execute => draft.execute = !draft.execute,
                        F::Exit => draft.exit = !draft.exit,
                        F::DirectoryHotlist => draft.directory_hotlist = !draft.directory_hotlist,
                        F::HistoryCleanup => draft.history_cleanup = !draft.history_cleanup,
                        F::Ok => {
                            app.confirm = *draft;
                            app.ui_mode = UiMode::Normal;
                        }
                        F::Cancel => {
                            app.ui_mode = UiMode::Normal;
                        }
                    },
                    _ => {}
                }
                return Ok(());
            }
            UiMode::PanelOptionsDialog { draft, focus } => {
                use rmc_core::app::PanelOptionsFocus as F;
                // Focus order: checkboxes then buttons
                let order = [
                    F::ShowHidden,
                    F::MixAllFiles,
                    F::MarkMovesDown,
                    F::ShowMiniStatus,
                    F::UseSiUnits,
                    F::FastReload,
                    F::ReverseFilesOnly,
                    F::SimpleSwap,
                    F::AutoSaveSetup,
                    F::LynxLikeMotion,
                    F::Ok,
                    F::Cancel,
                ];
                let mut idx = order.iter().position(|f0| f0 == focus).unwrap_or(0);
                match key.code {
                    KeyCode::Esc | KeyCode::F(10) => {
                        app.ui_mode = UiMode::Normal;
                    }
                    KeyCode::Tab => {
                        idx = (idx + 1) % order.len();
                        *focus = order[idx];
                    }
                    KeyCode::BackTab => {
                        idx = (idx + order.len() - 1) % order.len();
                        *focus = order[idx];
                    }
                    KeyCode::Up => {
                        if idx > 0 {
                            idx -= 1;
                            *focus = order[idx];
                        }
                    }
                    KeyCode::Down => {
                        if idx + 1 < order.len() {
                            idx += 1;
                            *focus = order[idx];
                        }
                    }
                    KeyCode::Left | KeyCode::Right => {
                        // Only swap buttons when a button is focused
                        if matches!(*focus, F::Ok | F::Cancel) {
                            *focus = if matches!(*focus, F::Ok) {
                                F::Cancel
                            } else {
                                F::Ok
                            };
                        }
                    }
                    KeyCode::Char(' ') => match *focus {
                        F::ShowHidden => draft.show_hidden = !draft.show_hidden,
                        F::MixAllFiles => draft.mix_all_files = !draft.mix_all_files,
                        F::MarkMovesDown => draft.mark_moves_down = !draft.mark_moves_down,
                        F::ShowMiniStatus => draft.show_mini_status = !draft.show_mini_status,
                        F::UseSiUnits => draft.kilobyte_si = !draft.kilobyte_si,
                        F::FastReload => draft.fast_reload = !draft.fast_reload,
                        F::ReverseFilesOnly => draft.reverse_files_only = !draft.reverse_files_only,
                        F::SimpleSwap => draft.simple_swap = !draft.simple_swap,
                        F::AutoSaveSetup => draft.auto_save_setup = !draft.auto_save_setup,
                        F::LynxLikeMotion => draft.lynx_like = !draft.lynx_like,
                        _ => {}
                    },
                    KeyCode::Enter => match *focus {
                        F::ShowHidden => draft.show_hidden = !draft.show_hidden,
                        F::MixAllFiles => draft.mix_all_files = !draft.mix_all_files,
                        F::MarkMovesDown => draft.mark_moves_down = !draft.mark_moves_down,
                        F::ShowMiniStatus => draft.show_mini_status = !draft.show_mini_status,
                        F::UseSiUnits => draft.kilobyte_si = !draft.kilobyte_si,
                        F::FastReload => draft.fast_reload = !draft.fast_reload,
                        F::ReverseFilesOnly => draft.reverse_files_only = !draft.reverse_files_only,
                        F::SimpleSwap => draft.simple_swap = !draft.simple_swap,
                        F::AutoSaveSetup => draft.auto_save_setup = !draft.auto_save_setup,
                        F::LynxLikeMotion => draft.lynx_like = !draft.lynx_like,
                        F::Ok => {
                            let new_opts = *draft;
                            // Close dialog before applying to avoid borrow issues
                            app.ui_mode = UiMode::Normal;
                            // Apply options
                            app.panel_opts = new_opts;
                            // Mix all files -> dirs_first = !mix
                            app.left.dirs_first = !new_opts.mix_all_files;
                            app.right.dirs_first = !new_opts.mix_all_files;
                            // Show hidden files
                            app.show_hidden = new_opts.show_hidden;
                            // Reload listings (applies sorting with new dirs_first as well)
                            let _ = app.reload_panels();
                        }
                        F::Cancel => {
                            app.ui_mode = UiMode::Normal;
                        }
                    },
                    _ => {}
                }
                return Ok(());
            }
            UiMode::ConfigurationDialog { draft, focus } => {
                use rmc_core::app::ConfigOptionsFocus as F;
                // Focus order: checkboxes then buttons
                let order = [
                    F::Verbose,
                    F::ComputeTotals,
                    F::ClassicProgressbar,
                    F::UseInternalViewer,
                    F::UseInternalEditor,
                    F::PauseAfterRun,
                    F::ShellPatterns,
                    F::AutoMenus,
                    F::DropMenus,
                    F::MkdirAutoname,
                    F::Ok,
                    F::Cancel,
                ];
                let mut idx = order.iter().position(|f0| f0 == focus).unwrap_or(0);
                match key.code {
                    KeyCode::Esc | KeyCode::F(10) => {
                        app.ui_mode = UiMode::Normal;
                    }
                    KeyCode::Tab => {
                        idx = (idx + 1) % order.len();
                        *focus = order[idx];
                    }
                    KeyCode::BackTab => {
                        idx = (idx + order.len() - 1) % order.len();
                        *focus = order[idx];
                    }
                    KeyCode::Up => {
                        if idx > 0 {
                            idx -= 1;
                            *focus = order[idx];
                        }
                    }
                    KeyCode::Down => {
                        if idx + 1 < order.len() {
                            idx += 1;
                            *focus = order[idx];
                        }
                    }
                    KeyCode::Left | KeyCode::Right => {
                        if matches!(*focus, F::Ok | F::Cancel) {
                            *focus = if matches!(*focus, F::Ok) {
                                F::Cancel
                            } else {
                                F::Ok
                            };
                        }
                    }
                    KeyCode::Char(' ') => match *focus {
                        F::Verbose => draft.verbose = !draft.verbose,
                        F::ComputeTotals => draft.compute_totals = !draft.compute_totals,
                        F::ClassicProgressbar => {
                            draft.classic_progressbar = !draft.classic_progressbar
                        }
                        F::UseInternalViewer => draft.use_internal_view = !draft.use_internal_view,
                        F::UseInternalEditor => draft.use_internal_edit = !draft.use_internal_edit,
                        F::PauseAfterRun => draft.pause_after_run = !draft.pause_after_run,
                        F::ShellPatterns => draft.shell_patterns = !draft.shell_patterns,
                        F::AutoMenus => draft.auto_menus = !draft.auto_menus,
                        F::DropMenus => draft.drop_menus = !draft.drop_menus,
                        F::MkdirAutoname => draft.mkdir_autoname = !draft.mkdir_autoname,
                        _ => {}
                    },
                    KeyCode::Enter => match *focus {
                        F::Verbose => draft.verbose = !draft.verbose,
                        F::ComputeTotals => draft.compute_totals = !draft.compute_totals,
                        F::ClassicProgressbar => {
                            draft.classic_progressbar = !draft.classic_progressbar
                        }
                        F::UseInternalViewer => draft.use_internal_view = !draft.use_internal_view,
                        F::UseInternalEditor => draft.use_internal_edit = !draft.use_internal_edit,
                        F::PauseAfterRun => draft.pause_after_run = !draft.pause_after_run,
                        F::ShellPatterns => draft.shell_patterns = !draft.shell_patterns,
                        F::AutoMenus => draft.auto_menus = !draft.auto_menus,
                        F::DropMenus => draft.drop_menus = !draft.drop_menus,
                        F::MkdirAutoname => draft.mkdir_autoname = !draft.mkdir_autoname,
                        F::Ok => {
                            // Apply and close
                            let new_opts = *draft;
                            app.ui_mode = UiMode::Normal;
                            app.config_opts = new_opts;
                        }
                        F::Cancel => {
                            app.ui_mode = UiMode::Normal;
                        }
                    },
                    _ => {}
                }
                return Ok(());
            }
            UiMode::VfsOptionsDialog { draft, focus } => {
                use rmc_core::app::VfsOptionsFocus as F;
                // Focus order: checkbox, input, checkbox, input, input, buttons
                let order = [
                    F::AlwaysUseFtpProxy,
                    F::FtpProxyHost,
                    F::UseNetrc,
                    F::FtpAnonPassword,
                    F::DirCacheTimeout,
                    F::Ok,
                    F::Cancel,
                ];
                let mut idx = order.iter().position(|f0| f0 == focus).unwrap_or(0);
                match key.code {
                    KeyCode::Esc | KeyCode::F(10) => {
                        app.ui_mode = UiMode::Normal;
                    }
                    KeyCode::Tab => {
                        idx = (idx + 1) % order.len();
                        *focus = order[idx];
                    }
                    KeyCode::BackTab => {
                        idx = (idx + order.len() - 1) % order.len();
                        *focus = order[idx];
                    }
                    KeyCode::Up => {
                        if idx > 0 {
                            idx -= 1;
                            *focus = order[idx];
                        }
                    }
                    KeyCode::Down => {
                        if idx + 1 < order.len() {
                            idx += 1;
                            *focus = order[idx];
                        }
                    }
                    KeyCode::Left | KeyCode::Right => {
                        if matches!(*focus, F::Ok | F::Cancel) {
                            *focus = if matches!(*focus, F::Ok) {
                                F::Cancel
                            } else {
                                F::Ok
                            };
                        }
                    }
                    KeyCode::Backspace => match *focus {
                        F::FtpProxyHost => {
                            draft.ftp_proxy_host.pop();
                        }
                        F::FtpAnonPassword => {
                            draft.ftp_anon_password.pop();
                        }
                        F::DirCacheTimeout => {
                            // Operate on a temporary string then parse
                            let mut s = draft.dir_cache_timeout_secs.to_string();
                            s.pop();
                            if s.is_empty() {
                                draft.dir_cache_timeout_secs = 0;
                            } else if let Ok(n) = s.parse::<u32>() {
                                draft.dir_cache_timeout_secs = n;
                            }
                        }
                        _ => {}
                    },
                    // Space toggles checkboxes; on input focus, fall through to Char and push ' '
                    KeyCode::Char(' ') if matches!(*focus, F::AlwaysUseFtpProxy | F::UseNetrc) => {
                        match *focus {
                            F::AlwaysUseFtpProxy => {
                                draft.always_use_ftp_proxy = !draft.always_use_ftp_proxy
                            }
                            F::UseNetrc => {
                                draft.use_netrc = !draft.use_netrc;
                            }
                            _ => {}
                        }
                    }
                    KeyCode::Enter => match *focus {
                        F::AlwaysUseFtpProxy => {
                            draft.always_use_ftp_proxy = !draft.always_use_ftp_proxy
                        }
                        F::UseNetrc => {
                            draft.use_netrc = !draft.use_netrc;
                        }
                        F::Ok => {
                            let new_opts = draft.clone();
                            app.ui_mode = UiMode::Normal;
                            app.vfs_opts = new_opts;
                        }
                        F::Cancel => {
                            app.ui_mode = UiMode::Normal;
                        }
                        _ => {}
                    },
                    KeyCode::Char(c) if key.modifiers.is_empty() => match *focus {
                        F::FtpProxyHost => {
                            draft.ftp_proxy_host.push(c);
                        }
                        F::FtpAnonPassword => {
                            draft.ftp_anon_password.push(c);
                        }
                        F::DirCacheTimeout if c.is_ascii_digit() => {
                            let mut s = draft.dir_cache_timeout_secs.to_string();
                            if s == "0" {
                                s.clear();
                            }
                            s.push(c);
                            if let Ok(n) = s.parse::<u32>() {
                                draft.dir_cache_timeout_secs = n;
                            }
                        }
                        F::DirCacheTimeout => {}
                        _ => {}
                    },
                    _ => {}
                }
                return Ok(());
            }
            UiMode::AppearanceDialog {
                draft_skin,
                draft_shadows,
                skins,
                selected,
                focus,
            } => {
                use rmc_core::app::AppearanceFocus as F;
                // Focus order per spec
                let order = [F::SkinList, F::Shadows, F::Ok, F::Cancel];
                let mut idx = order.iter().position(|f0| f0 == focus).unwrap_or(0);
                match key.code {
                    KeyCode::Esc | KeyCode::F(10) => {
                        app.ui_mode = UiMode::Normal;
                    }
                    KeyCode::Tab => {
                        idx = (idx + 1) % order.len();
                        *focus = order[idx];
                    }
                    KeyCode::BackTab => {
                        idx = (idx + order.len() - 1) % order.len();
                        *focus = order[idx];
                    }
                    KeyCode::Up => {
                        if matches!(*focus, F::SkinList) && *selected > 0 {
                            *selected -= 1;
                        }
                    }
                    KeyCode::Down => {
                        if matches!(*focus, F::SkinList) {
                            let max = skins.len().saturating_sub(1);
                            if *selected < max {
                                *selected += 1;
                            }
                        }
                    }
                    KeyCode::Left | KeyCode::Right => {
                        if matches!(*focus, F::Ok | F::Cancel) {
                            *focus = if matches!(*focus, F::Ok) {
                                F::Cancel
                            } else {
                                F::Ok
                            };
                        }
                    }
                    KeyCode::Char(' ') => {
                        if *focus == F::Shadows {
                            *draft_shadows = !*draft_shadows;
                        }
                    }
                    KeyCode::Enter => match *focus {
                        F::SkinList => {
                            if let Some(name) = skins.get(*selected) {
                                *draft_skin = name.clone();
                            }
                        }
                        F::Shadows => {
                            *draft_shadows = !*draft_shadows;
                        }
                        F::Ok => {
                            app.skin_name = draft_skin.clone();
                            app.shadows = *draft_shadows;
                            app.ui_mode = UiMode::Normal;
                        }
                        F::Cancel => {
                            app.ui_mode = UiMode::Normal;
                        }
                    },
                    _ => {}
                }
                return Ok(());
            }
            UiMode::MkdirDialog { value, focus_ok } => {
                match key.code {
                    KeyCode::Esc => app.ui_mode = UiMode::Normal,
                    KeyCode::Tab => *focus_ok = !*focus_ok,
                    KeyCode::Enter => {
                        if *focus_ok {
                            if !value.is_empty() {
                                let dir = active_cwd.join(value.clone());
                                app.vfs.mkdir(&dir)?;
                            }
                            app.reload_panels()?;
                        }
                        app.ui_mode = UiMode::Normal;
                    }
                    KeyCode::Backspace => {
                        if !*focus_ok {
                            value.pop();
                        }
                    }
                    KeyCode::Char(c) if !*focus_ok && key.modifiers.is_empty() => {
                        value.push(c);
                    }
                    KeyCode::Char(_) => {}
                    _ => {}
                }
                return Ok(());
            }
            UiMode::UserMenu {
                title: _,
                entries,
                selected_index,
            } => {
                let mut run_command_text: Option<String> = None;
                match key.code {
                    KeyCode::Esc | KeyCode::F(10) => {
                        app.ui_mode = UiMode::Normal;
                    }
                    KeyCode::Up => {
                        if *selected_index > 0 {
                            *selected_index -= 1;
                        }
                    }
                    KeyCode::Down => {
                        if *selected_index + 1 < entries.len() {
                            *selected_index += 1;
                        }
                    }
                    KeyCode::Enter => {
                        if let Some(ent) = entries.get(*selected_index) {
                            run_command_text = Some(ent.command.clone());
                            app.ui_mode = UiMode::Normal;
                        }
                    }
                    KeyCode::Char(c) => {
                        // Hotkey invoke
                        let lc = c.to_ascii_lowercase();
                        if let Some((idx, _)) = entries
                            .iter()
                            .enumerate()
                            .find(|(_, e)| e.hotkey.map(|k| k.to_ascii_lowercase()) == Some(lc))
                        {
                            *selected_index = idx;
                            if let Some(ent) = entries.get(idx) {
                                run_command_text = Some(ent.command.clone());
                                app.ui_mode = UiMode::Normal;
                            }
                        }
                    }
                    _ => {}
                }
                // After releasing the mutable borrow on ui_mode, run the command if requested
                if let Some(txt) = run_command_text {
                    let cmd = rmc_core::user_menu::expand_macros(app, &txt);
                    let _ = rmc_core::user_menu::run_menu_command(app, &cmd);
                    app.reload_panels()?;
                }
                return Ok(());
            }
            UiMode::DeleteDialog {
                name: _,
                path,
                focus_ok,
            } => {
                match key.code {
                    KeyCode::Esc => app.ui_mode = UiMode::Normal,
                    KeyCode::Tab => *focus_ok = !*focus_ok,
                    KeyCode::Enter => {
                        if *focus_ok {
                            app.vfs.remove(path, true)?;
                            app.reload_panels()?;
                        }
                        app.ui_mode = UiMode::Normal;
                    }
                    _ => {}
                }
                return Ok(());
            }
            UiMode::CopyDialog {
                title,
                src_name: _,
                src_path,
                mask,
                to,
                using_shell_patterns,
                follow_links,
                preserve_attrs,
                dive_into_subdir,
                stable_symlinks,
                focus,
            } => {
                use rmc_core::app::CopyDialogFocus as F;
                match key.code {
                    KeyCode::Esc => app.ui_mode = UiMode::Normal,
                    KeyCode::Tab => {
                        *focus = match *focus {
                            F::Mask => F::To,
                            F::To => F::Checkbox1,
                            F::Checkbox1 => F::Checkbox2,
                            F::Checkbox2 => F::Checkbox3,
                            F::Checkbox3 => F::Checkbox4,
                            F::Checkbox4 => F::Checkbox5,
                            F::Checkbox5 => F::Ok,
                            F::Ok => F::Background,
                            F::Background => F::Cancel,
                            F::Cancel => F::Mask,
                        };
                    }
                    KeyCode::Backspace => match *focus {
                        F::Mask => {
                            mask.pop();
                        }
                        F::To => {
                            to.pop();
                        }
                        _ => {}
                    },
                    KeyCode::Char(c) => {
                        if key.modifiers.is_empty() {
                            if c == ' ' {
                                match *focus {
                                    F::Checkbox1 => *using_shell_patterns = !*using_shell_patterns,
                                    F::Checkbox2 => *follow_links = !*follow_links,
                                    F::Checkbox3 => *preserve_attrs = !*preserve_attrs,
                                    F::Checkbox4 => *dive_into_subdir = !*dive_into_subdir,
                                    F::Checkbox5 => *stable_symlinks = !*stable_symlinks,
                                    _ => {}
                                }
                            } else {
                                match *focus {
                                    F::Mask => mask.push(c),
                                    F::To => to.push(c),
                                    _ => {}
                                }
                            }
                        }
                    }
                    KeyCode::Enter => {
                        use std::path::Path;
                        match *focus {
                            F::Checkbox1 => *using_shell_patterns = !*using_shell_patterns,
                            F::Checkbox2 => *follow_links = !*follow_links,
                            F::Checkbox3 => *preserve_attrs = !*preserve_attrs,
                            F::Checkbox4 => *dive_into_subdir = !*dive_into_subdir,
                            F::Checkbox5 => *stable_symlinks = !*stable_symlinks,
                            F::Ok => {
                                // If destination exists, open overwrite dialog; else start progress.
                                let dst = Path::new(&*to).to_path_buf();
                                let src = src_path.clone();
                                let op = if title == "Copy" {
                                    rmc_core::app::CopyMoveOp::Copy
                                } else {
                                    rmc_core::app::CopyMoveOp::Move
                                };
                                let exists = app.vfs.stat(&dst).is_ok();
                                if exists {
                                    if app.confirm.overwrite {
                                        app.ui_mode = UiMode::OverwriteDialog {
                                            op,
                                            src_path: src,
                                            dst_path: dst,
                                            focus: rmc_core::app::OverwriteFocus::Yes,
                                        };
                                    } else {
                                        let _ = app.vfs.remove(&dst, false);
                                        app.ui_mode = Self::file_op_progress_mode(
                                            app.vfs.as_ref(),
                                            &app.jobs,
                                            &app.config_opts,
                                            op,
                                            src,
                                            dst,
                                        );
                                    }
                                } else {
                                    app.ui_mode = Self::file_op_progress_mode(
                                        app.vfs.as_ref(),
                                        &app.jobs,
                                        &app.config_opts,
                                        op,
                                        src,
                                        dst,
                                    );
                                }
                            }
                            F::Background => {
                                // Enqueue background job; do not block.
                                if title == "Copy" {
                                    app.jobs.spawn_copy(
                                        src_path.clone(),
                                        Path::new(&*to).to_path_buf(),
                                    );
                                } else {
                                    app.jobs.spawn_move(
                                        src_path.clone(),
                                        Path::new(&*to).to_path_buf(),
                                    );
                                }
                                app.ui_mode = UiMode::Normal;
                            }
                            F::Cancel => {
                                app.ui_mode = UiMode::Normal;
                            }
                            _ => {}
                        }
                    }

                    _ => {}
                }
                return Ok(());
            }
            UiMode::JobsDialog {
                selected_index,
                focus,
            } => {
                use rmc_core::app::JobsDialogFocus as JF;
                match key.code {
                    KeyCode::Esc | KeyCode::F(10) => {
                        app.ui_mode = UiMode::Normal;
                    }
                    KeyCode::Tab => {
                        *focus = match *focus {
                            JF::List => JF::Cancel,
                            JF::Cancel => JF::Cleanup,
                            JF::Cleanup => JF::Ok,
                            JF::Ok => JF::List,
                        };
                    }
                    KeyCode::BackTab => {
                        *focus = match *focus {
                            JF::List => JF::Ok,
                            JF::Cancel => JF::List,
                            JF::Cleanup => JF::Cancel,
                            JF::Ok => JF::Cleanup,
                        };
                    }
                    KeyCode::Up => {
                        if matches!(*focus, JF::List) && *selected_index > 0 {
                            *selected_index -= 1;
                        }
                    }
                    KeyCode::Down => {
                        if matches!(*focus, JF::List) {
                            let total = app.jobs.snapshot().len();
                            if *selected_index + 1 < total {
                                *selected_index += 1;
                            }
                        }
                    }
                    KeyCode::Enter => match *focus {
                        JF::Cancel => {
                            let snap = app.jobs.snapshot();
                            if !snap.is_empty() && *selected_index < snap.len() {
                                let id = snap[*selected_index].id;
                                let _ = app.jobs.cancel(id);
                            }
                        }
                        JF::Cleanup => {
                            app.jobs.drop_finished_jobs();
                            let total = app.jobs.snapshot().len();
                            if *selected_index >= total {
                                *selected_index = total.saturating_sub(1);
                            }
                        }
                        JF::Ok => {
                            app.ui_mode = UiMode::Normal;
                        }
                        JF::List => { /* no-op */ }
                    },
                    _ => {}
                }
                return Ok(());
            }
            UiMode::OverwriteDialog {
                op,
                src_path,
                dst_path,
                focus,
            } => {
                use rmc_core::app::OverwriteFocus as OF;
                match key.code {
                    KeyCode::Esc | KeyCode::F(10) => {
                        app.ui_mode = UiMode::Normal;
                    }
                    KeyCode::Tab => {
                        *focus = match *focus {
                            OF::Yes => OF::No,
                            OF::No => OF::All,
                            OF::All => OF::Older,
                            OF::Older => OF::None,
                            OF::None => OF::Smaller,
                            OF::Smaller => OF::SizeDiffers,
                            OF::SizeDiffers => OF::Append,
                            OF::Append => OF::Yes,
                        };
                    }
                    KeyCode::BackTab => {
                        *focus = match *focus {
                            OF::Yes => OF::Append,
                            OF::No => OF::Yes,
                            OF::All => OF::No,
                            OF::Older => OF::All,
                            OF::None => OF::Older,
                            OF::Smaller => OF::None,
                            OF::SizeDiffers => OF::Smaller,
                            OF::Append => OF::SizeDiffers,
                        };
                    }
                    KeyCode::Enter => {
                        // Apply selection
                        let act_yes = match *focus {
                            OF::Yes | OF::All => true,
                            OF::No | OF::None => false,
                            OF::Older => {
                                if let (Ok(s), Ok(d)) =
                                    (app.vfs.stat(src_path), app.vfs.stat(dst_path))
                                {
                                    s.modified > d.modified
                                } else {
                                    false
                                }
                            }
                            OF::Smaller => {
                                if let (Ok(s), Ok(d)) =
                                    (app.vfs.stat(src_path), app.vfs.stat(dst_path))
                                {
                                    s.size < d.size
                                } else {
                                    false
                                }
                            }
                            OF::SizeDiffers => {
                                if let (Ok(s), Ok(d)) =
                                    (app.vfs.stat(src_path), app.vfs.stat(dst_path))
                                {
                                    s.size != d.size
                                } else {
                                    false
                                }
                            }
                            OF::Append => false,
                        };
                        if *focus == OF::Append {
                            use std::fs::OpenOptions;
                            use std::io::{Read, Write};
                            let res = (|| -> anyhow::Result<()> {
                                let mut rdr = app.vfs.read_file(src_path)?;
                                let mut f = OpenOptions::new()
                                    .create(true)
                                    .append(true)
                                    .open(dst_path)?;
                                let mut buf = Vec::new();
                                rdr.read_to_end(&mut buf)?;
                                f.write_all(&buf)?;
                                Ok(())
                            })();
                            match res {
                                Ok(()) => {
                                    app.ui_mode = UiMode::Normal;
                                    app.reload_panels()?;
                                }
                                Err(err) => {
                                    app.ui_mode = UiMode::DialogConfirm {
                                        title: "Error".into(),
                                        message: format!("{err}"),
                                        on_ok: Box::new(|_| Ok(())),
                                    };
                                }
                            }
                        } else if act_yes {
                            let op = *op;
                            let src = src_path.clone();
                            let dst = dst_path.clone();
                            let _ = app.vfs.remove(&dst, false);
                            app.ui_mode = Self::file_op_progress_mode(
                                app.vfs.as_ref(),
                                &app.jobs,
                                &app.config_opts,
                                op,
                                src,
                                dst,
                            );
                        } else {
                            app.ui_mode = UiMode::Normal;
                        }
                    }
                    _ => {}
                }
                return Ok(());
            }
            UiMode::ChmodDialog {
                name: _,
                mode,
                ur,
                uw,
                ux,
                gr,
                gw,
                gx,
                or_,
                ow,
                ox,
                suid,
                sgid,
                sticky,
                recursive,
                focus_index,
            } => {
                // 0..8: rwx (u,g,o), 9..11: suid/sgid/sticky, 12: recursive, 13: OK, 14: Cancel
                let total_fields = 15usize;
                match key.code {
                    KeyCode::Esc | KeyCode::F(10) => app.ui_mode = UiMode::Normal,
                    KeyCode::Tab => {
                        *focus_index = (*focus_index + 1) % total_fields;
                    }
                    KeyCode::BackTab => {
                        *focus_index = (*focus_index + total_fields - 1) % total_fields;
                    }
                    KeyCode::Char(' ') => {
                        match *focus_index {
                            0 => *ur = !*ur,
                            1 => *uw = !*uw,
                            2 => *ux = !*ux,
                            3 => *gr = !*gr,
                            4 => *gw = !*gw,
                            5 => *gx = !*gx,
                            6 => *or_ = !*or_,
                            7 => *ow = !*ow,
                            8 => *ox = !*ox,
                            9 => *suid = !*suid,
                            10 => *sgid = !*sgid,
                            11 => *sticky = !*sticky,
                            12 => *recursive = !*recursive,
                            _ => {}
                        }
                        // Recompute mode from flags
                        let mut m = 0u32;
                        if *ur {
                            m |= 0o400
                        }
                        if *uw {
                            m |= 0o200
                        }
                        if *ux {
                            m |= 0o100
                        }
                        if *gr {
                            m |= 0o040
                        }
                        if *gw {
                            m |= 0o020
                        }
                        if *gx {
                            m |= 0o010
                        }
                        if *or_ {
                            m |= 0o004
                        }
                        if *ow {
                            m |= 0o002
                        }
                        if *ox {
                            m |= 0o001
                        }
                        if *suid {
                            m |= 0o4000
                        }
                        if *sgid {
                            m |= 0o2000
                        }
                        if *sticky {
                            m |= 0o1000
                        }
                        *mode = m;
                    }
                    KeyCode::Enter => {
                        // Cancel selected
                        if *focus_index == 14 {
                            app.ui_mode = UiMode::Normal;
                            return Ok(());
                        }
                        // Only OK applies
                        if *focus_index != 13 {
                            return Ok(());
                        }
                        let mode_val = *mode;
                        let recursive_val = *recursive;
                        app.ui_mode = UiMode::Normal;
                        // Collect paths: selected entries or current
                        let paths: Vec<std::path::PathBuf> = {
                            let p = app.active_panel();
                            let mut out = Vec::new();
                            if p.selection.is_empty() {
                                if let Some(ent) = p.current_entry() {
                                    if ent.name != ".." {
                                        out.push(ent.path.clone());
                                    }
                                }
                            } else {
                                for idx in p.selection.iter() {
                                    if let Some(ent) = p.entries.get(idx) {
                                        if ent.name != ".." {
                                            out.push(ent.path.clone());
                                        }
                                    }
                                }
                            }
                            out
                        };
                        // Try apply; on error, show error dialog
                        let mut first_err: Option<anyhow::Error> = None;
                        for p in paths {
                            if let Err(e) = app.vfs.chmod(&p, mode_val, recursive_val) {
                                first_err = Some(anyhow::Error::new(e));
                                break;
                            }
                        }
                        if let Some(err) = first_err {
                            app.ui_mode = UiMode::DialogConfirm {
                                title: "Error".into(),
                                message: format!("{err}"),
                                on_ok: Box::new(|_| Ok(())),
                            };
                        } else {
                            app.reload_panels()?;
                        }
                    }
                    _ => {}
                }
                return Ok(());
            }
            UiMode::ChownDialog {
                owner,
                group,
                recursive,
                focus_index,
            } => {
                match key.code {
                    KeyCode::Esc | KeyCode::F(10) => app.ui_mode = UiMode::Normal,
                    KeyCode::Tab => {
                        *focus_index = (*focus_index + 1) % 5;
                    }
                    KeyCode::BackTab => {
                        *focus_index = (*focus_index + 5 - 1) % 5;
                    }
                    KeyCode::Char(c) if *focus_index == 0 && key.modifiers.is_empty() => {
                        owner.push(c);
                    }
                    KeyCode::Backspace if *focus_index == 0 => {
                        owner.pop();
                    }
                    KeyCode::Char(c) if *focus_index == 1 && key.modifiers.is_empty() => {
                        group.push(c);
                    }
                    KeyCode::Backspace if *focus_index == 1 => {
                        group.pop();
                    }
                    KeyCode::Char(' ') if *focus_index == 2 => {
                        *recursive = !*recursive;
                    }
                    KeyCode::Enter => {
                        // Cancel
                        if *focus_index == 4 {
                            app.ui_mode = UiMode::Normal;
                            return Ok(());
                        }
                        // Only OK applies
                        if *focus_index != 3 {
                            return Ok(());
                        }
                        // Capture values and close dialog to avoid borrow conflicts
                        let owner_val = owner.clone();
                        let group_val = group.clone();
                        let recursive_val = *recursive;
                        app.ui_mode = UiMode::Normal;
                        // Apply to selected/current
                        let paths: Vec<std::path::PathBuf> = {
                            let p = app.active_panel();
                            let mut out = Vec::new();
                            if p.selection.is_empty() {
                                if let Some(ent) = p.current_entry() {
                                    if ent.name != ".." {
                                        out.push(ent.path.clone());
                                    }
                                }
                            } else {
                                for idx in p.selection.iter() {
                                    if let Some(ent) = p.entries.get(idx) {
                                        if ent.name != ".." {
                                            out.push(ent.path.clone());
                                        }
                                    }
                                }
                            }
                            out
                        };
                        let owner_opt = if owner_val.trim().is_empty() {
                            None
                        } else {
                            Some(owner_val.trim().to_string())
                        };
                        let group_opt = if group_val.trim().is_empty() {
                            None
                        } else {
                            Some(group_val.trim().to_string())
                        };
                        let mut first_err: Option<anyhow::Error> = None;
                        for p in paths {
                            if let Err(e) = app.vfs.chown(
                                &p,
                                owner_opt.as_deref(),
                                group_opt.as_deref(),
                                recursive_val,
                            ) {
                                first_err = Some(anyhow::Error::new(e));
                                break;
                            }
                        }
                        if let Some(err) = first_err {
                            app.ui_mode = UiMode::DialogConfirm {
                                title: "Error".into(),
                                message: format!("{err}"),
                                on_ok: Box::new(|_| Ok(())),
                            };
                        } else {
                            app.reload_panels()?;
                        }
                    }
                    _ => {}
                }
                return Ok(());
            }
            UiMode::Menu {
                top_index,
                selected_index,
                dropped,
            } => {
                let menus: [&[&str]; 5] = [
                    &[
                        "Copy",
                        "Move",
                        "Mkdir",
                        "Delete",
                        "FTP link",
                        "Shell link",
                        "SFTP link",
                        "SMB link",
                        "Listing mode...",
                        "Sort order...",
                        "Tree",
                        "Filter",
                    ],
                    &[
                        "View",
                        "Edit",
                        "Copy",
                        "Move",
                        "Mkdir",
                        "Delete",
                        "Chmod",
                        "Chown",
                        "Hard link",
                        "SymLink",
                        "Relative symlink",
                        "Quit",
                    ],
                    &[
                        "User menu",
                        "Find file",
                        "Directory hotlist",
                        "Compare dirs",
                        "External panelize",
                    ],
                    &[
                        "Configuration",
                        "Layout",
                        "Panels",
                        "Confirmations",
                        "Appearance",
                        "Virtual FS...",
                        "Learn keys",
                        "Save setup",
                    ],
                    &[
                        "Copy",
                        "Move",
                        "Mkdir",
                        "Delete",
                        "FTP link",
                        "Shell link",
                        "SFTP link",
                        "SMB link",
                        "Listing mode...",
                        "Sort order...",
                        "Tree",
                        "Filter",
                    ],
                ];
                match key.code {
                    KeyCode::Esc | KeyCode::F(9) | KeyCode::F(10) => {
                        app.ui_mode = UiMode::Normal;
                    }
                    KeyCode::Left => {
                        if *top_index > 0 {
                            *top_index -= 1;
                        }
                        *selected_index = 0;
                    }
                    KeyCode::Right => {
                        if *top_index < 4 {
                            *top_index += 1;
                        }
                        *selected_index = 0;
                    }
                    KeyCode::Up => {
                        if *dropped && *selected_index > 0 {
                            *selected_index -= 1;
                        }
                    }
                    KeyCode::Down => {
                        if !*dropped {
                            *dropped = true;
                            *selected_index = 0;
                        } else {
                            let max = menus[*top_index].len().saturating_sub(1);
                            if *selected_index < max {
                                *selected_index += 1;
                            }
                        }
                    }
                    KeyCode::Enter => {
                        if !*dropped {
                            *dropped = true;
                            *selected_index = 0;
                            return Ok(());
                        }
                        let item = menus[*top_index][*selected_index];
                        match item {
                            "Listing mode..." => {
                                let side = match *top_index {
                                    0 => rmc_core::actions::PaneSide::Left,
                                    4 => rmc_core::actions::PaneSide::Right,
                                    _ => rmc_core::actions::PaneSide::Left,
                                };
                                // Prefill from the chosen panel
                                let (listing, user_format) = {
                                    let p = if matches!(side, rmc_core::actions::PaneSide::Left) {
                                        &app.left
                                    } else {
                                        &app.right
                                    };
                                    (p.listing, p.user_format.clone())
                                };
                                let focus = match listing {
                                    rmc_core::panel::ListingFormat::Full => {
                                        rmc_core::app::ListingModeFocus::RadioFull
                                    }
                                    rmc_core::panel::ListingFormat::Brief => {
                                        rmc_core::app::ListingModeFocus::RadioBrief
                                    }
                                    rmc_core::panel::ListingFormat::Long => {
                                        rmc_core::app::ListingModeFocus::RadioLong
                                    }
                                    rmc_core::panel::ListingFormat::User => {
                                        rmc_core::app::ListingModeFocus::RadioUser
                                    }
                                };
                                app.ui_mode = UiMode::ListingModeDialog {
                                    side,
                                    listing,
                                    user_format,
                                    focus,
                                };
                            }
                            "Configuration" => {
                                let draft = app.config_opts;
                                app.ui_mode = UiMode::ConfigurationDialog {
                                    draft,
                                    focus: rmc_core::app::ConfigOptionsFocus::Verbose,
                                };
                            }
                            "Layout" => {
                                // Prefill dialog from current options
                                let draft = app.layout;
                                app.ui_mode = UiMode::LayoutDialog {
                                    draft,
                                    focus: LayoutFocus::MenuBar,
                                };
                            }
                            "Appearance" => {
                                // Build list of available skins; always include default
                                let mut skins = crate::skin::list_available_skins();
                                if skins.is_empty() {
                                    skins.push("default".to_string());
                                }
                                // Prefill selection from current app.skin_name
                                let mut selected = 0usize;
                                for (i, s) in skins.iter().enumerate() {
                                    if s == &app.skin_name {
                                        selected = i;
                                        break;
                                    }
                                }
                                app.ui_mode = UiMode::AppearanceDialog {
                                    draft_skin: app.skin_name.clone(),
                                    draft_shadows: app.shadows,
                                    skins,
                                    selected,
                                    focus: rmc_core::app::AppearanceFocus::SkinList,
                                };
                            }
                            "Virtual FS..." => {
                                let draft = app.vfs_opts.clone();
                                app.ui_mode = UiMode::VfsOptionsDialog {
                                    draft,
                                    focus: rmc_core::app::VfsOptionsFocus::AlwaysUseFtpProxy,
                                };
                            }
                            "Learn keys" => {
                                // Build draft list of actions with their current first key.
                                use rmc_core::actions::Action as A;
                                let actions: [(&str, A); 15] = [
                                    ("Help", A::ShowHelp),
                                    ("User menu", A::ShowUserMenu),
                                    ("View", A::ViewFile),
                                    ("Edit", A::FunctionKey(4)),
                                    ("Copy", A::Copy),
                                    ("Rename/Move", A::Move),
                                    ("Make directory", A::Mkdir),
                                    ("Delete", A::Delete),
                                    ("Pull down", A::FocusMenu),
                                    ("Quit", A::Quit),
                                    ("Select", A::ToggleSelect),
                                    ("Subshell", A::ToggleSubshell),
                                    ("Hidden files", A::ToggleHidden),
                                    ("Swap panels", A::SwapPanels),
                                    ("Refresh", A::Refresh),
                                ];
                                let mut draft: Vec<(A, crossterm::event::KeyEvent)> = Vec::new();
                                for (_label, act) in actions {
                                    if let Some(k) = app.keymap.first_key_for_action(&act) {
                                        draft.push((act.clone(), k));
                                    } else {
                                        // Fallback: keep row but use a dummy Enter; renderer will show it
                                        draft.push((
                                            act.clone(),
                                            crossterm::event::KeyEvent::new(
                                                crossterm::event::KeyCode::Char('?'),
                                                crossterm::event::KeyModifiers::NONE,
                                            ),
                                        ));
                                    }
                                }
                                app.ui_mode = UiMode::LearnKeysDialog {
                                    draft,
                                    selected: 0,
                                    capturing: false,
                                    focus_ok: true,
                                };
                            }
                            "Save setup" => {
                                // Save options + keymap to user config dir and show confirmation.
                                if let Err(e) = rmc_core::config::save_setup(app) {
                                    app.ui_mode = UiMode::DialogConfirm {
                                        title: "Error".into(),
                                        message: e.to_string(),
                                        on_ok: Box::new(|_| Ok(())),
                                    };
                                } else {
                                    app.ui_mode = UiMode::DialogConfirm {
                                        title: "Info".into(),
                                        message: "Setup saved".into(),
                                        on_ok: Box::new(|_| Ok(())),
                                    };
                                }
                            }
                            "Panels" => {
                                let draft = app.panel_opts;
                                app.ui_mode = UiMode::PanelOptionsDialog {
                                    draft,
                                    focus: rmc_core::app::PanelOptionsFocus::ShowHidden,
                                };
                            }
                            "Confirmations" => {
                                let draft = app.confirm;
                                app.ui_mode = UiMode::ConfirmationsDialog {
                                    draft,
                                    focus: rmc_core::app::ConfirmationsFocus::Delete,
                                };
                            }
                            "Filter" => {
                                // Open simple input dialog to set filename filter for the chosen side.
                                let set_left = *top_index == 0;
                                app.ui_mode = UiMode::InputDialog {
                                    title: "Filter".into(),
                                    prompt: "Enter glob (e.g. *.c):".into(),
                                    value: "*".into(),
                                    focus_ok: false,
                                    on_submit: Box::new(move |app, input| {
                                        let pat = input.trim().to_string();
                                        let panel = if set_left {
                                            &mut app.left
                                        } else {
                                            &mut app.right
                                        };
                                        if pat.is_empty() || pat == "*" {
                                            panel.filter_glob = None;
                                        } else {
                                            panel.filter_glob = Some(pat);
                                        }
                                        Ok(())
                                    }),
                                };
                            }
                            "FTP link" | "SFTP link" | "Shell link" | "SMB link" => {
                                match item {
                                    // New multi-field form for FTP/SFTP
                                    "FTP link" | "SFTP link" => {
                                        let scheme =
                                            if item == "FTP link" { "ftp" } else { "sftp" };
                                        app.ui_mode = UiMode::FtpConnectDialog {
                                            scheme: scheme.to_string(),
                                            host: String::new(),
                                            port: String::new(),
                                            user: String::new(),
                                            password: String::new(),
                                            directory: "/".to_string(),
                                            anonymous: false,
                                            focus_index: 0,
                                            focus_ok: false,
                                        };
                                    }
                                    // Keep existing URL input for Shell/SMB
                                    "Shell link" | "SMB link" => {
                                        let (scheme, title, prompt) = match item {
                                            "Shell link" => (
                                                "fish",
                                                "Shell link to machine".to_string(),
                                                "Enter fish URL (e.g. fish://user@host/path):"
                                                    .to_string(),
                                            ),
                                            "SMB link" => (
                                                "smb",
                                                "SMB link to machine".to_string(),
                                                "Enter smb URL (e.g. smb://host/share/path):"
                                                    .to_string(),
                                            ),
                                            _ => unreachable!(),
                                        };
                                        app.ui_mode = UiMode::InputDialog {
                                            title,
                                            prompt,
                                            value: String::new(),
                                            focus_ok: false,
                                            on_submit: Box::new(move |app, input| {
                                                let trimmed = input.trim();
                                                if trimmed.is_empty() {
                                                    return Ok(());
                                                }
                                                let url_str = if trimmed.starts_with("fish://")
                                                    || trimmed.starts_with("smb://")
                                                {
                                                    trimmed.to_string()
                                                } else {
                                                    format!("{scheme}://{trimmed}")
                                                };
                                                let path = std::path::PathBuf::from(url_str);
                                                match app.change_dir(&path) {
                                                    Ok(()) => Ok(()),
                                                    Err(err) => {
                                                        app.ui_mode = UiMode::DialogConfirm {
                                                            title: "Error".into(),
                                                            message: format!("{err}"),
                                                            on_ok: Box::new(|_| Ok(())),
                                                        };
                                                        Ok(())
                                                    }
                                                }
                                            }),
                                        };
                                    }
                                    _ => unreachable!(),
                                }
                            }
                            "Sort order..." => {
                                let side = match *top_index {
                                    0 => rmc_core::actions::PaneSide::Left,
                                    4 => rmc_core::actions::PaneSide::Right,
                                    _ => rmc_core::actions::PaneSide::Left,
                                };
                                // Prefill from the chosen panel
                                let (by, reverse, dirs_first) = {
                                    let p = if matches!(side, rmc_core::actions::PaneSide::Left) {
                                        &app.left
                                    } else {
                                        &app.right
                                    };
                                    (
                                        p.sort_by,
                                        matches!(p.sort_dir, rmc_core::sorting::SortDir::Desc),
                                        p.dirs_first,
                                    )
                                };
                                app.ui_mode = UiMode::SortDialog {
                                    side,
                                    focus_index: 0,
                                    by,
                                    reverse,
                                    dirs_first,
                                };
                            }
                            "Tree" => {
                                // Set Tree mode for the chosen side and make it active.
                                let set_left = *top_index == 0;
                                let panel = if set_left {
                                    &mut app.left
                                } else {
                                    &mut app.right
                                };
                                panel.mode = rmc_core::panel::PanelMode::Tree;
                                // Build a simple flattened tree starting at the panel's cwd
                                let start = panel.cwd.clone();
                                let max_entries = 2048usize;
                                let mut entries: Vec<rmc_core::panel::TreeEntry> = Vec::new();
                                build_tree_flat(&*app.vfs, &start, 0, max_entries, &mut entries);
                                panel.tree = Some(rmc_core::panel::TreeState {
                                    entries,
                                    cursor: 0,
                                    scroll_top: 0,
                                });
                                app.active = if set_left {
                                    rmc_core::actions::PaneSide::Left
                                } else {
                                    rmc_core::actions::PaneSide::Right
                                };
                                app.ui_mode = UiMode::Normal;
                            }
                            "Copy" => {
                                return Self::handle_key(
                                    app,
                                    KeyEvent::new(KeyCode::F(5), key.modifiers),
                                    page_rows,
                                );
                            }
                            "Move" => {
                                return Self::handle_key(
                                    app,
                                    KeyEvent::new(KeyCode::F(6), key.modifiers),
                                    page_rows,
                                );
                            }
                            "Mkdir" => {
                                return Self::handle_key(
                                    app,
                                    KeyEvent::new(KeyCode::F(7), key.modifiers),
                                    page_rows,
                                );
                            }
                            "Delete" => {
                                return Self::handle_key(
                                    app,
                                    KeyEvent::new(KeyCode::F(8), key.modifiers),
                                    page_rows,
                                );
                            }
                            "Chmod" => {
                                // Simulate C-x c chord
                                app.pending_ctrl_x = true;
                                return Self::handle_key(
                                    app,
                                    KeyEvent::new(KeyCode::Char('c'), key.modifiers),
                                    page_rows,
                                );
                            }
                            "Chown" => {
                                app.pending_ctrl_x = true;
                                return Self::handle_key(
                                    app,
                                    KeyEvent::new(KeyCode::Char('o'), key.modifiers),
                                    page_rows,
                                );
                            }
                            "Hard link" => {
                                app.pending_ctrl_x = true;
                                return Self::handle_key(
                                    app,
                                    KeyEvent::new(KeyCode::Char('l'), key.modifiers),
                                    page_rows,
                                );
                            }
                            "SymLink" => {
                                app.pending_ctrl_x = true;
                                return Self::handle_key(
                                    app,
                                    KeyEvent::new(KeyCode::Char('s'), key.modifiers),
                                    page_rows,
                                );
                            }
                            "Relative symlink" => {
                                app.pending_ctrl_x = true;
                                return Self::handle_key(
                                    app,
                                    KeyEvent::new(KeyCode::Char('v'), key.modifiers),
                                    page_rows,
                                );
                            }
                            "User menu" => {
                                return Self::handle_key(
                                    app,
                                    KeyEvent::new(KeyCode::F(2), key.modifiers),
                                    page_rows,
                                );
                            }
                            "Directory hotlist" => {
                                let st = rmc_core::hotlist::HotlistDialogState::new(
                                    app.hotlist.entries.clone(),
                                );
                                app.ui_mode = UiMode::HotlistDialog(st);
                            }
                            "Find file" => {
                                let start = app.active_panel().cwd.clone();
                                app.ui_mode = UiMode::FindDialog(FindDialogState::new(start));
                            }
                            "Compare dirs" => {
                                open_compare_dirs_dialog(app);
                            }
                            "External panelize" => {
                                let cwd = active_cwd.clone();
                                app.ui_mode = UiMode::InputDialog {
                                    title: "External panelize".into(),
                                    prompt: "Enter command:".into(),
                                    value: String::new(),
                                    focus_ok: false,
                                    on_submit: Box::new(move |app, input| {
                                        let cmd = input.trim().to_string();
                                        if cmd.is_empty() {
                                            return Ok(());
                                        }
                                        // Run the command in the active panel cwd and capture stdout
                                        let output = std::process::Command::new("sh")
                                            .arg("-c")
                                            .arg(&cmd)
                                            .current_dir(&cwd)
                                            .output();
                                        match output {
                                            Ok(out) => {
                                                let mut paths: Vec<std::path::PathBuf> = Vec::new();
                                                let stdout = String::from_utf8_lossy(&out.stdout);
                                                for line in stdout.lines() {
                                                    let t = line.trim();
                                                    if t.is_empty() {
                                                        continue;
                                                    }
                                                    let mut p = std::path::PathBuf::from(t);
                                                    if !p.is_absolute() {
                                                        p = cwd.join(t);
                                                    }
                                                    if app.vfs.stat(&p).is_ok() {
                                                        paths.push(p);
                                                    }
                                                }
                                                if paths.is_empty() {
                                                    app.ui_mode = UiMode::DialogConfirm {
                                                        title: "Error".into(),
                                                        message:
                                                            "Command failed or produced no files"
                                                                .into(),
                                                        on_ok: Box::new(|_| Ok(())),
                                                    };
                                                    return Ok(());
                                                }
                                                app.panelize_paths(&paths, Some(&cwd))?;
                                                app.ui_mode = UiMode::Normal;
                                                Ok(())
                                            }
                                            Err(_e) => {
                                                app.ui_mode = UiMode::DialogConfirm {
                                                    title: "Error".into(),
                                                    message: "Command failed or produced no files"
                                                        .into(),
                                                    on_ok: Box::new(|_| Ok(())),
                                                };
                                                Ok(())
                                            }
                                        }
                                    }),
                                };
                            }
                            "Quit" => {
                                app.handle_action(Action::Quit)?;
                            }
                            _ => {
                                app.ui_mode = UiMode::Normal;
                            }
                        }
                    }
                    _ => {}
                }
                return Ok(());
            }
            UiMode::Diff(state) => {
                // Confirm-exit overlay
                if let Some(confirm) = &mut state.confirm_exit {
                    match key.code {
                        KeyCode::Esc => {
                            state.confirm_exit = None;
                        }
                        KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
                            use rmc_core::app::YncFocus as F;
                            confirm.focus = match (key.code, confirm.focus) {
                                (KeyCode::Left, F::No) => F::Yes,
                                (KeyCode::Left, F::Cancel) => F::No,
                                (KeyCode::Right, F::Yes) => F::No,
                                (KeyCode::Right, F::No) => F::Cancel,
                                (KeyCode::Right, F::Cancel) => F::Cancel,
                                (_, f) => match f {
                                    F::Yes => F::No,
                                    F::No => F::Cancel,
                                    F::Cancel => F::Yes,
                                },
                            };
                        }
                        KeyCode::Enter => {
                            use rmc_core::app::YncFocus as F;
                            match confirm.focus {
                                F::Yes => {
                                    // Save modified then exit
                                    if state.left_modified {
                                        let mut w = app
                                            .vfs
                                            .write_file(&state.left_path)
                                            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
                                        use std::io::Write;
                                        let s = rmc_diff::join_lines(&state.left_lines);
                                        let _ = w.write_all(s.as_bytes());
                                    }
                                    if state.right_modified {
                                        let mut w = app
                                            .vfs
                                            .write_file(&state.right_path)
                                            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
                                        use std::io::Write;
                                        let s = rmc_diff::join_lines(&state.right_lines);
                                        let _ = w.write_all(s.as_bytes());
                                    }
                                    app.ui_mode = UiMode::Normal;
                                }
                                F::No => {
                                    // Discard changes and exit
                                    app.ui_mode = UiMode::Normal;
                                }
                                F::Cancel => {
                                    state.confirm_exit = None;
                                }
                            }
                        }
                        _ => {}
                    }
                    return Ok(());
                }
                // Handle inline search prompt
                if let Some(prompt) = &mut state.search_prompt {
                    match key.code {
                        KeyCode::Esc => {
                            state.search_prompt = None;
                        }
                        KeyCode::Enter => {
                            let q = prompt.clone();
                            state.search = if q.is_empty() { None } else { Some(q) };
                            state.search_prompt = None;
                            // Jump to first matching hunk from current
                            if let Some(ref qq) = state.search {
                                if let Some(idx) = Self::search_next_hunk_with(
                                    &state.hunks,
                                    &state.left_lines,
                                    &state.right_lines,
                                    state.current_hunk,
                                    qq,
                                ) {
                                    state.current_hunk = idx;
                                    Self::ensure_hunk_visible(state);
                                }
                            }
                        }
                        KeyCode::Backspace => {
                            prompt.pop();
                        }
                        KeyCode::Char(c) if key.modifiers.is_empty() => {
                            prompt.push(c);
                        }
                        _ => {}
                    }
                    return Ok(());
                }
                // Handle goto prompt
                if let Some(prompt) = &mut state.goto_prompt {
                    match key.code {
                        KeyCode::Esc => state.goto_prompt = None,
                        KeyCode::Enter => {
                            let val = prompt.clone();
                            if let Ok(n) = val.trim().parse::<usize>() {
                                let line = n.saturating_sub(1);
                                state.left_scroll = line;
                                state.right_scroll = line;
                            }
                            state.goto_prompt = None;
                        }
                        KeyCode::Backspace => {
                            prompt.pop();
                        }
                        KeyCode::Char(c) if key.modifiers.is_empty() => prompt.push(c),
                        _ => {}
                    }
                    return Ok(());
                }
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc | KeyCode::F(10) => {
                        if state.left_modified || state.right_modified {
                            state.confirm_exit = Some(rmc_core::app::YncDialog {
                                title: "Save modified files?".into(),
                                message: "You have unsaved merges. Save before quitting?".into(),
                                focus: rmc_core::app::YncFocus::Yes,
                            });
                        } else {
                            app.ui_mode = UiMode::Normal;
                        }
                    }
                    KeyCode::Enter | KeyCode::Char(' ') => {
                        if !state.hunks.is_empty() {
                            if let Some(next) =
                                Self::next_diff_hunk_index(&state.hunks, state.current_hunk)
                            {
                                state.current_hunk = next;
                            }
                            Self::ensure_hunk_visible(state);
                        }
                    }
                    KeyCode::Backspace | KeyCode::Char('p') => {
                        if !state.hunks.is_empty() {
                            if let Some(prev) =
                                Self::prev_diff_hunk_index(&state.hunks, state.current_hunk)
                            {
                                state.current_hunk = prev;
                            }
                            Self::ensure_hunk_visible(state);
                        }
                    }
                    KeyCode::Char('g') => {
                        state.goto_prompt = Some(String::new());
                    }
                    KeyCode::Char('f') => state.panel_ratio = 0.8,
                    KeyCode::Char('=') => state.panel_ratio = 0.5,
                    KeyCode::Char('>') => state.panel_ratio = (state.panel_ratio + 0.05).min(0.8),
                    KeyCode::Char('<') => state.panel_ratio = (state.panel_ratio - 0.05).max(0.2),
                    KeyCode::Char('l') => state.show_line_numbers = !state.show_line_numbers,
                    KeyCode::Char('s') => state.show_hunk_status = !state.show_hunk_status,
                    KeyCode::Char('2') => state.tab_width = 2,
                    KeyCode::Char('3') => state.tab_width = 3,
                    KeyCode::Char('4') => state.tab_width = 4,
                    KeyCode::Char('8') => state.tab_width = 8,
                    KeyCode::Char('/') | KeyCode::F(7) => state.search_prompt = Some(String::new()),
                    KeyCode::Char('n') | KeyCode::F(17) => {
                        if let Some(ref qq) = state.search {
                            if let Some(idx) = Self::search_next_hunk_with(
                                &state.hunks,
                                &state.left_lines,
                                &state.right_lines,
                                state.current_hunk,
                                qq,
                            ) {
                                state.current_hunk = idx;
                                Self::ensure_hunk_visible(state);
                            }
                        } else if !state.hunks.is_empty() {
                            if let Some(next) =
                                Self::next_diff_hunk_index(&state.hunks, state.current_hunk)
                            {
                                state.current_hunk = next;
                                Self::ensure_hunk_visible(state);
                            }
                        }
                    }
                    KeyCode::F(2) => {
                        // Save modified files
                        if state.left_modified {
                            let mut w = app
                                .vfs
                                .write_file(&state.left_path)
                                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
                            use std::io::Write;
                            let s = rmc_diff::join_lines(&state.left_lines);
                            let _ = w.write_all(s.as_bytes());
                            state.left_modified = false;
                        }
                        if state.right_modified {
                            let mut w = app
                                .vfs
                                .write_file(&state.right_path)
                                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
                            use std::io::Write;
                            let s = rmc_diff::join_lines(&state.right_lines);
                            let _ = w.write_all(s.as_bytes());
                            state.right_modified = false;
                        }
                        // Recompute diff
                        let la = rmc_diff::join_lines(&state.left_lines);
                        let ra = rmc_diff::join_lines(&state.right_lines);
                        state.hunks = rmc_diff::compute_diff(&la, &ra).hunks;
                        state.current_hunk =
                            state.current_hunk.min(state.hunks.len().saturating_sub(1));
                        Self::ensure_hunk_visible(state);
                    }
                    KeyCode::F(5) => {
                        if !state.hunks.is_empty() {
                            let idx = state.current_hunk.min(state.hunks.len() - 1);
                            if state.merge_target_right {
                                let _ = rmc_diff::apply_hunk_merge(
                                    &mut state.left_lines,
                                    &mut state.right_lines,
                                    &state.hunks,
                                    idx,
                                    rmc_diff::MergeTarget::Right,
                                );
                                state.right_modified = true;
                            } else {
                                let _ = rmc_diff::apply_hunk_merge(
                                    &mut state.left_lines,
                                    &mut state.right_lines,
                                    &state.hunks,
                                    idx,
                                    rmc_diff::MergeTarget::Left,
                                );
                                state.left_modified = true;
                            }
                            // Re-diff
                            let la = rmc_diff::join_lines(&state.left_lines);
                            let ra = rmc_diff::join_lines(&state.right_lines);
                            state.hunks = rmc_diff::compute_diff(&la, &ra).hunks;
                            state.current_hunk =
                                state.current_hunk.min(state.hunks.len().saturating_sub(1));
                            Self::ensure_hunk_visible(state);
                        }
                    }
                    KeyCode::Up => {
                        state.left_scroll = state.left_scroll.saturating_sub(1);
                        state.right_scroll = state.right_scroll.saturating_sub(1);
                    }
                    KeyCode::Down => {
                        state.left_scroll = state.left_scroll.saturating_add(1);
                        state.right_scroll = state.right_scroll.saturating_add(1);
                    }
                    KeyCode::PageUp => {
                        let step = page_rows.max(1);
                        state.left_scroll = state.left_scroll.saturating_sub(step);
                        state.right_scroll = state.right_scroll.saturating_sub(step);
                    }
                    KeyCode::PageDown => {
                        let step = page_rows.max(1);
                        state.left_scroll = state.left_scroll.saturating_add(step);
                        state.right_scroll = state.right_scroll.saturating_add(step);
                    }
                    _ => {
                        // Ctrl-U: swap sides; Ctrl-R: refresh
                        if let KeyCode::Char('u') = key.code {
                            if key
                                .modifiers
                                .contains(crossterm::event::KeyModifiers::CONTROL)
                            {
                                std::mem::swap(&mut state.left_path, &mut state.right_path);
                                std::mem::swap(&mut state.left_lines, &mut state.right_lines);
                                state.merge_target_right = !state.merge_target_right;
                                let la = rmc_diff::join_lines(&state.left_lines);
                                let ra = rmc_diff::join_lines(&state.right_lines);
                                state.hunks = rmc_diff::compute_diff(&la, &ra).hunks;
                                state.current_hunk = 0;
                                state.left_scroll = 0;
                                state.right_scroll = 0;
                            }
                        } else if let KeyCode::Char('r') = key.code {
                            if key
                                .modifiers
                                .contains(crossterm::event::KeyModifiers::CONTROL)
                            {
                                let mut ltxt = String::new();
                                let mut rtxt = String::new();
                                if let Ok(mut r) = app.vfs.read_file(&state.left_path) {
                                    use std::io::Read;
                                    let _ = r.read_to_string(&mut ltxt);
                                }
                                if let Ok(mut r) = app.vfs.read_file(&state.right_path) {
                                    use std::io::Read;
                                    let _ = r.read_to_string(&mut rtxt);
                                }
                                state.left_lines = rmc_diff::split_lines(&ltxt);
                                state.right_lines = rmc_diff::split_lines(&rtxt);
                                state.hunks = rmc_diff::compute_diff(&ltxt, &rtxt).hunks;
                                state.current_hunk = 0;
                                state.left_modified = false;
                                state.right_modified = false;
                                state.left_scroll = 0;
                                state.right_scroll = 0;
                            }
                        }
                    }
                }
                return Ok(());
            }
            UiMode::Viewer { .. } => {
                // Viewer File/Command/Options pull-down (GNU: click the topmost line).
                if let UiMode::Viewer {
                    viewer_menu: Some(menu),
                    search_dialog: None,
                    display_dialog: None,
                    goto_prompt: None,
                    ..
                } = &app.ui_mode
                {
                    let menu = *menu;
                    let items = menu.items();
                    let mut sel = menu.selected();
                    match key.code {
                        KeyCode::Esc => {
                            if let UiMode::Viewer { viewer_menu, .. } = &mut app.ui_mode {
                                *viewer_menu = None;
                            }
                            return Ok(());
                        }
                        KeyCode::Left => {
                            let next = match menu {
                                ViewerMenu::File { .. } => ViewerMenu::Options { selected: 0 },
                                ViewerMenu::Command { .. } => ViewerMenu::File { selected: 0 },
                                ViewerMenu::Options { .. } => ViewerMenu::Command { selected: 0 },
                            };
                            if let UiMode::Viewer { viewer_menu, .. } = &mut app.ui_mode {
                                *viewer_menu = Some(next);
                            }
                            return Ok(());
                        }
                        KeyCode::Right => {
                            let next = match menu {
                                ViewerMenu::File { .. } => ViewerMenu::Command { selected: 0 },
                                ViewerMenu::Command { .. } => ViewerMenu::Options { selected: 0 },
                                ViewerMenu::Options { .. } => ViewerMenu::File { selected: 0 },
                            };
                            if let UiMode::Viewer { viewer_menu, .. } = &mut app.ui_mode {
                                *viewer_menu = Some(next);
                            }
                            return Ok(());
                        }
                        KeyCode::Down => {
                            if !items.is_empty() {
                                sel = (sel + 1) % items.len();
                                if let UiMode::Viewer { viewer_menu, .. } = &mut app.ui_mode {
                                    *viewer_menu = Some(menu.with_selected(sel));
                                }
                            }
                            return Ok(());
                        }
                        KeyCode::Up => {
                            if !items.is_empty() {
                                sel = (sel + items.len() - 1) % items.len();
                                if let UiMode::Viewer { viewer_menu, .. } = &mut app.ui_mode {
                                    *viewer_menu = Some(menu.with_selected(sel));
                                }
                            }
                            return Ok(());
                        }
                        KeyCode::Enter | KeyCode::Char(' ') => {
                            match menu {
                                ViewerMenu::File { .. } => {
                                    app.handle_action(Action::ViewerQuit)?;
                                }
                                ViewerMenu::Command { .. } => {
                                    viewer_open_search_dialog(app, false);
                                }
                                ViewerMenu::Options { .. } => {
                                    viewer_open_display_options(app);
                                }
                            }
                            return Ok(());
                        }
                        _ => {
                            // Fall through so F-keys still work while the menu is open
                            // except F9 format (handled below after overlays).
                        }
                    }
                }
                // If viewer has an active goto or search prompt, handle overlay first
                if let UiMode::Viewer {
                    path,
                    offset,
                    hex,
                    goto_prompt: Some(prompt),
                    ..
                } = &mut app.ui_mode
                {
                    match key.code {
                        KeyCode::Esc | KeyCode::F(10) => {
                            *prompt = String::new();
                            if let UiMode::Viewer { goto_prompt, .. } = &mut app.ui_mode {
                                *goto_prompt = None;
                            }
                        }
                        KeyCode::Enter => {
                            let q = prompt.trim().to_string();
                            // Parse goto input:
                            // - Prefix "@": decimal byte offset
                            // - Prefix "0x": hex byte offset
                            // - Prefix ":" or "l " => line number (1-based)
                            // - Otherwise: if hex mode => offset; else => line number
                            let lower = q.to_ascii_lowercase();
                            let cpath = crate::terminal::viewer_ensure_view_for(path);
                            let res = if let Some(rest) = lower.strip_prefix('@') {
                                rest.parse::<u64>()
                                    .ok()
                                    .and_then(|v| rmc_view::clamp_offset(&cpath, v).ok())
                            } else if let Some(rest) = lower.strip_prefix("0x") {
                                u64::from_str_radix(rest, 16)
                                    .ok()
                                    .and_then(|v| rmc_view::clamp_offset(&cpath, v).ok())
                            } else if lower.starts_with(':') || lower.starts_with("l ") {
                                let num_str = if let Some(rest) = lower.strip_prefix(':') {
                                    rest
                                } else {
                                    // must start with "l " here
                                    &lower[2..]
                                };
                                num_str
                                    .trim()
                                    .parse::<u64>()
                                    .ok()
                                    .and_then(|ln| rmc_view::goto_line(&cpath, ln).ok())
                            } else if *hex {
                                // hex mode default: treat as offset (hex if contains 0x else decimal)
                                if let Some(rest) = lower.strip_prefix("0x") {
                                    u64::from_str_radix(rest, 16)
                                        .ok()
                                        .and_then(|v| rmc_view::clamp_offset(&cpath, v).ok())
                                } else {
                                    lower
                                        .parse::<u64>()
                                        .ok()
                                        .and_then(|v| rmc_view::clamp_offset(&cpath, v).ok())
                                }
                            } else {
                                // text mode default: treat as line number
                                lower
                                    .parse::<u64>()
                                    .ok()
                                    .and_then(|ln| rmc_view::goto_line(&cpath, ln).ok())
                            };
                            if let Some(new_off) = res {
                                *offset = new_off;
                            }
                            if let UiMode::Viewer { goto_prompt, .. } = &mut app.ui_mode {
                                *goto_prompt = None;
                            }
                        }
                        KeyCode::Backspace => {
                            prompt.pop();
                        }
                        KeyCode::Char(c) if key.modifiers.is_empty() => {
                            prompt.push(c);
                        }
                        _ => {}
                    }
                    return Ok(());
                }
                // GNU mcview F7 Search dialog (stays in Viewer).
                if let UiMode::Viewer {
                    path,
                    offset,
                    search,
                    search_case_sensitive,
                    search_backwards,
                    search_whole_words,
                    search_regexp,
                    status_msg,
                    search_dialog,
                    ..
                } = &mut app.ui_mode
                {
                    if let Some(dlg) = search_dialog {
                        use ViewerSearchFocus as F;
                        let order = [
                            F::Search,
                            F::CaseSensitive,
                            F::Backwards,
                            F::WholeWords,
                            F::RegularExpression,
                            F::Ok,
                            F::Cancel,
                        ];
                        let mut idx = order.iter().position(|f0| *f0 == dlg.focus).unwrap_or(0);
                        match key.code {
                            KeyCode::Esc | KeyCode::F(10) => {
                                *search_dialog = None;
                            }
                            KeyCode::F(7) => {
                                // Already open: do not nest.
                            }
                            KeyCode::Tab | KeyCode::Down => {
                                idx = (idx + 1) % order.len();
                                dlg.focus = order[idx];
                            }
                            KeyCode::BackTab | KeyCode::Up => {
                                idx = (idx + order.len() - 1) % order.len();
                                dlg.focus = order[idx];
                            }
                            KeyCode::Left | KeyCode::Right
                                if matches!(dlg.focus, F::Ok | F::Cancel) =>
                            {
                                dlg.focus = if matches!(dlg.focus, F::Ok) {
                                    F::Cancel
                                } else {
                                    F::Ok
                                };
                            }
                            KeyCode::Backspace if matches!(dlg.focus, F::Search) => {
                                dlg.search.pop();
                            }
                            // Space toggles checkboxes before generic Char so typing
                            // still inserts a space into the search field.
                            KeyCode::Char(' ')
                                if key.modifiers.is_empty() && dlg.focus.is_checkbox() =>
                            {
                                let _ = dlg.toggle_focused_checkbox();
                            }
                            KeyCode::Enter if dlg.focus.is_checkbox() => {
                                let _ = dlg.toggle_focused_checkbox();
                            }
                            KeyCode::Enter | KeyCode::Char(' ')
                                if matches!(dlg.focus, F::Ok | F::Cancel)
                                    || matches!(key.code, KeyCode::Enter) =>
                            {
                                match dlg.focus {
                                    F::Cancel => {
                                        *search_dialog = None;
                                    }
                                    F::Search | F::Ok => {
                                        if let Some(msg) = viewer_search_run(
                                            path,
                                            offset,
                                            search,
                                            search_case_sensitive,
                                            search_backwards,
                                            search_whole_words,
                                            search_regexp,
                                            dlg,
                                        )? {
                                            *status_msg = Some(msg);
                                        }
                                        *search_dialog = None;
                                    }
                                    _ => {}
                                }
                            }
                            KeyCode::Char(c)
                                if !key
                                    .modifiers
                                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                                    && matches!(dlg.focus, F::Search) =>
                            {
                                dlg.search.push(c);
                            }
                            _ => {}
                        }
                        return Ok(());
                    }
                }
                // mcview display-options dialog (Options → Display options). Stays in Viewer.
                if let UiMode::Viewer {
                    hex,
                    wrap,
                    show_line_numbers,
                    show_cr,
                    display_dialog,
                    ..
                } = &mut app.ui_mode
                {
                    if let Some(dlg) = display_dialog {
                        use ViewerDisplayFocus as F;
                        let order = [
                            F::ShowLineNumbers,
                            F::ShowCr,
                            F::WrapMode,
                            F::HexMode,
                            F::Ok,
                            F::Cancel,
                        ];
                        let mut idx = order.iter().position(|f0| *f0 == dlg.focus).unwrap_or(0);
                        match key.code {
                            KeyCode::Esc | KeyCode::F(10) => {
                                *display_dialog = None;
                            }
                            KeyCode::F(9) => {
                                // Already open: do not nest.
                            }
                            KeyCode::Tab | KeyCode::Down => {
                                idx = (idx + 1) % order.len();
                                dlg.focus = order[idx];
                            }
                            KeyCode::BackTab | KeyCode::Up => {
                                idx = (idx + order.len() - 1) % order.len();
                                dlg.focus = order[idx];
                            }
                            KeyCode::Left | KeyCode::Right
                                if matches!(dlg.focus, F::Ok | F::Cancel) =>
                            {
                                dlg.focus = if matches!(dlg.focus, F::Ok) {
                                    F::Cancel
                                } else {
                                    F::Ok
                                };
                            }
                            // Space / Enter toggle checkboxes before generic Char
                            // (same as Search).
                            KeyCode::Char(' ')
                                if key.modifiers.is_empty() && dlg.focus.is_checkbox() =>
                            {
                                let _ = dlg.toggle_focused_checkbox();
                            }
                            KeyCode::Enter if dlg.focus.is_checkbox() => {
                                let _ = dlg.toggle_focused_checkbox();
                            }
                            KeyCode::Enter | KeyCode::Char(' ')
                                if matches!(dlg.focus, F::Ok | F::Cancel)
                                    || matches!(key.code, KeyCode::Enter) =>
                            {
                                match dlg.focus {
                                    F::Cancel => {
                                        *display_dialog = None;
                                    }
                                    F::Ok => {
                                        *show_line_numbers = dlg.show_line_numbers;
                                        *show_cr = dlg.show_cr;
                                        *wrap = dlg.wrap;
                                        *hex = dlg.hex;
                                        *display_dialog = None;
                                    }
                                    _ => {}
                                }
                            }
                            _ => {}
                        }
                        return Ok(());
                    }
                }
                match key.code {
                    KeyCode::Char('q') | KeyCode::F(3) | KeyCode::F(10) | KeyCode::Esc => {
                        app.handle_action(Action::ViewerQuit)?
                    }
                    KeyCode::F(1) => {
                        app.handle_action(Action::ShowHelp)?;
                    }
                    KeyCode::Char('h') | KeyCode::Char('x') | KeyCode::F(4)
                        if !key.modifiers.contains(KeyModifiers::CONTROL) =>
                    {
                        app.handle_action(Action::ViewerToggleHex)?
                    }
                    KeyCode::F(5) => {
                        if let UiMode::Viewer {
                            goto_prompt,
                            viewer_menu,
                            ..
                        } = &mut app.ui_mode
                        {
                            *viewer_menu = None;
                            *goto_prompt = Some(String::new());
                        }
                    }
                    // MC viewer: open "Filter command" dialog
                    KeyCode::Char('|') => {
                        // Use existing generic InputDialog; on submit apply filter to current viewed bytes.
                        app.ui_mode = UiMode::InputDialog {
                            title: "Filter command".into(),
                            prompt: "Apply command to viewed bytes:".into(),
                            value: String::new(),
                            focus_ok: false,
                            on_submit: Box::new(|app, input| {
                                let cmd = input.trim();
                                if cmd.is_empty() {
                                    return Ok(());
                                }
                                if let Err(e) = crate::terminal::viewer_apply_filter_to_current(cmd)
                                {
                                    app.ui_mode = UiMode::DialogConfirm {
                                        title: "Error".into(),
                                        message: format!("{e}"),
                                        on_ok: Box::new(|_| Ok(())),
                                    };
                                } else if let UiMode::Viewer {
                                    offset,
                                    search,
                                    search_case_sensitive,
                                    search_backwards,
                                    search_whole_words,
                                    search_regexp,
                                    search_dialog,
                                    status_msg,
                                    ..
                                } = &mut app.ui_mode
                                {
                                    *offset = 0;
                                    *search = None;
                                    *search_case_sensitive = false;
                                    *search_backwards = false;
                                    *search_whole_words = false;
                                    *search_regexp = false;
                                    *search_dialog = None;
                                    *status_msg = None;
                                }
                                Ok(())
                            }),
                        };
                    }
                    KeyCode::F(2) | KeyCode::Char('w')
                        if !key.modifiers.contains(KeyModifiers::CONTROL) =>
                    {
                        if let UiMode::Viewer { hex, wrap, .. } = &mut app.ui_mode {
                            if !*hex {
                                *wrap = !*wrap;
                            }
                        }
                    }
                    KeyCode::F(8) => {
                        if let UiMode::Viewer {
                            path,
                            parsed,
                            offset,
                            sel_anchor,
                            sel_cursor,
                            status_msg,
                            ..
                        } = &mut app.ui_mode
                        {
                            let next = !*parsed;
                            let p = path.clone();
                            match viewer_reload_parsed(&p, next) {
                                Ok(()) => {
                                    *parsed = next;
                                    *offset = 0;
                                    *sel_cursor = 0;
                                    viewer_sel_clear(sel_anchor);
                                    *status_msg = None;
                                }
                                Err(e) => {
                                    *status_msg = Some(format!("{e}"));
                                }
                            }
                        }
                    }
                    KeyCode::F(9) => {
                        if let UiMode::Viewer { format_nroff, .. } = &mut app.ui_mode {
                            *format_nroff = !*format_nroff;
                        }
                    }
                    KeyCode::Char('l') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        // GNU mcview C-l: refresh (next draw is enough).
                    }
                    KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        app.handle_action(Action::ToggleSubshell)?;
                    }
                    KeyCode::Char('l') if key.modifiers.is_empty() => {
                        if let UiMode::Viewer {
                            show_line_numbers, ..
                        } = &mut app.ui_mode
                        {
                            *show_line_numbers = !*show_line_numbers;
                        }
                    }
                    KeyCode::Char('r') if key.modifiers.is_empty() => {
                        if let UiMode::Viewer { show_cr, .. } = &mut app.ui_mode {
                            *show_cr = !*show_cr;
                        }
                    }
                    KeyCode::Up => {
                        viewer_move_vertical(app, -1, key.modifiers.contains(KeyModifiers::SHIFT))?;
                    }
                    KeyCode::Down => {
                        viewer_move_vertical(app, 1, key.modifiers.contains(KeyModifiers::SHIFT))?;
                    }
                    KeyCode::Left => {
                        viewer_move_horizontal(
                            app,
                            -1,
                            key.modifiers.contains(KeyModifiers::SHIFT),
                        )?;
                    }
                    KeyCode::Right => {
                        viewer_move_horizontal(
                            app,
                            1,
                            key.modifiers.contains(KeyModifiers::SHIFT),
                        )?;
                    }
                    KeyCode::PageDown | KeyCode::Char(' ')
                        if !key.modifiers.contains(KeyModifiers::ALT) =>
                    {
                        if key.code == KeyCode::Char(' ')
                            && !key.modifiers.is_empty()
                            && !key.modifiers.contains(KeyModifiers::CONTROL)
                        {
                            // ignore other modified spaces
                        } else if key.code == KeyCode::Char(' ')
                            && key.modifiers.contains(KeyModifiers::CONTROL)
                        {
                            // not C-v
                        } else {
                            viewer_move_page(app, true, page_rows, false)?;
                        }
                    }
                    KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        viewer_move_page(app, true, page_rows, false)?;
                    }
                    KeyCode::PageUp | KeyCode::Backspace => {
                        viewer_move_page(app, false, page_rows, false)?;
                    }
                    KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::ALT) => {
                        viewer_move_page(app, false, page_rows, false)?;
                    }
                    KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        viewer_move_page(app, false, page_rows, false)?;
                    }
                    KeyCode::Home | KeyCode::Char('g')
                        if key.modifiers.is_empty()
                            || key.modifiers.contains(KeyModifiers::SHIFT)
                                && matches!(key.code, KeyCode::Home) =>
                    {
                        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
                        viewer_move_home(app, shift)?;
                    }
                    KeyCode::End | KeyCode::Char('G') => {
                        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
                        viewer_move_end(app, page_rows, shift)?;
                    }
                    KeyCode::Char('/') | KeyCode::F(7) => {
                        viewer_open_search_dialog(app, false);
                    }
                    KeyCode::Char('?') => {
                        viewer_open_search_dialog(app, true);
                    }
                    KeyCode::Char('n') | KeyCode::F(17) | KeyCode::F(19) => {
                        if let UiMode::Viewer {
                            path,
                            offset,
                            search,
                            search_case_sensitive,
                            search_backwards,
                            search_whole_words,
                            search_regexp,
                            status_msg,
                            ..
                        } = &mut app.ui_mode
                        {
                            if let Some(q) = search.clone() {
                                let opts = viewer_search_opts(
                                    *search_case_sensitive,
                                    *search_backwards,
                                    *search_whole_words,
                                    *search_regexp,
                                );
                                if let Some(msg) = viewer_search_next(path, offset, &q, opts)? {
                                    *status_msg = Some(msg);
                                }
                            }
                        }
                    }
                    KeyCode::Char('N') => {
                        if let UiMode::Viewer {
                            path,
                            offset,
                            search,
                            search_case_sensitive,
                            search_backwards,
                            search_whole_words,
                            search_regexp,
                            status_msg,
                            ..
                        } = &mut app.ui_mode
                        {
                            if let Some(q) = search.clone() {
                                // GNU mcview: N temporarily flips direction without
                                // persisting the opposite Backwards flag.
                                let opts = viewer_search_opts(
                                    *search_case_sensitive,
                                    !*search_backwards,
                                    *search_whole_words,
                                    *search_regexp,
                                );
                                if let Some(msg) = viewer_search_next(path, offset, &q, opts)? {
                                    *status_msg = Some(msg);
                                }
                            }
                        }
                    }
                    _ => {}
                }
                return Ok(());
            }
            _ => {}
        }

        // (C-x handling centralized below with app.pending_ctrl_x)

        // Global Alt-Enter: append filename to command line and enter ShellInput if necessary
        if matches!(key.code, KeyCode::Enter)
            && key.modifiers.contains(crossterm::event::KeyModifiers::ALT)
        {
            if let Some(ent) = app.active_panel().current_entry() {
                let name = ent.name.clone();
                app.subshell.append_filename(&name);
                app.ui_mode = UiMode::ShellInput;
            }
            return Ok(());
        }
        // If UI is Normal and Esc is pressed, exit QuickView/Info/Tree modes on panels back to Listing.
        if matches!(app.ui_mode, UiMode::Normal) && matches!(key.code, KeyCode::Esc) {
            let mut changed = false;
            for side in [
                rmc_core::actions::PaneSide::Left,
                rmc_core::actions::PaneSide::Right,
            ] {
                let p = if matches!(side, rmc_core::actions::PaneSide::Left) {
                    &mut app.left
                } else {
                    &mut app.right
                };
                if !matches!(p.mode, rmc_core::panel::PanelMode::Listing) {
                    p.mode = rmc_core::panel::PanelMode::Listing;
                    changed = true;
                }
            }
            if changed {
                return Ok(());
            }
        }
        // Key chord handling for C-x prefix (emulate MC prefixes)
        if app.pending_ctrl_x {
            app.pending_ctrl_x = false;
            // Special case: handle '!' even when SHIFT is pressed (C-x !)
            if matches!(key.code, KeyCode::Char('!')) {
                let cwd = active_cwd.clone();
                app.ui_mode = UiMode::InputDialog {
                    title: "External panelize".into(),
                    prompt: "Enter command:".into(),
                    value: String::new(),
                    focus_ok: false,
                    on_submit: Box::new(move |app, input| {
                        let cmd = input.trim().to_string();
                        if cmd.is_empty() {
                            return Ok(());
                        }
                        let output = std::process::Command::new("sh")
                            .arg("-c")
                            .arg(&cmd)
                            .current_dir(&cwd)
                            .output();
                        match output {
                            Ok(out) => {
                                let mut paths: Vec<std::path::PathBuf> = Vec::new();
                                let stdout = String::from_utf8_lossy(&out.stdout);
                                for line in stdout.lines() {
                                    let t = line.trim();
                                    if t.is_empty() {
                                        continue;
                                    }
                                    let mut p = std::path::PathBuf::from(t);
                                    if !p.is_absolute() {
                                        p = cwd.join(t);
                                    }
                                    if app.vfs.stat(&p).is_ok() {
                                        paths.push(p);
                                    }
                                }
                                if paths.is_empty() {
                                    app.ui_mode = UiMode::DialogConfirm {
                                        title: "Error".into(),
                                        message: "Command failed or produced no files".into(),
                                        on_ok: Box::new(|_| Ok(())),
                                    };
                                    return Ok(());
                                }
                                app.panelize_paths(&paths, Some(&cwd))?;
                                app.ui_mode = UiMode::Normal;
                                Ok(())
                            }
                            Err(_e) => {
                                app.ui_mode = UiMode::DialogConfirm {
                                    title: "Error".into(),
                                    message: "Command failed or produced no files".into(),
                                    on_ok: Box::new(|_| Ok(())),
                                };
                                Ok(())
                            }
                        }
                    }),
                };
                return Ok(());
            } else if key.modifiers.is_empty() {
                if let KeyCode::Char('h') = key.code {
                    // Add current dir to hotlist with label prompt
                    let cwd = active_cwd.clone();
                    app.ui_mode = UiMode::PromptInput {
                        title: "Add to hotlist: Label".into(),
                        value: cwd
                            .file_name()
                            .and_then(|s| s.to_str())
                            .unwrap_or("")
                            .to_string(),
                        on_submit: Box::new(move |app, label| {
                            if !label.trim().is_empty() {
                                app.hotlist.add_or_replace(label.trim().to_string(), cwd)?;
                                app.hotlist.save_to_default_path()?;
                            }
                            Ok(())
                        }),
                    };
                    return Ok(());
                } else if let KeyCode::Char(c) = key.code {
                    match c {
                        'd' => {
                            open_compare_dirs_dialog(app);
                            return Ok(());
                        }
                        'j' => {
                            // Open Background jobs dialog
                            app.ui_mode = UiMode::JobsDialog {
                                selected_index: 0,
                                focus: rmc_core::app::JobsDialogFocus::Cancel,
                            };
                            return Ok(());
                        }
                        'q' => {
                            // Toggle Quick view on the inactive panel
                            let p = app.inactive_panel_mut();
                            p.mode = if matches!(p.mode, rmc_core::panel::PanelMode::QuickView) {
                                rmc_core::panel::PanelMode::Listing
                            } else {
                                rmc_core::panel::PanelMode::QuickView
                            };
                            return Ok(());
                        }
                        'i' => {
                            // Toggle Info on the inactive panel
                            let p = app.inactive_panel_mut();
                            p.mode = if matches!(p.mode, rmc_core::panel::PanelMode::Info) {
                                rmc_core::panel::PanelMode::Listing
                            } else {
                                rmc_core::panel::PanelMode::Info
                            };
                            return Ok(());
                        }
                        'c' => {
                            if let Some(ent) = app.active_panel().current_entry().cloned() {
                                let m = ent.permissions & 0o7777;
                                app.ui_mode = UiMode::ChmodDialog {
                                    name: ent.name,
                                    mode: m,
                                    ur: (m & 0o400) != 0,
                                    uw: (m & 0o200) != 0,
                                    ux: (m & 0o100) != 0,
                                    gr: (m & 0o040) != 0,
                                    gw: (m & 0o020) != 0,
                                    gx: (m & 0o010) != 0,
                                    or_: (m & 0o004) != 0,
                                    ow: (m & 0o002) != 0,
                                    ox: (m & 0o001) != 0,
                                    suid: (m & 0o4000) != 0,
                                    sgid: (m & 0o2000) != 0,
                                    sticky: (m & 0o1000) != 0,
                                    recursive: false,
                                    focus_index: 0,
                                };
                            }
                            return Ok(());
                        }
                        'o' => {
                            if let Some(ent) = app.active_panel().current_entry().cloned() {
                                app.ui_mode = UiMode::ChownDialog {
                                    owner: ent.owner.unwrap_or_default(),
                                    group: ent.group.unwrap_or_default(),
                                    recursive: false,
                                    focus_index: 0,
                                };
                            } else {
                                app.ui_mode = UiMode::ChownDialog {
                                    owner: String::new(),
                                    group: String::new(),
                                    recursive: false,
                                    focus_index: 0,
                                };
                            }
                            return Ok(());
                        }
                        'l' | 's' | 'v' => {
                            if let Some(ent) = app.active_panel().current_entry().cloned() {
                                let dst_dir = app.inactive_panel_mut().cwd.clone();
                                let default_to = dst_dir.join(&ent.name).display().to_string();
                                let is_hard = c == 'l';
                                let is_symlink_abs = c == 's';
                                let (dlg_title, prompt) = if is_hard {
                                    (
                                        "Link".to_string(),
                                        "Enter the name of the hard link to:".to_string(),
                                    )
                                } else if is_symlink_abs {
                                    (
                                        "Symbolic link".to_string(),
                                        "Enter name of the symlink:".to_string(),
                                    )
                                } else {
                                    (
                                        "Relative symlink".to_string(),
                                        "Enter name of the symlink:".to_string(),
                                    )
                                };
                                app.ui_mode = UiMode::InputDialog {
                                    title: dlg_title,
                                    prompt,
                                    value: default_to.clone(),
                                    focus_ok: true,
                                    on_submit: Box::new(move |app, val| {
                                        let src = ent.path.clone();
                                        let dst = std::path::PathBuf::from(val);
                                        if is_hard {
                                            app.vfs.link_hard(&src, &dst)?;
                                        } else if is_symlink_abs {
                                            let abs_target = if src.is_absolute() {
                                                src.clone()
                                            } else {
                                                active_cwd.join(&src)
                                            };
                                            app.vfs.symlink(&abs_target, &dst)?;
                                        } else {
                                            let base = dst
                                                .parent()
                                                .unwrap_or_else(|| std::path::Path::new("."));
                                            let abs_src = if src.is_absolute() {
                                                src.clone()
                                            } else {
                                                active_cwd.join(&src)
                                            };
                                            let abs_base = if base.is_absolute() {
                                                base.to_path_buf()
                                            } else {
                                                active_cwd.join(base)
                                            };
                                            fn relpath(
                                                from: &std::path::Path,
                                                to: &std::path::Path,
                                            ) -> std::path::PathBuf
                                            {
                                                let from = from.components().collect::<Vec<_>>();
                                                let to = to.components().collect::<Vec<_>>();
                                                let mut i = 0usize;
                                                while i < from.len()
                                                    && i < to.len()
                                                    && from[i] == to[i]
                                                {
                                                    i += 1;
                                                }
                                                let mut out = std::path::PathBuf::new();
                                                for _ in i..from.len() {
                                                    out.push("..");
                                                }
                                                for comp in &to[i..] {
                                                    out.push(comp.as_os_str());
                                                }
                                                out
                                            }
                                            let rel = relpath(&abs_base, &abs_src);
                                            app.vfs.symlink(&rel, &dst)?;
                                        }
                                        Ok(())
                                    }),
                                };
                            }
                            return Ok(());
                        }
                        _ => {}
                    }
                }
            }
            // Unrecognized chord after C-x: fall through to normal handling
        } else if key
            .modifiers
            .contains(crossterm::event::KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('x'))
        {
            app.pending_ctrl_x = true;
            return Ok(());
        }
        // Panel quick search handling (only in UiMode::Normal), placed after C-x handling.
        if matches!(app.ui_mode, UiMode::Normal) {
            // Next-match helper with wrap-around
            let find_next =
                |app_ref: &App, pattern: &str, start_after: Option<usize>| -> Option<usize> {
                    if pattern.is_empty() {
                        return None;
                    }
                    let entries = &app_ref.active_panel().entries;
                    if entries.is_empty() {
                        return None;
                    }
                    let mut start = start_after
                        .map(|i| i.saturating_add(1))
                        .unwrap_or_else(|| app_ref.active_panel().cursor);
                    if start >= entries.len() {
                        start = 0;
                    }
                    let total = entries.len();
                    for pass in 0..2 {
                        let (begin, end) = if pass == 0 {
                            (start, total)
                        } else {
                            (0, start.min(total))
                        };
                        for (i, e) in entries
                            .iter()
                            .enumerate()
                            .skip(begin)
                            .take(end.saturating_sub(begin))
                        {
                            if e.name == ".." {
                                continue;
                            }
                            if rmc_core::matchutil::name_matches(pattern, &e.name) {
                                return Some(i);
                            }
                        }
                    }
                    None
                };
            if app.quick_search.is_some() {
                let mut qs = app.quick_search.take().unwrap();
                match key.code {
                    KeyCode::Esc | KeyCode::Enter => {
                        app.quick_search = None;
                        return Ok(());
                    }
                    KeyCode::Backspace => {
                        qs.pattern.pop();
                        if let Some(idx) = find_next(app, &qs.pattern, Some(usize::MAX)) {
                            app.active_panel_mut().cursor = idx;
                            app.active_panel_mut().ensure_visible(page_rows);
                        }
                        app.quick_search = Some(qs);
                        return Ok(());
                    }
                    KeyCode::Char(c) if key.modifiers.is_empty() => {
                        if !c.is_control() {
                            qs.pattern.push(c);
                            if let Some(idx) = find_next(app, &qs.pattern, Some(usize::MAX)) {
                                app.active_panel_mut().cursor = idx;
                                app.active_panel_mut().ensure_visible(page_rows);
                            }
                        }
                        app.quick_search = Some(qs);
                        return Ok(());
                    }
                    _ => {
                        // C-s / Alt-s repeats next match
                        if (matches!(key.code, KeyCode::Char('s'))
                            && (key
                                .modifiers
                                .contains(crossterm::event::KeyModifiers::CONTROL)
                                || key.modifiers.contains(crossterm::event::KeyModifiers::ALT)))
                        {
                            let cur = app.active_panel().cursor;
                            if let Some(idx) = find_next(app, &qs.pattern, Some(cur)) {
                                app.active_panel_mut().cursor = idx;
                                app.active_panel_mut().ensure_visible(page_rows);
                            }
                            app.quick_search = Some(qs);
                            return Ok(());
                        }
                        // Not a search key: exit search and fall through (do not swallow C-x etc.)
                        app.quick_search = None;
                    }
                }
            } else if matches!(key.code, KeyCode::Char('s'))
                && (key
                    .modifiers
                    .contains(crossterm::event::KeyModifiers::CONTROL)
                    || key.modifiers.contains(crossterm::event::KeyModifiers::ALT))
            {
                app.quick_search = Some(rmc_core::quicksearch::QuickSearchState::new());
                return Ok(());
            }
        }
        // Lynx-like motion (Options → Panels): Left = parent, Right = panel Enter
        // (dirs / archives / executables / mc.ext Open). Listing mode only.
        if matches!(app.ui_mode, UiMode::Normal)
            && key.modifiers.is_empty()
            && matches!(app.active_panel().mode, rmc_core::panel::PanelMode::Listing)
        {
            if let Some(action) = lynx_like_arrow_action(app.panel_opts.lynx_like, key.code) {
                match action {
                    Action::Enter => handle_panel_enter(app)?,
                    other => app.handle_action(other)?,
                }
                return Ok(());
            }
        }
        if let Some(action) = app.keymap.resolve(&key) {
            // Intercept navigation and Enter for Tree mode on the ACTIVE panel.
            if matches!(app.ui_mode, UiMode::Normal) {
                let is_tree_active = {
                    let ap = if matches!(app.active, rmc_core::actions::PaneSide::Left) {
                        &app.left
                    } else {
                        &app.right
                    };
                    matches!(ap.mode, rmc_core::panel::PanelMode::Tree)
                };
                if is_tree_active {
                    match action {
                        Action::MoveUp => {
                            let p = if matches!(app.active, rmc_core::actions::PaneSide::Left) {
                                &mut app.left
                            } else {
                                &mut app.right
                            };
                            if let Some(tree) = &mut p.tree {
                                if tree.cursor > 0 {
                                    tree.cursor -= 1;
                                    if tree.cursor < tree.scroll_top {
                                        tree.scroll_top = tree.cursor;
                                    }
                                }
                            }
                            return Ok(());
                        }
                        Action::MoveDown => {
                            let p = if matches!(app.active, rmc_core::actions::PaneSide::Left) {
                                &mut app.left
                            } else {
                                &mut app.right
                            };
                            if let Some(tree) = &mut p.tree {
                                if tree.cursor + 1 < tree.entries.len() {
                                    tree.cursor += 1;
                                    let (c, r) = crossterm::terminal::size()?;
                                    let geom2 = compute_chrome_geom(c, r, &app.layout);
                                    let panel_h =
                                        geom2.content_bottom.saturating_sub(geom2.panel_top);
                                    let content_rows = panel_h.saturating_sub(4) as usize;
                                    if tree.cursor >= tree.scroll_top + content_rows {
                                        tree.scroll_top = tree
                                            .cursor
                                            .saturating_sub(content_rows.saturating_sub(1));
                                    }
                                }
                            }
                            return Ok(());
                        }
                        Action::Home => {
                            let p = if matches!(app.active, rmc_core::actions::PaneSide::Left) {
                                &mut app.left
                            } else {
                                &mut app.right
                            };
                            if let Some(tree) = &mut p.tree {
                                tree.cursor = 0;
                                tree.scroll_top = 0;
                            }
                            return Ok(());
                        }
                        Action::End => {
                            let p = if matches!(app.active, rmc_core::actions::PaneSide::Left) {
                                &mut app.left
                            } else {
                                &mut app.right
                            };
                            if let Some(tree) = &mut p.tree {
                                if !tree.entries.is_empty() {
                                    tree.cursor = tree.entries.len() - 1;
                                    let (c, r) = crossterm::terminal::size()?;
                                    let geom2 = compute_chrome_geom(c, r, &app.layout);
                                    let panel_h =
                                        geom2.content_bottom.saturating_sub(geom2.panel_top);
                                    let content_rows = panel_h.saturating_sub(4) as usize;
                                    tree.scroll_top =
                                        tree.cursor.saturating_sub(content_rows.saturating_sub(1));
                                }
                            }
                            return Ok(());
                        }
                        Action::PageUp => {
                            let p = if matches!(app.active, rmc_core::actions::PaneSide::Left) {
                                &mut app.left
                            } else {
                                &mut app.right
                            };
                            if let Some(tree) = &mut p.tree {
                                let prev = tree.cursor;
                                tree.cursor = tree.cursor.saturating_sub(page_rows);
                                if tree.cursor < tree.scroll_top {
                                    tree.scroll_top = tree.cursor;
                                }
                                if prev != tree.cursor {
                                    // adjusted above
                                }
                                return Ok(());
                            }
                        }
                        Action::PageDown => {
                            let p = if matches!(app.active, rmc_core::actions::PaneSide::Left) {
                                &mut app.left
                            } else {
                                &mut app.right
                            };
                            if let Some(tree) = &mut p.tree {
                                let max = tree.entries.len().saturating_sub(1);
                                let prev = tree.cursor;
                                tree.cursor = (tree.cursor + page_rows).min(max);
                                if tree.cursor >= tree.scroll_top + page_rows {
                                    tree.scroll_top =
                                        tree.cursor.saturating_sub(page_rows.saturating_sub(1));
                                }
                                if prev != tree.cursor {
                                    // adjusted
                                }
                                return Ok(());
                            }
                        }
                        Action::Enter => {
                            // Enter in Tree changes directory of the other panel (keep focus)
                            let (tree_side_left, dest_opt) = {
                                let is_left =
                                    matches!(app.active, rmc_core::actions::PaneSide::Left);
                                let p = if is_left { &app.left } else { &app.right };
                                let path = p
                                    .tree
                                    .as_ref()
                                    .and_then(|t| t.entries.get(t.cursor))
                                    .map(|e| e.path.clone());
                                (is_left, path)
                            };
                            if let Some(dest) = dest_opt {
                                let original_active = app.active;
                                app.active = if tree_side_left {
                                    rmc_core::actions::PaneSide::Right
                                } else {
                                    rmc_core::actions::PaneSide::Left
                                };
                                let _ = app.change_dir(&dest);
                                app.active = original_active;
                            }
                            return Ok(());
                        }
                        _ => {}
                    }
                }
            }
            match action {
                Action::ViewFile => {
                    view_current_file(app)?;
                }
                Action::FunctionKey(4) => {
                    // Open editor on selected file (panels Normal mode)
                    if matches!(app.ui_mode, UiMode::Normal) {
                        if let Some(ent) = app.active_panel().current_entry().cloned() {
                            if !ent.is_dir {
                                if app.config_opts.use_internal_edit {
                                    // Read file bytes via VFS
                                    let mut data = Vec::new();
                                    if let Ok(mut r) = app.vfs.read_file(&ent.path) {
                                        use std::io::Read;
                                        let _ = r.read_to_end(&mut data);
                                    }
                                    let buf = rmc_edit::EditorBuffer::from_bytes(
                                        &data,
                                        Some(ent.path.clone()),
                                    );
                                    app.ui_mode = editor_ui_mode(buf, None);
                                } else {
                                    // Spawn external editor (EDITOR or VISUAL or vi)
                                    let prog = std::env::var("EDITOR")
                                        .ok()
                                        .filter(|s| !s.trim().is_empty())
                                        .or_else(|| {
                                            std::env::var("VISUAL")
                                                .ok()
                                                .filter(|s| !s.trim().is_empty())
                                        })
                                        .unwrap_or_else(|| "vi".to_string());
                                    let _ = std::process::Command::new(&prog)
                                        .arg(&ent.path)
                                        .current_dir(&app.active_panel().cwd)
                                        .status();
                                    app.reload_panels()?;
                                }
                            }
                        }
                    }
                }
                Action::PageUp => app.page_up_by(page_rows),
                Action::PageDown => app.page_down_by(page_rows),
                Action::ToggleSubshell => {
                    app.handle_action(Action::ToggleSubshell)?;
                    // If toggled ON, ensure PTY session exists and is alive; spawn otherwise.
                    if app.subshell.show_output_screen {
                        let (c, r) = crossterm::terminal::size()?;
                        if let Ok(mut guard) = SUBSHELL_PTY.lock() {
                            let need_spawn = match guard.as_mut() {
                                Some(sess) => !sess.is_alive(),
                                None => true,
                            };
                            if need_spawn {
                                if let Ok(sess) =
                                    rmc_core::subshell::PtySession::spawn(&active_cwd, r, c)
                                {
                                    *guard = Some(sess);
                                } else {
                                    // Spawn failed: keep captured-output fallback; do nothing.
                                }
                            }
                        }
                    }
                }
                Action::OpenHotlist => {
                    let st =
                        rmc_core::hotlist::HotlistDialogState::new(app.hotlist.entries.clone());
                    app.ui_mode = UiMode::HotlistDialog(st);
                }
                Action::Mkdir => {
                    let value = mkdir_dialog_initial_name(
                        app.config_opts.mkdir_autoname,
                        app.active_panel().current_entry(),
                    );
                    app.ui_mode = UiMode::MkdirDialog {
                        value,
                        focus_ok: false,
                    };
                }
                Action::Delete => {
                    if let Some(ent) = app.active_panel().current_entry().cloned() {
                        let path = ent.path.clone();
                        if app.confirm.delete {
                            app.ui_mode = UiMode::DeleteDialog {
                                name: ent.name,
                                path,
                                focus_ok: true,
                            };
                        } else {
                            let _ = app.vfs.remove(&path, true);
                            app.reload_panels()?;
                        }
                    }
                }
                Action::Chmod => {
                    if let Some(ent) = app.active_panel().current_entry().cloned() {
                        let m = ent.permissions & 0o7777;
                        app.ui_mode = UiMode::ChmodDialog {
                            name: ent.name,
                            mode: m,
                            ur: (m & 0o400) != 0,
                            uw: (m & 0o200) != 0,
                            ux: (m & 0o100) != 0,
                            gr: (m & 0o040) != 0,
                            gw: (m & 0o020) != 0,
                            gx: (m & 0o010) != 0,
                            or_: (m & 0o004) != 0,
                            ow: (m & 0o002) != 0,
                            ox: (m & 0o001) != 0,
                            suid: (m & 0o4000) != 0,
                            sgid: (m & 0o2000) != 0,
                            sticky: (m & 0o1000) != 0,
                            recursive: false,
                            focus_index: 0,
                        };
                    }
                }
                Action::Chown => {
                    if let Some(ent) = app.active_panel().current_entry().cloned() {
                        app.ui_mode = UiMode::ChownDialog {
                            owner: ent.owner.unwrap_or_default(),
                            group: ent.group.unwrap_or_default(),
                            recursive: false,
                            focus_index: 0,
                        };
                    } else {
                        app.ui_mode = UiMode::ChownDialog {
                            owner: String::new(),
                            group: String::new(),
                            recursive: false,
                            focus_index: 0,
                        };
                    }
                }
                Action::LinkHard | Action::SymlinkAbs | Action::SymlinkRel => {
                    if let Some(ent) = app.active_panel().current_entry().cloned() {
                        let dst_dir = app.inactive_panel_mut().cwd.clone();
                        let default_to = dst_dir.join(&ent.name).display().to_string();
                        let is_hard = matches!(action, Action::LinkHard);
                        let is_symlink_abs = matches!(action, Action::SymlinkAbs);
                        let (dlg_title, prompt) = if is_hard {
                            (
                                "Link".to_string(),
                                "Enter the name of the hard link to:".to_string(),
                            )
                        } else if is_symlink_abs {
                            (
                                "Symbolic link".to_string(),
                                "Enter name of the symlink:".to_string(),
                            )
                        } else {
                            (
                                "Relative symlink".to_string(),
                                "Enter name of the symlink:".to_string(),
                            )
                        };
                        app.ui_mode = UiMode::InputDialog {
                            title: dlg_title,
                            prompt,
                            value: default_to.clone(),
                            focus_ok: true,
                            on_submit: Box::new(move |app, val| {
                                let src = ent.path.clone();
                                let dst = std::path::PathBuf::from(val);
                                if is_hard {
                                    app.vfs.link_hard(&src, &dst)?;
                                } else if is_symlink_abs {
                                    let abs_target = if src.is_absolute() {
                                        src.clone()
                                    } else {
                                        active_cwd.join(&src)
                                    };
                                    app.vfs.symlink(&abs_target, &dst)?;
                                } else {
                                    let base =
                                        dst.parent().unwrap_or_else(|| std::path::Path::new("."));
                                    let abs_src = if src.is_absolute() {
                                        src.clone()
                                    } else {
                                        active_cwd.join(&src)
                                    };
                                    let abs_base = if base.is_absolute() {
                                        base.to_path_buf()
                                    } else {
                                        active_cwd.join(base)
                                    };
                                    let rel = {
                                        fn relpath(
                                            from: &std::path::Path,
                                            to: &std::path::Path,
                                        ) -> std::path::PathBuf
                                        {
                                            let from = from.components().collect::<Vec<_>>();
                                            let to = to.components().collect::<Vec<_>>();
                                            let mut i = 0usize;
                                            while i < from.len() && i < to.len() && from[i] == to[i]
                                            {
                                                i += 1;
                                            }
                                            let mut out = std::path::PathBuf::new();
                                            for _ in i..from.len() {
                                                out.push("..");
                                            }
                                            for comp in &to[i..] {
                                                out.push(comp.as_os_str());
                                            }
                                            out
                                        }
                                        relpath(&abs_base, &abs_src)
                                    };
                                    app.vfs.symlink(&rel, &dst)?;
                                }
                                Ok(())
                            }),
                        };
                    }
                }
                Action::Copy => {
                    if let Some(ent) = app.active_panel().current_entry().cloned() {
                        let dst_dir = app.inactive_panel_mut().cwd.clone();
                        let default_to = dst_dir.join(&ent.name).display().to_string();
                        app.ui_mode = UiMode::CopyDialog {
                            title: "Copy".into(),
                            src_name: ent.name.clone(),
                            src_path: ent.path.clone(),
                            mask: "*".into(),
                            to: default_to,
                            using_shell_patterns: true,
                            follow_links: false,
                            preserve_attrs: true,
                            dive_into_subdir: false,
                            stable_symlinks: false,
                            focus: rmc_core::app::CopyDialogFocus::To,
                        };
                    }
                }
                Action::Move => {
                    if let Some(ent) = app.active_panel().current_entry().cloned() {
                        let dst_dir = app.inactive_panel_mut().cwd.clone();
                        let default_to = dst_dir.join(&ent.name).display().to_string();
                        app.ui_mode = UiMode::CopyDialog {
                            title: "Move".into(),
                            src_name: ent.name.clone(),
                            src_path: ent.path.clone(),
                            mask: "*".into(),
                            to: default_to,
                            using_shell_patterns: true,
                            follow_links: false,
                            preserve_attrs: true,
                            dive_into_subdir: false,
                            stable_symlinks: false,
                            focus: rmc_core::app::CopyDialogFocus::To,
                        };
                    }
                }
                Action::Enter => {
                    handle_panel_enter(app)?;
                }
                _ => app.handle_action(action)?,
            }
        } else {
            // Only if not a mapped action, treat plain char as command-line typing.
            if let KeyCode::Char(c) = key.code {
                if key.modifiers.is_empty() {
                    app.subshell.cmdline.push(c);
                    app.subshell.clear_history_nav();
                    app.ui_mode = UiMode::ShellInput;
                    return Ok(());
                }
            }
        }
        // Dedicated handling for F2 User Menu action
        if let Some(Action::ShowUserMenu) = app.keymap.resolve(&key) {
            // Load menu and open dialog
            let cwd = app.active_panel().cwd.clone();
            match rmc_core::user_menu::load_menu(&cwd) {
                Ok(menu) => {
                    app.ui_mode = UiMode::UserMenu {
                        title: menu.title,
                        entries: menu.entries,
                        selected_index: 0,
                    };
                }
                Err(_) => {
                    // No menu found — ignore
                }
            }
            return Ok(());
        }
        Ok(())
    }
}

pub(crate) const HISTORY_DIALOG_TITLE: &str = "History";
pub(crate) const HISTORY_CLEAN_TITLE: &str = "History cleaning";
pub(crate) const HISTORY_CLEAN_MESSAGE: &str = "Do you want to clean this history?";

fn open_command_history(app: &mut App) {
    let selected_index = app.subshell.history_len().saturating_sub(1);
    app.ui_mode = UiMode::HistoryDialog {
        selected_index,
        scroll_top: 0,
        focus: HistoryDialogFocus::List,
        confirm_clean: false,
    };
}

fn history_list_rows() -> usize {
    crossterm::terminal::size()
        .ok()
        .map(|(_, rows)| rows.saturating_sub(6).clamp(4, 16) as usize)
        .unwrap_or(8)
}

fn request_history_clear(
    subshell: &mut rmc_core::subshell::Subshell,
    history_cleanup: bool,
    selected_index: &mut usize,
    scroll_top: &mut usize,
    focus: &mut HistoryDialogFocus,
    confirm_clean: &mut bool,
) {
    if subshell.history_len() == 0 {
        return;
    }
    if history_cleanup {
        *confirm_clean = true;
    } else {
        subshell.clear_history();
        *selected_index = 0;
        *scroll_top = 0;
        *focus = HistoryDialogFocus::List;
        *confirm_clean = false;
    }
}

/// Map Left/Right to ParentDir/Enter when GNU mc Lynx-like motion is enabled.
/// When the flag is off, Left/Right stay unbound (today's listing-mode behavior).
fn lynx_like_arrow_action(lynx_like: bool, code: KeyCode) -> Option<Action> {
    if !lynx_like {
        return None;
    }
    match code {
        KeyCode::Left => Some(Action::ParentDir),
        KeyCode::Right => Some(Action::Enter),
        _ => None,
    }
}

/// POSIX single-quote a path so it can be passed to `sh -lc` as the command.
fn quote_exec_path(path: &std::path::Path) -> String {
    let s = path.to_string_lossy();
    let mut out = String::from("'");
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// Panel Enter: directory / `..` / panelized and VFS archives stay in core;
/// executables run first; then mc.ext `[open]` for regular files.
fn handle_panel_enter(app: &mut App) -> Result<()> {
    if try_enter_executable(app)? {
        return Ok(());
    }
    if try_open_by_extension(app)? {
        return Ok(());
    }
    app.handle_action(Action::Enter)
}

/// GNU mc `mkdir_autoname`: F7 prefills Mkdir with the current panel entry name,
/// skipping the `..` parent marker. Flag off (default) leaves the field empty.
fn mkdir_dialog_initial_name(
    autoname: bool,
    current: Option<&rmc_core::panel::FileEntry>,
) -> String {
    match current {
        Some(ent) if autoname && !ent.is_parent_marker() => ent.name.clone(),
        _ => String::new(),
    }
}

/// Prompt shown after a waited external command when `pause_after_run` is set.
/// Public mailing-list wording for GNU mc's pause line (not copied from GPL C).
pub(crate) const PAUSE_AFTER_RUN_PROMPT: &str = "Press any key to continue...";

/// F3 / `[open] = view`: internal viewer when configured, else $PAGER / view / less.
fn view_current_file(app: &mut App) -> Result<()> {
    view_current_file_with_pager(app, None)
}

/// `pager_override` replaces `$PAGER` when `Some` (tests pass a command that exits immediately).
fn view_current_file_with_pager(app: &mut App, pager_override: Option<&str>) -> Result<()> {
    if app.config_opts.use_internal_view {
        if let Some(ent) = app.active_panel().current_entry().cloned() {
            if !ent.is_dir {
                // Archives (.tar.gz / .tgz / …): F3 enters VFS, same as Enter.
                // Single-file .gz/.bz2/.xz stay in the viewer with decoded bytes.
                if let Some(entered) = app.vfs.enter_path(&ent.path) {
                    app.change_dir(&entered)?;
                    return Ok(());
                }
                match rmc_view::ViewData::open_view(&ent.path) {
                    Ok(view) => {
                        if let Ok(mut g) = VIEWER_STATE.lock() {
                            *g = Some(ViewerState {
                                display_path: ent.path.clone(),
                                view,
                            });
                        }
                        app.handle_action(Action::ViewFile)?;
                    }
                    Err(err) => {
                        app.ui_mode = UiMode::DialogConfirm {
                            title: "Error".into(),
                            message: format!("{err}"),
                            on_ok: Box::new(|_| Ok(())),
                        };
                    }
                }
            }
        }
        return Ok(());
    }
    if let Some(ent) = app.active_panel().current_entry().cloned() {
        if !ent.is_dir {
            run_waited_external_viewer(&ent.path, &app.active_panel().cwd, pager_override);
            pause_after_waited_external(app);
            app.reload_panels()?;
        }
    }
    Ok(())
}

/// Waited `$PAGER` / `view` / `less`. Does not spawn fire-and-forget desktop open.
fn run_waited_external_viewer(
    path: &std::path::Path,
    cwd: &std::path::Path,
    pager_override: Option<&str>,
) {
    let pager = pager_override
        .map(str::to_string)
        .or_else(|| std::env::var("PAGER").ok().filter(|s| !s.trim().is_empty()));
    if let Some(pager) = pager {
        let _ = std::process::Command::new(&pager)
            .arg(path)
            .current_dir(cwd)
            .status();
        return;
    }
    let tried_view = std::process::Command::new("view")
        .arg(path)
        .current_dir(cwd)
        .status()
        .is_ok();
    if !tried_view {
        let _ = std::process::Command::new("less")
            .arg(path)
            .current_dir(cwd)
            .status();
    }
}

/// After a blocking external command returns, optionally keep the output visible
/// until the user presses a key. No-op when the flag is off, and never used for
/// fire-and-forget `.spawn()` (xdg-open).
fn pause_after_waited_external(app: &mut App) {
    if app.config_opts.pause_after_run {
        app.ui_mode = UiMode::PauseAfterRun;
    }
}

/// Enter on a regular non-executable file: consult mc.ext `[open]`.
/// Directories, VFS-enterable archives, and unknown extensions return false
/// so core `Action::Enter` can run (or no-op).
fn try_open_by_extension(app: &mut App) -> Result<bool> {
    let (path, is_dir) = match app.active_panel().current_entry() {
        Some(ent) => (ent.path.clone(), ent.is_dir),
        None => return Ok(false),
    };
    if is_dir {
        return Ok(false);
    }
    // Archives / extfs: let Action::Enter VFS-enter instead of opening.
    if app.vfs.enter_path(&path).is_some() {
        return Ok(false);
    }
    match crate::mc_ext::lookup_open(&path) {
        Some(crate::mc_ext::OpenAction::View) => {
            view_current_file(app)?;
            Ok(true)
        }
        Some(crate::mc_ext::OpenAction::XdgOpen) => {
            run_desktop_open(&path, &app.active_panel().cwd);
            Ok(true)
        }
        // Listed so lookup is non-empty for archives; VFS enter already ran above
        // when `enter_path` succeeded. Returning false lets core Enter try.
        Some(crate::mc_ext::OpenAction::Enter) | None => Ok(false),
    }
}

fn run_desktop_open(path: &std::path::Path, cwd: &std::path::Path) {
    let prog = crate::mc_ext::desktop_open_program();
    let _ = std::process::Command::new(&prog)
        .arg(path)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// Enter on a regular executable file: run it, or prompt when `confirm.execute`.
/// Returns true when the current entry was an executable that is not VFS-enterable
/// (so core `Action::Enter` / mc.ext Open must not run). Directories, archives, and
/// non-executables return false.
fn try_enter_executable(app: &mut App) -> Result<bool> {
    let (path, is_dir, is_exe) = match app.active_panel().current_entry() {
        Some(ent) => (ent.path.clone(), ent.is_dir, ent.is_exe),
        None => return Ok(false),
    };
    if is_dir || !is_exe {
        return Ok(false);
    }
    // Archives / extfs: let Action::Enter VFS-enter instead of executing.
    if app.vfs.enter_path(&path).is_some() {
        return Ok(false);
    }
    // Only run real local files (not archive-internal virtual paths).
    if !path.is_file() {
        return Ok(false);
    }
    let cmd = quote_exec_path(&path);
    if app.confirm.execute {
        let cmd_run = cmd.clone();
        app.ui_mode = UiMode::DialogConfirm {
            title: "Execute command".to_string(),
            message: format!("Do you want to execute? {cmd}"),
            on_ok: Box::new(move |app| {
                let _ = rmc_core::user_menu::run_menu_command(app, &cmd_run);
                pause_after_waited_external(app);
                Ok(())
            }),
        };
        return Ok(true);
    }
    let _ = rmc_core::user_menu::run_menu_command(app, &cmd);
    pause_after_waited_external(app);
    app.reload_panels()?;
    Ok(true)
}

/// Build a flattened directory tree starting at `start`, up to `max_entries`, depth-first.
fn build_tree_flat(
    vfs: &dyn rmc_fs::Vfs,
    start: &std::path::Path,
    depth: usize,
    max_entries: usize,
    out: &mut Vec<rmc_core::panel::TreeEntry>,
) {
    if out.len() >= max_entries {
        return;
    }
    let mut dirs: Vec<(std::path::PathBuf, usize)> = Vec::new();
    if let Ok(entries) = vfs.list_dir(start, false) {
        for e in entries {
            if e.meta.is_dir {
                dirs.push((e.path, depth + 1));
            }
        }
    }
    dirs.sort_by(|a, b| a.0.cmp(&b.0));
    for (p, d) in dirs {
        if out.len() >= max_entries {
            break;
        }
        out.push(rmc_core::panel::TreeEntry {
            path: p.clone(),
            depth: d,
        });
        build_tree_flat(vfs, &p, d, max_entries, out);
        if out.len() >= max_entries {
            break;
        }
    }
}
impl TerminalApp {
    fn ensure_hunk_visible(state: &mut rmc_core::app::DiffState) {
        if state.hunks.is_empty() {
            return;
        }
        let cur = state.current_hunk.min(state.hunks.len().saturating_sub(1));
        if let Some(h) = state.hunks.get(cur) {
            state.left_scroll = h.left_start.saturating_sub(1);
            state.right_scroll = h.right_start.saturating_sub(1);
        }
    }
}

impl TerminalApp {
    fn is_non_equal(h: &rmc_diff::Hunk) -> bool {
        !matches!(h.kind, rmc_diff::HunkKind::Equal)
    }
    fn next_diff_hunk_index(hunks: &[rmc_diff::Hunk], current: usize) -> Option<usize> {
        let mut i = current.saturating_add(1);
        while i < hunks.len() {
            if Self::is_non_equal(&hunks[i]) {
                return Some(i);
            }
            i += 1;
        }
        None
    }
    fn prev_diff_hunk_index(hunks: &[rmc_diff::Hunk], current: usize) -> Option<usize> {
        if current == 0 {
            return None;
        }
        let mut i = current.saturating_sub(1);
        loop {
            if Self::is_non_equal(&hunks[i]) {
                return Some(i);
            }
            if i == 0 {
                break;
            }
            i -= 1;
        }
        None
    }
    fn hunk_contains_query(h: &rmc_diff::Hunk, left: &[String], right: &[String], q: &str) -> bool {
        let ql = q;
        for li in h.left_start..h.left_start.saturating_add(h.left_len) {
            if let Some(s) = left.get(li) {
                if s.contains(ql) {
                    return true;
                }
            }
        }
        for ri in h.right_start..h.right_start.saturating_add(h.right_len) {
            if let Some(s) = right.get(ri) {
                if s.contains(ql) {
                    return true;
                }
            }
        }
        false
    }
    fn search_next_hunk_with(
        hunks: &[rmc_diff::Hunk],
        left: &[String],
        right: &[String],
        current: usize,
        q: &str,
    ) -> Option<usize> {
        let mut i = current.saturating_add(1);
        while i < hunks.len() {
            let h = &hunks[i];
            if Self::is_non_equal(h) && Self::hunk_contains_query(h, left, right, q) {
                return Some(i);
            }
            i += 1;
        }
        // Wrap around
        i = 0;
        while i <= current && i < hunks.len() {
            let h = &hunks[i];
            if Self::is_non_equal(h) && Self::hunk_contains_query(h, left, right, q) {
                return Some(i);
            }
            i += 1;
        }
        None
    }
}

#[cfg(test)]
mod enter_executable_tests {
    use super::*;
    use rmc_core::config::KeyMap;
    use rmc_fs::local::LocalFs;
    use std::os::unix::fs::PermissionsExt;

    fn temp_workspace() -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "rmc-enter-exe-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn make_app(cwd: &std::path::Path) -> App {
        let vfs = LocalFs::new();
        let mut app = App::new(Box::new(vfs), KeyMap::mc_defaults()).unwrap();
        app.confirm.execute = false;
        app.change_dir(cwd).unwrap();
        app
    }

    fn select_named(app: &mut App, name: &str) {
        let idx = app
            .active_panel()
            .entries
            .iter()
            .position(|e| e.name == name)
            .unwrap_or_else(|| panic!("missing {name}"));
        app.active_panel_mut().cursor = idx;
    }

    #[test]
    fn quote_exec_path_always_single_quotes() {
        assert_eq!(quote_exec_path(std::path::Path::new("/tmp/a")), "'/tmp/a'");
        assert_eq!(
            quote_exec_path(std::path::Path::new("/tmp/a b")),
            "'/tmp/a b'"
        );
        assert_eq!(
            quote_exec_path(std::path::Path::new("/tmp/a'b")),
            "'/tmp/a'\\''b'"
        );
    }

    #[test]
    fn enter_executable_runs_without_confirm() {
        let root = temp_workspace();
        let marker = root.join("ran");
        let script = root.join("runme.sh");
        std::fs::write(
            &script,
            format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
        )
        .unwrap();
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();

        let mut app = make_app(&root);
        select_named(&mut app, "runme.sh");
        assert!(app.active_panel().current_entry().unwrap().is_exe);
        assert!(try_enter_executable(&mut app).unwrap());
        assert!(marker.exists());
        assert!(matches!(app.ui_mode, UiMode::Normal));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn enter_executable_prompts_when_confirm_execute() {
        let root = temp_workspace();
        let marker = root.join("ran");
        let script = root.join("runme.sh");
        std::fs::write(
            &script,
            format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
        )
        .unwrap();
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();

        let mut app = make_app(&root);
        app.confirm.execute = true;
        select_named(&mut app, "runme.sh");
        assert!(try_enter_executable(&mut app).unwrap());
        assert!(!marker.exists());
        match &app.ui_mode {
            UiMode::DialogConfirm { title, message, .. } => {
                assert_eq!(title, "Execute command");
                assert!(message.contains("Do you want to execute?"));
                assert!(message.contains("runme.sh"));
            }
            _ => panic!("expected DialogConfirm"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn enter_non_executable_is_noop() {
        let root = temp_workspace();
        let file = root.join("notes.dat");
        std::fs::write(&file, "hi").unwrap();
        let mut app = make_app(&root);
        select_named(&mut app, "notes.dat");
        assert!(!app.active_panel().current_entry().unwrap().is_exe);
        assert!(!try_enter_executable(&mut app).unwrap());
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod enter_open_tests {
    use super::*;
    use rmc_core::config::KeyMap;
    use rmc_fs::local::LocalFs;

    fn temp_workspace() -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "rmc-enter-open-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn make_app(cwd: &std::path::Path) -> App {
        let vfs = LocalFs::new();
        let mut app = App::new(Box::new(vfs), KeyMap::mc_defaults()).unwrap();
        // App::new overlays user setup; keep F3/Open on the internal viewer so
        // tests never spawn `view`/`less` with `.status()`.
        app.config_opts.use_internal_view = true;
        app.confirm.execute = true;
        app.change_dir(cwd).unwrap();
        app
    }

    fn select_named(app: &mut App, name: &str) {
        let idx = app
            .active_panel()
            .entries
            .iter()
            .position(|e| e.name == name)
            .unwrap_or_else(|| panic!("missing {name}"));
        app.active_panel_mut().cursor = idx;
    }

    fn press_enter(app: &mut App) {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        TerminalApp::handle_key(app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), 10)
            .unwrap();
    }

    #[test]
    fn open_txt_uses_internal_viewer_without_execute_confirm() {
        let root = temp_workspace();
        let file = root.join("notes.txt");
        std::fs::write(&file, "hello").unwrap();
        let mut app = make_app(&root);
        select_named(&mut app, "notes.txt");
        assert!(!try_enter_executable(&mut app).unwrap());
        assert!(try_open_by_extension(&mut app).unwrap());
        match &app.ui_mode {
            UiMode::Viewer { path, .. } => assert_eq!(path, &file),
            _ => panic!("expected Viewer"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn open_csv_uses_internal_viewer() {
        let root = temp_workspace();
        let file = root.join("data.csv");
        std::fs::write(&file, "a,b\n1,2\n").unwrap();
        let mut app = make_app(&root);
        select_named(&mut app, "data.csv");
        press_enter(&mut app);
        match &app.ui_mode {
            UiMode::Viewer { path, .. } => assert_eq!(path, &file),
            _ => panic!("expected Viewer for mapped .csv"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn enter_key_opens_mapped_text_file() {
        let root = temp_workspace();
        let file = root.join("lib.rs");
        std::fs::write(&file, "fn main() {}").unwrap();
        let mut app = make_app(&root);
        select_named(&mut app, "lib.rs");
        press_enter(&mut app);
        match &app.ui_mode {
            UiMode::Viewer { path, .. } => assert_eq!(path, &file),
            _ => panic!("expected Viewer"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn unknown_extension_is_still_noop() {
        let root = temp_workspace();
        std::fs::write(root.join("data.bin"), b"\x00\x01").unwrap();
        let mut app = make_app(&root);
        select_named(&mut app, "data.bin");
        assert!(!try_open_by_extension(&mut app).unwrap());
        press_enter(&mut app);
        assert!(matches!(app.ui_mode, UiMode::Normal));
        assert_eq!(app.active_panel().cwd, root);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn directory_is_not_opened_by_extension() {
        let root = temp_workspace();
        let sub = root.join("src.rs");
        std::fs::create_dir(&sub).unwrap();
        let mut app = make_app(&root);
        select_named(&mut app, "src.rs");
        assert!(!try_open_by_extension(&mut app).unwrap());
        press_enter(&mut app);
        assert_eq!(app.active_panel().cwd, sub);
        assert!(matches!(app.ui_mode, UiMode::Normal));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn xdg_open_mapping_does_not_prompt_execute() {
        let root = temp_workspace();
        std::fs::write(root.join("shot.png"), b"not-a-real-png").unwrap();
        let mut app = make_app(&root);
        select_named(&mut app, "shot.png");
        assert!(try_open_by_extension(&mut app).unwrap());
        assert!(matches!(app.ui_mode, UiMode::Normal));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn executable_mapped_file_still_runs_execute_path() {
        use std::os::unix::fs::PermissionsExt;
        let root = temp_workspace();
        let script = root.join("notes.txt");
        std::fs::write(&script, "#!/bin/sh\n").unwrap();
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();
        let mut app = make_app(&root);
        select_named(&mut app, "notes.txt");
        press_enter(&mut app);
        match &app.ui_mode {
            UiMode::DialogConfirm { title, .. } => assert_eq!(title, "Execute command"),
            _ => panic!("expected execute confirm, not Open/view"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod lynx_like_motion_tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use rmc_core::config::KeyMap;
    use rmc_fs::local::LocalFs;

    fn temp_workspace() -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "rmc-lynx-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn make_app(cwd: &std::path::Path) -> App {
        let vfs = LocalFs::new();
        let mut app = App::new(Box::new(vfs), KeyMap::mc_defaults()).unwrap();
        // App::new overlays user setup; keep Open→view on the internal viewer.
        app.config_opts.use_internal_view = true;
        app.change_dir(cwd).unwrap();
        app
    }

    fn select_named(app: &mut App, name: &str) {
        let idx = app
            .active_panel()
            .entries
            .iter()
            .position(|e| e.name == name)
            .unwrap_or_else(|| panic!("missing {name}"));
        app.active_panel_mut().cursor = idx;
    }

    fn press(app: &mut App, code: KeyCode) {
        TerminalApp::handle_key(app, KeyEvent::new(code, KeyModifiers::NONE), 10).unwrap();
    }

    #[test]
    fn maps_arrows_only_when_lynx_like_is_on() {
        use rmc_core::actions::Action;
        assert_eq!(lynx_like_arrow_action(false, KeyCode::Left), None);
        assert_eq!(lynx_like_arrow_action(false, KeyCode::Right), None);
        assert_eq!(
            lynx_like_arrow_action(true, KeyCode::Left),
            Some(Action::ParentDir)
        );
        assert_eq!(
            lynx_like_arrow_action(true, KeyCode::Right),
            Some(Action::Enter)
        );
        assert_eq!(lynx_like_arrow_action(true, KeyCode::Up), None);
    }

    #[test]
    fn left_goes_to_parent_even_when_cursor_is_on_a_file() {
        let root = temp_workspace();
        let sub = root.join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("notes.txt"), "hi").unwrap();
        let mut app = make_app(&sub);
        app.panel_opts.lynx_like = true;
        select_named(&mut app, "notes.txt");
        press(&mut app, KeyCode::Left);
        assert_eq!(app.active_panel().cwd, root);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn left_at_filesystem_root_is_noop() {
        let mut app = make_app(std::path::Path::new("/"));
        app.panel_opts.lynx_like = true;
        let before = app.active_panel().cwd.clone();
        press(&mut app, KeyCode::Left);
        assert_eq!(app.active_panel().cwd, before);
    }

    #[test]
    fn right_enters_directory_and_ignores_unmapped_files() {
        let root = temp_workspace();
        let sub = root.join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(root.join("notes.dat"), "hi").unwrap();
        let mut app = make_app(&root);
        app.panel_opts.lynx_like = true;
        select_named(&mut app, "notes.dat");
        press(&mut app, KeyCode::Right);
        assert_eq!(app.active_panel().cwd, root);
        assert!(matches!(app.ui_mode, UiMode::Normal));
        select_named(&mut app, "sub");
        press(&mut app, KeyCode::Right);
        assert_eq!(app.active_panel().cwd, sub);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn right_opens_mapped_text_file_when_lynx_like() {
        let root = temp_workspace();
        let file = root.join("notes.txt");
        std::fs::write(&file, "hi").unwrap();
        let mut app = make_app(&root);
        app.panel_opts.lynx_like = true;
        select_named(&mut app, "notes.txt");
        press(&mut app, KeyCode::Right);
        assert_eq!(app.active_panel().cwd, root);
        match &app.ui_mode {
            UiMode::Viewer { path, .. } => assert_eq!(path, &file),
            _ => panic!("expected Viewer"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn arrows_do_nothing_when_flag_is_off() {
        let root = temp_workspace();
        let sub = root.join("sub");
        std::fs::create_dir(&sub).unwrap();
        let mut app = make_app(&sub);
        app.panel_opts.lynx_like = false;
        press(&mut app, KeyCode::Left);
        assert_eq!(app.active_panel().cwd, sub);
        app.change_dir(&root).unwrap();
        select_named(&mut app, "sub");
        press(&mut app, KeyCode::Right);
        assert_eq!(app.active_panel().cwd, root);
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod ftp_proxy_opts_tests {
    use super::ftp_proxy_for_vfs_opts;

    #[test]
    fn flag_off_or_empty_host_is_direct() {
        assert_eq!(ftp_proxy_for_vfs_opts(false, "proxy.example.net"), None);
        assert_eq!(ftp_proxy_for_vfs_opts(true, ""), None);
        assert_eq!(ftp_proxy_for_vfs_opts(true, "   "), None);
        assert_eq!(ftp_proxy_for_vfs_opts(false, ""), None);
    }

    #[test]
    fn flag_on_with_host_returns_trimmed_proxy() {
        assert_eq!(
            ftp_proxy_for_vfs_opts(true, "proxy.example.net"),
            Some("proxy.example.net")
        );
        assert_eq!(ftp_proxy_for_vfs_opts(true, "  gw:3128  "), Some("gw:3128"));
    }
}

#[cfg(test)]
mod pause_after_run_tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use rmc_core::config::KeyMap;
    use rmc_fs::local::LocalFs;
    use std::os::unix::fs::PermissionsExt;

    fn temp_workspace() -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "rmc-pause-run-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn make_app(cwd: &std::path::Path) -> App {
        let vfs = LocalFs::new();
        let mut app = App::new(Box::new(vfs), KeyMap::mc_defaults()).unwrap();
        app.confirm.execute = false;
        app.config_opts.use_internal_view = true;
        app.change_dir(cwd).unwrap();
        app
    }

    fn select_named(app: &mut App, name: &str) {
        let idx = app
            .active_panel()
            .entries
            .iter()
            .position(|e| e.name == name)
            .unwrap_or_else(|| panic!("missing {name}"));
        app.active_panel_mut().cursor = idx;
    }

    fn write_exe(root: &std::path::Path, name: &str, body: &str) {
        let script = root.join(name);
        std::fs::write(&script, body).unwrap();
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();
    }

    fn press(app: &mut App, code: KeyCode) {
        TerminalApp::handle_key(app, KeyEvent::new(code, KeyModifiers::NONE), 10).unwrap();
    }

    #[test]
    fn flag_false_execute_does_not_pause() {
        let root = temp_workspace();
        let marker = root.join("ran");
        write_exe(
            &root,
            "runme.sh",
            &format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
        );
        let mut app = make_app(&root);
        app.config_opts.pause_after_run = false;
        select_named(&mut app, "runme.sh");
        assert!(try_enter_executable(&mut app).unwrap());
        assert!(marker.exists());
        assert!(
            matches!(app.ui_mode, UiMode::Normal),
            "pause_after_run=false must not show a pause dialog"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn flag_true_execute_shows_pause_ui() {
        let root = temp_workspace();
        let marker = root.join("ran");
        write_exe(
            &root,
            "runme.sh",
            &format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
        );
        let mut app = make_app(&root);
        app.config_opts.pause_after_run = true;
        select_named(&mut app, "runme.sh");
        assert!(try_enter_executable(&mut app).unwrap());
        assert!(marker.exists());
        assert!(
            matches!(app.ui_mode, UiMode::PauseAfterRun),
            "waited execute must pause when the flag is on"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn flag_true_execute_confirm_ok_shows_pause_ui() {
        let root = temp_workspace();
        let marker = root.join("ran");
        write_exe(
            &root,
            "runme.sh",
            &format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
        );
        let mut app = make_app(&root);
        app.confirm.execute = true;
        app.config_opts.pause_after_run = true;
        select_named(&mut app, "runme.sh");
        assert!(try_enter_executable(&mut app).unwrap());
        assert!(!marker.exists());
        match &app.ui_mode {
            UiMode::DialogConfirm { title, .. } => assert_eq!(title, "Execute command"),
            _ => panic!("expected execute confirm before the command runs"),
        }
        press(&mut app, KeyCode::Enter);
        assert!(marker.exists());
        assert!(
            matches!(app.ui_mode, UiMode::PauseAfterRun),
            "waited execute-confirm path must pause after the command returns"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn flag_false_external_viewer_does_not_pause() {
        let root = temp_workspace();
        std::fs::write(root.join("notes.txt"), "hello").unwrap();
        let mut app = make_app(&root);
        app.config_opts.use_internal_view = false;
        app.config_opts.pause_after_run = false;
        select_named(&mut app, "notes.txt");
        view_current_file_with_pager(&mut app, Some("true")).unwrap();
        assert!(
            matches!(app.ui_mode, UiMode::Normal),
            "waited viewer must not pause when the flag is off"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn flag_true_external_viewer_shows_pause_ui() {
        let root = temp_workspace();
        std::fs::write(root.join("notes.txt"), "hello").unwrap();
        let mut app = make_app(&root);
        app.config_opts.use_internal_view = false;
        app.config_opts.pause_after_run = true;
        select_named(&mut app, "notes.txt");
        view_current_file_with_pager(&mut app, Some("true")).unwrap();
        assert!(
            matches!(app.ui_mode, UiMode::PauseAfterRun),
            "waited external viewer must pause when the flag is on"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn flag_true_internal_viewer_does_not_pause() {
        let root = temp_workspace();
        let file = root.join("notes.txt");
        std::fs::write(&file, "hello").unwrap();
        let mut app = make_app(&root);
        app.config_opts.use_internal_view = true;
        app.config_opts.pause_after_run = true;
        select_named(&mut app, "notes.txt");
        view_current_file(&mut app).unwrap();
        match &app.ui_mode {
            UiMode::Viewer { path, .. } => assert_eq!(path, &file),
            UiMode::PauseAfterRun => panic!("internal viewer must not pause"),
            _ => panic!("expected internal Viewer"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn xdg_open_spawn_does_not_pause() {
        let root = temp_workspace();
        std::fs::write(root.join("shot.png"), b"not-a-real-png").unwrap();
        let mut app = make_app(&root);
        app.config_opts.pause_after_run = true;
        select_named(&mut app, "shot.png");
        assert!(try_open_by_extension(&mut app).unwrap());
        assert!(
            matches!(app.ui_mode, UiMode::Normal),
            "fire-and-forget xdg-open must not pause"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn any_key_dismisses_pause_ui() {
        let root = temp_workspace();
        let mut app = make_app(&root);
        app.ui_mode = UiMode::PauseAfterRun;
        press(&mut app, KeyCode::Char('x'));
        assert!(matches!(app.ui_mode, UiMode::Normal));
        app.ui_mode = UiMode::PauseAfterRun;
        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.ui_mode, UiMode::Normal));
        app.ui_mode = UiMode::PauseAfterRun;
        press(&mut app, KeyCode::Esc);
        assert!(matches!(app.ui_mode, UiMode::Normal));
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod mkdir_autoname_tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use rmc_core::config::KeyMap;
    use rmc_fs::local::LocalFs;

    fn temp_workspace() -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "rmc-mkdir-autoname-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn make_app(cwd: &std::path::Path) -> App {
        let vfs = LocalFs::new();
        let mut app = App::new(Box::new(vfs), KeyMap::mc_defaults()).unwrap();
        app.change_dir(cwd).unwrap();
        app
    }

    fn select_named(app: &mut App, name: &str) {
        let idx = app
            .active_panel()
            .entries
            .iter()
            .position(|e| e.name == name)
            .unwrap_or_else(|| panic!("missing {name}"));
        app.active_panel_mut().cursor = idx;
    }

    fn press(app: &mut App, code: KeyCode) {
        TerminalApp::handle_key(app, KeyEvent::new(code, KeyModifiers::NONE), 10).unwrap();
    }

    fn mkdir_value(app: &App) -> &str {
        match &app.ui_mode {
            UiMode::MkdirDialog { value, .. } => value.as_str(),
            _ => panic!("expected MkdirDialog"),
        }
    }

    #[test]
    fn flag_false_mkdir_input_empty() {
        let root = temp_workspace();
        std::fs::write(root.join("notes.txt"), "hi").unwrap();
        std::fs::create_dir(root.join("subdir")).unwrap();
        let mut app = make_app(&root);
        assert!(!app.config_opts.mkdir_autoname);
        select_named(&mut app, "notes.txt");
        press(&mut app, KeyCode::F(7));
        assert_eq!(mkdir_value(&app), "");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn flag_true_file_prefills_name() {
        let root = temp_workspace();
        std::fs::write(root.join("notes.txt"), "hi").unwrap();
        let mut app = make_app(&root);
        app.config_opts.mkdir_autoname = true;
        select_named(&mut app, "notes.txt");
        press(&mut app, KeyCode::F(7));
        assert_eq!(mkdir_value(&app), "notes.txt");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn flag_true_dir_prefills_name() {
        let root = temp_workspace();
        std::fs::create_dir(root.join("subdir")).unwrap();
        let mut app = make_app(&root);
        app.config_opts.mkdir_autoname = true;
        select_named(&mut app, "subdir");
        press(&mut app, KeyCode::F(7));
        assert_eq!(mkdir_value(&app), "subdir");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn flag_true_parent_dotdot_empty() {
        let root = temp_workspace();
        std::fs::write(root.join("notes.txt"), "hi").unwrap();
        let mut app = make_app(&root);
        app.config_opts.mkdir_autoname = true;
        select_named(&mut app, "..");
        press(&mut app, KeyCode::F(7));
        assert_eq!(mkdir_value(&app), "");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn empty_ok_is_noop() {
        let root = temp_workspace();
        let before: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        let mut app = make_app(&root);
        press(&mut app, KeyCode::F(7));
        assert_eq!(mkdir_value(&app), "");
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.ui_mode, UiMode::Normal));
        let after: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(before, after);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn ok_creates_directory_from_input() {
        let root = temp_workspace();
        std::fs::write(root.join("notes.txt"), "hi").unwrap();
        let mut app = make_app(&root);
        app.config_opts.mkdir_autoname = true;
        select_named(&mut app, "notes.txt");
        press(&mut app, KeyCode::F(7));
        assert_eq!(mkdir_value(&app), "notes.txt");
        press(&mut app, KeyCode::Char('_'));
        press(&mut app, KeyCode::Char('d'));
        assert_eq!(mkdir_value(&app), "notes.txt_d");
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.ui_mode, UiMode::Normal));
        assert!(root.join("notes.txt_d").is_dir());
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod drop_menus_tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use rmc_core::config::KeyMap;
    use rmc_fs::local::LocalFs;

    fn temp_workspace() -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "rmc-drop-menus-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn make_app(cwd: &std::path::Path) -> App {
        let vfs = LocalFs::new();
        let mut app = App::new(Box::new(vfs), KeyMap::mc_defaults()).unwrap();
        app.change_dir(cwd).unwrap();
        app
    }

    fn press(app: &mut App, code: KeyCode) {
        TerminalApp::handle_key(app, KeyEvent::new(code, KeyModifiers::NONE), 10).unwrap();
    }

    fn assert_menu(app: &App, top: usize, sel: usize, dropped: bool) {
        match &app.ui_mode {
            UiMode::Menu {
                top_index,
                selected_index,
                dropped: d,
            } => {
                assert_eq!(*top_index, top, "top_index");
                assert_eq!(*selected_index, sel, "selected_index");
                assert_eq!(*d, dropped, "dropped");
            }
            _ => panic!("expected UiMode::Menu"),
        }
    }

    #[test]
    fn flag_false_f9_is_menu_bar_only() {
        let root = temp_workspace();
        let mut app = make_app(&root);
        app.config_opts.drop_menus = false;
        press(&mut app, KeyCode::F(9));
        assert_menu(&app, 0, 0, false);
        press(&mut app, KeyCode::Esc);
        assert!(matches!(app.ui_mode, UiMode::Normal));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn flag_true_f9_drops_left_menu_with_item_selected() {
        let root = temp_workspace();
        let mut app = make_app(&root);
        app.config_opts.drop_menus = true;
        press(&mut app, KeyCode::F(9));
        assert_menu(&app, 0, 0, true);
        press(&mut app, KeyCode::Esc);
        assert!(matches!(app.ui_mode, UiMode::Normal));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn flag_false_down_or_enter_drops_current_menu() {
        let root = temp_workspace();
        let mut app = make_app(&root);
        app.config_opts.drop_menus = false;
        press(&mut app, KeyCode::F(9));
        assert_menu(&app, 0, 0, false);
        press(&mut app, KeyCode::Down);
        assert_menu(&app, 0, 0, true);
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::F(9));
        press(&mut app, KeyCode::Right);
        assert_menu(&app, 1, 0, false);
        press(&mut app, KeyCode::Enter);
        assert_menu(&app, 1, 0, true);
        press(&mut app, KeyCode::Esc);
        assert!(matches!(app.ui_mode, UiMode::Normal));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn left_right_switch_top_menus_while_dropped() {
        let root = temp_workspace();
        let mut app = make_app(&root);
        app.config_opts.drop_menus = true;
        press(&mut app, KeyCode::F(9));
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Right);
        assert_menu(&app, 2, 0, true);
        // Command menu still includes External panelize (index 4).
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        assert_menu(&app, 2, 4, true);
        press(&mut app, KeyCode::Enter);
        match &app.ui_mode {
            UiMode::InputDialog { title, .. } => {
                assert_eq!(title, "External panelize");
            }
            _ => panic!("expected External panelize InputDialog"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod file_op_abort_keys_tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use rmc_core::app::CopyMoveOp;
    use rmc_core::config::KeyMap;
    use rmc_fs::local::LocalFs;

    fn temp_workspace() -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "rmc-fileop-abort-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn make_app(cwd: &std::path::Path) -> App {
        let vfs = LocalFs::new();
        let mut app = App::new(Box::new(vfs), KeyMap::mc_defaults()).unwrap();
        app.change_dir(cwd).unwrap();
        app
    }

    fn press(app: &mut App, code: KeyCode) {
        TerminalApp::handle_key(app, KeyEvent::new(code, KeyModifiers::NONE), 10).unwrap();
    }

    fn wait_until_running_with_progress(app: &App, job_id: rmc_core::jobs::JobId) {
        let start = Instant::now();
        loop {
            if let Some(j) = app.jobs.get(job_id) {
                if j.status == rmc_core::jobs::JobStatus::Running && j.bytes_done > 0 {
                    return;
                }
                if matches!(
                    j.status,
                    rmc_core::jobs::JobStatus::Done
                        | rmc_core::jobs::JobStatus::Failed
                        | rmc_core::jobs::JobStatus::Cancelled
                ) {
                    panic!("job finished before abort key: {:?}", j.status);
                }
            }
            if start.elapsed() > Duration::from_millis(5_000) {
                panic!("job never started running");
            }
            // Tight poll: a 4 MiB local copy can finish in a couple of ms, so a
            // 2 ms sleep here would miss the Running window.
            std::thread::yield_now();
        }
    }

    #[test]
    fn abort_keys_cancel_in_flight_started_copy() {
        let root = temp_workspace();
        let src = root.join("big.bin");
        let dst = root.join("big.dst");
        std::fs::write(&src, vec![0xABu8; 4 * 1024 * 1024]).unwrap();
        let mut app = make_app(&root);
        app.begin_file_op(CopyMoveOp::Copy, src, dst).unwrap();
        let job_id = match &app.ui_mode {
            UiMode::FileOpProgress {
                started, job_id, ..
            } => {
                assert!(*started, "Abort must work after the transfer has started");
                *job_id
            }
            _ => panic!("expected FileOpProgress"),
        };
        wait_until_running_with_progress(&app, job_id);
        press(&mut app, KeyCode::Esc);
        assert!(matches!(app.ui_mode, UiMode::Normal));

        let start = Instant::now();
        loop {
            if let Some(j) = app.jobs.get(job_id) {
                if j.status != rmc_core::jobs::JobStatus::Running
                    && j.status != rmc_core::jobs::JobStatus::Queued
                {
                    assert_eq!(j.status, rmc_core::jobs::JobStatus::Cancelled);
                    break;
                }
            }
            if start.elapsed() > Duration::from_millis(5_000) {
                panic!("Esc did not cancel in-flight copy");
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn abort_letter_and_f10_cancel_started_copy() {
        for code in [KeyCode::Char('A'), KeyCode::F(10)] {
            let root = temp_workspace();
            let src = root.join("big.bin");
            let dst = root.join("big.dst");
            std::fs::write(&src, vec![0xABu8; 4 * 1024 * 1024]).unwrap();
            let mut app = make_app(&root);
            app.begin_file_op(CopyMoveOp::Copy, src, dst).unwrap();
            match &app.ui_mode {
                UiMode::FileOpProgress { started: true, .. } => {}
                _ => panic!("expected started FileOpProgress"),
            }
            press(&mut app, code);
            assert!(
                matches!(app.ui_mode, UiMode::Normal),
                "{code:?} must Abort while started=true"
            );
            drop(app);
            let _ = std::fs::remove_dir_all(&root);
        }
    }
}

#[cfg(test)]
mod command_history_tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use rmc_core::app::{ConfirmOptions, ConfirmationsFocus, HistoryDialogFocus};
    use rmc_core::config::KeyMap;
    use rmc_fs::local::LocalFs;

    fn temp_workspace() -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "rmc-history-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn make_app(cwd: &std::path::Path) -> App {
        let vfs = LocalFs::new();
        let mut app = App::new(Box::new(vfs), KeyMap::mc_defaults()).unwrap();
        app.change_dir(cwd).unwrap();
        app.confirm.history_cleanup = true;
        app
    }

    fn press(app: &mut App, code: KeyCode) {
        TerminalApp::handle_key(app, KeyEvent::new(code, KeyModifiers::NONE), 10).unwrap();
    }

    fn press_alt(app: &mut App, c: char) {
        TerminalApp::handle_key(app, KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT), 10)
            .unwrap();
    }

    fn seed_history(app: &mut App, cmds: &[&str]) {
        let cwd = app.active_panel().cwd.clone();
        for cmd in cmds {
            app.subshell.cmdline = cmd.to_string();
            app.subshell.execute_current(&cwd).unwrap();
            app.subshell.clear_cmdline();
        }
    }

    fn open_history(app: &mut App) {
        app.ui_mode = UiMode::ShellInput;
        press_alt(app, 'h');
    }

    fn assert_history_dialog(app: &App, confirm_clean: bool) {
        match &app.ui_mode {
            UiMode::HistoryDialog {
                confirm_clean: cc, ..
            } => assert_eq!(*cc, confirm_clean, "confirm_clean"),
            _ => panic!("expected HistoryDialog"),
        }
    }

    #[test]
    fn alt_h_opens_history_enter_pastes_esc_keeps_cmdline() {
        let root = temp_workspace();
        let mut app = make_app(&root);
        seed_history(&mut app, &["echo first", "echo second"]);
        app.subshell.cmdline = "typed".to_string();
        open_history(&mut app);
        match &app.ui_mode {
            UiMode::HistoryDialog {
                selected_index,
                confirm_clean,
                focus,
                ..
            } => {
                assert!(!*confirm_clean);
                assert!(matches!(*focus, HistoryDialogFocus::List));
                assert_eq!(*selected_index, 1);
            }
            _ => panic!("expected HistoryDialog"),
        }

        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.ui_mode, UiMode::ShellInput));
        assert_eq!(app.subshell.cmdline, "echo second");

        app.subshell.cmdline = "keep-me".to_string();
        open_history(&mut app);
        press(&mut app, KeyCode::Esc);
        assert!(matches!(app.ui_mode, UiMode::ShellInput));
        assert_eq!(app.subshell.cmdline, "keep-me");

        app.subshell.cmdline = "keep-f10".to_string();
        open_history(&mut app);
        press(&mut app, KeyCode::F(10));
        assert!(matches!(app.ui_mode, UiMode::ShellInput));
        assert_eq!(app.subshell.cmdline, "keep-f10");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn alt_h_opens_empty_history_dialog() {
        let root = temp_workspace();
        let mut app = make_app(&root);
        assert!(app.subshell.history().is_empty());
        open_history(&mut app);
        match &app.ui_mode {
            UiMode::HistoryDialog {
                selected_index,
                confirm_clean,
                ..
            } => {
                assert_eq!(*selected_index, 0);
                assert!(!*confirm_clean);
            }
            _ => panic!("expected HistoryDialog even when empty"),
        }
        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.ui_mode, UiMode::HistoryDialog { .. }));
        assert!(app.subshell.cmdline.is_empty());
        press(&mut app, KeyCode::Esc);
        assert!(matches!(app.ui_mode, UiMode::ShellInput));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn alt_h_from_panels_does_not_open_history() {
        let root = temp_workspace();
        let mut app = make_app(&root);
        seed_history(&mut app, &["echo x"]);
        app.ui_mode = UiMode::Normal;
        press_alt(&mut app, 'h');
        assert!(matches!(app.ui_mode, UiMode::Normal));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn clear_with_history_cleanup_prompts_and_waits_for_yes() {
        let root = temp_workspace();
        let mut app = make_app(&root);
        app.confirm.history_cleanup = true;
        seed_history(&mut app, &["echo a", "echo b"]);
        open_history(&mut app);
        press(&mut app, KeyCode::F(8));
        assert_history_dialog(&app, true);
        assert_eq!(app.subshell.history_len(), 2);

        press(&mut app, KeyCode::Esc);
        assert_history_dialog(&app, false);
        assert_eq!(app.subshell.history_len(), 2);

        press(&mut app, KeyCode::F(8));
        assert_history_dialog(&app, true);
        press(&mut app, KeyCode::Enter);
        assert_history_dialog(&app, false);
        assert_eq!(app.subshell.history_len(), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn clear_without_history_cleanup_wipes_immediately() {
        let root = temp_workspace();
        let mut app = make_app(&root);
        app.confirm.history_cleanup = false;
        seed_history(&mut app, &["echo a", "echo b"]);
        open_history(&mut app);
        press(&mut app, KeyCode::F(8));
        assert_history_dialog(&app, false);
        assert_eq!(app.subshell.history_len(), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn confirmations_dialog_has_history_cleanup_checkbox() {
        let root = temp_workspace();
        let mut app = make_app(&root);
        assert!(ConfirmOptions::default().history_cleanup);
        app.ui_mode = UiMode::ConfirmationsDialog {
            draft: ConfirmOptions::default(),
            focus: ConfirmationsFocus::HistoryCleanup,
        };
        assert!(app.confirm.history_cleanup);
        press(&mut app, KeyCode::Char(' '));
        match &app.ui_mode {
            UiMode::ConfirmationsDialog { draft, focus } => {
                assert!(matches!(*focus, ConfirmationsFocus::HistoryCleanup));
                assert!(!draft.history_cleanup);
            }
            _ => panic!("expected ConfirmationsDialog"),
        }
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.ui_mode, UiMode::Normal));
        assert!(
            !app.confirm.history_cleanup,
            "OK must apply the History cleanup toggle"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn toggling_confirmations_history_cleanup_changes_clear_prompt() {
        let root = temp_workspace();
        let mut app = make_app(&root);
        seed_history(&mut app, &["echo a", "echo b"]);

        app.ui_mode = UiMode::ConfirmationsDialog {
            draft: app.confirm,
            focus: ConfirmationsFocus::HistoryCleanup,
        };
        // Default is on; turn it off and apply.
        assert!(app.confirm.history_cleanup);
        press(&mut app, KeyCode::Char(' '));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert!(!app.confirm.history_cleanup);

        open_history(&mut app);
        press(&mut app, KeyCode::F(8));
        assert_history_dialog(&app, false);
        assert_eq!(app.subshell.history_len(), 0);

        seed_history(&mut app, &["echo c"]);
        app.ui_mode = UiMode::ConfirmationsDialog {
            draft: app.confirm,
            focus: ConfirmationsFocus::HistoryCleanup,
        };
        press(&mut app, KeyCode::Char(' '));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert!(app.confirm.history_cleanup);

        open_history(&mut app);
        press(&mut app, KeyCode::F(8));
        assert_history_dialog(&app, true);
        assert_eq!(app.subshell.history_len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn clear_button_matches_f8() {
        let root = temp_workspace();
        let mut app = make_app(&root);
        app.confirm.history_cleanup = false;
        seed_history(&mut app, &["echo z"]);
        open_history(&mut app);
        press(&mut app, KeyCode::Tab); // Ok
        press(&mut app, KeyCode::Tab); // Cancel
        press(&mut app, KeyCode::Tab); // Clear
        match &app.ui_mode {
            UiMode::HistoryDialog { focus, .. } => {
                assert!(matches!(*focus, HistoryDialogFocus::Clear));
            }
            _ => panic!("expected HistoryDialog"),
        }
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.subshell.history_len(), 0);
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod editor_replace_tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use rmc_core::app::EditorReplaceFocus;
    use rmc_core::config::KeyMap;
    use rmc_core::find::FindDialogState;
    use rmc_edit::EditorBuffer;
    use rmc_fs::local::LocalFs;

    fn make_app() -> App {
        let vfs = LocalFs::new();
        App::new(Box::new(vfs), KeyMap::mc_defaults()).unwrap()
    }

    fn open_editor(app: &mut App, text: &[u8]) {
        app.ui_mode = UiMode::Editor {
            buf: EditorBuffer::from_bytes(text, None),
            show_menu: None,
            status_msg: None,
            search_input: None,
            save_as_dialog: None,
            search_dialog: None,
            replace_dialog: None,
            pipe_dialog: None,
            goto_dialog: None,
            pending_quit: false,
            confirm_exit: None,
            return_to: None,
        };
    }

    fn press(app: &mut App, code: KeyCode) {
        TerminalApp::handle_key(app, KeyEvent::new(code, KeyModifiers::NONE), 10).unwrap();
    }

    fn press_alt(app: &mut App, c: char) {
        TerminalApp::handle_key(app, KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT), 10)
            .unwrap();
    }

    fn type_text(app: &mut App, s: &str) {
        for c in s.chars() {
            press(app, KeyCode::Char(c));
        }
    }

    fn editor_buf(app: &App) -> &EditorBuffer {
        match &app.ui_mode {
            UiMode::Editor { buf, .. } => buf,
            _ => panic!("expected Editor"),
        }
    }

    fn editor_bytes(app: &App) -> Vec<u8> {
        editor_buf(app).to_bytes()
    }

    fn replace_dialog(app: &App) -> &EditorReplaceDialog {
        match &app.ui_mode {
            UiMode::Editor {
                replace_dialog: Some(dlg),
                ..
            } => dlg.as_ref(),
            UiMode::Editor {
                search_input: Some(_),
                ..
            } => panic!("expected Replace dialog, got Search"),
            _ => panic!("expected Editor Replace dialog"),
        }
    }

    fn tab_until(app: &mut App, want: EditorReplaceFocus) {
        for _ in 0..16 {
            if replace_dialog(app).focus == want {
                return;
            }
            press(app, KeyCode::Tab);
        }
        panic!(
            "did not reach {want:?}, stuck on {:?}",
            replace_dialog(app).focus
        );
    }

    #[test]
    fn f4_opens_replace_not_search() {
        let mut app = make_app();
        open_editor(&mut app, b"abc abc");
        press(&mut app, KeyCode::F(4));
        match &app.ui_mode {
            UiMode::Editor {
                replace_dialog: Some(dlg),
                search_input,
                save_as_dialog,
                search_dialog,
                pipe_dialog,
                goto_dialog,
                ..
            } => {
                assert!(search_input.is_none(), "F4 must not open Search");
                assert!(save_as_dialog.is_none());
                assert!(search_dialog.is_none());
                assert!(pipe_dialog.is_none());
                assert!(goto_dialog.is_none());
                assert!(dlg.search.is_empty());
                assert!(dlg.replacement.is_empty());
                assert!(!dlg.case_sensitive);
                assert!(!dlg.backwards);
                assert!(!dlg.whole_words);
                assert!(!dlg.regular_expression);
                assert!(matches!(dlg.focus, EditorReplaceFocus::Search));
            }
            _ => panic!("F4 must open the Replace dialog"),
        }
    }

    #[test]
    fn enter_replace_replaces_next_match() {
        let mut app = make_app();
        open_editor(&mut app, b"abc abc");
        press(&mut app, KeyCode::F(4));
        type_text(&mut app, "abc");
        press(&mut app, KeyCode::Tab);
        type_text(&mut app, "X");
        press(&mut app, KeyCode::Enter);
        assert_eq!(editor_bytes(&app), b"X abc");
        match &app.ui_mode {
            UiMode::Editor {
                replace_dialog: Some(_),
                status_msg,
                ..
            } => {
                assert_eq!(status_msg.as_deref(), Some("Replaced"));
            }
            _ => panic!("Replace next should keep the dialog open"),
        }
    }

    #[test]
    fn all_replaces_every_match_and_reports_count() {
        let mut app = make_app();
        // GNU Case sensitive defaults off, so "abc" also matches ABC.
        open_editor(&mut app, b"abc ABC abc");
        press(&mut app, KeyCode::F(4));
        type_text(&mut app, "abc");
        press(&mut app, KeyCode::Tab);
        type_text(&mut app, "Y");
        tab_until(&mut app, EditorReplaceFocus::All);
        press(&mut app, KeyCode::Enter);
        assert_eq!(editor_bytes(&app), b"Y Y Y");
        match &app.ui_mode {
            UiMode::Editor {
                replace_dialog,
                status_msg,
                ..
            } => {
                assert!(replace_dialog.is_none());
                let msg = status_msg.as_deref().expect("status_msg after All");
                assert!(
                    msg.contains('3'),
                    "status_msg must mention the replacement count, got {msg:?}"
                );
            }
            _ => panic!("expected Editor after All"),
        }
    }

    #[test]
    fn esc_f10_cancel_leave_buffer_unchanged() {
        let mut app = make_app();
        open_editor(&mut app, b"hello");
        press(&mut app, KeyCode::F(4));
        type_text(&mut app, "hello");
        press(&mut app, KeyCode::Tab);
        type_text(&mut app, "bye");
        press(&mut app, KeyCode::Esc);
        assert_eq!(editor_bytes(&app), b"hello");
        match &app.ui_mode {
            UiMode::Editor {
                replace_dialog,
                search_input,
                ..
            } => {
                assert!(replace_dialog.is_none());
                assert!(search_input.is_none());
            }
            _ => panic!("Esc should stay in the editor"),
        }

        press(&mut app, KeyCode::F(4));
        type_text(&mut app, "hello");
        press(&mut app, KeyCode::Tab);
        type_text(&mut app, "bye");
        press(&mut app, KeyCode::F(10));
        assert_eq!(editor_bytes(&app), b"hello");
        match &app.ui_mode {
            UiMode::Editor { replace_dialog, .. } => {
                assert!(replace_dialog.is_none());
            }
            _ => panic!("F10 should stay in the editor"),
        }

        press(&mut app, KeyCode::F(4));
        type_text(&mut app, "hello");
        press(&mut app, KeyCode::Tab);
        type_text(&mut app, "bye");
        tab_until(&mut app, EditorReplaceFocus::Cancel);
        press(&mut app, KeyCode::Enter);
        assert_eq!(editor_bytes(&app), b"hello");
        match &app.ui_mode {
            UiMode::Editor { replace_dialog, .. } => {
                assert!(replace_dialog.is_none());
            }
            _ => panic!("Cancel should stay in the editor"),
        }
    }

    #[test]
    fn empty_needle_does_not_clear_file() {
        let mut app = make_app();
        open_editor(&mut app, b"keep me");
        press(&mut app, KeyCode::F(4));
        press(&mut app, KeyCode::Enter);
        assert_eq!(editor_bytes(&app), b"keep me");
        match &app.ui_mode {
            UiMode::Editor {
                replace_dialog: Some(_),
                ..
            } => {}
            _ => panic!("empty Replace must keep the dialog open"),
        }
        // All with empty needle is also a no-op
        tab_until(&mut app, EditorReplaceFocus::All);
        press(&mut app, KeyCode::Enter);
        assert_eq!(editor_bytes(&app), b"keep me");
        match &app.ui_mode {
            UiMode::Editor {
                replace_dialog: Some(_),
                ..
            } => {}
            _ => panic!("empty All must not close by wiping; dialog stays"),
        }
    }

    #[test]
    fn f7_search_still_works() {
        let mut app = make_app();
        open_editor(&mut app, b"hello world");
        press(&mut app, KeyCode::F(7));
        match &app.ui_mode {
            UiMode::Editor {
                search_dialog: Some(_),
                search_input,
                replace_dialog,
                ..
            } => {
                assert!(search_input.is_none(), "F7 must not use the Find: overlay");
                assert!(replace_dialog.is_none(), "F7 must open Search, not Replace");
            }
            _ => panic!("F7 must open Search"),
        }
        type_text(&mut app, "world");
        press(&mut app, KeyCode::Enter);
        let buf = editor_buf(&app);
        assert_eq!(buf.last_search, b"world");
        assert_eq!((buf.row, buf.col), (0, 6));
        match &app.ui_mode {
            UiMode::Editor {
                search_dialog,
                search_input,
                replace_dialog,
                status_msg,
                ..
            } => {
                assert!(search_dialog.is_none());
                assert!(search_input.is_none());
                assert!(replace_dialog.is_none());
                assert_eq!(status_msg.as_deref(), Some("Found"));
            }
            _ => panic!("expected Editor after Search"),
        }
    }

    #[test]
    fn replace_prefills_last_search() {
        let mut app = make_app();
        open_editor(&mut app, b"foo bar foo");
        press(&mut app, KeyCode::F(7));
        type_text(&mut app, "foo");
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::F(4));
        assert_eq!(replace_dialog(&app).search, "foo");
        assert!(!replace_dialog(&app).case_sensitive);
        assert!(!replace_dialog(&app).backwards);
        assert!(!replace_dialog(&app).whole_words);
        assert!(!replace_dialog(&app).regular_expression);
        assert!(matches!(
            replace_dialog(&app).focus,
            EditorReplaceFocus::Search
        ));
    }

    #[test]
    fn tab_reaches_checkboxes_and_buttons_space_enter_toggle_space_inserts() {
        let mut app = make_app();
        open_editor(&mut app, b"abc");
        press(&mut app, KeyCode::F(4));
        type_text(&mut app, "a b");
        assert_eq!(replace_dialog(&app).search, "a b");

        press(&mut app, KeyCode::Tab);
        assert!(matches!(
            replace_dialog(&app).focus,
            EditorReplaceFocus::Replacement
        ));
        type_text(&mut app, "x y");
        assert_eq!(replace_dialog(&app).replacement, "x y");

        press(&mut app, KeyCode::Tab);
        assert!(matches!(
            replace_dialog(&app).focus,
            EditorReplaceFocus::CaseSensitive
        ));
        assert!(!replace_dialog(&app).case_sensitive);
        press(&mut app, KeyCode::Char(' '));
        assert!(replace_dialog(&app).case_sensitive);
        press(&mut app, KeyCode::Enter);
        assert!(!replace_dialog(&app).case_sensitive);

        press(&mut app, KeyCode::Tab);
        assert!(matches!(
            replace_dialog(&app).focus,
            EditorReplaceFocus::Backwards
        ));
        press(&mut app, KeyCode::Char(' '));
        assert!(replace_dialog(&app).backwards);

        press(&mut app, KeyCode::Tab);
        assert!(matches!(
            replace_dialog(&app).focus,
            EditorReplaceFocus::WholeWords
        ));
        press(&mut app, KeyCode::Enter);
        assert!(replace_dialog(&app).whole_words);

        press(&mut app, KeyCode::Tab);
        assert!(matches!(
            replace_dialog(&app).focus,
            EditorReplaceFocus::RegularExpression
        ));
        press(&mut app, KeyCode::Char(' '));
        assert!(replace_dialog(&app).regular_expression);

        press(&mut app, KeyCode::Tab);
        assert!(matches!(
            replace_dialog(&app).focus,
            EditorReplaceFocus::Replace
        ));
        press(&mut app, KeyCode::Tab);
        assert!(matches!(
            replace_dialog(&app).focus,
            EditorReplaceFocus::All
        ));
        press(&mut app, KeyCode::Tab);
        assert!(matches!(
            replace_dialog(&app).focus,
            EditorReplaceFocus::Skip
        ));
        press(&mut app, KeyCode::Tab);
        assert!(matches!(
            replace_dialog(&app).focus,
            EditorReplaceFocus::Cancel
        ));
        press(&mut app, KeyCode::Tab);
        assert!(matches!(
            replace_dialog(&app).focus,
            EditorReplaceFocus::Search
        ));
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(replace_dialog(&app).search, "a b ");
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(replace_dialog(&app).replacement, "x y ");
    }

    #[test]
    fn case_sensitive_on_off_replace_and_all() {
        let mut app = make_app();
        open_editor(&mut app, b"Abc abc");
        press(&mut app, KeyCode::F(4));
        type_text(&mut app, "abc");
        press(&mut app, KeyCode::Tab);
        type_text(&mut app, "X");
        tab_until(&mut app, EditorReplaceFocus::CaseSensitive);
        press(&mut app, KeyCode::Char(' '));
        assert!(replace_dialog(&app).case_sensitive);
        tab_until(&mut app, EditorReplaceFocus::Replace);
        press(&mut app, KeyCode::Enter);
        assert_eq!(editor_bytes(&app), b"Abc X");

        // All with Case sensitive still off would fold; reopen and All with it on.
        press(&mut app, KeyCode::Esc);
        open_editor(&mut app, b"Abc abc Abc");
        press(&mut app, KeyCode::F(4));
        type_text(&mut app, "abc");
        press(&mut app, KeyCode::Tab);
        type_text(&mut app, "Y");
        tab_until(&mut app, EditorReplaceFocus::CaseSensitive);
        press(&mut app, KeyCode::Char(' '));
        tab_until(&mut app, EditorReplaceFocus::All);
        press(&mut app, KeyCode::Enter);
        assert_eq!(editor_bytes(&app), b"Abc Y Abc");

        open_editor(&mut app, b"Abc abc");
        press(&mut app, KeyCode::F(4));
        type_text(&mut app, "abc");
        press(&mut app, KeyCode::Tab);
        type_text(&mut app, "Z");
        // Case sensitive stays off (GNU default): both match.
        tab_until(&mut app, EditorReplaceFocus::All);
        press(&mut app, KeyCode::Enter);
        assert_eq!(editor_bytes(&app), b"Z Z");
    }

    #[test]
    fn backwards_replace_hits_earlier_match() {
        let mut app = make_app();
        open_editor(&mut app, b"cat x cat");
        // Move cursor to the end so Backwards sees the second "cat" first.
        for _ in 0..9 {
            press(&mut app, KeyCode::Right);
        }
        press(&mut app, KeyCode::F(4));
        type_text(&mut app, "cat");
        press(&mut app, KeyCode::Tab);
        type_text(&mut app, "Y");
        tab_until(&mut app, EditorReplaceFocus::Backwards);
        press(&mut app, KeyCode::Char(' '));
        tab_until(&mut app, EditorReplaceFocus::Replace);
        press(&mut app, KeyCode::Enter);
        assert_eq!(editor_bytes(&app), b"cat x Y");
        press(&mut app, KeyCode::Enter);
        assert_eq!(editor_bytes(&app), b"Y x Y");
    }

    #[test]
    fn whole_words_replace_skips_category() {
        let mut app = make_app();
        open_editor(&mut app, b"category cat");
        press(&mut app, KeyCode::F(4));
        type_text(&mut app, "cat");
        press(&mut app, KeyCode::Tab);
        type_text(&mut app, "X");
        tab_until(&mut app, EditorReplaceFocus::WholeWords);
        press(&mut app, KeyCode::Char(' '));
        tab_until(&mut app, EditorReplaceFocus::Replace);
        press(&mut app, KeyCode::Enter);
        assert_eq!(editor_bytes(&app), b"category X");

        open_editor(&mut app, b"category cat cat");
        press(&mut app, KeyCode::F(4));
        type_text(&mut app, "cat");
        press(&mut app, KeyCode::Tab);
        type_text(&mut app, "X");
        tab_until(&mut app, EditorReplaceFocus::WholeWords);
        press(&mut app, KeyCode::Char(' '));
        tab_until(&mut app, EditorReplaceFocus::All);
        press(&mut app, KeyCode::Enter);
        assert_eq!(editor_bytes(&app), b"category X X");
    }

    #[test]
    fn regex_replace_and_all_and_invalid() {
        let mut app = make_app();
        open_editor(&mut app, b"aaab xx aaab");
        press(&mut app, KeyCode::F(4));
        type_text(&mut app, "a+b");
        press(&mut app, KeyCode::Tab);
        type_text(&mut app, "Z");
        tab_until(&mut app, EditorReplaceFocus::RegularExpression);
        press(&mut app, KeyCode::Char(' '));
        tab_until(&mut app, EditorReplaceFocus::Replace);
        press(&mut app, KeyCode::Enter);
        assert_eq!(editor_bytes(&app), b"Z xx aaab");
        press(&mut app, KeyCode::Enter);
        assert_eq!(editor_bytes(&app), b"Z xx Z");

        open_editor(&mut app, b"aaab xx aaab");
        press(&mut app, KeyCode::F(4));
        type_text(&mut app, "a+b");
        press(&mut app, KeyCode::Tab);
        type_text(&mut app, "Q");
        tab_until(&mut app, EditorReplaceFocus::RegularExpression);
        press(&mut app, KeyCode::Char(' '));
        tab_until(&mut app, EditorReplaceFocus::All);
        press(&mut app, KeyCode::Enter);
        assert_eq!(editor_bytes(&app), b"Q xx Q");

        open_editor(&mut app, b"keep");
        press(&mut app, KeyCode::F(4));
        type_text(&mut app, "(");
        press(&mut app, KeyCode::Tab);
        type_text(&mut app, "nope");
        tab_until(&mut app, EditorReplaceFocus::RegularExpression);
        press(&mut app, KeyCode::Char(' '));
        tab_until(&mut app, EditorReplaceFocus::Replace);
        press(&mut app, KeyCode::Enter);
        assert_eq!(editor_bytes(&app), b"keep");
        match &app.ui_mode {
            UiMode::Editor {
                replace_dialog: Some(_),
                status_msg,
                ..
            } => {
                assert_eq!(status_msg.as_deref(), Some("Not found"));
            }
            _ => panic!("invalid regex must keep the dialog open"),
        }
    }

    #[test]
    fn skip_moves_to_next_without_replacing() {
        let mut app = make_app();
        open_editor(&mut app, b"abc abc");
        press(&mut app, KeyCode::F(4));
        type_text(&mut app, "abc");
        press(&mut app, KeyCode::Tab);
        type_text(&mut app, "X");
        tab_until(&mut app, EditorReplaceFocus::Skip);
        press(&mut app, KeyCode::Enter);
        assert_eq!(editor_bytes(&app), b"abc abc");
        assert_eq!((editor_buf(&app).row, editor_buf(&app).col), (0, 0));
        match &app.ui_mode {
            UiMode::Editor {
                replace_dialog: Some(_),
                status_msg,
                ..
            } => {
                assert_eq!(status_msg.as_deref(), Some("Found"));
            }
            _ => panic!("Skip should keep the dialog open"),
        }
        press(&mut app, KeyCode::Enter);
        assert_eq!(editor_bytes(&app), b"abc abc");
        assert_eq!((editor_buf(&app).row, editor_buf(&app).col), (0, 4));
        press(&mut app, KeyCode::Enter);
        match &app.ui_mode {
            UiMode::Editor {
                replace_dialog: Some(_),
                status_msg,
                ..
            } => {
                assert_eq!(status_msg.as_deref(), Some("Not found"));
            }
            _ => panic!("Skip with no further match keeps the dialog open"),
        }
        assert_eq!(editor_bytes(&app), b"abc abc");
    }

    #[test]
    fn f7_pipe_goto_still_open_and_clear_replace() {
        let mut app = make_app();
        open_editor(&mut app, b"aaa\nbbb\nabc abc");
        press(&mut app, KeyCode::F(4));
        assert!(matches!(
            &app.ui_mode,
            UiMode::Editor {
                replace_dialog: Some(_),
                ..
            }
        ));
        press(&mut app, KeyCode::Esc);

        press(&mut app, KeyCode::F(7));
        match &app.ui_mode {
            UiMode::Editor {
                search_dialog: Some(_),
                replace_dialog,
                pipe_dialog,
                goto_dialog,
                ..
            } => {
                assert!(replace_dialog.is_none());
                assert!(pipe_dialog.is_none());
                assert!(goto_dialog.is_none());
            }
            _ => panic!("F7 must still open Search"),
        }
        press(&mut app, KeyCode::Esc);

        press(&mut app, KeyCode::Char('|'));
        match &app.ui_mode {
            UiMode::Editor {
                pipe_dialog: Some(_),
                replace_dialog,
                search_dialog,
                goto_dialog,
                ..
            } => {
                assert!(replace_dialog.is_none());
                assert!(search_dialog.is_none());
                assert!(goto_dialog.is_none());
            }
            UiMode::InputDialog { title, .. } => {
                panic!("| must open the editor Pipe dialog, not InputDialog {title:?}")
            }
            _ => panic!("| must still open the Pipe dialog"),
        }
        press(&mut app, KeyCode::Esc);

        press_alt(&mut app, 'l');
        match &app.ui_mode {
            UiMode::Editor {
                goto_dialog: Some(_),
                replace_dialog,
                search_dialog,
                pipe_dialog,
                ..
            } => {
                assert!(replace_dialog.is_none());
                assert!(search_dialog.is_none());
                assert!(pipe_dialog.is_none());
            }
            UiMode::InputDialog { title, .. } => {
                panic!("Alt-l must open the editor Goto dialog, not InputDialog {title:?}")
            }
            _ => panic!("Alt-l must still open the Goto dialog"),
        }
    }

    #[test]
    fn opening_replace_does_not_clobber_find_file_or_history() {
        let mut app = make_app();
        let cwd = app.active_panel().cwd.clone();
        app.ui_mode = UiMode::FindDialog(FindDialogState::new(cwd));
        press(&mut app, KeyCode::F(4));
        assert!(
            matches!(app.ui_mode, UiMode::FindDialog(_)),
            "F4 in Find File must not switch to Editor Replace"
        );

        app.ui_mode = UiMode::ShellInput;
        press_alt(&mut app, 'h');
        assert!(
            matches!(app.ui_mode, UiMode::HistoryDialog { .. }),
            "Alt-h should open History"
        );
        press(&mut app, KeyCode::F(4));
        assert!(
            matches!(app.ui_mode, UiMode::HistoryDialog { .. }),
            "F4 in History must not switch to Editor Replace"
        );
    }
}

#[cfg(test)]
mod editor_pipe_tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use rmc_core::app::EditorPipeFocus;
    use rmc_core::config::KeyMap;
    use rmc_edit::EditorBuffer;
    use rmc_fs::local::LocalFs;

    fn make_app() -> App {
        let vfs = LocalFs::new();
        App::new(Box::new(vfs), KeyMap::mc_defaults()).unwrap()
    }

    fn open_editor(app: &mut App, text: &[u8]) {
        app.ui_mode = UiMode::Editor {
            buf: EditorBuffer::from_bytes(text, None),
            show_menu: None,
            status_msg: None,
            search_input: None,
            save_as_dialog: None,
            search_dialog: None,
            replace_dialog: None,
            pipe_dialog: None,
            goto_dialog: None,
            pending_quit: false,
            confirm_exit: None,
            return_to: None,
        };
    }

    fn press(app: &mut App, code: KeyCode) {
        TerminalApp::handle_key(app, KeyEvent::new(code, KeyModifiers::NONE), 10).unwrap();
    }

    fn type_text(app: &mut App, s: &str) {
        for c in s.chars() {
            press(app, KeyCode::Char(c));
        }
    }

    fn editor_buf(app: &App) -> &EditorBuffer {
        match &app.ui_mode {
            UiMode::Editor { buf, .. } => buf,
            _ => panic!("expected Editor"),
        }
    }

    fn editor_bytes(app: &App) -> Vec<u8> {
        editor_buf(app).to_bytes()
    }

    fn pipe_dialog(app: &App) -> &EditorPipeDialog {
        match &app.ui_mode {
            UiMode::Editor {
                pipe_dialog: Some(dlg),
                ..
            } => dlg,
            UiMode::Editor {
                save_as_dialog: Some(_),
                ..
            } => panic!("expected Pipe dialog, got Save-as"),
            UiMode::Editor {
                search_input: Some(_),
                ..
            } => panic!("expected Pipe dialog, got Search"),
            UiMode::Editor {
                replace_dialog: Some(_),
                ..
            } => panic!("expected Pipe dialog, got Replace"),
            UiMode::InputDialog { title, .. } => {
                panic!("expected editor Pipe dialog, got InputDialog {title:?}")
            }
            _ => panic!("expected Editor Pipe dialog"),
        }
    }

    #[test]
    fn pipe_key_opens_pipe_not_save_as_search_or_replace() {
        let mut app = make_app();
        open_editor(&mut app, b"hello");
        press(&mut app, KeyCode::Char('|'));
        match &app.ui_mode {
            UiMode::Editor {
                pipe_dialog: Some(dlg),
                search_input,
                save_as_dialog,
                replace_dialog,
                ..
            } => {
                assert!(search_input.is_none(), "| must not open Search");
                assert!(save_as_dialog.is_none(), "| must not open Save-as");
                assert!(replace_dialog.is_none(), "| must not open Replace");
                assert!(dlg.command.is_empty());
                assert!(matches!(dlg.focus, EditorPipeFocus::Command));
            }
            UiMode::InputDialog { title, .. } => {
                panic!("| must open the editor Pipe dialog, not InputDialog {title:?}")
            }
            _ => panic!("| must open the Pipe dialog"),
        }
    }

    #[test]
    fn enter_tr_uppercases_whole_buffer() {
        let mut app = make_app();
        open_editor(&mut app, b"hello");
        press(&mut app, KeyCode::Char('|'));
        type_text(&mut app, "tr a-z A-Z");
        press(&mut app, KeyCode::Enter);
        assert_eq!(editor_bytes(&app), b"HELLO");
        match &app.ui_mode {
            UiMode::Editor {
                pipe_dialog,
                search_input,
                save_as_dialog,
                replace_dialog,
                ..
            } => {
                assert!(pipe_dialog.is_none());
                assert!(search_input.is_none());
                assert!(save_as_dialog.is_none());
                assert!(replace_dialog.is_none());
            }
            _ => panic!("expected Editor after Pipe"),
        }
    }

    #[test]
    fn esc_leaves_buffer_unchanged() {
        let mut app = make_app();
        open_editor(&mut app, b"hello");
        press(&mut app, KeyCode::Char('|'));
        type_text(&mut app, "tr a-z A-Z");
        press(&mut app, KeyCode::Esc);
        assert_eq!(editor_bytes(&app), b"hello");
        match &app.ui_mode {
            UiMode::Editor {
                pipe_dialog,
                search_input,
                ..
            } => {
                assert!(pipe_dialog.is_none());
                assert!(search_input.is_none());
            }
            UiMode::Normal => panic!("Esc must stay in the editor, not return to panels"),
            _ => panic!("Esc should stay in the editor"),
        }
    }

    #[test]
    fn empty_command_does_not_wipe_file() {
        let mut app = make_app();
        open_editor(&mut app, b"keep me");
        press(&mut app, KeyCode::Char('|'));
        press(&mut app, KeyCode::Enter);
        assert_eq!(editor_bytes(&app), b"keep me");
        match &app.ui_mode {
            UiMode::Editor { pipe_dialog, .. } => {
                assert!(pipe_dialog.is_none());
            }
            _ => panic!("expected Editor after empty Pipe"),
        }
    }

    #[test]
    fn failed_command_sets_status_and_leaves_buffer() {
        let mut app = make_app();
        open_editor(&mut app, b"hello");
        press(&mut app, KeyCode::Char('|'));
        type_text(&mut app, "false");
        press(&mut app, KeyCode::Enter);
        assert_eq!(editor_bytes(&app), b"hello");
        match &app.ui_mode {
            UiMode::Editor {
                pipe_dialog,
                status_msg,
                ..
            } => {
                assert!(pipe_dialog.is_none());
                let msg = status_msg.as_deref().expect("status_msg after failed pipe");
                assert!(
                    !msg.is_empty(),
                    "failed pipe must surface an error in status_msg"
                );
            }
            _ => panic!("expected Editor after failed Pipe"),
        }
    }

    #[test]
    fn f4_replace_still_opens_replace() {
        let mut app = make_app();
        open_editor(&mut app, b"abc abc");
        press(&mut app, KeyCode::F(4));
        match &app.ui_mode {
            UiMode::Editor {
                replace_dialog: Some(_),
                pipe_dialog,
                search_input,
                ..
            } => {
                assert!(pipe_dialog.is_none());
                assert!(search_input.is_none());
            }
            _ => panic!("F4 must still open the Replace dialog"),
        }
    }

    #[test]
    fn cancel_button_leaves_buffer_unchanged() {
        let mut app = make_app();
        open_editor(&mut app, b"hello");
        press(&mut app, KeyCode::Char('|'));
        type_text(&mut app, "tr a-z A-Z");
        press(&mut app, KeyCode::Tab); // Ok
        press(&mut app, KeyCode::Tab); // Cancel
        assert!(matches!(pipe_dialog(&app).focus, EditorPipeFocus::Cancel));
        press(&mut app, KeyCode::Enter);
        assert_eq!(editor_bytes(&app), b"hello");
        match &app.ui_mode {
            UiMode::Editor { pipe_dialog, .. } => {
                assert!(pipe_dialog.is_none());
            }
            _ => panic!("Cancel should stay in the editor"),
        }
    }
}

#[cfg(test)]
mod editor_goto_tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use rmc_core::app::EditorGotoFocus;
    use rmc_core::config::KeyMap;
    use rmc_edit::EditorBuffer;
    use rmc_fs::local::LocalFs;

    fn make_app() -> App {
        let vfs = LocalFs::new();
        App::new(Box::new(vfs), KeyMap::mc_defaults()).unwrap()
    }

    fn open_editor(app: &mut App, text: &[u8]) {
        app.ui_mode = UiMode::Editor {
            buf: EditorBuffer::from_bytes(text, None),
            show_menu: None,
            status_msg: None,
            search_input: None,
            save_as_dialog: None,
            search_dialog: None,
            replace_dialog: None,
            pipe_dialog: None,
            goto_dialog: None,
            pending_quit: false,
            confirm_exit: None,
            return_to: None,
        };
    }

    fn press(app: &mut App, code: KeyCode) {
        TerminalApp::handle_key(app, KeyEvent::new(code, KeyModifiers::NONE), 10).unwrap();
    }

    fn press_alt(app: &mut App, c: char) {
        TerminalApp::handle_key(app, KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT), 10)
            .unwrap();
    }

    fn type_text(app: &mut App, s: &str) {
        for c in s.chars() {
            press(app, KeyCode::Char(c));
        }
    }

    fn editor_buf(app: &App) -> &EditorBuffer {
        match &app.ui_mode {
            UiMode::Editor { buf, .. } => buf,
            _ => panic!("expected Editor"),
        }
    }

    fn goto_dialog(app: &App) -> &EditorGotoDialog {
        match &app.ui_mode {
            UiMode::Editor {
                goto_dialog: Some(dlg),
                ..
            } => dlg.as_ref(),
            UiMode::Editor {
                save_as_dialog: Some(_),
                ..
            } => panic!("expected Goto dialog, got Save-as"),
            UiMode::Editor {
                search_input: Some(_),
                ..
            } => panic!("expected Goto dialog, got Search"),
            UiMode::Editor {
                replace_dialog: Some(_),
                ..
            } => panic!("expected Goto dialog, got Replace"),
            UiMode::Editor {
                pipe_dialog: Some(_),
                ..
            } => panic!("expected Goto dialog, got Pipe"),
            UiMode::InputDialog { title, .. } => {
                panic!("expected editor Goto dialog, got InputDialog {title:?}")
            }
            _ => panic!("expected Editor Goto dialog"),
        }
    }

    fn clear_line_field(app: &mut App) {
        let len = goto_dialog(app).line.len();
        for _ in 0..len {
            press(app, KeyCode::Backspace);
        }
    }

    #[test]
    fn alt_l_opens_goto_not_save_as_search_replace_or_pipe() {
        let mut app = make_app();
        open_editor(&mut app, b"aaa\nbbb\nccc");
        press(&mut app, KeyCode::Down);
        press_alt(&mut app, 'l');
        match &app.ui_mode {
            UiMode::Editor {
                goto_dialog: Some(dlg),
                search_input,
                save_as_dialog,
                replace_dialog,
                pipe_dialog,
                ..
            } => {
                assert!(search_input.is_none(), "Alt-l must not open Search");
                assert!(save_as_dialog.is_none(), "Alt-l must not open Save-as");
                assert!(replace_dialog.is_none(), "Alt-l must not open Replace");
                assert!(pipe_dialog.is_none(), "Alt-l must not open Pipe");
                assert_eq!(dlg.line, "2");
                assert!(matches!(dlg.focus, EditorGotoFocus::Line));
            }
            UiMode::InputDialog { title, .. } => {
                panic!("Alt-l must open the editor Goto dialog, not InputDialog {title:?}")
            }
            _ => panic!("Alt-l must open the Goto dialog"),
        }
    }

    #[test]
    fn alt_shift_l_also_opens_goto() {
        let mut app = make_app();
        open_editor(&mut app, b"aaa\nbbb\nccc");
        press_alt(&mut app, 'L');
        assert_eq!(goto_dialog(&app).line, "1");
        assert!(matches!(goto_dialog(&app).focus, EditorGotoFocus::Line));
    }

    #[test]
    fn enter_moves_to_typed_line_col_zero_and_closes() {
        let mut app = make_app();
        open_editor(&mut app, b"aaa\nbbb\nccc");
        press_alt(&mut app, 'l');
        clear_line_field(&mut app);
        type_text(&mut app, "3");
        press(&mut app, KeyCode::Enter);
        let buf = editor_buf(&app);
        assert_eq!((buf.row, buf.col), (2, 0));
        match &app.ui_mode {
            UiMode::Editor {
                goto_dialog,
                search_input,
                save_as_dialog,
                replace_dialog,
                pipe_dialog,
                ..
            } => {
                assert!(goto_dialog.is_none());
                assert!(search_input.is_none());
                assert!(save_as_dialog.is_none());
                assert!(replace_dialog.is_none());
                assert!(pipe_dialog.is_none());
            }
            UiMode::Normal => panic!("Enter must stay in the editor, not return to panels"),
            _ => panic!("expected Editor after Goto"),
        }
    }

    #[test]
    fn esc_leaves_cursor_unchanged() {
        let mut app = make_app();
        open_editor(&mut app, b"aaa\nbbb\nccc");
        press(&mut app, KeyCode::Down);
        press_alt(&mut app, 'l');
        clear_line_field(&mut app);
        type_text(&mut app, "3");
        press(&mut app, KeyCode::Esc);
        let buf = editor_buf(&app);
        assert_eq!((buf.row, buf.col), (1, 0));
        match &app.ui_mode {
            UiMode::Editor {
                goto_dialog,
                search_input,
                ..
            } => {
                assert!(goto_dialog.is_none());
                assert!(search_input.is_none());
            }
            UiMode::Normal => panic!("Esc must stay in the editor, not return to panels"),
            _ => panic!("Esc should stay in the editor"),
        }
    }

    #[test]
    fn cancel_button_leaves_cursor_unchanged() {
        let mut app = make_app();
        open_editor(&mut app, b"aaa\nbbb\nccc");
        press(&mut app, KeyCode::Down);
        press_alt(&mut app, 'l');
        clear_line_field(&mut app);
        type_text(&mut app, "3");
        press(&mut app, KeyCode::Tab); // Ok
        press(&mut app, KeyCode::Tab); // Cancel
        assert!(matches!(goto_dialog(&app).focus, EditorGotoFocus::Cancel));
        press(&mut app, KeyCode::Enter);
        let buf = editor_buf(&app);
        assert_eq!((buf.row, buf.col), (1, 0));
        match &app.ui_mode {
            UiMode::Editor { goto_dialog, .. } => {
                assert!(goto_dialog.is_none());
            }
            _ => panic!("Cancel should stay in the editor"),
        }
    }

    #[test]
    fn out_of_range_clamps_to_last_line() {
        let mut app = make_app();
        open_editor(&mut app, b"aaa\nbbb\nccc");
        press_alt(&mut app, 'l');
        clear_line_field(&mut app);
        type_text(&mut app, "9999");
        press(&mut app, KeyCode::Enter);
        let buf = editor_buf(&app);
        assert_eq!((buf.row, buf.col), (2, 0));
        match &app.ui_mode {
            UiMode::Editor { goto_dialog, .. } => {
                assert!(goto_dialog.is_none());
            }
            _ => panic!("expected Editor after out-of-range Goto"),
        }
    }

    #[test]
    fn empty_field_enter_does_not_panic_stays_editor() {
        let mut app = make_app();
        open_editor(&mut app, b"aaa\nbbb\nccc");
        press_alt(&mut app, 'l');
        clear_line_field(&mut app);
        press(&mut app, KeyCode::Enter);
        let buf = editor_buf(&app);
        assert_eq!((buf.row, buf.col), (0, 0));
        match &app.ui_mode {
            UiMode::Editor { goto_dialog, .. } => {
                assert!(goto_dialog.is_none());
            }
            _ => panic!("expected Editor after empty Goto"),
        }
    }

    #[test]
    fn plain_l_inserts_l_and_does_not_open_goto() {
        let mut app = make_app();
        open_editor(&mut app, b"");
        press(&mut app, KeyCode::Char('l'));
        match &app.ui_mode {
            UiMode::Editor {
                buf,
                goto_dialog,
                search_input,
                save_as_dialog,
                replace_dialog,
                pipe_dialog,
                ..
            } => {
                assert!(goto_dialog.is_none());
                assert!(search_input.is_none());
                assert!(save_as_dialog.is_none());
                assert!(replace_dialog.is_none());
                assert!(pipe_dialog.is_none());
                assert_eq!(buf.to_bytes(), b"l");
            }
            _ => panic!("plain l must stay in the editor and insert"),
        }
    }

    #[test]
    fn f4_replace_and_pipe_still_open() {
        let mut app = make_app();
        open_editor(&mut app, b"abc abc");
        press(&mut app, KeyCode::F(4));
        match &app.ui_mode {
            UiMode::Editor {
                replace_dialog: Some(_),
                goto_dialog,
                pipe_dialog,
                search_input,
                ..
            } => {
                assert!(goto_dialog.is_none());
                assert!(pipe_dialog.is_none());
                assert!(search_input.is_none());
            }
            _ => panic!("F4 must still open the Replace dialog"),
        }
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Char('|'));
        match &app.ui_mode {
            UiMode::Editor {
                pipe_dialog: Some(_),
                goto_dialog,
                replace_dialog,
                search_input,
                ..
            } => {
                assert!(goto_dialog.is_none());
                assert!(replace_dialog.is_none());
                assert!(search_input.is_none());
            }
            _ => panic!("| must still open the Pipe dialog"),
        }
    }
}

#[cfg(test)]
mod editor_search_tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use rmc_core::app::EditorSearchFocus;
    use rmc_core::config::KeyMap;
    use rmc_core::find::FindDialogState;
    use rmc_edit::EditorBuffer;
    use rmc_fs::local::LocalFs;

    fn make_app() -> App {
        let vfs = LocalFs::new();
        App::new(Box::new(vfs), KeyMap::mc_defaults()).unwrap()
    }

    fn open_editor(app: &mut App, text: &[u8]) {
        app.ui_mode = UiMode::Editor {
            buf: EditorBuffer::from_bytes(text, None),
            show_menu: None,
            status_msg: None,
            search_input: None,
            save_as_dialog: None,
            search_dialog: None,
            replace_dialog: None,
            pipe_dialog: None,
            goto_dialog: None,
            pending_quit: false,
            confirm_exit: None,
            return_to: None,
        };
    }

    fn press(app: &mut App, code: KeyCode) {
        TerminalApp::handle_key(app, KeyEvent::new(code, KeyModifiers::NONE), 10).unwrap();
    }

    fn press_alt(app: &mut App, c: char) {
        TerminalApp::handle_key(app, KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT), 10)
            .unwrap();
    }

    fn type_text(app: &mut App, s: &str) {
        for c in s.chars() {
            press(app, KeyCode::Char(c));
        }
    }

    fn editor_buf(app: &App) -> &EditorBuffer {
        match &app.ui_mode {
            UiMode::Editor { buf, .. } => buf,
            _ => panic!("expected Editor"),
        }
    }

    fn search_dialog(app: &App) -> &EditorSearchDialog {
        match &app.ui_mode {
            UiMode::Editor {
                search_dialog: Some(dlg),
                ..
            } => dlg,
            UiMode::Editor {
                search_input: Some(_),
                ..
            } => panic!("expected Search dialog, got Find: overlay"),
            UiMode::Editor {
                replace_dialog: Some(_),
                ..
            } => panic!("expected Search dialog, got Replace"),
            UiMode::Editor {
                pipe_dialog: Some(_),
                ..
            } => panic!("expected Search dialog, got Pipe"),
            UiMode::Editor {
                goto_dialog: Some(_),
                ..
            } => panic!("expected Search dialog, got Goto"),
            UiMode::InputDialog { title, .. } => {
                panic!("expected editor Search dialog, got InputDialog {title:?}")
            }
            UiMode::PromptInput { title, .. } => {
                panic!("expected editor Search dialog, got PromptInput {title:?}")
            }
            _ => panic!("expected Editor Search dialog"),
        }
    }

    #[test]
    fn f7_opens_search_dialog_defaults_stay_editor() {
        let mut app = make_app();
        open_editor(&mut app, b"abc");
        press(&mut app, KeyCode::F(7));
        match &app.ui_mode {
            UiMode::Editor {
                search_dialog: Some(dlg),
                search_input,
                save_as_dialog,
                replace_dialog,
                pipe_dialog,
                goto_dialog,
                ..
            } => {
                assert!(search_input.is_none(), "F7 must not use the Find: overlay");
                assert!(save_as_dialog.is_none());
                assert!(replace_dialog.is_none());
                assert!(pipe_dialog.is_none());
                assert!(goto_dialog.is_none());
                assert!(dlg.search.is_empty());
                assert!(!dlg.case_sensitive);
                assert!(!dlg.backwards);
                assert!(!dlg.whole_words);
                assert!(!dlg.regular_expression);
                assert!(matches!(dlg.focus, EditorSearchFocus::Search));
            }
            UiMode::InputDialog { title, .. } => {
                panic!("F7 must stay in Editor, not InputDialog {title:?}")
            }
            _ => panic!("F7 must open the Search dialog in Editor"),
        }
    }

    #[test]
    fn f7_prefills_last_search() {
        let mut app = make_app();
        open_editor(&mut app, b"foo bar foo");
        press(&mut app, KeyCode::F(7));
        type_text(&mut app, "foo");
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::F(7));
        assert_eq!(search_dialog(&app).search, "foo");
        assert!(matches!(
            search_dialog(&app).focus,
            EditorSearchFocus::Search
        ));
        assert!(!search_dialog(&app).case_sensitive);
        assert!(!search_dialog(&app).backwards);
        assert!(!search_dialog(&app).whole_words);
        assert!(!search_dialog(&app).regular_expression);
    }

    #[test]
    fn tab_reaches_checkboxes_and_buttons_space_enter_toggle_space_inserts() {
        let mut app = make_app();
        open_editor(&mut app, b"abc");
        press(&mut app, KeyCode::F(7));
        type_text(&mut app, "a b");
        assert_eq!(search_dialog(&app).search, "a b");

        press(&mut app, KeyCode::Tab);
        assert!(matches!(
            search_dialog(&app).focus,
            EditorSearchFocus::CaseSensitive
        ));
        assert!(!search_dialog(&app).case_sensitive);
        press(&mut app, KeyCode::Char(' '));
        assert!(search_dialog(&app).case_sensitive);
        press(&mut app, KeyCode::Enter);
        assert!(!search_dialog(&app).case_sensitive);

        press(&mut app, KeyCode::Tab);
        assert!(matches!(
            search_dialog(&app).focus,
            EditorSearchFocus::Backwards
        ));
        press(&mut app, KeyCode::Char(' '));
        assert!(search_dialog(&app).backwards);

        press(&mut app, KeyCode::Tab);
        assert!(matches!(
            search_dialog(&app).focus,
            EditorSearchFocus::WholeWords
        ));
        press(&mut app, KeyCode::Enter);
        assert!(search_dialog(&app).whole_words);

        press(&mut app, KeyCode::Tab);
        assert!(matches!(
            search_dialog(&app).focus,
            EditorSearchFocus::RegularExpression
        ));
        press(&mut app, KeyCode::Char(' '));
        assert!(search_dialog(&app).regular_expression);

        press(&mut app, KeyCode::Tab);
        assert!(matches!(search_dialog(&app).focus, EditorSearchFocus::Ok));
        press(&mut app, KeyCode::Tab);
        assert!(matches!(
            search_dialog(&app).focus,
            EditorSearchFocus::Cancel
        ));
        press(&mut app, KeyCode::Tab);
        assert!(matches!(
            search_dialog(&app).focus,
            EditorSearchFocus::Search
        ));
        // Space in the search field still inserts.
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(search_dialog(&app).search, "a b ");
    }

    #[test]
    fn enter_ok_on_abc_moves_cursor_closes_status_found() {
        let mut app = make_app();
        open_editor(&mut app, b"xx abc yy");
        press(&mut app, KeyCode::F(7));
        type_text(&mut app, "abc");
        press(&mut app, KeyCode::Enter);
        let buf = editor_buf(&app);
        assert_eq!((buf.row, buf.col), (0, 3));
        assert_eq!(buf.last_search, b"abc");
        match &app.ui_mode {
            UiMode::Editor {
                search_dialog,
                search_input,
                status_msg,
                ..
            } => {
                assert!(search_dialog.is_none());
                assert!(search_input.is_none());
                assert_eq!(status_msg.as_deref(), Some("Found"));
            }
            _ => panic!("expected Editor after Search"),
        }
    }

    #[test]
    fn ok_button_also_runs_search() {
        let mut app = make_app();
        open_editor(&mut app, b"hello abc");
        press(&mut app, KeyCode::F(7));
        type_text(&mut app, "abc");
        press(&mut app, KeyCode::Tab); // Case sensitive
        press(&mut app, KeyCode::Tab); // Backwards
        press(&mut app, KeyCode::Tab); // Whole words
        press(&mut app, KeyCode::Tab); // Regular expression
        press(&mut app, KeyCode::Tab); // Ok
        assert!(matches!(search_dialog(&app).focus, EditorSearchFocus::Ok));
        press(&mut app, KeyCode::Enter);
        assert_eq!((editor_buf(&app).row, editor_buf(&app).col), (0, 6));
        match &app.ui_mode {
            UiMode::Editor {
                search_dialog,
                status_msg,
                ..
            } => {
                assert!(search_dialog.is_none());
                assert_eq!(status_msg.as_deref(), Some("Found"));
            }
            _ => panic!("expected Editor after OK"),
        }
    }

    #[test]
    fn esc_f10_cancel_leave_cursor_unmoved() {
        let mut app = make_app();
        open_editor(&mut app, b"abc");
        press(&mut app, KeyCode::Right);
        let start = (editor_buf(&app).row, editor_buf(&app).col);

        press(&mut app, KeyCode::F(7));
        type_text(&mut app, "abc");
        press(&mut app, KeyCode::Esc);
        assert_eq!((editor_buf(&app).row, editor_buf(&app).col), start);
        match &app.ui_mode {
            UiMode::Editor { search_dialog, .. } => {
                assert!(search_dialog.is_none());
            }
            UiMode::Normal => panic!("Esc must stay in the editor"),
            _ => panic!("Esc should stay in the editor"),
        }

        press(&mut app, KeyCode::F(7));
        type_text(&mut app, "abc");
        press(&mut app, KeyCode::F(10));
        assert_eq!((editor_buf(&app).row, editor_buf(&app).col), start);
        match &app.ui_mode {
            UiMode::Editor { search_dialog, .. } => {
                assert!(search_dialog.is_none());
            }
            _ => panic!("F10 should stay in the editor"),
        }

        press(&mut app, KeyCode::F(7));
        type_text(&mut app, "abc");
        press(&mut app, KeyCode::Tab); // Case sensitive
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab); // Ok
        press(&mut app, KeyCode::Tab); // Cancel
        assert!(matches!(
            search_dialog(&app).focus,
            EditorSearchFocus::Cancel
        ));
        press(&mut app, KeyCode::Enter);
        assert_eq!((editor_buf(&app).row, editor_buf(&app).col), start);
        match &app.ui_mode {
            UiMode::Editor { search_dialog, .. } => {
                assert!(search_dialog.is_none());
            }
            _ => panic!("Cancel should stay in the editor"),
        }
    }

    #[test]
    fn empty_ok_is_noop_close() {
        let mut app = make_app();
        open_editor(&mut app, b"keep me");
        press(&mut app, KeyCode::F(7));
        press(&mut app, KeyCode::Enter);
        assert_eq!(editor_buf(&app).to_bytes(), b"keep me");
        assert_eq!((editor_buf(&app).row, editor_buf(&app).col), (0, 0));
        match &app.ui_mode {
            UiMode::Editor {
                search_dialog,
                status_msg,
                ..
            } => {
                assert!(search_dialog.is_none());
                assert!(status_msg.is_none());
            }
            _ => panic!("expected Editor after empty Search"),
        }
    }

    #[test]
    fn f4_pipe_goto_still_open_and_clear_search_dialog() {
        let mut app = make_app();
        open_editor(&mut app, b"aaa\nbbb\nabc abc");
        press(&mut app, KeyCode::F(7));
        assert!(matches!(
            &app.ui_mode,
            UiMode::Editor {
                search_dialog: Some(_),
                ..
            }
        ));
        press(&mut app, KeyCode::Esc);

        press(&mut app, KeyCode::F(4));
        match &app.ui_mode {
            UiMode::Editor {
                replace_dialog: Some(_),
                search_dialog,
                pipe_dialog,
                goto_dialog,
                search_input,
                ..
            } => {
                assert!(search_dialog.is_none());
                assert!(pipe_dialog.is_none());
                assert!(goto_dialog.is_none());
                assert!(search_input.is_none());
            }
            _ => panic!("F4 must still open the Replace dialog"),
        }
        press(&mut app, KeyCode::Esc);

        press(&mut app, KeyCode::Char('|'));
        match &app.ui_mode {
            UiMode::Editor {
                pipe_dialog: Some(_),
                search_dialog,
                replace_dialog,
                goto_dialog,
                ..
            } => {
                assert!(search_dialog.is_none());
                assert!(replace_dialog.is_none());
                assert!(goto_dialog.is_none());
            }
            UiMode::InputDialog { title, .. } => {
                panic!("| must open the editor Pipe dialog, not InputDialog {title:?}")
            }
            _ => panic!("| must still open the Pipe dialog"),
        }
        press(&mut app, KeyCode::Esc);

        press_alt(&mut app, 'l');
        match &app.ui_mode {
            UiMode::Editor {
                goto_dialog: Some(_),
                search_dialog,
                replace_dialog,
                pipe_dialog,
                ..
            } => {
                assert!(search_dialog.is_none());
                assert!(replace_dialog.is_none());
                assert!(pipe_dialog.is_none());
            }
            UiMode::InputDialog { title, .. } => {
                panic!("Alt-l must open the editor Goto dialog, not InputDialog {title:?}")
            }
            _ => panic!("Alt-l must still open the Goto dialog"),
        }
    }

    #[test]
    fn opening_search_does_not_clobber_find_file_or_history() {
        let mut app = make_app();
        let cwd = app.active_panel().cwd.clone();
        app.ui_mode = UiMode::FindDialog(FindDialogState::new(cwd));
        press(&mut app, KeyCode::F(7));
        assert!(
            matches!(app.ui_mode, UiMode::FindDialog(_)),
            "F7 in Find File must not switch to Editor Search"
        );

        app.ui_mode = UiMode::ShellInput;
        press_alt(&mut app, 'h');
        assert!(
            matches!(app.ui_mode, UiMode::HistoryDialog { .. }),
            "Alt-h should open History"
        );
        press(&mut app, KeyCode::F(7));
        assert!(
            matches!(app.ui_mode, UiMode::HistoryDialog { .. }),
            "F7 in History must not switch to Editor Search"
        );
    }
}

#[cfg(test)]
mod editor_save_as_tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use rmc_core::app::{EditorSaveAsFocus, YncFocus};
    use rmc_core::config::KeyMap;
    use rmc_core::find::FindDialogState;
    use rmc_edit::EditorBuffer;
    use rmc_fs::local::LocalFs;
    use std::path::PathBuf;

    fn temp_workspace() -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "rmc-save-as-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn make_app() -> App {
        let vfs = LocalFs::new();
        App::new(Box::new(vfs), KeyMap::mc_defaults()).unwrap()
    }

    fn open_editor(app: &mut App, text: &[u8], path: Option<PathBuf>) {
        app.ui_mode = UiMode::Editor {
            buf: EditorBuffer::from_bytes(text, path),
            show_menu: None,
            status_msg: None,
            search_input: None,
            save_as_dialog: None,
            search_dialog: None,
            replace_dialog: None,
            pipe_dialog: None,
            goto_dialog: None,
            pending_quit: false,
            confirm_exit: None,
            return_to: None,
        };
    }

    fn press(app: &mut App, code: KeyCode) {
        TerminalApp::handle_key(app, KeyEvent::new(code, KeyModifiers::NONE), 10).unwrap();
    }

    fn press_mod(app: &mut App, code: KeyCode, mods: KeyModifiers) {
        TerminalApp::handle_key(app, KeyEvent::new(code, mods), 10).unwrap();
    }

    fn press_alt(app: &mut App, c: char) {
        press_mod(app, KeyCode::Char(c), KeyModifiers::ALT);
    }

    fn type_text(app: &mut App, s: &str) {
        for c in s.chars() {
            press(app, KeyCode::Char(c));
        }
    }

    fn editor_buf(app: &App) -> &EditorBuffer {
        match &app.ui_mode {
            UiMode::Editor { buf, .. } => buf,
            _ => panic!("expected Editor"),
        }
    }

    fn save_as_dialog(app: &App) -> &EditorSaveAsDialog {
        match &app.ui_mode {
            UiMode::Editor {
                save_as_dialog: Some(dlg),
                ..
            } => dlg.as_ref(),
            UiMode::Editor {
                search_dialog: Some(_),
                ..
            } => panic!("expected Save as dialog, got Search"),
            UiMode::Editor {
                replace_dialog: Some(_),
                ..
            } => panic!("expected Save as dialog, got Replace"),
            UiMode::Editor {
                pipe_dialog: Some(_),
                ..
            } => panic!("expected Save as dialog, got Pipe"),
            UiMode::Editor {
                goto_dialog: Some(_),
                ..
            } => panic!("expected Save as dialog, got Goto"),
            UiMode::InputDialog { title, .. } => {
                panic!("expected editor Save as dialog, got InputDialog {title:?}")
            }
            UiMode::PromptInput { title, .. } => {
                panic!("expected editor Save as dialog, got PromptInput {title:?}")
            }
            _ => panic!("expected Editor Save as dialog"),
        }
    }

    fn clear_filename(app: &mut App) {
        let len = save_as_dialog(app).filename.len();
        for _ in 0..len {
            press(app, KeyCode::Backspace);
        }
    }

    #[test]
    fn f12_opens_save_as_dialog_prefilled_stay_editor() {
        let root = temp_workspace();
        let src = root.join("notes.txt");
        std::fs::write(&src, "hello").unwrap();
        let mut app = make_app();
        open_editor(&mut app, b"hello", Some(src.clone()));
        press(&mut app, KeyCode::F(12));
        match &app.ui_mode {
            UiMode::Editor {
                save_as_dialog: Some(dlg),
                search_input,
                search_dialog,
                replace_dialog,
                pipe_dialog,
                goto_dialog,
                ..
            } => {
                assert!(search_input.is_none());
                assert!(search_dialog.is_none());
                assert!(replace_dialog.is_none());
                assert!(pipe_dialog.is_none());
                assert!(goto_dialog.is_none());
                assert_eq!(dlg.filename, src.to_string_lossy());
                assert!(matches!(dlg.focus, EditorSaveAsFocus::Filename));
                assert!(dlg.overwrite.is_none());
            }
            UiMode::InputDialog { title, .. } => {
                panic!("F12 must open the editor Save as dialog, not InputDialog {title:?}")
            }
            _ => panic!("F12 must open the Save as dialog"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn shift_f2_also_opens_save_as() {
        let mut app = make_app();
        open_editor(&mut app, b"x", None);
        press_mod(&mut app, KeyCode::F(2), KeyModifiers::SHIFT);
        assert!(
            matches!(
                app.ui_mode,
                UiMode::Editor {
                    save_as_dialog: Some(_),
                    ..
                }
            ),
            "Shift-F2 must open Save as"
        );
        assert!(save_as_dialog(&app).filename.is_empty());
    }

    #[test]
    fn f2_without_path_opens_save_as() {
        let mut app = make_app();
        open_editor(&mut app, b"x", None);
        press(&mut app, KeyCode::F(2));
        match &app.ui_mode {
            UiMode::Editor {
                save_as_dialog: Some(dlg),
                ..
            } => {
                assert!(dlg.filename.is_empty());
                assert!(matches!(dlg.focus, EditorSaveAsFocus::Filename));
            }
            UiMode::InputDialog { title, .. } => {
                panic!("F2 without a path must open Save as, not InputDialog {title:?}")
            }
            _ => panic!("F2 without a path must open Save as"),
        }
    }

    #[test]
    fn tab_reaches_ok_then_cancel_space_inserts_in_field() {
        let mut app = make_app();
        open_editor(&mut app, b"x", None);
        press(&mut app, KeyCode::F(12));
        type_text(&mut app, "a");
        press(&mut app, KeyCode::Char(' '));
        type_text(&mut app, "b");
        assert_eq!(save_as_dialog(&app).filename, "a b");
        assert!(matches!(
            save_as_dialog(&app).focus,
            EditorSaveAsFocus::Filename
        ));
        press(&mut app, KeyCode::Tab);
        assert!(matches!(save_as_dialog(&app).focus, EditorSaveAsFocus::Ok));
        press(&mut app, KeyCode::Tab);
        assert!(matches!(
            save_as_dialog(&app).focus,
            EditorSaveAsFocus::Cancel
        ));
        press(&mut app, KeyCode::Tab);
        assert!(matches!(
            save_as_dialog(&app).focus,
            EditorSaveAsFocus::Filename
        ));
        press(&mut app, KeyCode::Down);
        assert!(matches!(save_as_dialog(&app).focus, EditorSaveAsFocus::Ok));
        press(&mut app, KeyCode::Right);
        assert!(matches!(
            save_as_dialog(&app).focus,
            EditorSaveAsFocus::Cancel
        ));
        press(&mut app, KeyCode::Left);
        assert!(matches!(save_as_dialog(&app).focus, EditorSaveAsFocus::Ok));
        press(&mut app, KeyCode::BackTab);
        assert!(matches!(
            save_as_dialog(&app).focus,
            EditorSaveAsFocus::Filename
        ));
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(save_as_dialog(&app).filename, "a b ");
    }

    #[test]
    fn esc_f10_cancel_leave_file_unwritten() {
        let root = temp_workspace();
        let dest = root.join("out.txt");
        let mut app = make_app();
        open_editor(&mut app, b"secret", None);
        press(&mut app, KeyCode::F(12));
        type_text(&mut app, dest.to_string_lossy().as_ref());
        press(&mut app, KeyCode::Esc);
        assert!(!dest.exists());
        match &app.ui_mode {
            UiMode::Editor {
                save_as_dialog,
                buf,
                ..
            } => {
                assert!(save_as_dialog.is_none());
                assert!(buf.path.is_none());
                assert_eq!(buf.to_bytes(), b"secret");
            }
            _ => panic!("Esc should stay in the editor"),
        }

        press(&mut app, KeyCode::F(12));
        type_text(&mut app, dest.to_string_lossy().as_ref());
        press(&mut app, KeyCode::F(10));
        assert!(!dest.exists());
        match &app.ui_mode {
            UiMode::Editor { save_as_dialog, .. } => {
                assert!(save_as_dialog.is_none());
            }
            _ => panic!("F10 should stay in the editor"),
        }

        press(&mut app, KeyCode::F(12));
        type_text(&mut app, dest.to_string_lossy().as_ref());
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        assert!(matches!(
            save_as_dialog(&app).focus,
            EditorSaveAsFocus::Cancel
        ));
        press(&mut app, KeyCode::Enter);
        assert!(!dest.exists());
        match &app.ui_mode {
            UiMode::Editor {
                save_as_dialog,
                buf,
                ..
            } => {
                assert!(save_as_dialog.is_none());
                assert!(buf.path.is_none());
            }
            _ => panic!("Cancel should stay in the editor"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn empty_ok_is_noop_close() {
        let mut app = make_app();
        open_editor(&mut app, b"keep me", None);
        press(&mut app, KeyCode::F(12));
        press(&mut app, KeyCode::Enter);
        match &app.ui_mode {
            UiMode::Editor {
                save_as_dialog,
                buf,
                status_msg,
                ..
            } => {
                assert!(save_as_dialog.is_none());
                assert!(buf.path.is_none());
                assert_eq!(buf.to_bytes(), b"keep me");
                assert!(status_msg.is_none());
            }
            _ => panic!("expected Editor after empty Save as"),
        }
    }

    #[test]
    fn ok_writes_new_path_and_f2_saves_current_without_dialog() {
        let root = temp_workspace();
        let src = root.join("src.txt");
        let dest = root.join("dest.txt");
        std::fs::write(&src, "orig").unwrap();
        let mut app = make_app();
        open_editor(&mut app, b"hello", Some(src.clone()));
        press(&mut app, KeyCode::F(12));
        clear_filename(&mut app);
        type_text(&mut app, dest.to_string_lossy().as_ref());
        press(&mut app, KeyCode::Enter);
        assert_eq!(std::fs::read(&dest).unwrap(), b"hello");
        match &app.ui_mode {
            UiMode::Editor {
                save_as_dialog,
                buf,
                status_msg,
                ..
            } => {
                assert!(save_as_dialog.is_none());
                assert_eq!(buf.path.as_deref(), Some(dest.as_path()));
                assert!(!buf.dirty);
                assert_eq!(status_msg.as_deref(), Some("Saved"));
            }
            _ => panic!("expected Editor after Save as OK"),
        }

        // F2 writes the current (new) path without opening Save as.
        match &mut app.ui_mode {
            UiMode::Editor { buf, .. } => buf.insert_char('!'),
            _ => panic!("expected Editor"),
        }
        let after_edit = editor_buf(&app).to_bytes();
        press(&mut app, KeyCode::F(2));
        match &app.ui_mode {
            UiMode::Editor {
                save_as_dialog,
                buf,
                status_msg,
                ..
            } => {
                assert!(save_as_dialog.is_none(), "F2 must not open Save as");
                assert_eq!(buf.path.as_deref(), Some(dest.as_path()));
                assert!(!buf.dirty);
                assert_eq!(status_msg.as_deref(), Some("Saved"));
            }
            UiMode::InputDialog { title, .. } => {
                panic!("F2 must save in place, not InputDialog {title:?}")
            }
            _ => panic!("F2 must stay in the editor"),
        }
        assert_eq!(std::fs::read(&dest).unwrap(), after_edit);
        assert_eq!(std::fs::read(&src).unwrap(), b"orig");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn overwrite_confirm_when_dest_exists() {
        let root = temp_workspace();
        let src = root.join("src.txt");
        let dest = root.join("dest.txt");
        std::fs::write(&src, "from-src").unwrap();
        std::fs::write(&dest, "keep-me").unwrap();
        let mut app = make_app();
        assert!(app.confirm.overwrite);
        open_editor(&mut app, b"new-body", Some(src.clone()));
        press(&mut app, KeyCode::F(12));
        clear_filename(&mut app);
        type_text(&mut app, dest.to_string_lossy().as_ref());
        press(&mut app, KeyCode::Enter);
        match &app.ui_mode {
            UiMode::Editor {
                save_as_dialog: Some(dlg),
                buf,
                ..
            } => {
                assert!(dlg.overwrite.is_some(), "existing dest must confirm");
                assert_eq!(buf.path.as_deref(), Some(src.as_path()));
            }
            _ => panic!("Save as must stay open with overwrite confirm"),
        }
        assert_eq!(std::fs::read(&dest).unwrap(), b"keep-me");

        press(&mut app, KeyCode::Tab); // Yes -> No
        assert!(matches!(
            save_as_dialog(&app).overwrite.as_ref().map(|c| c.focus),
            Some(YncFocus::No)
        ));
        press(&mut app, KeyCode::Enter);
        assert!(save_as_dialog(&app).overwrite.is_none());
        assert_eq!(std::fs::read(&dest).unwrap(), b"keep-me");
        assert_eq!(editor_buf(&app).path.as_deref(), Some(src.as_path()));

        press(&mut app, KeyCode::Enter);
        assert!(save_as_dialog(&app).overwrite.is_some());
        press(&mut app, KeyCode::Enter); // Yes
        match &app.ui_mode {
            UiMode::Editor {
                save_as_dialog,
                buf,
                status_msg,
                ..
            } => {
                assert!(save_as_dialog.is_none());
                assert_eq!(buf.path.as_deref(), Some(dest.as_path()));
                assert_eq!(status_msg.as_deref(), Some("Saved"));
            }
            _ => panic!("Yes should write and close Save as"),
        }
        assert_eq!(std::fs::read(&dest).unwrap(), b"new-body");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn save_as_while_open_does_not_nest() {
        let mut app = make_app();
        open_editor(&mut app, b"x", None);
        press(&mut app, KeyCode::F(12));
        type_text(&mut app, "keep");
        press(&mut app, KeyCode::F(12));
        press_mod(&mut app, KeyCode::F(2), KeyModifiers::SHIFT);
        assert_eq!(save_as_dialog(&app).filename, "keep");
        assert!(matches!(
            save_as_dialog(&app).focus,
            EditorSaveAsFocus::Filename
        ));
    }

    #[test]
    fn f7_f4_pipe_goto_still_open_and_clear_save_as() {
        let mut app = make_app();
        open_editor(&mut app, b"abc abc", None);
        press(&mut app, KeyCode::F(12));
        assert!(matches!(
            app.ui_mode,
            UiMode::Editor {
                save_as_dialog: Some(_),
                ..
            }
        ));
        press(&mut app, KeyCode::Esc);

        press(&mut app, KeyCode::F(7));
        match &app.ui_mode {
            UiMode::Editor {
                search_dialog: Some(_),
                save_as_dialog,
                replace_dialog,
                pipe_dialog,
                goto_dialog,
                ..
            } => {
                assert!(save_as_dialog.is_none());
                assert!(replace_dialog.is_none());
                assert!(pipe_dialog.is_none());
                assert!(goto_dialog.is_none());
            }
            UiMode::InputDialog { title, .. } => {
                panic!("F7 must open Search, not InputDialog {title:?}")
            }
            _ => panic!("F7 must still open the Search dialog"),
        }
        press(&mut app, KeyCode::Esc);

        press(&mut app, KeyCode::F(4));
        match &app.ui_mode {
            UiMode::Editor {
                replace_dialog: Some(_),
                save_as_dialog,
                ..
            } => {
                assert!(save_as_dialog.is_none());
            }
            _ => panic!("F4 must still open the Replace dialog"),
        }
        press(&mut app, KeyCode::Esc);

        press(&mut app, KeyCode::Char('|'));
        match &app.ui_mode {
            UiMode::Editor {
                pipe_dialog: Some(_),
                save_as_dialog,
                ..
            } => {
                assert!(save_as_dialog.is_none());
            }
            _ => panic!("| must still open the Pipe dialog"),
        }
        press(&mut app, KeyCode::Esc);

        press_alt(&mut app, 'l');
        match &app.ui_mode {
            UiMode::Editor {
                goto_dialog: Some(_),
                save_as_dialog,
                ..
            } => {
                assert!(save_as_dialog.is_none());
            }
            _ => panic!("Alt-l must still open the Goto dialog"),
        }
    }

    #[test]
    fn opening_save_as_clears_other_overlays() {
        let mut app = make_app();
        open_editor(&mut app, b"abc", None);
        press(&mut app, KeyCode::F(7));
        press(&mut app, KeyCode::F(12));
        match &app.ui_mode {
            UiMode::Editor {
                save_as_dialog: Some(_),
                search_dialog,
                replace_dialog,
                pipe_dialog,
                goto_dialog,
                search_input,
                ..
            } => {
                assert!(search_dialog.is_none());
                assert!(replace_dialog.is_none());
                assert!(pipe_dialog.is_none());
                assert!(goto_dialog.is_none());
                assert!(search_input.is_none());
            }
            _ => panic!("Save as must clear other editor overlays"),
        }
    }

    #[test]
    fn opening_save_as_does_not_clobber_find_file_or_history() {
        let mut app = make_app();
        let cwd = app.active_panel().cwd.clone();
        app.ui_mode = UiMode::FindDialog(FindDialogState::new(cwd));
        press(&mut app, KeyCode::F(12));
        assert!(
            matches!(app.ui_mode, UiMode::FindDialog(_)),
            "F12 in Find File must not switch to Editor Save as"
        );

        app.ui_mode = UiMode::ShellInput;
        press_alt(&mut app, 'h');
        assert!(
            matches!(app.ui_mode, UiMode::HistoryDialog { .. }),
            "Alt-h should open History"
        );
        press(&mut app, KeyCode::F(12));
        assert!(
            matches!(app.ui_mode, UiMode::HistoryDialog { .. }),
            "F12 in History must not switch to Editor Save as"
        );
    }
}

#[cfg(test)]
mod editor_f9_menu_tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use rmc_core::app::{DiffState, EditorMenu};
    use rmc_core::config::KeyMap;
    use rmc_edit::EditorBuffer;
    use rmc_fs::local::LocalFs;

    fn temp_workspace() -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "rmc-editor-f9-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn make_app() -> App {
        let vfs = LocalFs::new();
        let mut app = App::new(Box::new(vfs), KeyMap::mc_defaults()).unwrap();
        app.config_opts.use_internal_edit = true;
        app.config_opts.use_internal_view = true;
        app
    }

    fn make_app_at(cwd: &std::path::Path) -> App {
        let mut app = make_app();
        app.change_dir(cwd).unwrap();
        app
    }

    fn open_editor(app: &mut App, text: &[u8]) {
        app.ui_mode = UiMode::Editor {
            buf: EditorBuffer::from_bytes(text, None),
            show_menu: None,
            status_msg: None,
            search_input: None,
            save_as_dialog: None,
            search_dialog: None,
            replace_dialog: None,
            pipe_dialog: None,
            goto_dialog: None,
            pending_quit: false,
            confirm_exit: None,
            return_to: None,
        };
    }

    fn press(app: &mut App, code: KeyCode) {
        TerminalApp::handle_key(app, KeyEvent::new(code, KeyModifiers::NONE), 10).unwrap();
    }

    fn assert_editor_menu(app: &App, expected: EditorMenu) {
        match &app.ui_mode {
            UiMode::Editor { show_menu, .. } => {
                assert_eq!(*show_menu, Some(expected));
            }
            UiMode::Viewer { .. } => panic!("expected Editor menu, got Viewer"),
            UiMode::InputDialog { title, .. } => {
                panic!("expected Editor menu, got InputDialog {title:?}")
            }
            _ => panic!("expected Editor menu"),
        }
    }

    #[test]
    fn f9_opens_file_menu_and_stays_in_editor() {
        let mut app = make_app();
        open_editor(&mut app, b"hello");
        press(&mut app, KeyCode::F(9));
        match &app.ui_mode {
            UiMode::Editor {
                show_menu,
                search_dialog,
                save_as_dialog,
                replace_dialog,
                pipe_dialog,
                goto_dialog,
                ..
            } => {
                assert_eq!(*show_menu, Some(EditorMenu::File { selected: 0 }));
                assert!(search_dialog.is_none());
                assert!(save_as_dialog.is_none());
                assert!(replace_dialog.is_none());
                assert!(pipe_dialog.is_none());
                assert!(goto_dialog.is_none());
            }
            UiMode::Viewer { .. } => panic!("F9 must stay in Editor, not Viewer"),
            UiMode::InputDialog { title, .. } => {
                panic!("F9 must stay in Editor, not InputDialog {title:?}")
            }
            _ => panic!("F9 must stay in Editor"),
        }
    }

    #[test]
    fn enter_file_save_as_opens_dialog_esc_closes_menu() {
        let mut app = make_app();
        open_editor(&mut app, b"hello");
        press(&mut app, KeyCode::F(9));
        assert_editor_menu(&app, EditorMenu::File { selected: 0 });
        press(&mut app, KeyCode::Down);
        assert_editor_menu(&app, EditorMenu::File { selected: 1 });
        press(&mut app, KeyCode::Enter);
        match &app.ui_mode {
            UiMode::Editor {
                show_menu,
                save_as_dialog: Some(_),
                search_dialog,
                ..
            } => {
                assert!(show_menu.is_none(), "Save as closes the menu");
                assert!(search_dialog.is_none());
            }
            UiMode::Editor {
                save_as_dialog: None,
                ..
            } => panic!("File→Save as must open the Save as dialog"),
            UiMode::InputDialog { title, .. } => {
                panic!("Save as must use EditorSaveAsDialog, not InputDialog {title:?}")
            }
            _ => panic!("expected Editor Save as dialog"),
        }

        let mut app = make_app();
        open_editor(&mut app, b"hello");
        press(&mut app, KeyCode::F(9));
        press(&mut app, KeyCode::Esc);
        match &app.ui_mode {
            UiMode::Editor {
                show_menu,
                save_as_dialog,
                search_dialog,
                confirm_exit,
                ..
            } => {
                assert!(show_menu.is_none());
                assert!(save_as_dialog.is_none());
                assert!(search_dialog.is_none());
                assert!(confirm_exit.is_none(), "Esc must not quit the editor");
            }
            _ => panic!("Esc on the menu must return to editing"),
        }
        press(&mut app, KeyCode::F(9));
        press(&mut app, KeyCode::F(10));
        match &app.ui_mode {
            UiMode::Editor {
                show_menu,
                confirm_exit,
                ..
            } => {
                assert!(show_menu.is_none());
                assert!(confirm_exit.is_none(), "F10 on the menu must not quit");
            }
            UiMode::Normal => panic!("F10 on the menu must not quit the editor"),
            _ => panic!("F10 on the menu must stay in Editor"),
        }
    }

    #[test]
    fn f7_search_still_opens_while_menu_closed() {
        let mut app = make_app();
        open_editor(&mut app, b"hello");
        press(&mut app, KeyCode::F(7));
        match &app.ui_mode {
            UiMode::Editor {
                show_menu,
                search_dialog: Some(_),
                ..
            } => {
                assert!(show_menu.is_none());
            }
            _ => panic!("F7 must open Search while the menu is closed"),
        }
    }

    #[test]
    fn search_overlay_keeps_keys_while_open() {
        let mut app = make_app();
        open_editor(&mut app, b"hello");
        press(&mut app, KeyCode::F(7));
        press(&mut app, KeyCode::F(9));
        match &app.ui_mode {
            UiMode::Editor {
                show_menu,
                search_dialog: Some(_),
                ..
            } => {
                assert!(show_menu.is_none(), "F9 must not steal the Search dialog");
            }
            _ => panic!("Search dialog must keep keys"),
        }
    }

    #[test]
    fn space_on_file_save_as_matches_enter() {
        let mut app = make_app();
        open_editor(&mut app, b"hello");
        press(&mut app, KeyCode::F(9));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char(' '));
        match &app.ui_mode {
            UiMode::Editor {
                save_as_dialog: Some(_),
                show_menu,
                ..
            } => {
                assert!(show_menu.is_none());
            }
            _ => panic!("Space on File→Save as must open Save as"),
        }
    }

    #[test]
    fn left_right_switch_menus_up_down_move_items() {
        let mut app = make_app();
        open_editor(&mut app, b"hello");
        press(&mut app, KeyCode::F(9));
        press(&mut app, KeyCode::Right);
        assert_editor_menu(&app, EditorMenu::Edit { selected: 0 });
        press(&mut app, KeyCode::Right);
        assert_editor_menu(&app, EditorMenu::Search { selected: 0 });
        press(&mut app, KeyCode::Down);
        assert_editor_menu(&app, EditorMenu::Search { selected: 1 });
        press(&mut app, KeyCode::Enter);
        match &app.ui_mode {
            UiMode::Editor {
                show_menu,
                replace_dialog: Some(_),
                ..
            } => {
                assert!(show_menu.is_none());
            }
            _ => panic!("Search→Replace must open Replace"),
        }
    }

    #[test]
    fn command_go_to_line_and_pipe_open_existing_dialogs() {
        let mut app = make_app();
        open_editor(&mut app, b"hello");
        press(&mut app, KeyCode::F(9));
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Right);
        assert_editor_menu(&app, EditorMenu::Command { selected: 0 });
        press(&mut app, KeyCode::Enter);
        match &app.ui_mode {
            UiMode::Editor {
                goto_dialog: Some(_),
                show_menu,
                ..
            } => {
                assert!(show_menu.is_none());
            }
            _ => panic!("Command→Go to line must open Goto"),
        }

        open_editor(&mut app, b"hello");
        press(&mut app, KeyCode::F(9));
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        match &app.ui_mode {
            UiMode::Editor {
                pipe_dialog: Some(_),
                show_menu,
                ..
            } => {
                assert!(show_menu.is_none());
            }
            _ => panic!("Command→Pipe must open Pipe"),
        }
    }

    #[test]
    fn viewer_f9_format_panel_pulldn_diff_f4_unchanged() {
        let root = temp_workspace();
        let file = root.join("notes.txt");
        std::fs::write(&file, "hello\n").unwrap();
        let other = root.join("other.txt");
        std::fs::write(&other, "world\n").unwrap();

        let mut app = make_app_at(&root);
        app.ui_mode = UiMode::new_viewer(file.clone());
        press(&mut app, KeyCode::F(9));
        match &app.ui_mode {
            UiMode::Viewer {
                format_nroff,
                display_dialog,
                viewer_menu,
                ..
            } => {
                assert!(*format_nroff, "Viewer F9 still toggles format");
                assert!(display_dialog.is_none());
                assert!(viewer_menu.is_none());
            }
            UiMode::Editor { .. } => panic!("Viewer F9 must not open Editor"),
            _ => panic!("Viewer F9 must stay in Viewer"),
        }

        app.ui_mode = UiMode::Normal;
        press(&mut app, KeyCode::F(9));
        match &app.ui_mode {
            UiMode::Menu { .. } => {}
            UiMode::Editor { .. } => panic!("panel F9 must stay PullDn, not Editor"),
            _ => panic!("panel F9 must open PullDn"),
        }
        press(&mut app, KeyCode::Esc);

        let ltxt = "hello\n";
        let rtxt = "world\n";
        app.ui_mode = UiMode::Diff(DiffState {
            left_path: file,
            right_path: other,
            left_lines: rmc_diff::split_lines(ltxt),
            right_lines: rmc_diff::split_lines(rtxt),
            hunks: rmc_diff::compute_diff(ltxt, rtxt).hunks,
            current_hunk: 0,
            left_modified: false,
            right_modified: false,
            show_line_numbers: true,
            show_hunk_status: true,
            search: None,
            search_prompt: None,
            goto_prompt: None,
            confirm_exit: None,
            left_scroll: 0,
            right_scroll: 0,
            panel_ratio: 0.6,
            tab_width: 4,
            merge_target_right: true,
        });
        press(&mut app, KeyCode::F(4));
        match &app.ui_mode {
            UiMode::Editor {
                show_menu,
                return_to,
                replace_dialog,
                ..
            } => {
                assert!(show_menu.is_none());
                assert!(replace_dialog.is_none(), "Diff F4 must not open Replace");
                assert!(
                    matches!(return_to.as_deref(), Some(UiMode::Diff(_))),
                    "Diff F4 still nests editor"
                );
            }
            _ => panic!("Diff F4 must nest editor"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod find_file_dialog_tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use rmc_core::config::KeyMap;
    use rmc_core::find::{FindDialogFocus as FF, FindDialogState};
    use rmc_fs::local::LocalFs;

    fn temp_workspace() -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "rmc-find-file-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn make_app(cwd: &std::path::Path) -> App {
        let vfs = LocalFs::new();
        let mut app = App::new(Box::new(vfs), KeyMap::mc_defaults()).unwrap();
        app.change_dir(cwd).unwrap();
        app
    }

    fn press(app: &mut App, code: KeyCode) {
        TerminalApp::handle_key(app, KeyEvent::new(code, KeyModifiers::NONE), 10).unwrap();
    }

    fn find_state(app: &App) -> &FindDialogState {
        match &app.ui_mode {
            UiMode::FindDialog(st) => st,
            _ => panic!("expected FindDialog"),
        }
    }

    fn find_state_mut(app: &mut App) -> &mut FindDialogState {
        match &mut app.ui_mode {
            UiMode::FindDialog(st) => st,
            _ => panic!("expected FindDialog"),
        }
    }

    fn open_find_via_command_menu(app: &mut App) {
        app.config_opts.drop_menus = true;
        press(app, KeyCode::F(9));
        press(app, KeyCode::Right);
        press(app, KeyCode::Right);
        press(app, KeyCode::Down);
        press(app, KeyCode::Enter);
    }

    #[test]
    fn command_menu_opens_find_file() {
        let root = temp_workspace();
        let mut app = make_app(&root);
        open_find_via_command_menu(&mut app);
        let st = find_state(&app);
        assert_eq!(st.focus, FF::NamePattern);
        assert!(!st.params.regular_expression);
        assert!(st.params.find_recursively);
        assert!(!st.params.follow_symlinks);
        assert!(!st.params.skip_hidden);
        assert!(!st.params.case_sensitive);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn tab_and_arrows_reach_gnu_checkboxes_and_space_toggles() {
        let root = temp_workspace();
        let mut app = make_app(&root);
        app.ui_mode = UiMode::FindDialog(FindDialogState::new(root.clone()));

        // NamePattern -> Content -> Case sensitive
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        assert_eq!(find_state(&app).focus, FF::CaseSensitive);
        assert!(!find_state(&app).params.case_sensitive);
        press(&mut app, KeyCode::Char(' '));
        assert!(find_state(&app).params.case_sensitive);
        press(&mut app, KeyCode::Enter);
        assert!(!find_state(&app).params.case_sensitive);

        press(&mut app, KeyCode::Tab);
        assert_eq!(find_state(&app).focus, FF::RegularExpression);
        press(&mut app, KeyCode::Char(' '));
        assert!(find_state(&app).params.regular_expression);

        press(&mut app, KeyCode::Tab);
        assert_eq!(find_state(&app).focus, FF::FindRecursively);
        assert!(find_state(&app).params.find_recursively);
        press(&mut app, KeyCode::Char(' '));
        assert!(!find_state(&app).params.find_recursively);

        press(&mut app, KeyCode::Tab);
        assert_eq!(find_state(&app).focus, FF::FollowSymlinks);
        press(&mut app, KeyCode::Char(' '));
        assert!(find_state(&app).params.follow_symlinks);

        press(&mut app, KeyCode::Tab);
        assert_eq!(find_state(&app).focus, FF::SkipHidden);
        press(&mut app, KeyCode::Char(' '));
        assert!(find_state(&app).params.skip_hidden);

        press(&mut app, KeyCode::BackTab);
        assert_eq!(find_state(&app).focus, FF::FollowSymlinks);

        // Down/Up also walk the checkbox row.
        press(&mut app, KeyCode::Down);
        assert_eq!(find_state(&app).focus, FF::SkipHidden);
        press(&mut app, KeyCode::Up);
        assert_eq!(find_state(&app).focus, FF::FollowSymlinks);

        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        assert_eq!(find_state(&app).focus, FF::ButtonStart);

        // Typing still goes to Filename when that field is focused; Space inserts.
        find_state_mut(&mut app).focus = FF::NamePattern;
        press(&mut app, KeyCode::Char(' '));
        match &find_state(&app).params.name_pattern {
            rmc_core::find::NamePattern::Glob(s) => assert_eq!(s, "* "),
        }

        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod viewer_search_tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use rmc_core::app::ViewerSearchFocus;
    use rmc_core::config::KeyMap;
    use rmc_core::find::FindDialogState;
    use rmc_edit::EditorBuffer;
    use rmc_fs::local::LocalFs;

    fn temp_workspace() -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "rmc-viewer-search-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn make_app() -> App {
        let vfs = LocalFs::new();
        App::new(Box::new(vfs), KeyMap::mc_defaults()).unwrap()
    }

    fn open_viewer(app: &mut App, path: std::path::PathBuf) {
        app.ui_mode = UiMode::new_viewer(path);
    }

    fn press(app: &mut App, code: KeyCode) {
        TerminalApp::handle_key(app, KeyEvent::new(code, KeyModifiers::NONE), 10).unwrap();
    }

    fn press_alt(app: &mut App, c: char) {
        TerminalApp::handle_key(app, KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT), 10)
            .unwrap();
    }

    fn type_text(app: &mut App, s: &str) {
        for c in s.chars() {
            press(app, KeyCode::Char(c));
        }
    }

    fn search_dialog(app: &App) -> &ViewerSearchDialog {
        match &app.ui_mode {
            UiMode::Viewer {
                search_dialog: Some(dlg),
                search_prompt,
                ..
            } => {
                assert!(
                    search_prompt.is_none(),
                    "F7 must not use the inline search prompt"
                );
                dlg
            }
            UiMode::Viewer {
                search_prompt: Some(_),
                ..
            } => panic!("expected Search dialog, got inline search_prompt"),
            UiMode::InputDialog { title, .. } => {
                panic!("expected viewer Search dialog, got InputDialog {title:?}")
            }
            _ => panic!("expected Viewer Search dialog"),
        }
    }

    fn viewer_offset(app: &App) -> u64 {
        match &app.ui_mode {
            UiMode::Viewer { offset, .. } => *offset,
            _ => panic!("expected Viewer"),
        }
    }

    #[test]
    fn f7_opens_search_dialog_defaults_stay_viewer() {
        let root = temp_workspace();
        let file = root.join("notes.txt");
        std::fs::write(&file, "hello").unwrap();
        let mut app = make_app();
        open_viewer(&mut app, file);
        press(&mut app, KeyCode::F(7));
        match &app.ui_mode {
            UiMode::Viewer {
                search_dialog: Some(dlg),
                search_prompt,
                goto_prompt,
                ..
            } => {
                assert!(search_prompt.is_none(), "F7 must not use the inline prompt");
                assert!(goto_prompt.is_none());
                assert!(dlg.search.is_empty());
                assert!(!dlg.case_sensitive);
                assert!(!dlg.backwards);
                assert!(!dlg.whole_words);
                assert!(!dlg.regular_expression);
                assert!(matches!(dlg.focus, ViewerSearchFocus::Search));
            }
            UiMode::InputDialog { title, .. } => {
                panic!("F7 must stay in Viewer, not InputDialog {title:?}")
            }
            _ => panic!("F7 must open the Search dialog in Viewer"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn f7_prefills_last_search_resets_checkboxes() {
        let root = temp_workspace();
        let file = root.join("notes.txt");
        std::fs::write(&file, "foo bar foo").unwrap();
        let mut app = make_app();
        open_viewer(&mut app, file);
        press(&mut app, KeyCode::F(7));
        type_text(&mut app, "foo");
        press(&mut app, KeyCode::Tab); // Case sensitive
        press(&mut app, KeyCode::Char(' '));
        assert!(search_dialog(&app).case_sensitive);
        press(&mut app, KeyCode::BackTab);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::F(7));
        assert_eq!(search_dialog(&app).search, "foo");
        assert!(!search_dialog(&app).case_sensitive);
        assert!(!search_dialog(&app).backwards);
        assert!(!search_dialog(&app).whole_words);
        assert!(!search_dialog(&app).regular_expression);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn tab_reaches_checkboxes_and_buttons_space_enter_toggle_space_inserts() {
        let root = temp_workspace();
        let file = root.join("notes.txt");
        std::fs::write(&file, "abc").unwrap();
        let mut app = make_app();
        open_viewer(&mut app, file);
        press(&mut app, KeyCode::F(7));
        type_text(&mut app, "a b");
        press(&mut app, KeyCode::Tab);
        assert_eq!(search_dialog(&app).focus, ViewerSearchFocus::CaseSensitive);
        assert!(!search_dialog(&app).case_sensitive);
        press(&mut app, KeyCode::Char(' '));
        assert!(search_dialog(&app).case_sensitive);
        press(&mut app, KeyCode::Enter);
        assert!(!search_dialog(&app).case_sensitive);

        press(&mut app, KeyCode::Tab);
        assert_eq!(search_dialog(&app).focus, ViewerSearchFocus::Backwards);
        press(&mut app, KeyCode::Char(' '));
        assert!(search_dialog(&app).backwards);

        press(&mut app, KeyCode::Tab);
        assert_eq!(search_dialog(&app).focus, ViewerSearchFocus::WholeWords);
        press(&mut app, KeyCode::Char(' '));
        assert!(search_dialog(&app).whole_words);

        press(&mut app, KeyCode::Tab);
        assert_eq!(
            search_dialog(&app).focus,
            ViewerSearchFocus::RegularExpression
        );
        press(&mut app, KeyCode::Char(' '));
        assert!(search_dialog(&app).regular_expression);

        press(&mut app, KeyCode::Tab);
        assert!(matches!(search_dialog(&app).focus, ViewerSearchFocus::Ok));
        press(&mut app, KeyCode::Tab);
        assert_eq!(search_dialog(&app).focus, ViewerSearchFocus::Cancel);
        press(&mut app, KeyCode::Tab);
        assert_eq!(search_dialog(&app).focus, ViewerSearchFocus::Search);
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(search_dialog(&app).search, "a b ");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn enter_ok_on_known_needle_jumps_found() {
        let root = temp_workspace();
        let file = root.join("notes.txt");
        std::fs::write(&file, "xx abc yy").unwrap();
        let mut app = make_app();
        open_viewer(&mut app, file);
        press(&mut app, KeyCode::F(7));
        type_text(&mut app, "abc");
        press(&mut app, KeyCode::Enter);
        match &app.ui_mode {
            UiMode::Viewer {
                search_dialog,
                search_prompt,
                offset,
                status_msg,
                search,
                ..
            } => {
                assert!(search_dialog.is_none());
                assert!(search_prompt.is_none());
                assert_eq!(*offset, 3);
                assert_eq!(status_msg.as_deref(), Some("Found"));
                assert_eq!(search.as_deref(), Some("abc"));
            }
            _ => panic!("expected Viewer after Search"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn ok_button_missing_needle_not_found() {
        let root = temp_workspace();
        let file = root.join("notes.txt");
        std::fs::write(&file, "hello").unwrap();
        let mut app = make_app();
        open_viewer(&mut app, file);
        press(&mut app, KeyCode::F(7));
        type_text(&mut app, "zzz");
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab); // Ok
        assert!(matches!(search_dialog(&app).focus, ViewerSearchFocus::Ok));
        press(&mut app, KeyCode::Enter);
        match &app.ui_mode {
            UiMode::Viewer {
                search_dialog,
                offset,
                status_msg,
                ..
            } => {
                assert!(search_dialog.is_none());
                assert_eq!(*offset, 0);
                assert_eq!(status_msg.as_deref(), Some("Not found"));
            }
            _ => panic!("expected Viewer after OK"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn esc_f10_cancel_leave_offset_unmoved() {
        let root = temp_workspace();
        let file = root.join("notes.txt");
        std::fs::write(&file, "abc").unwrap();
        let mut app = make_app();
        open_viewer(&mut app, file);
        match &mut app.ui_mode {
            UiMode::Viewer { offset, .. } => *offset = 1,
            _ => panic!("expected Viewer"),
        }
        let start = viewer_offset(&app);

        press(&mut app, KeyCode::F(7));
        type_text(&mut app, "abc");
        press(&mut app, KeyCode::Esc);
        assert_eq!(viewer_offset(&app), start);
        match &app.ui_mode {
            UiMode::Viewer { search_dialog, .. } => assert!(search_dialog.is_none()),
            UiMode::Normal => panic!("Esc must stay in the viewer"),
            _ => panic!("Esc should stay in the viewer"),
        }

        press(&mut app, KeyCode::F(7));
        type_text(&mut app, "abc");
        press(&mut app, KeyCode::F(10));
        assert_eq!(viewer_offset(&app), start);
        match &app.ui_mode {
            UiMode::Viewer { search_dialog, .. } => assert!(search_dialog.is_none()),
            _ => panic!("F10 should stay in the viewer"),
        }

        press(&mut app, KeyCode::F(7));
        type_text(&mut app, "abc");
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab); // Ok
        press(&mut app, KeyCode::Tab); // Cancel
        assert_eq!(search_dialog(&app).focus, ViewerSearchFocus::Cancel);
        press(&mut app, KeyCode::Enter);
        assert_eq!(viewer_offset(&app), start);
        match &app.ui_mode {
            UiMode::Viewer { search_dialog, .. } => assert!(search_dialog.is_none()),
            _ => panic!("Cancel should stay in the viewer"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn empty_ok_is_noop_close() {
        let root = temp_workspace();
        let file = root.join("notes.txt");
        std::fs::write(&file, "keep me").unwrap();
        let mut app = make_app();
        open_viewer(&mut app, file);
        press(&mut app, KeyCode::F(7));
        press(&mut app, KeyCode::Enter);
        match &app.ui_mode {
            UiMode::Viewer {
                search_dialog,
                offset,
                status_msg,
                search,
                ..
            } => {
                assert!(search_dialog.is_none());
                assert_eq!(*offset, 0);
                assert!(status_msg.is_none());
                assert!(search.is_none());
            }
            _ => panic!("expected Viewer after empty Search"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn checkboxes_change_search_behavior() {
        let root = temp_workspace();
        let file = root.join("notes.txt");
        std::fs::write(&file, "Abc abc category cat").unwrap();
        let mut app = make_app();
        open_viewer(&mut app, file);

        // Case sensitive on: "abc" skips "Abc"
        press(&mut app, KeyCode::F(7));
        type_text(&mut app, "abc");
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char(' '));
        press(&mut app, KeyCode::BackTab); // back to the field
        press(&mut app, KeyCode::Enter);
        assert_eq!(viewer_offset(&app), 4);

        // Backwards + case sensitive from there: inclusive match still at 4
        press(&mut app, KeyCode::F(7));
        press(&mut app, KeyCode::Tab); // case
        press(&mut app, KeyCode::Char(' '));
        press(&mut app, KeyCode::Tab); // backwards
        press(&mut app, KeyCode::Char(' '));
        press(&mut app, KeyCode::BackTab);
        press(&mut app, KeyCode::BackTab); // back to the field
        press(&mut app, KeyCode::Enter);
        assert_eq!(viewer_offset(&app), 4);

        // Whole words: "cat" skips "category"
        match &mut app.ui_mode {
            UiMode::Viewer { offset, search, .. } => {
                *offset = 0;
                *search = None;
            }
            _ => panic!("expected Viewer"),
        }
        press(&mut app, KeyCode::F(7));
        type_text(&mut app, "cat");
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab); // Whole words
        press(&mut app, KeyCode::Char(' '));
        press(&mut app, KeyCode::BackTab);
        press(&mut app, KeyCode::BackTab);
        press(&mut app, KeyCode::BackTab); // back to the field
        press(&mut app, KeyCode::Enter);
        assert_eq!(viewer_offset(&app), 17);

        // Regular expression
        match &mut app.ui_mode {
            UiMode::Viewer { offset, search, .. } => {
                *offset = 0;
                *search = None;
            }
            _ => panic!("expected Viewer"),
        }
        press(&mut app, KeyCode::F(7));
        type_text(&mut app, "A.c");
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab); // regexp
        press(&mut app, KeyCode::Char(' '));
        press(&mut app, KeyCode::BackTab);
        press(&mut app, KeyCode::BackTab);
        press(&mut app, KeyCode::BackTab);
        press(&mut app, KeyCode::BackTab); // back to the field
        press(&mut app, KeyCode::Enter);
        assert_eq!(viewer_offset(&app), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn next_match_honors_stored_options() {
        let root = temp_workspace();
        let file = root.join("notes.txt");
        std::fs::write(&file, "cat category cat").unwrap();
        let mut app = make_app();
        open_viewer(&mut app, file);
        press(&mut app, KeyCode::F(7));
        type_text(&mut app, "cat");
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char(' ')); // case sensitive
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char(' ')); // whole words
        press(&mut app, KeyCode::BackTab);
        press(&mut app, KeyCode::BackTab);
        press(&mut app, KeyCode::BackTab); // back to the field
        press(&mut app, KeyCode::Enter);
        assert_eq!(viewer_offset(&app), 0);
        press(&mut app, KeyCode::Char('n'));
        assert_eq!(viewer_offset(&app), 13);
        press(&mut app, KeyCode::F(17));
        assert_eq!(viewer_offset(&app), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn f7_while_open_does_not_nest() {
        let root = temp_workspace();
        let file = root.join("notes.txt");
        std::fs::write(&file, "abc").unwrap();
        let mut app = make_app();
        open_viewer(&mut app, file);
        press(&mut app, KeyCode::F(7));
        type_text(&mut app, "ab");
        press(&mut app, KeyCode::F(7));
        assert_eq!(search_dialog(&app).search, "ab");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn f7_in_find_file_history_editor_does_not_open_viewer_search() {
        let mut app = make_app();
        let cwd = app.active_panel().cwd.clone();
        app.ui_mode = UiMode::FindDialog(FindDialogState::new(cwd));
        press(&mut app, KeyCode::F(7));
        assert!(
            matches!(app.ui_mode, UiMode::FindDialog(_)),
            "F7 in Find File must not open viewer Search"
        );

        app.ui_mode = UiMode::ShellInput;
        press_alt(&mut app, 'h');
        assert!(
            matches!(app.ui_mode, UiMode::HistoryDialog { .. }),
            "Alt-h should open History"
        );
        press(&mut app, KeyCode::F(7));
        assert!(
            matches!(app.ui_mode, UiMode::HistoryDialog { .. }),
            "F7 in History must not open viewer Search"
        );

        app.ui_mode = UiMode::Editor {
            buf: EditorBuffer::from_bytes(b"abc", None),
            show_menu: None,
            status_msg: None,
            search_input: None,
            save_as_dialog: None,
            search_dialog: None,
            replace_dialog: None,
            pipe_dialog: None,
            goto_dialog: None,
            pending_quit: false,
            confirm_exit: None,
            return_to: None,
        };
        press(&mut app, KeyCode::F(7));
        match &app.ui_mode {
            UiMode::Editor {
                search_dialog: Some(_),
                ..
            } => {}
            UiMode::Viewer { .. } => panic!("F7 in Editor must not switch to Viewer"),
            _ => panic!("F7 in Editor must open editor Search"),
        }
    }

    #[test]
    fn hex_wrap_goto_still_work() {
        let root = temp_workspace();
        let file = root.join("notes.txt");
        std::fs::write(&file, "hello\nworld\n").unwrap();
        let mut app = make_app();
        open_viewer(&mut app, file);

        press(&mut app, KeyCode::F(4));
        match &app.ui_mode {
            UiMode::Viewer {
                hex, search_dialog, ..
            } => {
                assert!(*hex);
                assert!(search_dialog.is_none());
            }
            _ => panic!("F4 must toggle hex in Viewer"),
        }
        press(&mut app, KeyCode::F(7));
        type_text(&mut app, "el");
        press(&mut app, KeyCode::Enter);
        match &app.ui_mode {
            UiMode::Viewer {
                hex,
                offset,
                status_msg,
                search_dialog,
                ..
            } => {
                assert!(*hex, "Search must not leave hex mode");
                assert_eq!(*offset, 1);
                assert_eq!(status_msg.as_deref(), Some("Found"));
                assert!(search_dialog.is_none());
            }
            _ => panic!("expected Viewer after hex Search"),
        }

        press(&mut app, KeyCode::F(4));
        match &app.ui_mode {
            UiMode::Viewer { hex, .. } => assert!(!*hex),
            _ => panic!("expected Viewer"),
        }

        press(&mut app, KeyCode::Char('w'));
        match &app.ui_mode {
            UiMode::Viewer { wrap, hex, .. } => {
                assert!(*wrap);
                assert!(!*hex);
            }
            _ => panic!("expected Viewer"),
        }

        press(&mut app, KeyCode::F(5));
        match &app.ui_mode {
            UiMode::Viewer {
                goto_prompt: Some(_),
                search_dialog,
                ..
            } => {
                assert!(search_dialog.is_none());
            }
            _ => panic!("F5 must open goto"),
        }
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Enter);
        match &app.ui_mode {
            UiMode::Viewer {
                goto_prompt,
                offset,
                ..
            } => {
                assert!(goto_prompt.is_none());
                assert!(*offset > 0, "goto line 2 should move off start");
            }
            _ => panic!("expected Viewer after Goto"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod viewer_display_options_tests {
    use super::*;
    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use rmc_core::app::{EditorMenu, ViewerDisplayFocus};
    use rmc_core::config::KeyMap;
    use rmc_core::find::FindDialogState;
    use rmc_edit::EditorBuffer;
    use rmc_fs::local::LocalFs;

    fn temp_workspace() -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "rmc-viewer-display-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn make_app() -> App {
        let vfs = LocalFs::new();
        App::new(Box::new(vfs), KeyMap::mc_defaults()).unwrap()
    }

    fn open_viewer(app: &mut App, path: std::path::PathBuf) {
        app.ui_mode = UiMode::new_viewer(path);
    }

    fn press(app: &mut App, code: KeyCode) {
        TerminalApp::handle_key(app, KeyEvent::new(code, KeyModifiers::NONE), 10).unwrap();
    }

    fn press_alt(app: &mut App, c: char) {
        TerminalApp::handle_key(app, KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT), 10)
            .unwrap();
    }

    fn click_viewer_options_menu(app: &mut App) {
        TerminalApp::handle_mouse(
            app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 18,
                row: 0,
                modifiers: KeyModifiers::NONE,
            },
            10,
        )
        .unwrap();
    }

    /// GNU path: click topmost line on Options, then Enter on Display options.
    fn open_display_options(app: &mut App) {
        click_viewer_options_menu(app);
        press(app, KeyCode::Enter);
    }

    fn display_dialog(app: &App) -> &ViewerDisplayDialog {
        match &app.ui_mode {
            UiMode::Viewer {
                display_dialog: Some(dlg),
                ..
            } => dlg,
            UiMode::InputDialog { title, .. } => {
                panic!("expected viewer Display options dialog, got InputDialog {title:?}")
            }
            _ => panic!("expected Viewer display-options dialog"),
        }
    }

    #[test]
    fn options_menu_opens_display_dialog_defaults_match_viewer_state() {
        let root = temp_workspace();
        let file = root.join("notes.txt");
        std::fs::write(&file, "hello").unwrap();
        let mut app = make_app();
        open_viewer(&mut app, file);
        match &mut app.ui_mode {
            UiMode::Viewer {
                show_line_numbers,
                show_cr,
                wrap,
                hex,
                ..
            } => {
                *show_line_numbers = true;
                *show_cr = true;
                *wrap = true;
                *hex = false;
            }
            _ => panic!("expected Viewer"),
        }
        open_display_options(&mut app);
        match &app.ui_mode {
            UiMode::Viewer {
                display_dialog: Some(dlg),
                search_prompt,
                goto_prompt,
                search_dialog,
                ..
            } => {
                assert!(search_prompt.is_none());
                assert!(goto_prompt.is_none());
                assert!(search_dialog.is_none());
                assert!(dlg.show_line_numbers);
                assert!(dlg.show_cr);
                assert!(dlg.wrap);
                assert!(!dlg.hex);
                assert_eq!(dlg.focus, ViewerDisplayFocus::ShowLineNumbers);
            }
            UiMode::InputDialog { title, .. } => {
                panic!("Display options must stay in Viewer, not InputDialog {title:?}")
            }
            _ => panic!("Options menu must open the Display options dialog in Viewer"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn tab_reaches_every_checkbox_then_ok_cancel_space_enter_toggle() {
        let root = temp_workspace();
        let file = root.join("notes.txt");
        std::fs::write(&file, "abc").unwrap();
        let mut app = make_app();
        open_viewer(&mut app, file);
        open_display_options(&mut app);
        assert_eq!(
            display_dialog(&app).focus,
            ViewerDisplayFocus::ShowLineNumbers
        );
        assert!(!display_dialog(&app).show_line_numbers);
        press(&mut app, KeyCode::Char(' '));
        assert!(display_dialog(&app).show_line_numbers);
        press(&mut app, KeyCode::Enter);
        assert!(!display_dialog(&app).show_line_numbers);

        press(&mut app, KeyCode::Tab);
        assert_eq!(display_dialog(&app).focus, ViewerDisplayFocus::ShowCr);
        press(&mut app, KeyCode::Char(' '));
        assert!(display_dialog(&app).show_cr);

        press(&mut app, KeyCode::Tab);
        assert_eq!(display_dialog(&app).focus, ViewerDisplayFocus::WrapMode);
        press(&mut app, KeyCode::Char(' '));
        assert!(display_dialog(&app).wrap);

        press(&mut app, KeyCode::Tab);
        assert_eq!(display_dialog(&app).focus, ViewerDisplayFocus::HexMode);
        press(&mut app, KeyCode::Char(' '));
        assert!(display_dialog(&app).hex);

        press(&mut app, KeyCode::Tab);
        assert_eq!(display_dialog(&app).focus, ViewerDisplayFocus::Ok);
        press(&mut app, KeyCode::Tab);
        assert_eq!(display_dialog(&app).focus, ViewerDisplayFocus::Cancel);
        press(&mut app, KeyCode::Left);
        assert_eq!(display_dialog(&app).focus, ViewerDisplayFocus::Ok);
        press(&mut app, KeyCode::Right);
        assert_eq!(display_dialog(&app).focus, ViewerDisplayFocus::Cancel);
        press(&mut app, KeyCode::Tab);
        assert_eq!(
            display_dialog(&app).focus,
            ViewerDisplayFocus::ShowLineNumbers
        );
        // Space on a checkbox toggles; it must not insert junk or leave Viewer.
        press(&mut app, KeyCode::Char(' '));
        assert!(display_dialog(&app).show_line_numbers);
        match &app.ui_mode {
            UiMode::Viewer {
                display_dialog: Some(_),
                ..
            } => {}
            _ => panic!("Space must stay in the display-options dialog"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn esc_f10_cancel_leave_flags_unchanged() {
        let root = temp_workspace();
        let file = root.join("notes.txt");
        std::fs::write(&file, "abc").unwrap();
        let mut app = make_app();
        open_viewer(&mut app, file);

        open_display_options(&mut app);
        press(&mut app, KeyCode::Char(' ')); // line numbers
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char(' ')); // CR
        press(&mut app, KeyCode::Esc);
        match &app.ui_mode {
            UiMode::Viewer {
                display_dialog,
                show_line_numbers,
                show_cr,
                wrap,
                hex,
                ..
            } => {
                assert!(display_dialog.is_none());
                assert!(!*show_line_numbers);
                assert!(!*show_cr);
                assert!(!*wrap);
                assert!(!*hex);
            }
            UiMode::Normal => panic!("Esc must stay in the viewer"),
            _ => panic!("Esc should stay in the viewer"),
        }

        open_display_options(&mut app);
        press(&mut app, KeyCode::Char(' '));
        press(&mut app, KeyCode::F(10));
        match &app.ui_mode {
            UiMode::Viewer {
                display_dialog,
                show_line_numbers,
                ..
            } => {
                assert!(display_dialog.is_none());
                assert!(!*show_line_numbers);
            }
            _ => panic!("F10 should stay in the viewer"),
        }

        open_display_options(&mut app);
        press(&mut app, KeyCode::Char(' '));
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab); // Ok
        press(&mut app, KeyCode::Tab); // Cancel
        assert_eq!(display_dialog(&app).focus, ViewerDisplayFocus::Cancel);
        press(&mut app, KeyCode::Enter);
        match &app.ui_mode {
            UiMode::Viewer {
                display_dialog,
                show_line_numbers,
                ..
            } => {
                assert!(display_dialog.is_none());
                assert!(!*show_line_numbers);
            }
            _ => panic!("Cancel should stay in the viewer"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn ok_applies_toggled_flags() {
        let root = temp_workspace();
        let file = root.join("notes.txt");
        std::fs::write(&file, "hello\r\nworld\n").unwrap();
        let mut app = make_app();
        open_viewer(&mut app, file);
        open_display_options(&mut app);
        press(&mut app, KeyCode::Char(' ')); // line numbers
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char(' ')); // CR
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char(' ')); // wrap
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab); // skip hex, to Ok
        assert_eq!(display_dialog(&app).focus, ViewerDisplayFocus::Ok);
        press(&mut app, KeyCode::Char(' '));
        match &app.ui_mode {
            UiMode::Viewer {
                display_dialog,
                show_line_numbers,
                show_cr,
                wrap,
                hex,
                ..
            } => {
                assert!(display_dialog.is_none());
                assert!(*show_line_numbers);
                assert!(*show_cr);
                assert!(*wrap);
                assert!(!*hex);
            }
            _ => panic!("expected Viewer after OK"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn dialog_while_open_does_not_nest() {
        let root = temp_workspace();
        let file = root.join("notes.txt");
        std::fs::write(&file, "abc").unwrap();
        let mut app = make_app();
        open_viewer(&mut app, file);
        open_display_options(&mut app);
        press(&mut app, KeyCode::Char(' '));
        press(&mut app, KeyCode::F(9));
        assert!(display_dialog(&app).show_line_numbers);
        assert_eq!(
            display_dialog(&app).focus,
            ViewerDisplayFocus::ShowLineNumbers
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn f7_search_f5_goto_f4_hex_w_wrap_still_work_when_closed() {
        let root = temp_workspace();
        let file = root.join("notes.txt");
        std::fs::write(&file, "hello\nworld\n").unwrap();
        let mut app = make_app();
        open_viewer(&mut app, file);

        press(&mut app, KeyCode::F(7));
        match &app.ui_mode {
            UiMode::Viewer {
                search_dialog: Some(_),
                display_dialog,
                ..
            } => {
                assert!(display_dialog.is_none());
            }
            _ => panic!("F7 must still open Search"),
        }
        press(&mut app, KeyCode::Esc);

        press(&mut app, KeyCode::F(5));
        match &app.ui_mode {
            UiMode::Viewer {
                goto_prompt: Some(_),
                display_dialog,
                ..
            } => {
                assert!(display_dialog.is_none());
            }
            _ => panic!("F5 must still open Goto"),
        }
        press(&mut app, KeyCode::Esc);

        press(&mut app, KeyCode::F(4));
        match &app.ui_mode {
            UiMode::Viewer {
                hex,
                display_dialog,
                ..
            } => {
                assert!(*hex);
                assert!(display_dialog.is_none());
            }
            _ => panic!("F4 must still toggle hex"),
        }
        press(&mut app, KeyCode::F(4));

        press(&mut app, KeyCode::Char('w'));
        match &app.ui_mode {
            UiMode::Viewer {
                wrap,
                hex,
                display_dialog,
                ..
            } => {
                assert!(*wrap);
                assert!(!*hex);
                assert!(display_dialog.is_none());
            }
            _ => panic!("w must still toggle wrap"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn f9_does_not_steal_open_search() {
        let root = temp_workspace();
        let file = root.join("notes.txt");
        std::fs::write(&file, "abc").unwrap();
        let mut app = make_app();
        open_viewer(&mut app, file);
        press(&mut app, KeyCode::F(7));
        press(&mut app, KeyCode::Char('a'));
        press(&mut app, KeyCode::F(9));
        match &app.ui_mode {
            UiMode::Viewer {
                search_dialog: Some(dlg),
                display_dialog,
                ..
            } => {
                assert!(display_dialog.is_none(), "F9 must not steal Search");
                assert_eq!(dlg.search, "a");
            }
            _ => panic!("expected Search dialog still open"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn f9_in_find_file_history_editor_does_not_open_display_dialog() {
        let mut app = make_app();
        let cwd = app.active_panel().cwd.clone();
        app.ui_mode = UiMode::FindDialog(FindDialogState::new(cwd));
        press(&mut app, KeyCode::F(9));
        assert!(
            matches!(app.ui_mode, UiMode::FindDialog(_)),
            "F9 in Find File must not open viewer Display options"
        );

        app.ui_mode = UiMode::ShellInput;
        press_alt(&mut app, 'h');
        assert!(
            matches!(app.ui_mode, UiMode::HistoryDialog { .. }),
            "Alt-h should open History"
        );
        press(&mut app, KeyCode::F(9));
        assert!(
            matches!(app.ui_mode, UiMode::HistoryDialog { .. }),
            "F9 in History must not open viewer Display options"
        );

        app.ui_mode = UiMode::Editor {
            buf: EditorBuffer::from_bytes(b"abc", None),
            show_menu: None,
            status_msg: None,
            search_input: None,
            save_as_dialog: None,
            search_dialog: None,
            replace_dialog: None,
            pipe_dialog: None,
            goto_dialog: None,
            pending_quit: false,
            confirm_exit: None,
            return_to: None,
        };
        press(&mut app, KeyCode::F(9));
        match &app.ui_mode {
            UiMode::Editor { show_menu, .. } => {
                assert!(
                    matches!(show_menu, Some(EditorMenu::File { selected: 0 })),
                    "F9 in Editor opens the mcedit File menu, not viewer Display options"
                );
            }
            UiMode::Viewer { .. } => panic!("F9 in Editor must not switch to Viewer"),
            UiMode::InputDialog { title, .. } => {
                panic!("F9 in Editor must not open InputDialog {title:?}")
            }
            _ => panic!("F9 in Editor must not open viewer Display options"),
        }
    }

    #[test]
    fn options_menu_clears_leftover_search_prompt() {
        let root = temp_workspace();
        let file = root.join("notes.txt");
        std::fs::write(&file, "abc").unwrap();
        let mut app = make_app();
        open_viewer(&mut app, file);
        match &mut app.ui_mode {
            UiMode::Viewer { search_prompt, .. } => {
                *search_prompt = Some("leftover".into());
            }
            _ => panic!("expected Viewer"),
        }
        open_display_options(&mut app);
        match &app.ui_mode {
            UiMode::Viewer {
                display_dialog: Some(_),
                search_prompt,
                goto_prompt,
                ..
            } => {
                assert!(search_prompt.is_none());
                assert!(goto_prompt.is_none());
            }
            _ => panic!("Options menu must open Display options and clear leftover prompts"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod viewer_compressed_filter_tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use rmc_core::config::KeyMap;
    use rmc_core::find::FindDialogState;
    use rmc_fs::composite::CompositeFs;
    use rmc_fs::local::LocalFs;
    use std::io::Write;
    use std::process::{Command, Stdio};

    const PAYLOAD: &[u8] = b"decoded-gzip-payload-unique\nline-two\n";

    fn temp_workspace() -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "rmc-view-gz-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn make_app(cwd: &std::path::Path) -> App {
        let vfs = LocalFs::new();
        let mut app = App::new(Box::new(vfs), KeyMap::mc_defaults()).unwrap();
        app.config_opts.use_internal_view = true;
        app.config_opts.use_internal_edit = true;
        app.change_dir(cwd).unwrap();
        app
    }

    fn make_composite_app(cwd: &std::path::Path) -> App {
        let vfs = CompositeFs::new();
        let mut app = App::new(Box::new(vfs), KeyMap::mc_defaults()).unwrap();
        app.config_opts.use_internal_view = true;
        app.config_opts.use_internal_edit = true;
        app.change_dir(cwd).unwrap();
        app
    }

    fn select_named(app: &mut App, name: &str) {
        let idx = app
            .active_panel()
            .entries
            .iter()
            .position(|e| e.name == name)
            .unwrap_or_else(|| panic!("missing {name}"));
        app.active_panel_mut().cursor = idx;
    }

    fn press(app: &mut App, code: KeyCode) {
        TerminalApp::handle_key(app, KeyEvent::new(code, KeyModifiers::NONE), 10).unwrap();
    }

    fn press_alt(app: &mut App, c: char) {
        TerminalApp::handle_key(app, KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT), 10)
            .unwrap();
    }

    fn gzip_file(src: &std::path::Path, dest: &std::path::Path) -> bool {
        if !rmc_view::helper_on_path("gzip") {
            return false;
        }
        let out = match std::fs::File::create(dest) {
            Ok(f) => f,
            Err(_) => return false,
        };
        Command::new("gzip")
            .arg("-c")
            .arg(src)
            .stdout(Stdio::from(out))
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn decoded_bytes(display: &std::path::Path) -> Vec<u8> {
        let p = viewer_ensure_view_for(display);
        std::fs::read(p).unwrap()
    }

    #[test]
    fn f3_on_gz_shows_decoded_text_not_gzip_magic() {
        if !rmc_view::helper_on_path("gzip") {
            return;
        }
        let root = temp_workspace();
        let txt = root.join("notes.txt");
        std::fs::write(&txt, PAYLOAD).unwrap();
        let gz = root.join("notes.txt.gz");
        assert!(gzip_file(&txt, &gz), "gzip -c");
        let mut app = make_app(&root);
        select_named(&mut app, "notes.txt.gz");
        press(&mut app, KeyCode::F(3));
        match &app.ui_mode {
            UiMode::Viewer { path, .. } => assert_eq!(path, &gz),
            _ => panic!("expected Viewer for F3 on notes.txt.gz"),
        }
        let bytes = decoded_bytes(&gz);
        assert!(
            !bytes.starts_with(&[0x1f, 0x8b]),
            "viewer must not show gzip magic as text"
        );
        assert_eq!(bytes, PAYLOAD);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn viewer_search_hex_wrap_display_work_on_decoded_gz() {
        if !rmc_view::helper_on_path("gzip") {
            return;
        }
        let root = temp_workspace();
        let txt = root.join("notes.txt");
        std::fs::write(&txt, PAYLOAD).unwrap();
        let gz = root.join("notes.txt.gz");
        assert!(gzip_file(&txt, &gz), "gzip -c");
        let mut app = make_app(&root);
        select_named(&mut app, "notes.txt.gz");
        view_current_file(&mut app).unwrap();
        assert!(matches!(app.ui_mode, UiMode::Viewer { .. }));

        press(&mut app, KeyCode::F(7));
        for c in "decoded-gzip-payload".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Enter);
        match &app.ui_mode {
            UiMode::Viewer {
                status_msg,
                search_dialog,
                ..
            } => {
                assert!(search_dialog.is_none());
                assert_eq!(status_msg.as_deref(), Some("Found"));
            }
            _ => panic!("expected Viewer after Search"),
        }

        press(&mut app, KeyCode::Char('w'));
        match &app.ui_mode {
            UiMode::Viewer { wrap, hex, .. } => {
                assert!(*wrap);
                assert!(!*hex);
            }
            _ => panic!("expected Viewer wrap"),
        }

        press(&mut app, KeyCode::F(4));
        match &app.ui_mode {
            UiMode::Viewer { hex, wrap, .. } => {
                assert!(*hex, "F4 hex on decoded stream");
                assert!(*wrap);
            }
            _ => panic!("expected Viewer hex"),
        }
        let hex_bytes = decoded_bytes(&gz);
        assert_eq!(hex_bytes, PAYLOAD, "hex mode still uses decoded bytes");
        assert!(!hex_bytes.starts_with(&[0x1f, 0x8b]));

        press(&mut app, KeyCode::F(9));
        match &app.ui_mode {
            UiMode::Viewer {
                format_nroff,
                hex,
                display_dialog,
                ..
            } => {
                assert!(*hex, "F4 hex still on");
                assert!(
                    *format_nroff,
                    "F9 toggles format/unformat on decoded viewer"
                );
                assert!(display_dialog.is_none(), "F9 is not Display options");
            }
            _ => panic!("F9 must toggle format on decoded viewer"),
        }
        press(&mut app, KeyCode::F(10));
        assert!(matches!(app.ui_mode, UiMode::Normal));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn f3_on_plain_txt_is_unfiltered() {
        let root = temp_workspace();
        let file = root.join("notes.txt");
        std::fs::write(&file, "plain-text-payload\n").unwrap();
        let mut app = make_app(&root);
        select_named(&mut app, "notes.txt");
        view_current_file(&mut app).unwrap();
        match &app.ui_mode {
            UiMode::Viewer { path, .. } => assert_eq!(path, &file),
            _ => panic!("expected Viewer"),
        }
        let bytes = decoded_bytes(&file);
        assert_eq!(bytes, b"plain-text-payload\n");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn f3_on_targz_archive_enters_vfs() {
        if !rmc_view::helper_on_path("tar") || !rmc_view::helper_on_path("gzip") {
            return;
        }
        let root = temp_workspace();
        let src = root.join("archsrc");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("inside.txt"), b"inside-archive").unwrap();
        let dest = root.join("sample.tar.gz");
        let status = Command::new("tar")
            .args([
                "-czf",
                dest.to_str().unwrap(),
                "-C",
                src.to_str().unwrap(),
                ".",
            ])
            .status()
            .unwrap();
        if !status.success() {
            let _ = std::fs::remove_dir_all(&root);
            return;
        }
        let mut app = make_composite_app(&root);
        select_named(&mut app, "sample.tar.gz");
        press(&mut app, KeyCode::F(3));
        assert!(
            matches!(app.ui_mode, UiMode::Normal),
            "F3 on tar.gz archive must VFS-enter, not open Viewer"
        );
        let cwd = app.active_panel().cwd.clone();
        let cwd_s = cwd.to_string_lossy();
        assert!(
            cwd_s.contains("sample.tar.gz#")
                || cwd
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().ends_with('#')),
            "cwd should be archive VFS anchor, got {cwd:?}"
        );
        let names: Vec<_> = app
            .active_panel()
            .entries
            .iter()
            .map(|e| e.name.clone())
            .collect();
        assert!(
            names
                .iter()
                .any(|n| n == "inside.txt" || n == "archsrc" || n == "."),
            "archive listing should be visible, got {names:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_or_failed_helper_does_not_open_raw_bytes() {
        let root = temp_workspace();
        // .zst is in [view] → zstd -dc. Dummy bytes are not a valid zstd stream,
        // and zstd may be absent: either way Viewer must not open as text.
        let zst = root.join("log.zst");
        {
            let mut f = std::fs::File::create(&zst).unwrap();
            f.write_all(&[0x28, 0xb5, 0x2f, 0xfd, 0x00, 0x00, b'G', b'A', b'R', b'B'])
                .unwrap();
        }
        let mut app = make_app(&root);
        select_named(&mut app, "log.zst");
        view_current_file(&mut app).unwrap();
        match &app.ui_mode {
            UiMode::DialogConfirm { title, message, .. } => {
                assert_eq!(title, "Error");
                assert!(
                    message.contains("Cannot view")
                        || message.contains("not found")
                        || message.contains("failed"),
                    "GNU-like error, got {message}"
                );
            }
            UiMode::Viewer { .. } => panic!("must not open Viewer on failed decompress"),
            _ => panic!("expected Error dialog, not Viewer"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn find_file_history_editor_do_not_decompress() {
        if !rmc_view::helper_on_path("gzip") {
            return;
        }
        let root = temp_workspace();
        let txt = root.join("notes.txt");
        std::fs::write(&txt, PAYLOAD).unwrap();
        let gz = root.join("notes.txt.gz");
        assert!(gzip_file(&txt, &gz), "gzip -c");
        let mut app = make_app(&root);
        select_named(&mut app, "notes.txt.gz");

        app.ui_mode = UiMode::FindDialog(FindDialogState::new(root.clone()));
        press(&mut app, KeyCode::F(7));
        assert!(
            matches!(app.ui_mode, UiMode::FindDialog(_)),
            "Find File must not start decompressing / open Viewer"
        );

        app.ui_mode = UiMode::ShellInput;
        press_alt(&mut app, 'h');
        assert!(
            matches!(app.ui_mode, UiMode::HistoryDialog { .. }),
            "History must still open"
        );
        press(&mut app, KeyCode::Esc);

        app.ui_mode = UiMode::Normal;
        select_named(&mut app, "notes.txt.gz");
        press(&mut app, KeyCode::F(4));
        match &app.ui_mode {
            UiMode::Editor { buf, .. } => {
                let bytes = buf.to_bytes();
                assert!(
                    bytes.starts_with(&[0x1f, 0x8b]) || bytes != PAYLOAD,
                    "Editor must read compressed bytes, not decoded payload"
                );
                assert_ne!(bytes, PAYLOAD);
            }
            UiMode::Viewer { .. } => panic!("F4 must open Editor, not Viewer"),
            _ => panic!("expected Editor for F4 on .gz"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod diff_edit_in_place_tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use rmc_core::app::DiffState;
    use rmc_core::config::KeyMap;
    use rmc_core::find::FindDialogState;
    use rmc_fs::local::LocalFs;

    fn temp_workspace() -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "rmc-diff-edit-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn make_app(cwd: &std::path::Path) -> App {
        let vfs = LocalFs::new();
        let mut app = App::new(Box::new(vfs), KeyMap::mc_defaults()).unwrap();
        app.config_opts.use_internal_edit = true;
        app.config_opts.use_internal_view = true;
        app.change_dir(cwd).unwrap();
        app
    }

    fn press(app: &mut App, code: KeyCode) {
        TerminalApp::handle_key(app, KeyEvent::new(code, KeyModifiers::NONE), 10).unwrap();
    }

    fn press_mod(app: &mut App, code: KeyCode, mods: KeyModifiers) {
        TerminalApp::handle_key(app, KeyEvent::new(code, mods), 10).unwrap();
    }

    fn press_alt(app: &mut App, c: char) {
        press_mod(app, KeyCode::Char(c), KeyModifiers::ALT);
    }

    fn select_named(app: &mut App, name: &str) {
        let idx = app
            .active_panel()
            .entries
            .iter()
            .position(|e| e.name == name)
            .unwrap_or_else(|| panic!("missing {name}"));
        app.active_panel_mut().cursor = idx;
    }

    fn open_diff(app: &mut App, left: std::path::PathBuf, right: std::path::PathBuf) {
        let ltxt = std::fs::read_to_string(&left).unwrap_or_default();
        let rtxt = std::fs::read_to_string(&right).unwrap_or_default();
        app.ui_mode = UiMode::Diff(DiffState {
            left_path: left,
            right_path: right,
            left_lines: rmc_diff::split_lines(&ltxt),
            right_lines: rmc_diff::split_lines(&rtxt),
            hunks: rmc_diff::compute_diff(&ltxt, &rtxt).hunks,
            current_hunk: 0,
            left_modified: false,
            right_modified: false,
            show_line_numbers: true,
            show_hunk_status: true,
            search: None,
            search_prompt: None,
            goto_prompt: None,
            confirm_exit: None,
            left_scroll: 0,
            right_scroll: 0,
            panel_ratio: 0.6,
            tab_width: 4,
            merge_target_right: true,
        });
    }

    fn diff_state(app: &App) -> &DiffState {
        match &app.ui_mode {
            UiMode::Diff(s) => s,
            _ => panic!("expected Diff"),
        }
    }

    fn non_equal_hunks(state: &DiffState) -> usize {
        state
            .hunks
            .iter()
            .filter(|h| !matches!(h.kind, rmc_diff::HunkKind::Equal))
            .count()
    }

    fn editor_path(app: &App) -> std::path::PathBuf {
        match &app.ui_mode {
            UiMode::Editor { buf, .. } => buf.path.clone().expect("editor path"),
            UiMode::Viewer { .. } => panic!("expected Editor, got Viewer"),
            UiMode::InputDialog { .. } => panic!("expected Editor, got InputDialog"),
            _ => panic!("expected Editor"),
        }
    }

    #[test]
    fn f4_from_diff_opens_editor_on_left_path() {
        let root = temp_workspace();
        let left = root.join("left.txt");
        let right = root.join("right.txt");
        std::fs::write(&left, "aaa\n").unwrap();
        std::fs::write(&right, "bbb\n").unwrap();
        let mut app = make_app(&root);
        open_diff(&mut app, left.clone(), right);
        press(&mut app, KeyCode::F(4));
        assert_eq!(editor_path(&app), left);
        match &app.ui_mode {
            UiMode::Editor {
                replace_dialog,
                search_dialog,
                return_to,
                ..
            } => {
                assert!(replace_dialog.is_none());
                assert!(search_dialog.is_none());
                assert!(matches!(return_to.as_deref(), Some(UiMode::Diff(_))));
            }
            _ => panic!("F4 must open Editor"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn f14_from_diff_opens_editor_on_right_path() {
        let root = temp_workspace();
        let left = root.join("left.txt");
        let right = root.join("right.txt");
        std::fs::write(&left, "aaa\n").unwrap();
        std::fs::write(&right, "bbb\n").unwrap();
        let mut app = make_app(&root);
        open_diff(&mut app, left, right.clone());
        press(&mut app, KeyCode::F(14));
        assert_eq!(editor_path(&app), right);

        press(&mut app, KeyCode::F(10));
        assert!(matches!(app.ui_mode, UiMode::Diff(_)));

        press_mod(&mut app, KeyCode::F(4), KeyModifiers::SHIFT);
        assert_eq!(editor_path(&app), right);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn save_and_quit_returns_to_diff_with_refreshed_hunks() {
        let root = temp_workspace();
        let left = root.join("left.txt");
        let right = root.join("right.txt");
        std::fs::write(&left, "same\n").unwrap();
        std::fs::write(&right, "same\n").unwrap();
        let mut app = make_app(&root);
        open_diff(&mut app, left.clone(), right.clone());
        assert_eq!(non_equal_hunks(diff_state(&app)), 0);

        press(&mut app, KeyCode::F(4));
        press(&mut app, KeyCode::Char('X'));
        press(&mut app, KeyCode::F(10));
        press(&mut app, KeyCode::Enter); // Yes: save
        match &app.ui_mode {
            UiMode::Diff(state) => {
                assert_eq!(state.left_path, left);
                assert_eq!(state.right_path, right);
                assert!(state.show_line_numbers);
                assert!((state.panel_ratio - 0.6).abs() < f32::EPSILON);
                assert!(
                    non_equal_hunks(state) >= 1,
                    "saved edit must appear as a hunk"
                );
            }
            _ => panic!("save+quit must return to Diff"),
        }
        assert_eq!(std::fs::read_to_string(&left).unwrap(), "Xsame\n");
        assert_eq!(std::fs::read_to_string(&right).unwrap(), "same\n");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn save_making_files_equal_clears_hunks() {
        let root = temp_workspace();
        let left = root.join("left.txt");
        let right = root.join("right.txt");
        std::fs::write(&left, "x\n").unwrap();
        std::fs::write(&right, "y\n").unwrap();
        let mut app = make_app(&root);
        open_diff(&mut app, left.clone(), right.clone());
        assert!(non_equal_hunks(diff_state(&app)) >= 1);

        press(&mut app, KeyCode::F(4));
        press(&mut app, KeyCode::Delete);
        press(&mut app, KeyCode::Char('y'));
        press(&mut app, KeyCode::F(2));
        press(&mut app, KeyCode::F(10));
        match &app.ui_mode {
            UiMode::Diff(state) => {
                assert_eq!(
                    non_equal_hunks(state),
                    0,
                    "equal files have no remaining hunks"
                );
            }
            _ => panic!("expected Diff after clean quit"),
        }
        assert_eq!(std::fs::read_to_string(&left).unwrap(), "y\n");
        assert_eq!(std::fs::read_to_string(&right).unwrap(), "y\n");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn discard_dirty_returns_to_diff_with_hunks_unchanged() {
        let root = temp_workspace();
        let left = root.join("left.txt");
        let right = root.join("right.txt");
        std::fs::write(&left, "aaa\n").unwrap();
        std::fs::write(&right, "bbb\n").unwrap();
        let mut app = make_app(&root);
        open_diff(&mut app, left.clone(), right.clone());
        let before = diff_state(&app).hunks.clone();

        press(&mut app, KeyCode::F(4));
        press(&mut app, KeyCode::Char('Z'));
        press(&mut app, KeyCode::F(10));
        press(&mut app, KeyCode::Right); // Yes -> No
        press(&mut app, KeyCode::Enter);
        match &app.ui_mode {
            UiMode::Diff(state) => {
                assert_eq!(state.hunks, before);
            }
            _ => panic!("discard must return to Diff"),
        }
        assert_eq!(std::fs::read_to_string(&left).unwrap(), "aaa\n");
        assert_eq!(std::fs::read_to_string(&right).unwrap(), "bbb\n");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn swap_merge_search_work_after_return() {
        let root = temp_workspace();
        let left = root.join("left.txt");
        let right = root.join("right.txt");
        std::fs::write(&left, "aaa\n").unwrap();
        std::fs::write(&right, "bbb\n").unwrap();
        let mut app = make_app(&root);
        open_diff(&mut app, left.clone(), right.clone());

        press(&mut app, KeyCode::F(4));
        press(&mut app, KeyCode::Char('X'));
        press(&mut app, KeyCode::F(10));
        press(&mut app, KeyCode::Enter); // save
        assert!(matches!(app.ui_mode, UiMode::Diff(_)));

        press_mod(&mut app, KeyCode::Char('u'), KeyModifiers::CONTROL);
        match &app.ui_mode {
            UiMode::Diff(state) => {
                assert_eq!(state.left_path, right);
                assert_eq!(state.right_path, left);
                assert!(!state.merge_target_right);
            }
            _ => panic!("C-u must stay in Diff"),
        }

        press(&mut app, KeyCode::F(5));
        match &app.ui_mode {
            UiMode::Diff(state) => {
                assert!(state.left_modified, "F5 merge into current left after swap");
            }
            _ => panic!("F5 must stay in Diff"),
        }

        press(&mut app, KeyCode::F(7));
        match &app.ui_mode {
            UiMode::Diff(state) => {
                assert!(state.search_prompt.is_some());
            }
            _ => panic!("F7 must open Diff search"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn panel_f4_find_history_viewer_unaffected() {
        let root = temp_workspace();
        let file = root.join("notes.txt");
        std::fs::write(&file, "hello\n").unwrap();
        let mut app = make_app(&root);

        select_named(&mut app, "notes.txt");
        press(&mut app, KeyCode::F(4));
        match &app.ui_mode {
            UiMode::Editor { buf, return_to, .. } => {
                assert_eq!(buf.path.as_deref(), Some(file.as_path()));
                assert!(return_to.is_none(), "panel F4 must not nest Diff");
            }
            UiMode::Diff(_) => panic!("panel F4 must open Editor, not Diff"),
            UiMode::Viewer { .. } => panic!("panel F4 must open Editor, not Viewer"),
            _ => panic!("panel F4 must open Editor"),
        }
        press(&mut app, KeyCode::F(10));
        assert!(matches!(app.ui_mode, UiMode::Normal));

        app.ui_mode = UiMode::FindDialog(FindDialogState::new(root.clone()));
        press(&mut app, KeyCode::F(4));
        assert!(
            matches!(app.ui_mode, UiMode::FindDialog(_)),
            "Find File F4 must not steal the Diff edit path"
        );
        press(&mut app, KeyCode::Esc);

        app.ui_mode = UiMode::ShellInput;
        press_alt(&mut app, 'h');
        assert!(matches!(app.ui_mode, UiMode::HistoryDialog { .. }));
        press(&mut app, KeyCode::F(4));
        assert!(
            matches!(app.ui_mode, UiMode::HistoryDialog { .. }),
            "History F4 must not open Diff editor"
        );
        press(&mut app, KeyCode::Esc);

        app.ui_mode = UiMode::Normal;
        select_named(&mut app, "notes.txt");
        press(&mut app, KeyCode::F(3));
        match &app.ui_mode {
            UiMode::Viewer { hex, .. } => assert!(!*hex),
            _ => panic!("F3 must open Viewer"),
        }
        press(&mut app, KeyCode::F(4));
        match &app.ui_mode {
            UiMode::Viewer { hex, .. } => assert!(*hex, "Viewer F4 toggles hex"),
            UiMode::Editor { .. } => panic!("Viewer F4 must not open Editor"),
            _ => panic!("Viewer F4 must stay in Viewer"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod viewer_selection_keybindings_tests {
    use super::*;
    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use rmc_core::config::KeyMap;
    use rmc_core::find::FindDialogState;
    use rmc_fs::local::LocalFs;

    fn temp_workspace() -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "rmc-viewer-keys-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn make_app() -> App {
        let vfs = LocalFs::new();
        App::new(Box::new(vfs), KeyMap::mc_defaults()).unwrap()
    }

    fn make_app_at(cwd: &std::path::Path) -> App {
        let vfs = LocalFs::new();
        let mut app = App::new(Box::new(vfs), KeyMap::mc_defaults()).unwrap();
        app.config_opts.use_internal_view = true;
        app.config_opts.use_internal_edit = true;
        app.change_dir(cwd).unwrap();
        app
    }

    fn open_viewer(app: &mut App, path: std::path::PathBuf) {
        app.ui_mode = UiMode::new_viewer(path);
    }

    fn press(app: &mut App, code: KeyCode) {
        TerminalApp::handle_key(app, KeyEvent::new(code, KeyModifiers::NONE), 10).unwrap();
    }

    fn press_mod(app: &mut App, code: KeyCode, mods: KeyModifiers) {
        TerminalApp::handle_key(app, KeyEvent::new(code, mods), 10).unwrap();
    }

    fn many_lines() -> String {
        (0..80).map(|i| format!("line-{i:02} abcdefgh\n")).collect()
    }

    fn decoded_bytes_path(display: &std::path::Path) -> Vec<u8> {
        let p = viewer_ensure_view_for(display);
        std::fs::read(p).unwrap()
    }

    #[test]
    fn f5_goto_stays_in_viewer_not_editor() {
        let root = temp_workspace();
        let file = root.join("notes.txt");
        std::fs::write(&file, many_lines()).unwrap();
        let mut app = make_app();
        open_viewer(&mut app, file);
        press(&mut app, KeyCode::F(5));
        match &app.ui_mode {
            UiMode::Viewer {
                goto_prompt: Some(_),
                ..
            } => {}
            UiMode::Editor { .. } => panic!("F5 must not open Editor"),
            _ => panic!("F5 must open viewer Goto"),
        }
        press(&mut app, KeyCode::Esc);
        match &app.ui_mode {
            UiMode::Viewer { goto_prompt, .. } => assert!(goto_prompt.is_none()),
            _ => panic!("Esc closes Goto, stays in Viewer"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn space_pages_down_backspace_pages_up() {
        let root = temp_workspace();
        let file = root.join("notes.txt");
        std::fs::write(&file, many_lines()).unwrap();
        let mut app = make_app();
        open_viewer(&mut app, file);
        let start = match &app.ui_mode {
            UiMode::Viewer { offset, .. } => *offset,
            _ => panic!("expected Viewer"),
        };
        press(&mut app, KeyCode::Char(' '));
        let mid = match &app.ui_mode {
            UiMode::Viewer { offset, .. } => *offset,
            _ => panic!("expected Viewer"),
        };
        assert!(mid > start, "Space pages down, got {mid} from {start}");
        press(&mut app, KeyCode::Backspace);
        let back = match &app.ui_mode {
            UiMode::Viewer { offset, .. } => *offset,
            _ => panic!("expected Viewer"),
        };
        assert!(back < mid, "Backspace pages up, got {back} from {mid}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn shift_arrows_set_and_clear_selection() {
        let root = temp_workspace();
        let file = root.join("notes.txt");
        std::fs::write(&file, many_lines()).unwrap();
        let mut app = make_app();
        open_viewer(&mut app, file);
        press_mod(&mut app, KeyCode::Down, KeyModifiers::SHIFT);
        match &app.ui_mode {
            UiMode::Viewer {
                sel_anchor,
                sel_cursor,
                ..
            } => {
                assert!(sel_anchor.is_some(), "Shift+Down sets a selection");
                assert!(*sel_cursor > 0);
                let a = sel_anchor.unwrap();
                assert_ne!(a, *sel_cursor);
            }
            _ => panic!("expected Viewer"),
        }
        press(&mut app, KeyCode::Down);
        match &app.ui_mode {
            UiMode::Viewer { sel_anchor, .. } => {
                assert!(sel_anchor.is_none(), "plain Down clears selection");
            }
            _ => panic!("expected Viewer"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn f2_wrap_f4_hex_f7_search_f9_format_f10_quit() {
        let root = temp_workspace();
        let file = root.join("notes.txt");
        std::fs::write(&file, many_lines()).unwrap();
        let mut app = make_app();
        open_viewer(&mut app, file);
        press(&mut app, KeyCode::F(2));
        match &app.ui_mode {
            UiMode::Viewer { wrap, hex, .. } => {
                assert!(*wrap);
                assert!(!*hex);
            }
            _ => panic!("F2 wrap"),
        }
        press(&mut app, KeyCode::F(4));
        match &app.ui_mode {
            UiMode::Viewer { hex, .. } => assert!(*hex),
            UiMode::Editor { .. } => panic!("F4 must not open Editor"),
            _ => panic!("F4 hex"),
        }
        press(&mut app, KeyCode::F(4));
        press(&mut app, KeyCode::F(7));
        match &app.ui_mode {
            UiMode::Viewer {
                search_dialog: Some(_),
                ..
            } => {}
            _ => panic!("F7 Search"),
        }
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::F(9));
        match &app.ui_mode {
            UiMode::Viewer {
                format_nroff,
                display_dialog,
                ..
            } => {
                assert!(*format_nroff);
                assert!(display_dialog.is_none());
            }
            _ => panic!("F9 format"),
        }
        press(&mut app, KeyCode::F(10));
        assert!(matches!(app.ui_mode, UiMode::Normal));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn f8_toggles_raw_parsed_on_gz() {
        if !rmc_view::helper_on_path("gzip") {
            return;
        }
        let root = temp_workspace();
        let txt = root.join("notes.txt");
        std::fs::write(&txt, b"decoded-payload\n").unwrap();
        let gz = root.join("notes.txt.gz");
        let out = std::fs::File::create(&gz).unwrap();
        let ok = std::process::Command::new("gzip")
            .arg("-c")
            .arg(&txt)
            .stdout(std::process::Stdio::from(out))
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            let _ = std::fs::remove_dir_all(&root);
            return;
        }
        let mut app = make_app_at(&root);
        let idx = app
            .active_panel()
            .entries
            .iter()
            .position(|e| e.name == "notes.txt.gz")
            .unwrap();
        app.active_panel_mut().cursor = idx;
        view_current_file(&mut app).unwrap();
        let parsed_bytes = decoded_bytes_path(&gz);
        assert_eq!(parsed_bytes, b"decoded-payload\n");
        press(&mut app, KeyCode::F(8));
        match &app.ui_mode {
            UiMode::Viewer { parsed, .. } => assert!(!*parsed, "F8 enters Raw"),
            _ => panic!("expected Viewer"),
        }
        let raw = decoded_bytes_path(&gz);
        assert!(
            raw.starts_with(&[0x1f, 0x8b]),
            "Raw shows gzip magic, got {raw:?}"
        );
        press(&mut app, KeyCode::F(8));
        match &app.ui_mode {
            UiMode::Viewer { parsed, .. } => assert!(*parsed, "F8 returns to Parsed"),
            _ => panic!("expected Viewer"),
        }
        assert_eq!(decoded_bytes_path(&gz), b"decoded-payload\n");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn panel_f4_diff_f4_find_file_f4_unaffected() {
        let root = temp_workspace();
        let file = root.join("notes.txt");
        std::fs::write(&file, "hello\n").unwrap();
        let other = root.join("other.txt");
        std::fs::write(&other, "world\n").unwrap();
        let mut app = make_app_at(&root);

        let idx = app
            .active_panel()
            .entries
            .iter()
            .position(|e| e.name == "notes.txt")
            .unwrap();
        app.active_panel_mut().cursor = idx;
        press(&mut app, KeyCode::F(4));
        match &app.ui_mode {
            UiMode::Editor { .. } => {}
            UiMode::Viewer { .. } => panic!("panel F4 must open Editor"),
            _ => panic!("panel F4 must open Editor"),
        }
        press(&mut app, KeyCode::F(10));

        app.ui_mode = UiMode::FindDialog(FindDialogState::new(root.clone()));
        press(&mut app, KeyCode::F(4));
        assert!(
            matches!(app.ui_mode, UiMode::FindDialog(_)),
            "Find File F4 unaffected"
        );
        press(&mut app, KeyCode::Esc);

        let left_lines = rmc_diff::split_lines("hello\n");
        let right_lines = rmc_diff::split_lines("world\n");
        let hunks = rmc_diff::compute_diff("hello\n", "world\n").hunks;
        app.ui_mode = UiMode::Diff(rmc_core::app::DiffState {
            left_path: file.clone(),
            right_path: other,
            left_lines,
            right_lines,
            hunks,
            current_hunk: 0,
            left_modified: false,
            right_modified: false,
            show_line_numbers: false,
            show_hunk_status: true,
            search: None,
            search_prompt: None,
            goto_prompt: None,
            confirm_exit: None,
            left_scroll: 0,
            right_scroll: 0,
            panel_ratio: 0.5,
            tab_width: 4,
            merge_target_right: true,
        });
        press(&mut app, KeyCode::F(4));
        match &app.ui_mode {
            UiMode::Editor { return_to, .. } => {
                assert!(return_to.is_some(), "Diff F4 nests editor");
            }
            UiMode::Viewer { .. } => panic!("Diff F4 must not open Viewer"),
            UiMode::Diff(_) => panic!("Diff F4 must open editor in place"),
            _ => panic!("Diff F4 must open Editor"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn click_options_menu_opens_display_not_f9() {
        let root = temp_workspace();
        let file = root.join("notes.txt");
        std::fs::write(&file, "abc").unwrap();
        let mut app = make_app();
        open_viewer(&mut app, file);
        TerminalApp::handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 18,
                row: 0,
                modifiers: KeyModifiers::NONE,
            },
            10,
        )
        .unwrap();
        match &app.ui_mode {
            UiMode::Viewer {
                viewer_menu: Some(rmc_core::app::ViewerMenu::Options { .. }),
                display_dialog,
                ..
            } => {
                assert!(display_dialog.is_none());
            }
            _ => panic!("click Options must drop the Options menu"),
        }
        press(&mut app, KeyCode::Enter);
        match &app.ui_mode {
            UiMode::Viewer {
                display_dialog: Some(_),
                viewer_menu,
                ..
            } => {
                assert!(viewer_menu.is_none());
            }
            _ => panic!("Enter on Display options must open the dialog"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
