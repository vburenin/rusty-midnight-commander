use crate::mc_colors::McPalette;
use crate::widgets::Painter;
use anyhow::Result;
use crossterm::style::Color;
use crossterm::terminal::{self, Clear, ClearType};
use crossterm::QueueableCommand;
use rmc_core::app::App;
use rmc_core::panel::FileEntry;
use std::io::{stdout, Stdout};
use time::OffsetDateTime;

pub struct Renderer {
    palette: McPalette,
    out: Stdout,
}

impl Renderer {
    pub fn new(palette: McPalette) -> Self {
        Self { palette, out: stdout() }
    }

    pub fn draw(&mut self, app: &App) -> Result<()> {
        let (cols, rows) = terminal::size()?;
        let mut painter = Painter { out: &mut self.out };
        // Clear to panel background (blue)
        painter.out.queue(Clear(ClearType::All))?;
        painter.fill_line(0, cols, self.palette.core_default_bg, self.palette.core_default_fg);
        // Menu bar
        draw_menu_bar(&mut painter, cols, self.palette);
        // Panels area layout:
        // rows: 1 menu + 1 frame top + content + frame bottom + 1 gauge + 1 hint + 1 cmd + 1 fbar
        let panel_top = 1;
        let gauge_row = rows.saturating_sub(4);
        let hint_row = rows.saturating_sub(3);
        let cmd_row = rows.saturating_sub(2);
        let fbar_row = rows.saturating_sub(1);
        let content_bottom = gauge_row.saturating_sub(1);
        // Split columns
        let mid = cols / 2;
        draw_panel(
            &mut painter,
            0,
            panel_top,
            mid,
            content_bottom - panel_top,
            true,
            app,
            true,
            self.palette,
        )?;
        draw_panel(
            &mut painter,
            mid,
            panel_top,
            cols - mid,
            content_bottom - panel_top,
            false,
            app,
            false,
            self.palette,
        )?;
        // Gauge/status line between panels
        draw_gauge(&mut painter, gauge_row, cols, self.palette, app);
        draw_hint(&mut painter, hint_row, cols, self.palette);
        draw_cmdline(&mut painter, cmd_row, cols, self.palette, app);
        draw_fbar(&mut painter, fbar_row, cols, self.palette);
        // Overlays (dialogs/viewer)
        draw_overlays(&mut painter, app, cols, rows, self.palette)?;
        painter.out.flush()?;
        Ok(())
    }
}

fn draw_menu_bar(p: &mut Painter, cols: u16, pal: McPalette) {
    p.set_fg_bg(pal.menu_fg, pal.menu_bg);
    p.goto(0, 0);
    let items = [" Left ", " File ", " Command ", " Options ", " Right "];
    let mut x = 0u16;
    for it in items.iter() {
        p.goto(x, 0);
        p.text(it);
        x += it.len() as u16;
    }
    // Fill rest
    p.goto(x, 0);
    let rest = " ".repeat(cols.saturating_sub(x) as usize);
    p.text(&rest);
}

fn draw_overlays(p: &mut Painter, app: &App, cols: u16, rows: u16, pal: McPalette) -> Result<()> {
    match &app.ui_mode {
        rmc_core::app::UiMode::DialogConfirm { title, message, .. } => {
            draw_dialog_box(p, cols, rows, pal, title, message, &["< OK >", "Cancel"]);
        }
        rmc_core::app::UiMode::PromptInput { title, value, .. } => {
            let msg = value.to_string();
            draw_dialog_box(p, cols, rows, pal, title, &msg, &["< OK >", "Cancel"]);
        }
        rmc_core::app::UiMode::MkdirDialog { value, focus_ok } => {
            draw_mkdir_dialog(p, cols, rows, pal, value, *focus_ok);
        }
        rmc_core::app::UiMode::DeleteDialog { name, .. } => {
            draw_dialog_box(p, cols, rows, pal, "Delete", &format!("Delete \"{name}\"?"), &["< Yes >", "No"]);
        }
        rmc_core::app::UiMode::CopyDialog {
            title, src_name, mask, to, using_shell_patterns, follow_links, preserve_attrs, dive_into_subdir, stable_symlinks, focus, ..
        } => {
            draw_copy_move_dialog(
                p,
                cols,
                rows,
                pal,
                title,
                src_name,
                mask,
                to,
                *using_shell_patterns,
                *follow_links,
                *preserve_attrs,
                *dive_into_subdir,
                *stable_symlinks,
                *focus,
            );
        }
        rmc_core::app::UiMode::Viewer { path, hex } => {
            draw_viewer(p, app, cols, rows, pal, path, *hex)?;
        }
        rmc_core::app::UiMode::Menu { top_index, selected_index } => {
            draw_menu_dropdown(p, pal, *top_index, *selected_index);
        }
        _ => {}
    }
    Ok(())
}

fn draw_dialog_box(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    title: &str,
    message: &str,
    buttons: &[&str],
) {
    // Centered box
    let w = (cols as usize).min(60) as u16;
    let h = 7u16;
    let x = (cols - w) / 2;
    let y = (rows - h) / 2;
    // Frame
    p.set_fg_bg(pal.frame_fg, pal.dialog_default_bg);
    p.goto(x, y);
    p.text("┌");
    p.hline(x + 1, y, w - 2, '─', pal.frame_fg, pal.dialog_default_bg);
    p.goto(x + w - 1, y);
    p.text("┐");
    p.vline(x, y + 1, h - 2, '│', pal.frame_fg, pal.dialog_default_bg);
    p.vline(x + w - 1, y + 1, h - 2, '│', pal.frame_fg, pal.dialog_default_bg);
    p.goto(x, y + h - 1);
    p.text("└");
    p.hline(x + 1, y + h - 1, w - 2, '─', pal.frame_fg, pal.dialog_default_bg);
    p.goto(x + w - 1, y + h - 1);
    p.text("┘");
    // Title
    p.set_fg_bg(pal.dtitle_fg, pal.dtitle_bg);
    let title_str = format!(" {title} ");
    let tx = x + (w.saturating_sub(title_str.len() as u16)) / 2;
    p.goto(tx, y);
    p.text(&title_str);
    // Message
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(x + 2, y + 2);
    let msg = truncate(message, (w - 4) as usize);
    p.text(&msg);
    // Buttons: centered
    let btns = buttons.join("  ");
    let bx = x + (w.saturating_sub(btns.len() as u16)) / 2;
    p.goto(bx, y + h - 2);
    p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
    p.text(&btns);
    // Shadow
    p.set_fg_bg(pal.shadow_fg, pal.shadow_bg);
    p.hline(x + 1, y + h, w.saturating_sub(1), ' ', pal.shadow_fg, pal.shadow_bg);
    p.vline(x + w, y + 1, h, ' ', pal.shadow_fg, pal.shadow_bg);
}

fn draw_viewer(
    p: &mut Painter,
    app: &App,
    cols: u16,
    rows: u16,
    pal: McPalette,
    path: &std::path::Path,
    hex: bool,
) -> Result<()> {
    // MC-style viewer: blue background with frame and title
    p.set_fg_bg(pal.core_default_fg, pal.core_default_bg);
    for y in 0..rows {
        p.goto(0, y);
        p.text(&" ".repeat(cols as usize));
    }
    // Frame
    p.set_fg_bg(pal.frame_fg, pal.core_default_bg);
    p.goto(0, 0);
    p.text("┌");
    p.hline(1, 0, cols.saturating_sub(2), '─', pal.frame_fg, pal.core_default_bg);
    p.goto(cols - 1, 0);
    p.text("┐");
    p.vline(0, 1, rows.saturating_sub(2), '│', pal.frame_fg, pal.core_default_bg);
    p.vline(cols - 1, 1, rows.saturating_sub(2), '│', pal.frame_fg, pal.core_default_bg);
    p.goto(0, rows - 1);
    p.text("└");
    p.hline(1, rows - 1, cols.saturating_sub(2), '─', pal.frame_fg, pal.core_default_bg);
    p.goto(cols - 1, rows - 1);
    p.text("┘");
    // Title
    let title = format!(" {} ", path.display());
    let tx = (cols.saturating_sub(title.len() as u16)) / 2;
    p.goto(tx, 0);
    p.text(&title);
    // Read file
    let mut reader = app.vfs.read_file(path)?;
    let mut buf = Vec::new();
    use std::io::Read;
    reader.read_to_end(&mut buf)?;
    let lines = if hex {
        bytes_to_hex_lines(&buf)
    } else {
        let s = String::from_utf8_lossy(&buf);
        s.lines().map(|l| l.to_string()).collect::<Vec<_>>()
    };
    let max_lines = rows.saturating_sub(2) as usize;
    for (i, line) in lines.into_iter().take(max_lines).enumerate() {
        p.goto(1, 1 + i as u16);
        let t = truncate(&line, cols as usize);
        p.text(&t);
    }
    // Footer/status
    p.set_fg_bg(pal.statusbar_fg, pal.statusbar_bg);
    p.goto(0, rows - 1);
    let foot = format!(" {}  {}  Press q to quit, h to toggle hex ", path.display(), if hex { "[HEX]" } else { "[TEXT]" });
    let t = truncate(&foot, cols as usize);
    p.text(&t);
    Ok(())
}

fn bytes_to_hex_lines(data: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    for chunk in data.chunks(16).take(1024) {
        let hexs: Vec<String> = chunk.iter().map(|b| format!("{b:02X}")).collect();
        let text: String = chunk
            .iter()
            .map(|&b| if (32..=126).contains(&b) { b as char } else { '.' })
            .collect();
        out.push(format!("{:47}  {}", hexs.join(" "), text));
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn draw_panel(
    p: &mut Painter,
    x: u16,
    y: u16,
    w: u16,
    h: u16,
    _left: bool,
    app: &App,
    is_left: bool,
    pal: McPalette,
) -> Result<()> {
    // Frame single-line, with path caption in the top frame
    let frame_fg = pal.frame_fg;
    let frame_bg = pal.frame_bg;
    // top border
    p.set_fg_bg(frame_fg, frame_bg);
    p.goto(x, y);
    p.text("┌");
    p.hline(x + 1, y, w.saturating_sub(2), '─', frame_fg, frame_bg);
    p.goto(x + w - 1, y);
    p.text("┐");
    // bottom
    p.goto(x, y + h - 1);
    p.text("└");
    p.hline(x + 1, y + h - 1, w.saturating_sub(2), '─', frame_fg, frame_bg);
    p.goto(x + w - 1, y + h - 1);
    p.text("┘");
    // sides
    p.vline(x, y + 1, h.saturating_sub(2), '│', frame_fg, frame_bg);
    p.vline(x + w - 1, y + 1, h.saturating_sub(2), '│', frame_fg, frame_bg);
    // caption path in top frame
    let path = if is_left { &app.left.cwd } else { &app.right.cwd };
    let path_str = format!(" {} ", path.display());
    let cap_x = x + ((w.saturating_sub(path_str.len() as u16)) / 2);
    p.goto(cap_x.max(x + 1), y);
    p.text(&path_str);

    // Headers
    let header_fg = pal.header_fg;
    let header_bg = pal.header_bg;
    p.set_fg_bg(header_fg, header_bg);
    p.goto(x + 1, y + 1);
    p.text("Name");
    let size_col = x + w / 2;
    p.goto(size_col, y + 1);
    p.text("Size");
    p.goto(x + w - 15, y + 1);
    p.text("Modify time");

    // Content rows
    let content_top = y + 2;
    let content_h = h.saturating_sub(4);
    let _panel = if is_left { &app.left } else { &app.right };
    // Viewport uses panel.scroll_top, updated by the event loop per content height
    let panel = if is_left { &app.left } else { &app.right };
        for i in 0..content_h as usize {
        let row_y = content_top + i as u16;
        // Clear row
        p.set_fg_bg(pal.core_default_fg, pal.core_default_bg);
        p.goto(x + 1, row_y);
        p.text(&" ".repeat((w - 2) as usize));
            let idx = panel.scroll_top + i;
        if let Some(ent) = panel.entries.get(idx) {
            // active row highlight
            let is_active_panel = (is_left && matches!(app.active, rmc_core::actions::PaneSide::Left))
                || (!is_left && matches!(app.active, rmc_core::actions::PaneSide::Right));
            let is_cursor = idx == panel.cursor;
            let selected = panel.selection.is_selected(idx);
            // Determine colors following MC rules
                let (fg, bg) = if is_cursor && is_active_panel {
                (pal.selected_fg, pal.selected_bg)
                } else if selected && is_cursor {
                    (pal.markselect_fg, pal.markselect_bg)
            } else if selected {
                (pal.marked_fg, pal.marked_bg)
            } else {
                (pal.core_default_fg, pal.core_default_bg)
            };
            p.set_fg_bg(fg, bg);
            // Name
            let display_name = format_entry_name(ent);
            p.goto(x + 1, row_y);
            let name_width = (w - 2).saturating_sub(26);
            let name_trunc = truncate(&display_name, name_width as usize);
            p.text(&name_trunc);
            // Size
            p.goto(size_col, row_y);
            p.text(&format_size(ent));
            // Time
            p.goto(x + w - 15, row_y);
            p.text(&format_time(ent));
        }
    }
    // Mini status
    let status_y = y + h - 2;
    p.set_fg_bg(pal.statusbar_fg, pal.statusbar_bg);
    p.goto(x + 1, status_y);
    if let Some(cur) = panel.current_entry() {
            let s = format_mini_status(cur);
        let s = truncate(&s, (w - 2) as usize);
        p.text(&s);
    } else {
        let s = " ".repeat((w - 2) as usize);
        p.text(&s);
    }
    Ok(())
}

fn draw_gauge(p: &mut Painter, y: u16, cols: u16, pal: McPalette, app: &App) {
    p.set_fg_bg(pal.core_default_fg, pal.core_default_bg);
    p.goto(0, y);
    let path = &app.active_panel().cwd;
    let text = match (fs2::available_space(path), fs2::total_space(path)) {
        (Ok(avail), Ok(total)) => {
            let used = total.saturating_sub(avail);
            let pct = if total > 0 { (used as f64 / total as f64) * 100.0 } else { 0.0 };
            format!("{} / {} ({:.0}%)", human_bytes(used), human_bytes(total), pct)
        }
        _ => "".to_string(),
    };
    let t = truncate(&text, cols as usize);
    p.text(&t);
    if t.len() < cols as usize {
        p.text(&" ".repeat(cols as usize - t.len()));
    }
}
fn draw_hint(p: &mut Painter, y: u16, cols: u16, pal: McPalette) {
    p.set_fg_bg(pal.core_default_fg, pal.core_default_bg);
    p.goto(0, y);
    let hint = "Hint: Use C-x t to copy tagged file names to the command line.";
    let t = truncate(hint, cols as usize);
    p.text(&t);
    if t.len() < cols as usize {
        p.text(&" ".repeat(cols as usize - t.len()));
    }
}
fn draw_cmdline(p: &mut Painter, y: u16, cols: u16, _pal: McPalette, app: &App) {
    p.set_fg_bg(Color::White, Color::Black);
    p.goto(0, y);
    let user = whoami::username();
    let host = hostname::get().ok().and_then(|s| s.into_string().ok()).unwrap_or_default();
    let cwd = app.active_panel().cwd.display().to_string();
    let s = format!("{user}@{host}:{cwd}$ ");
    let t = truncate(&s, cols as usize);
    p.text(&t);
    if t.len() < cols as usize {
        p.text(&" ".repeat(cols as usize - t.len()));
    }
}
fn draw_fbar(p: &mut Painter, y: u16, cols: u16, pal: McPalette) {
    let labels = [
        "Help", "Menu", "View", "Edit", "Copy", "RenMov", "Mkdir", "Delete", "PullDn", "Quit",
    ];
    let mut x = 0u16;
    for (i, lab) in labels.iter().enumerate() {
        let num = if i == 9 { "10" } else { &(i + 1).to_string() };
        p.set_fg_bg(pal.buttonbar_hotkey_fg, pal.buttonbar_hotkey_bg);
        p.goto(x, y);
        p.text(num);
        x += num.len() as u16;
        p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
        p.goto(x, y);
        p.text(lab);
        x += lab.len() as u16;
        if x < cols {
            p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
            p.goto(x, y);
            p.text(" ");
            x += 1;
        }
        if x >= cols {
            break;
        }
    }
    if x < cols {
        p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
        p.goto(x, y);
        p.text(&" ".repeat(cols.saturating_sub(x) as usize));
    }
}

fn format_entry_name(ent: &FileEntry) -> String {
    if ent.name == ".." {
        "..".to_string()
    } else if ent.is_dir {
        ent.name.clone()
    } else if ent.is_exe {
        format!("*{}", ent.name)
    } else {
        ent.name.clone()
    }
}

fn format_size(ent: &FileEntry) -> String {
    if ent.name == ".." {
        "UP--DIR".to_string()
    } else if ent.is_dir {
        "        ".to_string()
    } else {
        format!("{:>8}", ent.size)
    }
}

fn format_time(ent: &FileEntry) -> String {
    let dt: OffsetDateTime = ent.modified.into();
    dt.format(&time::macros::format_description!("[month repr:short] [day padding:space] [hour]:[minute]")).unwrap_or_default()
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max.saturating_sub(1)).chain("…".chars()).collect()
    }
}

fn format_mini_status(ent: &FileEntry) -> String {
    let perms = perm_string(ent.permissions, ent.is_dir);
    let owner = ent.owner.as_deref().unwrap_or("-");
    let group = ent.group.as_deref().unwrap_or("-");
    let size = if ent.is_dir { 0 } else { ent.size };
    let tm: OffsetDateTime = ent.modified.into();
    let ts = tm
        .format(&time::macros::format_description!("[month repr:short] [day padding:space] [hour]:[minute]"))
        .unwrap_or_default();
    format!("{perms}  {owner:>8} {group:>8} {size:>8} {ts}")
}

fn perm_string(mode: u32, is_dir: bool) -> String {
    let mut s = String::new();
    s.push(if is_dir { 'd' } else { '-' });
    let bits = [
        (0o400, 'r'),
        (0o200, 'w'),
        (0o100, 'x'),
        (0o040, 'r'),
        (0o020, 'w'),
        (0o010, 'x'),
        (0o004, 'r'),
        (0o002, 'w'),
        (0o001, 'x'),
    ];
    for (bit, ch) in bits {
        s.push(if mode & bit != 0 { ch } else { '-' });
    }
    s
}

fn human_bytes(b: u64) -> String {
    const G: f64 = 1024.0 * 1024.0 * 1024.0;
    const M: f64 = 1024.0 * 1024.0;
    if b as f64 >= G {
        format!("{:.0}G", (b as f64) / G)
    } else if b as f64 >= M {
        format!("{:.0}M", (b as f64) / M)
    } else {
        format!("{b}B")
    }
}

fn draw_mkdir_dialog(p: &mut Painter, cols: u16, rows: u16, pal: McPalette, value: &str, focus_ok: bool) {
    let w = (cols as usize).min(60) as u16;
    let h = 7u16;
    let x = (cols - w) / 2;
    let y = (rows - h) / 2;
    // Frame
    p.set_fg_bg(pal.frame_fg, pal.dialog_default_bg);
    p.goto(x, y);
    p.text("┌");
    p.hline(x + 1, y, w - 2, '─', pal.frame_fg, pal.dialog_default_bg);
    p.goto(x + w - 1, y);
    p.text("┐");
    p.vline(x, y + 1, h - 2, '│', pal.frame_fg, pal.dialog_default_bg);
    p.vline(x + w - 1, y + 1, h - 2, '│', pal.frame_fg, pal.dialog_default_bg);
    p.goto(x, y + h - 1);
    p.text("└");
    p.hline(x + 1, y + h - 1, w - 2, '─', pal.frame_fg, pal.dialog_default_bg);
    p.goto(x + w - 1, y + h - 1);
    p.text("┘");
    // Title
    p.set_fg_bg(pal.dtitle_fg, pal.dtitle_bg);
    let title = " Create a new Directory ";
    let tx = x + (w.saturating_sub(title.len() as u16)) / 2;
    p.goto(tx, y);
    p.text(title);
    // Input line
    p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
    p.goto(x + 2, y + 2);
    let t = truncate(value, (w - 4) as usize);
    p.text(&format!("{t}{}", " ".repeat((w - 4) as usize - t.len())));
    // Buttons
    p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
    let ok = if focus_ok { "< OK >" } else { "  OK  " };
    let cancel = if focus_ok { " Cancel " } else { "[ Cancel ]" };
    let btns = format!("{ok}  {cancel}");
    let bx = x + (w.saturating_sub(btns.len() as u16)) / 2;
    p.goto(bx, y + h - 2);
    p.text(&btns);
    // Shadow
    p.set_fg_bg(pal.shadow_fg, pal.shadow_bg);
    p.hline(x + 1, y + h, w.saturating_sub(1), ' ', pal.shadow_fg, pal.shadow_bg);
    p.vline(x + w, y + 1, h, ' ', pal.shadow_fg, pal.shadow_bg);
}

#[allow(clippy::too_many_arguments)]
fn draw_copy_move_dialog(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    title: &str,
    src_name: &str,
    mask: &str,
    to: &str,
    using_shell_patterns: bool,
    follow_links: bool,
    preserve_attrs: bool,
    dive_into_subdir: bool,
    stable_symlinks: bool,
    focus: rmc_core::app::CopyDialogFocus,
) {
    use rmc_core::app::CopyDialogFocus as F;
    let w = (cols as usize).min(74) as u16;
    let h = 15u16;
    let x = (cols - w) / 2;
    let y = (rows - h) / 2;
    // Frame
    p.set_fg_bg(pal.frame_fg, pal.dialog_default_bg);
    p.goto(x, y);
    p.text("┌");
    p.hline(x + 1, y, w - 2, '─', pal.frame_fg, pal.dialog_default_bg);
    p.goto(x + w - 1, y);
    p.text("┐");
    p.vline(x, y + 1, h - 2, '│', pal.frame_fg, pal.dialog_default_bg);
    p.vline(x + w - 1, y + 1, h - 2, '│', pal.frame_fg, pal.dialog_default_bg);
    p.goto(x, y + h - 1);
    p.text("└");
    p.hline(x + 1, y + h - 1, w - 2, '─', pal.frame_fg, pal.dialog_default_bg);
    p.goto(x + w - 1, y + h - 1);
    p.text("┘");
    // Title
    p.set_fg_bg(pal.dtitle_fg, pal.dtitle_bg);
    let ttl = format!(" {title} ");
    let tx = x + (w.saturating_sub(ttl.len() as u16)) / 2;
    p.goto(tx, y);
    p.text(&ttl);
    // Lines
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(x + 2, y + 2);
    p.text(&truncate(&format!("{title} file \"{src_name}\" with source mask:"), (w - 4) as usize));
    // mask field
    let mask_focus = matches!(focus, F::Mask);
    p.set_fg_bg(if mask_focus { pal.dfocus_fg } else { pal.dialog_default_fg }, if mask_focus { pal.dfocus_bg } else { pal.dialog_default_bg });
    p.goto(x + 2, y + 3);
    let m = truncate(mask, (w - 4) as usize);
    p.text(&format!("{m}{}", " ".repeat((w - 4) as usize - m.len())));
    // to:
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(x + 2, y + 5);
    p.text("to:");
    let to_focus = matches!(focus, F::To);
    p.set_fg_bg(if to_focus { pal.dfocus_fg } else { pal.dialog_default_fg }, if to_focus { pal.dfocus_bg } else { pal.dialog_default_bg });
    p.goto(x + 6, y + 5);
    let t = truncate(to, (w - 8) as usize);
    p.text(&format!("{t}{}", " ".repeat((w - 8) as usize - t.len())));
    // Checkboxes
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    let checks = [
        ("Using shell patterns", using_shell_patterns),
        ("Follow links", follow_links),
        ("Preserve attributes", preserve_attrs),
        ("Dive into subdir if exists", dive_into_subdir),
        ("Stable symlinks", stable_symlinks),
    ];
    let mut cy = y + 7;
    for (i, (label, on)) in checks.iter().enumerate() {
        p.goto(x + 4, cy);
        let focused = matches!((i, focus), (0, F::Checkbox1) | (1, F::Checkbox2) | (2, F::Checkbox3) | (3, F::Checkbox4) | (4, F::Checkbox5));
        if focused {
            p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
        } else {
            p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
        }
        p.text(&format!("[{}] {}", if *on { 'x' } else { ' ' }, label));
        cy += 1;
    }
    // Buttons
    p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
    let sel = |f: F, txt: &str| if f == focus { format!("< {txt} >") } else { format!("[ {txt} ]") };
    let btns = format!("{}  {}  {}", sel(F::Ok, "OK"), sel(F::Background, "Background"), sel(F::Cancel, "Cancel"));
    let bx = x + (w.saturating_sub(btns.len() as u16)) / 2;
    p.goto(bx, y + h - 2);
    p.text(&btns);
    // Shadow
    p.set_fg_bg(pal.shadow_fg, pal.shadow_bg);
    p.hline(x + 1, y + h, w.saturating_sub(1), ' ', pal.shadow_fg, pal.shadow_bg);
    p.vline(x + w, y + 1, h, ' ', pal.shadow_fg, pal.shadow_bg);
}

fn draw_menu_dropdown(p: &mut Painter, pal: McPalette, top_index: usize, selected: usize) {
    // Real top menus and stub items
    let menus: [&[&str]; 5] = [
        &["Copy", "Move", "Mkdir", "Delete"],
        &["View", "Edit", "Copy", "Move", "Mkdir", "Delete", "Quit"],
        &["Find file", "Compare dirs"],
        &["Layout", "Panels", "Confirmations"],
        &["Copy", "Move", "Mkdir", "Delete"],
    ];
    let titles = [" Left ", " File ", " Command ", " Options ", " Right "];
    // Compute x position under the selected top title
    let mut x = 0u16;
    for title in titles.iter().take(top_index) {
        x += title.len() as u16;
    }
    let items = menus[top_index];
    let y = 1u16;
    let w = (items.iter().map(|s| s.len()).max().unwrap_or(8) + 4) as u16;
    let h = items.len() as u16 + 2;
    p.set_fg_bg(pal.menu_fg, pal.menu_bg);
    // Frame
    p.goto(x, y);
    p.text("┌");
    p.hline(x + 1, y, w - 2, '─', pal.menu_fg, pal.menu_bg);
    p.goto(x + w - 1, y);
    p.text("┐");
    p.vline(x, y + 1, h - 2, '│', pal.menu_fg, pal.menu_bg);
    p.vline(x + w - 1, y + 1, h - 2, '│', pal.menu_fg, pal.menu_bg);
    p.goto(x, y + h - 1);
    p.text("└");
    p.hline(x + 1, y + h - 1, w - 2, '─', pal.menu_fg, pal.menu_bg);
    p.goto(x + w - 1, y + h - 1);
    p.text("┘");
    // Items
    for (i, it) in items.iter().enumerate() {
        let row = y + 1 + i as u16;
        if i == selected {
            p.set_fg_bg(pal.menusel_fg, pal.menusel_bg);
        } else {
            p.set_fg_bg(pal.menu_fg, pal.menu_bg);
        }
        p.goto(x + 1, row);
        let mut line = String::new();
        line.push(' ');
        line.push_str(it);
        while line.len() < (w - 2) as usize {
            line.push(' ');
        }
        p.text(&line);
    }
}
