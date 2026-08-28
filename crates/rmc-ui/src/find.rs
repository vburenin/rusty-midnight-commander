use crate::mc_colors::McPalette;
use crate::widgets::Painter;
use rmc_core::find::{
    FIND_DIALOG_LIST_TOP, FindDialogFocus as F, FindDialogState, find_dialog_height,
    find_tree_picker_height, find_tree_picker_list_rows,
};

/// GNU mc(1) Find File bottom-row labels. Stop/Start is one slot: Stop while
/// running, Start while a search is paused. OK is the start-new-search button.
pub fn find_action_button_specs(focus: F, paused: bool) -> [(F, String); 6] {
    let lab = |f: F, txt: &str| -> (F, String) {
        let s = if focus == f {
            format!("< {txt} >")
        } else {
            format!("[ {txt} ]")
        };
        (f, s)
    };
    [
        lab(F::ButtonOk, "OK"),
        lab(F::ButtonStop, if paused { "Start" } else { "Stop" }),
        lab(F::ButtonAgain, "Again"),
        lab(F::ButtonChdir, "Chdir"),
        lab(F::ButtonPanelize, "Panelize"),
        lab(F::ButtonQuit, "Quit"),
    ]
}

pub fn find_action_button_bar(focus: F, paused: bool) -> String {
    find_action_button_specs(focus, paused)
        .into_iter()
        .map(|(_, s)| s)
        .collect::<Vec<_>>()
        .join("  ")
}

/// Hit-test the GNU action row. `None` if the click is not on a button.
pub fn find_action_button_at(
    cols: u16,
    rows: u16,
    mx: u16,
    my: u16,
    focus: F,
    paused: bool,
) -> Option<F> {
    let w = (cols as usize).min(76) as u16;
    let h = find_dialog_height(rows);
    if cols < w || rows < h {
        return None;
    }
    let x = (cols - w) / 2;
    let y = (rows - h) / 2;
    if my != y + h - 2 {
        return None;
    }
    let specs = find_action_button_specs(focus, paused);
    let bar: String = specs
        .iter()
        .map(|(_, s)| s.as_str())
        .collect::<Vec<_>>()
        .join("  ");
    let bx = x + (w.saturating_sub(bar.len() as u16)) / 2;
    let mut cx = bx;
    for (i, (f, s)) in specs.iter().enumerate() {
        let end = cx.saturating_add(s.len() as u16);
        if mx >= cx && mx < end {
            return Some(*f);
        }
        cx = end;
        if i + 1 < specs.len() {
            cx = cx.saturating_add(2);
        }
    }
    None
}

pub fn draw_find_dialog(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    st: &FindDialogState,
) {
    let w = (cols as usize).min(76) as u16;
    // Flexible height to fit GNU checkboxes plus a results list
    let h = find_dialog_height(rows);
    let x = (cols - w) / 2;
    let y = (rows - h) / 2;
    // Frame
    p.fill_rect(x, y, w, h, pal.dialog_default_fg, pal.dialog_default_bg);
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(x, y);
    p.text("┌");
    p.hline(
        x + 1,
        y,
        w - 2,
        '─',
        pal.dialog_default_fg,
        pal.dialog_default_bg,
    );
    p.goto(x + w - 1, y);
    p.text("┐");
    p.vline(
        x,
        y + 1,
        h - 2,
        '│',
        pal.dialog_default_fg,
        pal.dialog_default_bg,
    );
    p.vline(
        x + w - 1,
        y + 1,
        h - 2,
        '│',
        pal.dialog_default_fg,
        pal.dialog_default_bg,
    );
    p.goto(x, y + h - 1);
    p.text("└");
    p.hline(
        x + 1,
        y + h - 1,
        w - 2,
        '─',
        pal.dialog_default_fg,
        pal.dialog_default_bg,
    );
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
    // Fields
    let draw_field =
        |p: &mut Painter, xx: u16, yy: u16, w: u16, text: &str, focus: bool, pal: McPalette| {
            p.set_fg_bg(
                if focus {
                    pal.dfocus_fg
                } else {
                    pal.dialog_default_fg
                },
                if focus {
                    pal.dfocus_bg
                } else {
                    pal.dialog_default_bg
                },
            );
            p.goto(xx, yy);
            let t = truncate(text, (w - 2) as usize);
            p.text(&format!("{t}{}", " ".repeat((w - 2) as usize - t.len())));
        };
    let field_w = w - 4;
    let tree_txt = if matches!(st.focus, F::Tree) {
        "< Tree >"
    } else {
        "[ Tree ]"
    };
    let tree_w = tree_txt.len() as u16;
    let tree_x = x + w.saturating_sub(2).saturating_sub(tree_w);
    let start_field_w = tree_x.saturating_sub(x + 12 + 1).max(2);
    draw_field(
        p,
        x + 12,
        y + 2,
        start_field_w,
        &st.start_dir_edit,
        matches!(st.focus, F::StartDir),
        pal,
    );
    p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
    p.goto(tree_x, y + 2);
    p.text(tree_txt);
    // Name pattern
    let pat = match &st.params.name_pattern {
        rmc_core::find::NamePattern::Glob(s) => s.as_str(),
    };
    draw_field(
        p,
        x + 12,
        y + 4,
        field_w - 12,
        pat,
        matches!(st.focus, F::NamePattern),
        pal,
    );
    // Content
    let content = st.params.content_substring.as_deref().unwrap_or("");
    draw_field(
        p,
        x + 20,
        y + 6,
        field_w - 20,
        content,
        matches!(st.focus, F::Content),
        pal,
    );
    // GNU Find File checkboxes (public mc(1) "Find File" labels and order)
    draw_checkbox(
        p,
        x + 3,
        y + 8,
        st.params.whole_words,
        matches!(st.focus, F::WholeWords),
        "Whole words",
        pal,
    );
    draw_checkbox(
        p,
        x + 3,
        y + 9,
        st.params.case_sensitive,
        matches!(st.focus, F::CaseSensitive),
        "Case sensitive",
        pal,
    );
    draw_checkbox(
        p,
        x + 3,
        y + 10,
        st.params.regular_expression,
        matches!(st.focus, F::RegularExpression),
        "Regular expression",
        pal,
    );
    draw_checkbox(
        p,
        x + 3,
        y + 11,
        st.params.find_recursively,
        matches!(st.focus, F::FindRecursively),
        "Find recursively",
        pal,
    );
    draw_checkbox(
        p,
        x + 3,
        y + 12,
        st.params.follow_symlinks,
        matches!(st.focus, F::FollowSymlinks),
        "Follow symlinks",
        pal,
    );
    draw_checkbox(
        p,
        x + 3,
        y + 13,
        st.params.skip_hidden,
        matches!(st.focus, F::SkipHidden),
        "Skip hidden",
        pal,
    );
    draw_checkbox(
        p,
        x + 3,
        y + 14,
        st.params.enable_ignore_dirs,
        matches!(st.focus, F::EnableIgnoreDirs),
        "Enable ignore directories",
        pal,
    );
    draw_field(
        p,
        x + 3,
        y + 15,
        field_w.saturating_sub(1),
        &st.params.ignore_dirs,
        matches!(st.focus, F::IgnoreDirs),
        pal,
    );
    // Status line
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(x + 2, y + 16);
    let n = st.results.paths.len();
    let status = if st.is_paused() {
        format!("Stopped... {n} matches")
    } else if st.running {
        format!("Searching... {n} matches")
    } else {
        format!("{n} matches")
    };
    let t = truncate(&status, (w - 4) as usize);
    p.text(&t);
    // Results list area
    let list_top = y + FIND_DIALOG_LIST_TOP;
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
    // Buttons: GNU mc(1) OK, Stop/Start, Again, Chdir, Panelize, Quit
    p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
    let btns = find_action_button_bar(st.focus, st.is_paused());
    let bx = x + (w.saturating_sub(btns.len() as u16)) / 2;
    p.goto(bx, y + h - 2);
    p.text(&btns);
    // Shadow
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
    if let Some(picker) = &st.tree_picker {
        draw_find_tree_picker(p, cols, rows, pal, picker);
    }
}

fn draw_find_tree_picker(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    picker: &rmc_core::find::FindTreePicker,
) {
    let w = cols.min(60);
    let h = find_tree_picker_height(rows);
    let x = cols.saturating_sub(w) / 2;
    let y = rows.saturating_sub(h) / 2;
    p.fill_rect(x, y, w, h, pal.dialog_default_fg, pal.dialog_default_bg);
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(x, y);
    p.text("┌");
    p.hline(
        x + 1,
        y,
        w.saturating_sub(2),
        '─',
        pal.dialog_default_fg,
        pal.dialog_default_bg,
    );
    p.goto(x + w.saturating_sub(1), y);
    p.text("┐");
    p.vline(
        x,
        y + 1,
        h.saturating_sub(2),
        '│',
        pal.dialog_default_fg,
        pal.dialog_default_bg,
    );
    p.vline(
        x + w.saturating_sub(1),
        y + 1,
        h.saturating_sub(2),
        '│',
        pal.dialog_default_fg,
        pal.dialog_default_bg,
    );
    p.goto(x, y + h.saturating_sub(1));
    p.text("└");
    p.hline(
        x + 1,
        y + h.saturating_sub(1),
        w.saturating_sub(2),
        '─',
        pal.dialog_default_fg,
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
    let list_h = find_tree_picker_list_rows(h);
    for i in 0..list_h {
        let row_y = list_top + i as u16;
        p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
        p.goto(x + 2, row_y);
        p.text(&" ".repeat(w.saturating_sub(4) as usize));
        let idx = picker.scroll_top + i;
        if let Some(ent) = picker.entries.get(idx) {
            if idx == picker.selected_index {
                p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
            } else {
                p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
            }
            let name = ent.path.file_name().and_then(|s| s.to_str()).unwrap_or("/");
            let indent = "  ".repeat(ent.depth);
            let disp = if ent.path == std::path::Path::new("/") {
                "/".to_string()
            } else {
                format!("{indent}{name}/")
            };
            let t = truncate(&disp, w.saturating_sub(6) as usize);
            p.goto(x + 3, row_y);
            p.text(&t);
        }
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

fn draw_checkbox(
    p: &mut Painter,
    x: u16,
    y: u16,
    checked: bool,
    focused: bool,
    label: &str,
    pal: McPalette,
) {
    p.set_fg_bg(
        if focused {
            pal.dfocus_fg
        } else {
            pal.dialog_default_fg
        },
        if focused {
            pal.dfocus_bg
        } else {
            pal.dialog_default_bg
        },
    );
    p.goto(x, y);
    p.text(&format!("[{}] {label}", if checked { 'x' } else { ' ' }));
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

#[cfg(test)]
mod tests {
    use super::*;
    use rmc_core::find::FindDialogState;
    use std::path::PathBuf;

    fn rasterize(bytes: &[u8], cols: u16, rows: u16) -> Vec<String> {
        let mut grid = vec![vec![' '; cols as usize]; rows as usize];
        let mut x: usize = 0;
        let mut y: usize = 0;
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
                i += 2;
                let start = i;
                while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b';') {
                    i += 1;
                }
                if i >= bytes.len() {
                    break;
                }
                let cmd = bytes[i];
                let params = std::str::from_utf8(&bytes[start..i]).unwrap_or("");
                i += 1;
                if cmd == b'H' || cmd == b'f' {
                    let mut it = params.split(';');
                    let row = it.next().unwrap_or("1").parse::<usize>().unwrap_or(1);
                    let col = it.next().unwrap_or("1").parse::<usize>().unwrap_or(1);
                    y = row.saturating_sub(1);
                    x = col.saturating_sub(1);
                }
                continue;
            }
            if bytes[i] == 0x1b {
                i += 1;
                continue;
            }
            if (0x20..=0x7e).contains(&bytes[i]) {
                if y < rows as usize && x < cols as usize {
                    grid[y][x] = bytes[i] as char;
                    x += 1;
                }
            }
            i += 1;
        }
        grid.into_iter()
            .map(|row| row.into_iter().collect())
            .collect()
    }

    fn screen_text(grid: &[String]) -> String {
        grid.join("\n")
    }

    fn draw_state(st: &FindDialogState) -> Vec<String> {
        let pal = McPalette::default();
        let mut buf = Vec::new();
        {
            let mut p = Painter { out: &mut buf };
            draw_find_dialog(&mut p, 80, 28, pal, st);
        }
        rasterize(&buf, 80, 28)
    }

    #[test]
    fn raster_idle_buttons_are_ok_stop_again_chdir_panelize_quit() {
        let st = FindDialogState::new(PathBuf::from("/tmp"));
        let grid = draw_state(&st);
        let text = screen_text(&grid);
        assert!(
            text.contains("[ OK ]"),
            "OK must start a new search, got:\n{text}"
        );
        assert!(
            text.contains("[ Stop ]"),
            "idle Stop/Start slot is Stop, got:\n{text}"
        );
        assert!(text.contains("[ Again ]"), "{text}");
        assert!(text.contains("[ Chdir ]"), "{text}");
        assert!(text.contains("[ Panelize ]"), "{text}");
        assert!(text.contains("[ Quit ]"), "{text}");
        assert!(
            !text.contains("[ Start ]") && !text.contains("< Start >"),
            "standalone Start must not be the start-new-search button:\n{text}"
        );
        let btn_row = grid
            .iter()
            .find(|r| r.contains("[ OK ]") && r.contains("[ Stop ]"))
            .expect("button row");
        let ok = btn_row.find("[ OK ]").unwrap();
        let stop = btn_row.find("[ Stop ]").unwrap();
        let again = btn_row.find("[ Again ]").unwrap();
        let chdir = btn_row.find("[ Chdir ]").unwrap();
        let panelize = btn_row.find("[ Panelize ]").unwrap();
        let quit = btn_row.find("[ Quit ]").unwrap();
        assert!(ok < stop && stop < again && again < chdir && chdir < panelize && panelize < quit);
    }

    #[test]
    fn raster_paused_stop_slot_is_start() {
        let mut st = FindDialogState::new(PathBuf::from("/tmp"));
        let h = rmc_core::find::CancelHandle::new();
        h.pause();
        st.cancel = Some(h);
        st.running = true;
        st.focus = F::ButtonStop;
        let grid = draw_state(&st);
        let text = screen_text(&grid);
        assert!(text.contains("[ OK ]") || text.contains("< OK >"), "{text}");
        assert!(
            text.contains("< Start >"),
            "paused Stop/Start slot is Start:\n{text}"
        );
        assert!(
            !text.contains("[ Stop ]") && !text.contains("< Stop >"),
            "paused pair must not still say Stop:\n{text}"
        );
        assert!(text.contains("[ Again ]"), "{text}");
        assert!(text.contains("Stopped..."), "{text}");
    }

    #[test]
    fn action_button_hit_ok() {
        let st = FindDialogState::new(PathBuf::from("/tmp"));
        let h = find_dialog_height(28);
        let y = (28 - h) / 2;
        let btn_y = y + h - 2;
        let ok =
            (0..80u16).find_map(|mx| find_action_button_at(80, 28, mx, btn_y, st.focus, false));
        assert_eq!(ok, Some(F::ButtonOk));
    }
}
