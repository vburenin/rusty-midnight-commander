use crate::mc_colors::McPalette;
use crate::widgets::Painter;
use rmc_core::dirtree::DirectoryTreeState;
use std::path::Path;

pub fn directory_tree_height(rows: u16) -> u16 {
    let cap = rows.min(22);
    cap.max(rows.min(10))
}

pub fn directory_tree_list_rows(overlay_h: u16) -> usize {
    overlay_h.saturating_sub(4) as usize
}

pub fn draw_directory_tree_dialog(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    st: &DirectoryTreeState,
) {
    let w = cols.min(60);
    let h = directory_tree_height(rows);
    let x = cols.saturating_sub(w) / 2;
    let y = rows.saturating_sub(h) / 2;
    p.set_fg_bg(pal.frame_fg, pal.dialog_default_bg);
    p.goto(x, y);
    p.text("┌");
    p.hline(
        x + 1,
        y,
        w.saturating_sub(2),
        '─',
        pal.frame_fg,
        pal.dialog_default_bg,
    );
    p.goto(x + w.saturating_sub(1), y);
    p.text("┐");
    p.vline(
        x,
        y + 1,
        h.saturating_sub(2),
        '│',
        pal.frame_fg,
        pal.dialog_default_bg,
    );
    p.vline(
        x + w.saturating_sub(1),
        y + 1,
        h.saturating_sub(2),
        '│',
        pal.frame_fg,
        pal.dialog_default_bg,
    );
    p.goto(x, y + h.saturating_sub(1));
    p.text("└");
    p.hline(
        x + 1,
        y + h.saturating_sub(1),
        w.saturating_sub(2),
        '─',
        pal.frame_fg,
        pal.dialog_default_bg,
    );
    p.goto(x + w.saturating_sub(1), y + h.saturating_sub(1));
    p.text("┘");
    p.set_fg_bg(pal.dtitle_fg, pal.dtitle_bg);
    let ttl = " Directory Tree ";
    let tx = x + (w.saturating_sub(ttl.len() as u16)) / 2;
    p.goto(tx, y);
    p.text(ttl);
    let list_top = y + 2;
    let list_h = directory_tree_list_rows(h);
    for i in 0..list_h {
        let row_y = list_top + i as u16;
        p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
        p.goto(x + 2, row_y);
        p.text(&" ".repeat(w.saturating_sub(4) as usize));
        let idx = st.scroll_top + i;
        if let Some(ent) = st.entries.get(idx) {
            if idx == st.selected_index {
                p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
            } else {
                p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
            }
            let name = ent.path.file_name().and_then(|s| s.to_str()).unwrap_or("/");
            let indent = "  ".repeat(ent.depth);
            let disp = if ent.path == Path::new("/") {
                "/".to_string()
            } else {
                format!("{indent}{name}/")
            };
            let t = truncate(&disp, w.saturating_sub(6) as usize);
            p.goto(x + 3, row_y);
            p.text(&t);
        }
    }
    if !st.search.is_empty() && h > 3 {
        p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
        let sy = y + h.saturating_sub(2);
        p.goto(x + 2, sy);
        let msg = truncate(
            &format!("Search: {}", st.search),
            w.saturating_sub(4) as usize,
        );
        p.text(&msg);
    }
    p.set_fg_bg(pal.shadow_fg, pal.shadow_bg);
    p.hline(
        x + 1,
        y + h,
        w.saturating_sub(1),
        ' ',
        pal.shadow_fg,
        pal.shadow_bg,
    );
    p.vline(x + w, y + 1, h, ' ', pal.shadow_fg, pal.shadow_bg);
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars()
            .take(max.saturating_sub(1))
            .chain("…".chars())
            .collect()
    }
}
