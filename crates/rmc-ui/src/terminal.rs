use crate::help::{global_index, HelpItem};
use crate::render::Renderer;
use crate::skin::load_default_palette;
use anyhow::Result;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use rmc_core::actions::{Action, PaneSide};
use rmc_core::app::{App, LayoutFocus, UiMode};
use rmc_core::find::{
    search_files_streaming, CancelHandle, FindDialogFocus as FF, FindDialogState,
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
        let view = rmc_view::ViewData::open_view(display_path)
            .unwrap_or_else(|_| rmc_view::ViewData::from_path(display_path.to_path_buf()));
        let p = view.path().to_path_buf();
        *g = Some(ViewerState {
            display_path: display_path.to_path_buf(),
            view,
        });
        return p;
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
            let content_rows = panel_h.saturating_sub(4) as usize;
            // Compute per-panel visible capacity (rows or 2*rows for Brief two-column)
            let mid = cols / 2;
            let left_w = mid;
            let right_w = cols - mid;
            let left_two_cols =
                matches!(app.left.listing, rmc_core::panel::ListingFormat::Brief) && left_w >= 30;
            let right_two_cols =
                matches!(app.right.listing, rmc_core::panel::ListingFormat::Brief) && right_w >= 30;
            let left_capacity = content_rows * if left_two_cols { 2 } else { 1 };
            let right_capacity = content_rows * if right_two_cols { 2 } else { 1 };
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
            if last_draw.elapsed() > Duration::from_millis(33) {
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
                        // Ignore mouse outside Normal panels mode and while subshell full-screen
                        if app.subshell.show_output_screen {
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
                            let content_h = ph.saturating_sub(4);
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
                                // Apply: write draft into app.keymap
                                for (act, keyev) in draft.iter().cloned() {
                                    app.keymap.set_binding(keyev, act);
                                }
                                app.ui_mode = UiMode::Normal;
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
                show_menu: _,
                status_msg,
                search_input,
                save_as_input,
                pending_quit: _,
                confirm_exit,
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
                                    // Save and exit to panels
                                    if let Some(path) = &buf.path {
                                        let mut w = app
                                            .vfs
                                            .write_file(path)
                                            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
                                        use std::io::Write;
                                        let _ = w.write_all(&buf.to_bytes());
                                    }
                                    app.ui_mode = UiMode::Normal;
                                }
                                F::No => {
                                    app.ui_mode = UiMode::Normal;
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
                // Inline "Save as:" overlay used here for Replace-with text as well
                if let Some(val) = save_as_input {
                    // Disambiguate by status message hint (very lightweight)
                    let replacing = status_msg
                        .as_deref()
                        .is_some_and(|s| s.starts_with("Replace with"));
                    if replacing {
                        match key.code {
                            KeyCode::Esc | KeyCode::F(10) => {
                                *save_as_input = None;
                                *status_msg = None;
                            }
                            KeyCode::Enter => {
                                let repl = val.clone();
                                if !buf.last_search.is_empty() {
                                    let find = buf.last_search.clone();
                                    let ci = buf.last_search_case_insensitive;
                                    let _ = buf.replace_next(&find, repl.as_bytes(), ci, true);
                                    *status_msg = Some("Replaced".into());
                                }
                                *save_as_input = None;
                            }
                            KeyCode::Char('a')
                                if key.modifiers.contains(crossterm::event::KeyModifiers::ALT) =>
                            {
                                let repl = val.clone();
                                if !buf.last_search.is_empty() {
                                    let find = buf.last_search.clone();
                                    let ci = buf.last_search_case_insensitive;
                                    let n = buf.replace_all(&find, repl.as_bytes(), ci);
                                    *status_msg = Some(format!("Replaced all: {n}"));
                                }
                                *save_as_input = None;
                            }
                            KeyCode::Backspace => {
                                val.pop();
                            }
                            KeyCode::Char(c) if key.modifiers.is_empty() => {
                                val.push(c);
                            }
                            _ => {}
                        }
                        return Ok(());
                    } else {
                        // Save-as path input
                        match key.code {
                            KeyCode::Esc | KeyCode::F(10) => {
                                *save_as_input = None;
                            }
                            KeyCode::Enter => {
                                let p = std::path::PathBuf::from(val.clone());
                                let mut w = app
                                    .vfs
                                    .write_file(&p)
                                    .map_err(|e| anyhow::anyhow!(e.to_string()))?;
                                use std::io::Write;
                                let _ = w.write_all(&buf.to_bytes());
                                buf.path = Some(p);
                                buf.dirty = false;
                                *save_as_input = None;
                                *status_msg = Some("Saved".into());
                            }
                            KeyCode::Backspace => {
                                val.pop();
                            }
                            KeyCode::Char(c) if key.modifiers.is_empty() => {
                                val.push(c);
                            }
                            _ => {}
                        }
                        return Ok(());
                    }
                }
                // Base editor keys
                match key.code {
                    // MC: F7 search dialog
                    KeyCode::F(7) => {
                        *search_input = Some(String::new());
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
                    // F4 Replace: prompt find if empty, otherwise prompt replacement
                    KeyCode::F(4) => {
                        if buf.last_search.is_empty() {
                            *search_input = Some(String::new());
                            *status_msg = Some("Find (Enter to confirm)".into());
                        } else {
                            *save_as_input = Some(String::new());
                            *status_msg = Some("Replace with (Enter=Replace, Alt-a=All)".into());
                        }
                    }
                    // Pipe selection (or whole buffer) through external command (GNU mcedit behavior)
                    KeyCode::Char('|') => {
                        let buf_snapshot = buf.clone();
                        app.ui_mode = UiMode::InputDialog {
                            title: "Pipe command".into(),
                            prompt: "Enter shell command:".into(),
                            value: String::new(),
                            focus_ok: false,
                            on_submit: Box::new(move |app, input| {
                                let cmd = input.trim();
                                // Always return to editor; empty command is a no-op.
                                if cmd.is_empty() {
                                    app.ui_mode = UiMode::Editor {
                                        buf: buf_snapshot,
                                        show_menu: false,
                                        status_msg: None,
                                        search_input: None,
                                        save_as_input: None,
                                        pending_quit: false,
                                        confirm_exit: None,
                                    };
                                    return Ok(());
                                }
                                // Apply pipe to a working copy of the buffer; on success update editor, on error show dialog and restore.
                                let mut new_buf = buf_snapshot.clone();
                                if let Err(e) = new_buf.pipe_selection(cmd) {
                                    let restore_buf = buf_snapshot;
                                    app.ui_mode = UiMode::DialogConfirm {
                                        title: "Error".into(),
                                        message: format!("{e}"),
                                        on_ok: Box::new(move |app| {
                                            app.ui_mode = UiMode::Editor {
                                                buf: restore_buf,
                                                show_menu: false,
                                                status_msg: None,
                                                search_input: None,
                                                save_as_input: None,
                                                pending_quit: false,
                                                confirm_exit: None,
                                            };
                                            Ok(())
                                        }),
                                    };
                                } else {
                                    app.ui_mode = UiMode::Editor {
                                        buf: new_buf,
                                        show_menu: false,
                                        status_msg: None,
                                        search_input: None,
                                        save_as_input: None,
                                        pending_quit: false,
                                        confirm_exit: None,
                                    };
                                }
                                Ok(())
                            }),
                        };
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
                    // Save / Quit
                    KeyCode::F(2) => {
                        if let Some(path) = &buf.path {
                            let mut w = app
                                .vfs
                                .write_file(path)
                                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
                            use std::io::Write;
                            let _ = w.write_all(&buf.to_bytes());
                            buf.dirty = false;
                            *status_msg = Some("Saved".into());
                        } else {
                            *save_as_input = Some(String::new());
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
                            app.ui_mode = UiMode::Normal;
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
                            app.handle_action(Action::Enter)?;
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
                                app.hotlist.remove_at(state.selected_index);
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
                        let user_val = user.trim();
                        let pass_val = password.clone(); // allow empty
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
                    KeyCode::Esc => {
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
                        state.focus = match state.focus {
                            FF::StartDir => FF::NamePattern,
                            FF::NamePattern => FF::Content,
                            FF::Content => FF::CaseSensitive,
                            FF::CaseSensitive => FF::ButtonStart,
                            FF::ButtonStart => FF::ButtonStop,
                            FF::ButtonStop => FF::ButtonChdir,
                            FF::ButtonChdir => FF::ButtonAgain,
                            FF::ButtonAgain => FF::ButtonPanelize,
                            FF::ButtonPanelize => FF::ButtonQuit,
                            FF::ButtonQuit => FF::StartDir,
                        };
                    }
                    KeyCode::Up => {
                        if state.selected_index > 0 {
                            state.selected_index -= 1;
                        }
                        // ensure visible
                        let (_c, r) = crossterm::terminal::size()?;
                        let h = r.saturating_sub(4).clamp(16, 22);
                        let list_rows = (h - 12) as usize;
                        if state.selected_index < state.scroll_top {
                            state.scroll_top = state.selected_index;
                        } else if state.selected_index >= state.scroll_top + list_rows {
                            state.scroll_top = state
                                .selected_index
                                .saturating_sub(list_rows.saturating_sub(1));
                        }
                    }
                    KeyCode::Down => {
                        if state.selected_index + 1 < state.results.paths.len() {
                            state.selected_index += 1;
                        }
                        let (_c, r) = crossterm::terminal::size()?;
                        let h = r.saturating_sub(4).clamp(16, 22);
                        let list_rows = (h - 12) as usize;
                        if state.selected_index < state.scroll_top {
                            state.scroll_top = state.selected_index;
                        } else if state.selected_index >= state.scroll_top + list_rows {
                            state.scroll_top = state
                                .selected_index
                                .saturating_sub(list_rows.saturating_sub(1));
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
                            let h = r.saturating_sub(4).clamp(16, 22);
                            let list_rows = (h - 12) as usize;
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
                                FF::CaseSensitive if c == ' ' => {
                                    state.params.case_sensitive = !state.params.case_sensitive;
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
                                // If destination exists, open overwrite dialog; else perform action
                                let dst = Path::new(&*to).to_path_buf();
                                let exists = app.vfs.stat(&dst).is_ok();
                                if exists {
                                    if app.confirm.overwrite {
                                        let op = if title == "Copy" {
                                            rmc_core::app::CopyMoveOp::Copy
                                        } else {
                                            rmc_core::app::CopyMoveOp::Move
                                        };
                                        app.ui_mode = UiMode::OverwriteDialog {
                                            op,
                                            src_path: src_path.clone(),
                                            dst_path: dst,
                                            focus: rmc_core::app::OverwriteFocus::Yes,
                                        };
                                    } else {
                                        // Perform "Yes" path: remove destination then copy/move
                                        let _ = app.vfs.remove(&dst, false);
                                        let res = if title == "Copy" {
                                            app.vfs.copy(src_path, &dst)
                                        } else {
                                            app.vfs.move_path(src_path, &dst)
                                        };
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
                                    }
                                } else {
                                    if title == "Copy" {
                                        app.vfs.copy(src_path, Path::new(&*to))?;
                                    } else {
                                        app.vfs.move_path(src_path, Path::new(&*to))?;
                                    }
                                    app.ui_mode = UiMode::Normal;
                                    app.reload_panels()?;
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
                            let _ = app.vfs.remove(dst_path, false);
                            let res = match *op {
                                rmc_core::app::CopyMoveOp::Copy => app.vfs.copy(src_path, dst_path),
                                rmc_core::app::CopyMoveOp::Move => {
                                    app.vfs.move_path(src_path, dst_path)
                                }
                            };
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
                    &["Layout", "Panels", "Confirmations", "Learn keys", "Save setup"],
                    &[
                        "Copy",
                        "Move",
                        "Mkdir",
                        "Delete",
                        "FTP link",
                        "Shell link",
                        "SFTP link",
                        "SMB link",
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
                        if *selected_index > 0 {
                            *selected_index -= 1;
                        }
                    }
                    KeyCode::Down => {
                        let max = menus[*top_index].len().saturating_sub(1);
                        if *selected_index < max {
                            *selected_index += 1;
                        }
                    }
                    KeyCode::Enter => {
                        let item = menus[*top_index][*selected_index];
                        match item {
                            "Layout" => {
                                // Prefill dialog from current options
                                let draft = app.layout;
                                app.ui_mode = UiMode::LayoutDialog {
                                    draft,
                                    focus: LayoutFocus::MenuBar,
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
                                        draft.push((act.clone(), crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Char('?'), crossterm::event::KeyModifiers::NONE)));
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
                    KeyCode::F(4) => {
                        // Disabled for now to avoid losing diff session; leave PARITY unchecked
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
                // Then handle search overlay
                if let UiMode::Viewer {
                    path,
                    offset,
                    search,
                    search_prompt: Some(prompt),
                    ..
                } = &mut app.ui_mode
                {
                    match key.code {
                        KeyCode::Esc | KeyCode::F(10) => {
                            *prompt = String::new();
                            if let UiMode::Viewer { search_prompt, .. } = &mut app.ui_mode {
                                *search_prompt = None;
                            }
                        }
                        KeyCode::Enter => {
                            let q = prompt.clone();
                            let cpath = crate::terminal::viewer_ensure_view_for(path);
                            if let Some(pos) = rmc_view::search_forward(&cpath, *offset, &q)? {
                                *offset = pos;
                            }
                            *search = if q.is_empty() { None } else { Some(q) };
                            if let UiMode::Viewer { search_prompt, .. } = &mut app.ui_mode {
                                *search_prompt = None;
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
                match key.code {
                    KeyCode::Char('q') | KeyCode::F(3) | KeyCode::F(10) => {
                        app.handle_action(Action::ViewerQuit)?
                    }
                    KeyCode::Char('h') | KeyCode::Char('x') | KeyCode::F(4) => {
                        app.handle_action(Action::ViewerToggleHex)?
                    }
                    KeyCode::F(5) | KeyCode::Char('g') => {
                        if let UiMode::Viewer { goto_prompt, .. } = &mut app.ui_mode {
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
                                } else if let UiMode::Viewer { offset, search, .. } =
                                    &mut app.ui_mode
                                {
                                    *offset = 0;
                                    *search = None;
                                }
                                Ok(())
                            }),
                        };
                    }
                    KeyCode::F(2) => {
                        // Save — no-op stub for now
                    }
                    KeyCode::Char('w') => {
                        if let UiMode::Viewer { hex, wrap, .. } = &mut app.ui_mode {
                            if !*hex {
                                *wrap = !*wrap;
                            }
                        }
                    }
                    KeyCode::Char('l') => {
                        if let UiMode::Viewer {
                            show_line_numbers, ..
                        } = &mut app.ui_mode
                        {
                            *show_line_numbers = !*show_line_numbers;
                        }
                    }
                    KeyCode::Char('r') => {
                        if let UiMode::Viewer { show_cr, .. } = &mut app.ui_mode {
                            *show_cr = !*show_cr;
                        }
                    }
                    KeyCode::Up => {
                        if let UiMode::Viewer {
                            path, hex, offset, ..
                        } = &mut app.ui_mode
                        {
                            if *hex {
                                *offset = offset.saturating_sub(16);
                            } else {
                                let cpath = crate::terminal::viewer_ensure_view_for(path);
                                *offset = rmc_view::nav_line_up(&cpath, *offset)?;
                            }
                        }
                    }
                    KeyCode::Down => {
                        if let UiMode::Viewer {
                            path, hex, offset, ..
                        } = &mut app.ui_mode
                        {
                            if *hex {
                                *offset = offset.saturating_add(16);
                            } else {
                                let cpath = crate::terminal::viewer_ensure_view_for(path);
                                *offset = rmc_view::nav_line_down(&cpath, *offset)?;
                            }
                        }
                    }
                    KeyCode::PageDown => {
                        if let UiMode::Viewer {
                            path,
                            hex,
                            wrap,
                            offset,
                            ..
                        } = &mut app.ui_mode
                        {
                            let (cols, rows) = crossterm::terminal::size()?;
                            // content area inside frame is rows-3, cols-2
                            let content_rows = rows.saturating_sub(3);
                            if *hex {
                                let step = 16u64 * (content_rows as u64);
                                *offset = offset.saturating_add(step);
                            } else {
                                let cpath = crate::terminal::viewer_ensure_view_for(path);
                                *offset = rmc_view::nav_page_down(
                                    &cpath,
                                    *offset,
                                    cols.saturating_sub(2),
                                    content_rows,
                                    *wrap,
                                )?;
                            }
                        }
                    }
                    KeyCode::PageUp => {
                        if let UiMode::Viewer {
                            path,
                            hex,
                            wrap,
                            offset,
                            ..
                        } = &mut app.ui_mode
                        {
                            let (cols, rows) = crossterm::terminal::size()?;
                            let content_rows = rows.saturating_sub(3);
                            if *hex {
                                let step = 16u64 * (content_rows as u64);
                                *offset = offset.saturating_sub(step);
                            } else {
                                let cpath = crate::terminal::viewer_ensure_view_for(path);
                                *offset = rmc_view::nav_page_up(
                                    &cpath,
                                    *offset,
                                    cols.saturating_sub(2),
                                    content_rows,
                                    *wrap,
                                )?;
                            }
                        }
                    }
                    KeyCode::Home => {
                        if let UiMode::Viewer { offset, .. } = &mut app.ui_mode {
                            *offset = rmc_view::nav_home();
                        }
                    }
                    KeyCode::End => {
                        if let UiMode::Viewer {
                            path,
                            hex,
                            wrap,
                            offset,
                            ..
                        } = &mut app.ui_mode
                        {
                            let (cols, rows) = crossterm::terminal::size()?;
                            let content_rows = rows.saturating_sub(3);
                            if *hex {
                                let cpath = crate::terminal::viewer_ensure_view_for(path);
                                let len = rmc_view::file_len(&cpath)?;
                                let page = 16u64 * (content_rows as u64);
                                *offset = len.saturating_sub(page);
                            } else {
                                let cpath = crate::terminal::viewer_ensure_view_for(path);
                                *offset = rmc_view::nav_end(
                                    &cpath,
                                    cols.saturating_sub(2),
                                    content_rows,
                                    *wrap,
                                )?;
                            }
                        }
                    }
                    KeyCode::Char('/') | KeyCode::F(7) => {
                        if let UiMode::Viewer { search_prompt, .. } = &mut app.ui_mode {
                            *search_prompt = Some(String::new());
                        }
                    }
                    KeyCode::Char('n') => {
                        if let UiMode::Viewer {
                            path,
                            offset,
                            search,
                            ..
                        } = &mut app.ui_mode
                        {
                            if let Some(q) = search.clone() {
                                let cpath = crate::terminal::viewer_ensure_view_for(path);
                                if let Some(pos) =
                                    rmc_view::search_forward(&cpath, offset.saturating_add(1), &q)?
                                {
                                    *offset = pos;
                                }
                            }
                        }
                    }
                    KeyCode::Char('N') => {
                        if let UiMode::Viewer {
                            path,
                            offset,
                            search,
                            ..
                        } = &mut app.ui_mode
                        {
                            if let Some(q) = search.clone() {
                                let cpath = crate::terminal::viewer_ensure_view_for(path);
                                if let Some(pos) = rmc_view::search_backward(&cpath, *offset, &q)? {
                                    *offset = pos;
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
                Action::FunctionKey(4) => {
                    // Open editor on selected file (panels Normal mode)
                    if matches!(app.ui_mode, UiMode::Normal) {
                        if let Some(ent) = app.active_panel().current_entry().cloned() {
                            if !ent.is_dir {
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
                                app.ui_mode = UiMode::Editor {
                                    buf,
                                    show_menu: false,
                                    status_msg: None,
                                    search_input: None,
                                    save_as_input: None,
                                    pending_quit: false,
                                    confirm_exit: None,
                                };
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
                    app.ui_mode = UiMode::MkdirDialog {
                        value: String::new(),
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
