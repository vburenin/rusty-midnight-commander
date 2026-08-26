use crate::widgets::Painter;
use crate::mc_colors::McPalette;
use rmc_core::find::{FindDialogFocus as F, FindDialogState};

pub fn draw_find_dialog(p: &mut Painter, cols: u16, rows: u16, pal: McPalette, st: &FindDialogState) {
    let w = (cols as usize).min(76) as u16;
    // Flexible height to fit a results list
    let h = rows.saturating_sub(4).clamp(16, 22);
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
    let ttl = " Find File ";
    let tx = x + (w.saturating_sub(ttl.len() as u16)) / 2;
    p.goto(tx, y);
    p.text(ttl);
    // Labels and inputs
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(x + 2, y + 2);
    p.text("Start at:");
    p.goto(x + 2, y + 4);
    p.text("Filename:");
    p.goto(x + 2, y + 6);
    p.text("Content:");
    p.goto(x + 2, y + 8);
    p.text("[ ] Case sensitive");
    // Fields
    let draw_field = |p: &mut Painter, xx: u16, yy: u16, w: u16, text: &str, focus: bool, pal: McPalette| {
        p.set_fg_bg(if focus { pal.dfocus_fg } else { pal.dialog_default_fg }, if focus { pal.dfocus_bg } else { pal.dialog_default_bg });
        p.goto(xx, yy);
        let t = truncate(text, (w - 2) as usize);
        p.text(&format!("{t}{}", " ".repeat((w - 2) as usize - t.len())));
    };
    let field_w = w - 4;
    draw_field(p, x + 12, y + 2, field_w - 12, &st.start_dir_edit, matches!(st.focus, F::StartDir), pal);
    // Name pattern
    let pat = match &st.params.name_pattern { rmc_core::find::NamePattern::Glob(s) => s.as_str() };
    draw_field(p, x + 12, y + 4, field_w - 12, pat, matches!(st.focus, F::NamePattern), pal);
    // Content
    let content = st.params.content_substring.as_deref().unwrap_or("");
    draw_field(p, x + 20, y + 6, field_w - 20, content, matches!(st.focus, F::Content), pal);
    // Case checkbox
    p.set_fg_bg(if matches!(st.focus, F::CaseSensitive) { pal.dfocus_fg } else { pal.dialog_default_fg }, if matches!(st.focus, F::CaseSensitive) { pal.dfocus_bg } else { pal.dialog_default_bg });
    p.goto(x + 3, y + 8);
    p.text(if st.params.case_sensitive { "[x] Case sensitive" } else { "[ ] Case sensitive" });
    // Status line
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(x + 2, y + 9);
    let n = st.results.paths.len();
    let status = if st.running { format!("Searching... {n} matches") } else { format!("{n} matches") };
    let t = truncate(&status, (w - 4) as usize);
    p.text(&t);
    // Results list area
    let list_top = y + 10;
    let list_bottom = y + h - 3;
    let list_h = list_bottom.saturating_sub(list_top).saturating_add(1);
    for i in 0..list_h {
        let row_y = list_top + i;
        p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
        p.goto(x + 2, row_y);
        p.text(&" ".repeat((w - 4) as usize));
        let idx = st.scroll_top as u16 + i;
        let idx_usize = idx as usize;
        if let Some(path) = st.results.paths.get(idx_usize) {
            let mut disp = match path.strip_prefix(&st.params.start_dir) {
                Ok(rel) => rel.display().to_string(),
                Err(_) => path.display().to_string(),
            };
            if disp.is_empty() {
                disp = path.display().to_string();
            }
            // Highlight selection
            if idx_usize == st.selected_index {
                p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
            } else {
                p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
            }
            let t = truncate(&disp, (w - 6) as usize);
            p.goto(x + 3, row_y);
            p.text(&t);
        }
    }
    // Buttons
    let sel = |f: F, txt: &str| -> String {
        if st.focus == f { format!("< {txt} >") } else { format!("[ {txt} ]") }
    };
    p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
    let btns = format!(
        "{}  {}  {}  {}  {}  {}",
        sel(F::ButtonStart, "Start"),
        sel(F::ButtonStop, "Stop"),
        sel(F::ButtonAgain, "Again"),
        sel(F::ButtonChdir, "Chdir"),
        sel(F::ButtonPanelize, "Panelize"),
        sel(F::ButtonQuit, "Quit")
    );
    let bx = x + (w.saturating_sub(btns.len() as u16)) / 2;
    p.goto(bx, y + h - 2);
    p.text(&btns);
    // Shadow
    p.set_fg_bg(pal.shadow_fg, pal.shadow_bg);
    p.hline(x + 1, y + h, w.saturating_sub(1), ' ', pal.shadow_fg, pal.shadow_bg);
    p.vline(x + w, y + 1, h, ' ', pal.shadow_fg, pal.shadow_bg);
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max.saturating_sub(1)).chain("…".chars()).collect()
    }
}

