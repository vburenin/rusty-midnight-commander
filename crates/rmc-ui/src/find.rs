use crate::widgets::Painter;
use crate::mc_colors::McPalette;
use rmc_core::find::{FindDialogFocus as F, FindDialogState};

pub fn draw_find_dialog(p: &mut Painter, cols: u16, rows: u16, pal: McPalette, st: &FindDialogState) {
    let w = (cols as usize).min(76) as u16;
    let h = 13u16;
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
    p.text("File name:");
    p.goto(x + 2, y + 6);
    p.text("Content (optional):");
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
    draw_field(p, x + 12, y + 2, field_w - 12, &st.params.start_dir.display().to_string(), matches!(st.focus, F::StartDir), pal);
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
    let status = if st.running {
        "Searching... (Press Stop)"
    } else {
        let n = st.results.paths.len();
        if n == 0 { "No results yet" } else { "Search complete" }
    };
    let t = truncate(status, (w - 4) as usize);
    p.text(&t);
    // Buttons
    let sel = |f: F, txt: &str| -> String {
        if st.focus == f { format!("< {txt} >") } else { format!("[ {txt} ]") }
    };
    p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
    let btns = format!(
        "{}  {}  {}  {}  {}",
        sel(F::ButtonOk, "OK"),
        sel(F::ButtonStop, "Stop"),
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

