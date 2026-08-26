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
        draw_gauge(&mut painter, gauge_row, cols, self.palette);
        draw_hint(&mut painter, hint_row, cols, self.palette);
        draw_cmdline(&mut painter, cmd_row, cols, self.palette);
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
        rmc_core::app::UiMode::Viewer { path, hex } => {
            draw_viewer(p, app, cols, rows, pal, path, *hex)?;
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
    // Full-screen overlay with black background and light text
    p.set_fg_bg(Color::White, Color::Black);
    for y in 0..rows {
        p.goto(0, y);
        p.text(&" ".repeat(cols as usize));
    }
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
    let max_lines = rows.saturating_sub(1) as usize;
    for (i, line) in lines.into_iter().take(max_lines).enumerate() {
        p.goto(0, i as u16);
        let t = truncate(&line, cols as usize);
        p.text(&t);
    }
    // Footer
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
    let panel = if is_left { &app.left } else { &app.right };
    for i in 0..content_h as usize {
        let row_y = content_top + i as u16;
        // Clear row
        p.set_fg_bg(pal.core_default_fg, pal.core_default_bg);
        p.goto(x + 1, row_y);
        p.text(&" ".repeat((w - 2) as usize));
        let idx = i;
        if let Some(ent) = panel.entries.get(idx) {
            // active row highlight
            let is_active_panel = (is_left && matches!(app.active, rmc_core::actions::PaneSide::Left))
                || (!is_left && matches!(app.active, rmc_core::actions::PaneSide::Right));
            let is_cursor = idx == panel.cursor;
            let selected = panel.selection.is_selected(idx);
            // Determine colors following MC rules
            let (fg, bg) = if is_cursor && is_active_panel {
                (pal.selected_fg, pal.selected_bg)
            } else if is_cursor && !is_active_panel {
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

fn draw_gauge(p: &mut Painter, y: u16, cols: u16, pal: McPalette) {
    p.set_fg_bg(pal.core_default_fg, pal.core_default_bg);
    p.goto(0, y);
    let s = format!("{:>width$}", "", width = cols as usize);
    p.text(&s);
}
fn draw_hint(p: &mut Painter, y: u16, cols: u16, pal: McPalette) {
    p.set_fg_bg(pal.core_default_fg, pal.core_default_bg);
    p.goto(0, y);
    let hint = "C-x c to copy tagged files names to the command line.";
    let t = truncate(hint, cols as usize);
    p.text(&t);
    if t.len() < cols as usize {
        p.text(&" ".repeat(cols as usize - t.len()));
    }
}
fn draw_cmdline(p: &mut Painter, y: u16, cols: u16, _pal: McPalette) {
    p.set_fg_bg(Color::White, Color::Black);
    p.goto(0, y);
    let s = format!("{}{}", "prompt> ", "");
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
            p.set_fg_bg(pal.core_default_fg, pal.core_default_bg);
            p.goto(x, y);
            p.text(" ");
            x += 1;
        }
        if x >= cols {
            break;
        }
    }
    if x < cols {
        p.set_fg_bg(pal.core_default_fg, pal.core_default_bg);
        p.goto(x, y);
        p.text(&" ".repeat(cols.saturating_sub(x) as usize));
    }
}

fn format_entry_name(ent: &FileEntry) -> String {
    if ent.name == ".." {
        "..".to_string()
    } else if ent.is_dir {
        format!("{}/", ent.name)
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
        "--DIR--".to_string()
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
    // Simplified status: file name, size, type flags
    if ent.is_dir {
        format!("UP-DIR {}", ent.name)
    } else {
        format!("{} bytes  {}", ent.size, ent.name)
    }
}
