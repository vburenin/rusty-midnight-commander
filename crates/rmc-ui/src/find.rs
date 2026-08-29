use crate::mc_colors::McPalette;
use crate::widgets::Painter;
use rmc_core::find::{
    find_dialog_height, find_setup_origin, find_tree_picker_height, find_tree_picker_list_rows,
    FindDialogFocus as F, FindDialogPhase, FindDialogState, FIND_RESULTS_LIST_TOP,
    FIND_SETUP_CANCEL_X, FIND_SETUP_FIELD_W, FIND_SETUP_H, FIND_SETUP_OK_X, FIND_SETUP_TREE_X,
    FIND_SETUP_W, FIND_SETUP_X1, FIND_SETUP_X2,
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

fn setup_btn(label: &str, marked: bool) -> String {
    if marked {
        format!("[< {label} >]")
    } else {
        format!("[ {label} ]")
    }
}

fn setup_ok_cancel(focus: F) -> (String, String) {
    (
        setup_btn("OK", !matches!(focus, F::ButtonCancel)),
        setup_btn("Cancel", matches!(focus, F::ButtonCancel)),
    )
}

/// Hit-test the GNU 4.8.30 two-column setup (fields, checks, Tree, OK/Cancel).
pub fn find_setup_widget_at(cols: u16, rows: u16, mx: u16, my: u16) -> Option<F> {
    let (x, y) = find_setup_origin(cols, rows);
    let w = FIND_SETUP_W.min(cols);
    let h = FIND_SETUP_H.min(rows);
    if mx < x || mx >= x.saturating_add(w) || my < y || my >= y.saturating_add(h) {
        return None;
    }
    let rel_x = mx.saturating_sub(x);
    let rel_y = my.saturating_sub(y);
    let in_left = rel_x < FIND_SETUP_X2;
    let on_check = |col_x: u16, width: u16| rel_x >= col_x && rel_x < col_x.saturating_add(width);
    match rel_y {
        2 if on_check(FIND_SETUP_TREE_X, 8) => Some(F::Tree),
        2 if on_check(
            FIND_SETUP_X1,
            FIND_SETUP_TREE_X.saturating_sub(FIND_SETUP_X1),
        ) =>
        {
            Some(F::StartDir)
        }
        3 if on_check(FIND_SETUP_X1, 32) => Some(F::EnableIgnoreDirs),
        4 if on_check(FIND_SETUP_X1, w.saturating_sub(4)) => Some(F::IgnoreDirs),
        7 if in_left => Some(F::NamePattern),
        7 => Some(F::Content),
        8 if in_left => Some(F::FindRecursively),
        8 => Some(F::WholeWords),
        9 if in_left => Some(F::FollowSymlinks),
        9 => Some(F::RegularExpression),
        10 if in_left => Some(F::UsingShellPatterns),
        10 => Some(F::ContentCaseSensitive),
        11 if in_left => Some(F::CaseSensitive),
        11 => Some(F::ContentAllCharsets),
        12 if in_left => Some(F::FileAllCharsets),
        12 => Some(F::FirstHit),
        13 if in_left => Some(F::SkipHidden),
        15 => {
            let (ok, cancel) = setup_ok_cancel(F::NamePattern);
            if on_check(FIND_SETUP_OK_X, ok.len() as u16) {
                Some(F::ButtonOk)
            } else if on_check(FIND_SETUP_CANCEL_X, cancel.len() as u16) {
                Some(F::ButtonCancel)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn paint_hline(p: &mut Painter, x: u16, y: u16, w: u16, pal: McPalette) {
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(x, y);
    p.text("├");
    p.hline(
        x + 1,
        y,
        w.saturating_sub(2),
        '─',
        pal.dialog_default_fg,
        pal.dialog_default_bg,
    );
    p.goto(x + w - 1, y);
    p.text("┤");
}

fn paint_setup_field(
    p: &mut Painter,
    x: u16,
    y: u16,
    w: u16,
    text: &str,
    focus: bool,
    pal: McPalette,
) {
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
    p.goto(x, y);
    let t = truncate(text, w as usize);
    p.text(&format!("{t}{}", " ".repeat(w as usize - t.len())));
}

fn paint_setup_check(
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

fn paint_dialog_chrome(
    p: &mut Painter,
    x: u16,
    y: u16,
    w: u16,
    h: u16,
    title: &str,
    pal: McPalette,
) {
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
    p.set_fg_bg(pal.dtitle_fg, pal.dtitle_bg);
    let ttl = format!(" {title} ");
    let tx = x + (w.saturating_sub(ttl.len() as u16)) / 2;
    p.goto(tx, y);
    p.text(&ttl);
}

fn draw_find_setup_dialog(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    st: &FindDialogState,
) {
    let w = FIND_SETUP_W.min(cols);
    let h = FIND_SETUP_H.min(rows);
    let (x, y) = find_setup_origin(cols, rows);
    paint_dialog_chrome(p, x, y, w, h, "Find File", pal);
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(x + FIND_SETUP_X1, y + 1);
    p.text("Start at:");
    let tree = setup_btn("Tree", matches!(st.focus, F::Tree));
    paint_setup_field(
        p,
        x + FIND_SETUP_X1,
        y + 2,
        FIND_SETUP_TREE_X.saturating_sub(FIND_SETUP_X1),
        &st.start_dir_edit,
        matches!(st.focus, F::StartDir),
        pal,
    );
    p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
    p.goto(x + FIND_SETUP_TREE_X, y + 2);
    p.text(&tree);
    paint_setup_check(
        p,
        x + FIND_SETUP_X1,
        y + 3,
        st.params.enable_ignore_dirs,
        matches!(st.focus, F::EnableIgnoreDirs),
        "Enable ignore directories:",
        pal,
    );
    paint_setup_field(
        p,
        x + FIND_SETUP_X1,
        y + 4,
        w.saturating_sub(FIND_SETUP_X1 + 2),
        &st.params.ignore_dirs,
        matches!(st.focus, F::IgnoreDirs),
        pal,
    );
    paint_hline(p, x, y + 5, w, pal);
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(x + FIND_SETUP_X1, y + 6);
    p.text("File name:");
    p.goto(x + FIND_SETUP_X2, y + 6);
    p.text("Content:");
    let pat = match &st.params.name_pattern {
        rmc_core::find::NamePattern::Glob(s) => s.as_str(),
    };
    paint_setup_field(
        p,
        x + FIND_SETUP_X1,
        y + 7,
        FIND_SETUP_FIELD_W,
        pat,
        matches!(st.focus, F::NamePattern),
        pal,
    );
    let content = st.params.content_substring.as_deref().unwrap_or("");
    paint_setup_field(
        p,
        x + FIND_SETUP_X2,
        y + 7,
        FIND_SETUP_FIELD_W.min(w.saturating_sub(FIND_SETUP_X2 + 2)),
        content,
        matches!(st.focus, F::Content),
        pal,
    );
    let left = [
        (
            F::FindRecursively,
            st.params.find_recursively,
            "Find recursively",
        ),
        (
            F::FollowSymlinks,
            st.params.follow_symlinks,
            "Follow symlinks",
        ),
        (
            F::UsingShellPatterns,
            st.params.using_shell_patterns,
            "Using shell patterns",
        ),
        (F::CaseSensitive, st.params.case_sensitive, "Case sensitive"),
        (
            F::FileAllCharsets,
            st.params.file_all_charsets,
            "All charsets",
        ),
        (F::SkipHidden, st.params.skip_hidden, "Skip hidden"),
    ];
    let right = [
        (F::WholeWords, st.params.whole_words, "Whole words"),
        (
            F::RegularExpression,
            st.params.regular_expression,
            "Regular expression",
        ),
        (
            F::ContentCaseSensitive,
            st.params.content_case_sensitive,
            "Case sensitive",
        ),
        (
            F::ContentAllCharsets,
            st.params.content_all_charsets,
            "All charsets",
        ),
        (F::FirstHit, st.params.first_hit, "First hit"),
    ];
    for (i, (f, on, lab)) in left.iter().enumerate() {
        paint_setup_check(
            p,
            x + FIND_SETUP_X1,
            y + 8 + i as u16,
            *on,
            st.focus == *f,
            lab,
            pal,
        );
    }
    for (i, (f, on, lab)) in right.iter().enumerate() {
        paint_setup_check(
            p,
            x + FIND_SETUP_X2,
            y + 8 + i as u16,
            *on,
            st.focus == *f,
            lab,
            pal,
        );
    }
    paint_hline(p, x, y + 14, w, pal);
    let (ok, cancel) = setup_ok_cancel(st.focus);
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(x + FIND_SETUP_OK_X, y + 15);
    p.text(&ok);
    p.goto(x + FIND_SETUP_CANCEL_X, y + 15);
    p.text(&cancel);
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

pub fn draw_find_dialog(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    st: &FindDialogState,
) {
    if st.phase == FindDialogPhase::Setup {
        draw_find_setup_dialog(p, cols, rows, pal, st);
        if let Some(picker) = &st.tree_picker {
            draw_find_tree_picker(p, cols, rows, pal, picker);
        }
        return;
    }
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
    // Status line
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(x + 2, y + 1);
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
    let list_top = y + FIND_RESULTS_LIST_TOP;
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
            if (0x20..=0x7e).contains(&bytes[i]) && y < rows as usize && x < cols as usize {
                grid[y][x] = bytes[i] as char;
                x += 1;
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

    fn draw_state_at(st: &FindDialogState, cols: u16, rows: u16) -> Vec<String> {
        let pal = McPalette::default();
        let mut buf = Vec::new();
        {
            let mut p = Painter { out: &mut buf };
            draw_find_dialog(&mut p, cols, rows, pal, st);
        }
        rasterize(&buf, cols, rows)
    }

    fn draw_state(st: &FindDialogState) -> Vec<String> {
        draw_state_at(st, 80, 28)
    }

    fn results_state() -> FindDialogState {
        let mut st = FindDialogState::new(PathBuf::from("/tmp"));
        st.phase = FindDialogPhase::Results;
        st
    }

    #[test]
    fn raster_setup_matches_live_gnu_4_8_30_cells() {
        let st = FindDialogState::new(PathBuf::from("/tmp/mcr-fixture"));
        let grid = draw_state_at(&st, 80, 24);
        let text = screen_text(&grid);
        let (x, y) = find_setup_origin(80, 24);
        assert_eq!((x, y), (7, 3), "66×17 WPOS_CENTER on 80×24");
        let row = |dy: u16| grid[y as usize + dy as usize].as_str();
        assert!(row(0).contains(" Find File "), "{text}");
        assert_eq!(&row(1)[9..18], "Start at:");
        assert!(row(2).contains("[ Tree ]"), "{text}");
        assert_eq!(row(2).find("[ Tree ]"), Some(63));
        assert!(row(3).contains("[x] Enable ignore directories:"), "{text}");
        assert_eq!(row(3).find("[x] Enable ignore directories:"), Some(9));
        assert_eq!(&row(6)[9..19], "File name:");
        assert_eq!(&row(6)[41..49], "Content:");
        assert!(row(8).contains("[x] Find recursively"), "{text}");
        assert_eq!(row(8).find("[x] Find recursively"), Some(9));
        assert_eq!(row(8).find("[ ] Whole words"), Some(41));
        assert!(row(9).contains("[ ] Follow symlinks"), "{text}");
        assert!(row(9).contains("[ ] Regular expression"), "{text}");
        assert!(row(10).contains("[x] Using shell patterns"), "{text}");
        assert!(row(10).contains("[x] Case sensitive"), "{text}");
        assert!(row(11).contains("[x] Case sensitive"), "{text}");
        assert!(row(11).contains("[ ] All charsets"), "{text}");
        assert!(row(12).contains("[ ] All charsets"), "{text}");
        assert!(row(12).contains("[ ] First hit"), "{text}");
        assert!(row(13).contains("[ ] Skip hidden"), "{text}");
        assert!(row(15).contains("[< OK >]"), "{text}");
        assert!(row(15).contains("[ Cancel ]"), "{text}");
        assert_eq!(row(15).find("[< OK >]"), Some(30));
        assert_eq!(row(15).find("[ Cancel ]"), Some(39));
        assert!(
            !text.contains("[ Stop ]") && !text.contains("[ Again ]"),
            "setup must not paint the results action row:\n{text}"
        );
    }

    #[test]
    fn raster_results_buttons_are_ok_stop_again_chdir_panelize_quit() {
        let st = results_state();
        let grid = draw_state(&st);
        let text = screen_text(&grid);
        assert!(
            text.contains("[ OK ]") || text.contains("< OK >"),
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
            .find(|r| r.contains("[ Stop ]") && (r.contains("[ OK ]") || r.contains("< OK >")))
            .expect("button row");
        let ok = btn_row
            .find("[ OK ]")
            .or_else(|| btn_row.find("< OK >"))
            .unwrap();
        let stop = btn_row.find("[ Stop ]").unwrap();
        let again = btn_row.find("[ Again ]").unwrap();
        let chdir = btn_row.find("[ Chdir ]").unwrap();
        let panelize = btn_row.find("[ Panelize ]").unwrap();
        let quit = btn_row.find("[ Quit ]").unwrap();
        assert!(ok < stop && stop < again && again < chdir && chdir < panelize && panelize < quit);
    }

    #[test]
    fn raster_paused_stop_slot_is_start() {
        let mut st = results_state();
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
        let st = results_state();
        let h = find_dialog_height(28);
        let y = (28 - h) / 2;
        let btn_y = y + h - 2;
        let ok =
            (0..80u16).find_map(|mx| find_action_button_at(80, 28, mx, btn_y, st.focus, false));
        assert_eq!(ok, Some(F::ButtonOk));
    }

    #[test]
    fn setup_hit_ok_and_tree() {
        let (x, y) = find_setup_origin(80, 24);
        assert_eq!(
            find_setup_widget_at(80, 24, x + FIND_SETUP_OK_X + 1, y + 15),
            Some(F::ButtonOk)
        );
        assert_eq!(
            find_setup_widget_at(80, 24, x + FIND_SETUP_TREE_X + 1, y + 2),
            Some(F::Tree)
        );
        assert_eq!(
            find_setup_widget_at(80, 24, x + FIND_SETUP_X1, y + 8),
            Some(F::FindRecursively)
        );
        assert_eq!(
            find_setup_widget_at(80, 24, x + FIND_SETUP_X2, y + 8),
            Some(F::WholeWords)
        );
    }
}
