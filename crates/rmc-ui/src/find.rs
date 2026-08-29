use crate::mc_colors::McPalette;
use crate::widgets::Painter;
use rmc_core::find::{
    find_display_rows, find_results_height, find_results_list_rows, find_results_origin,
    find_results_width, find_setup_origin, find_tree_picker_height, find_tree_picker_list_rows,
    FindDialogFocus as F, FindDialogPhase, FindDialogState, FIND_RESULTS_LIST_TOP,
    FIND_SETUP_CANCEL_X, FIND_SETUP_FIELD_W, FIND_SETUP_H, FIND_SETUP_OK_X, FIND_SETUP_TREE_X,
    FIND_SETUP_W, FIND_SETUP_X1, FIND_SETUP_X2,
};

fn results_btn(label: &str, marked_default: bool) -> String {
    if marked_default {
        format!("[< {label} >]")
    } else {
        format!("[ {label} ]")
    }
}

/// Live GNU 4.8.30 results buttons. Chdir stays the default (`[< Chdir >]`).
/// Suspend becomes Continue while the find is stopped.
pub fn find_results_button_rows(stopped: bool) -> [Vec<(F, String)>; 2] {
    let slot = if stopped { "Continue" } else { "Suspend" };
    [
        vec![
            (F::ButtonChdir, results_btn("Chdir", true)),
            (F::ButtonAgain, results_btn("Again", false)),
            (F::ButtonSuspend, results_btn(slot, false)),
            (F::ButtonQuit, results_btn("Quit", false)),
        ],
        vec![
            (F::ButtonPanelize, results_btn("Panelize", false)),
            (F::ButtonView, results_btn("View - F3", false)),
            (F::ButtonEdit, results_btn("Edit - F4", false)),
        ],
    ]
}

fn center_button_row(inner_w: usize, specs: &[(F, String)]) -> (usize, String) {
    let bar = specs
        .iter()
        .map(|(_, s)| s.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let pad = inner_w.saturating_sub(bar.chars().count()) / 2;
    (pad, bar)
}

/// Hit-test the GNU two-row results footer. `None` if the click is not on a button.
pub fn find_action_button_at(
    cols: u16,
    rows: u16,
    mx: u16,
    my: u16,
    _focus: F,
    stopped: bool,
) -> Option<F> {
    let w = find_results_width(cols);
    let h = find_results_height(rows);
    let (x, y) = find_results_origin(cols, rows);
    if w < 8 || h < 8 {
        return None;
    }
    let rel_y = my.saturating_sub(y);
    let row_i = if rel_y == h.saturating_sub(3) {
        0usize
    } else if rel_y == h.saturating_sub(2) {
        1
    } else {
        return None;
    };
    if mx < x || mx >= x.saturating_add(w) {
        return None;
    }
    let specs = find_results_button_rows(stopped);
    let inner_w = w.saturating_sub(2) as usize;
    let (pad, _) = center_button_row(inner_w, &specs[row_i]);
    let mut cx = x as usize + 1 + pad;
    for (i, (f, s)) in specs[row_i].iter().enumerate() {
        let end = cx + s.len();
        if (mx as usize) >= cx && (mx as usize) < end {
            return Some(*f);
        }
        cx = end;
        if i + 1 < specs[row_i].len() {
            cx += 1;
        }
    }
    None
}

/// Hit-test a results listbox row. Returns the hit index when the cell is a file line.
pub fn find_results_hit_at(
    cols: u16,
    rows: u16,
    mx: u16,
    my: u16,
    st: &FindDialogState,
) -> Option<usize> {
    let w = find_results_width(cols);
    let h = find_results_height(rows);
    let (x, y) = find_results_origin(cols, rows);
    if mx <= x || mx >= x.saturating_add(w).saturating_sub(1) {
        return None;
    }
    let list_top = y + FIND_RESULTS_LIST_TOP;
    let list_h = find_results_list_rows(h) as u16;
    if my < list_top || my >= list_top.saturating_add(list_h) {
        return None;
    }
    let disp = find_display_rows(&st.results.paths);
    let idx = st.scroll_top + (my - list_top) as usize;
    disp.get(idx).and_then(|r| r.hit_index())
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
    draw_find_results_dialog(p, cols, rows, pal, st);
    if let Some(picker) = &st.tree_picker {
        draw_find_tree_picker(p, cols, rows, pal, picker);
    }
}

fn find_results_title(st: &FindDialogState) -> String {
    let pat = match &st.params.name_pattern {
        rmc_core::find::NamePattern::Glob(s) => s.as_str(),
    };
    format!("Find File: \"{pat}\"")
}

fn find_results_state_text(st: &FindDialogState) -> String {
    if st.is_stopped() {
        "Stopped".into()
    } else if st.running {
        st.progress_dir
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| st.params.start_dir.display().to_string())
    } else {
        "Finished".into()
    }
}

fn draw_find_results_dialog(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    st: &FindDialogState,
) {
    let w = find_results_width(cols).min(cols);
    let h = find_results_height(rows).min(rows);
    if w < 8 || h < 8 {
        return;
    }
    let (x, y) = find_results_origin(cols, rows);
    paint_dialog_chrome(p, x, y, w, h, &find_results_title(st), pal);
    let list_top = y + FIND_RESULTS_LIST_TOP;
    let list_h = find_results_list_rows(h) as u16;
    let inner_w = w.saturating_sub(2) as usize;
    let text_w = inner_w.saturating_sub(1);
    let disp = find_display_rows(&st.results.paths);
    for i in 0..list_h {
        let row_y = list_top + i;
        p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
        p.goto(x + 1, row_y);
        p.text(&" ".repeat(inner_w));
        let idx = st.scroll_top + i as usize;
        if let Some(row) = disp.get(idx) {
            let highlight = row.hit_index() == Some(st.selected_index);
            p.set_fg_bg(
                if highlight {
                    pal.dfocus_fg
                } else {
                    pal.dialog_default_fg
                },
                if highlight {
                    pal.dfocus_bg
                } else {
                    pal.dialog_default_bg
                },
            );
            let t = truncate(&row.display_text(), text_w);
            p.goto(x + 1, row_y);
            p.text(&format!(
                "{t}{}",
                " ".repeat(text_w.saturating_sub(t.chars().count()))
            ));
        }
        // GNU listbox scrollbar in the last inner column.
        let sb = scrollbar_glyph(i, list_h, st.scroll_top, disp.len());
        p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
        p.goto(x + w - 2, row_y);
        p.text(&sb.to_string());
    }
    paint_hline(p, x, y + h - 7, w, pal);
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    let found = format!("  Found: {}", st.results.paths.len());
    p.goto(x + 1, y + h - 6);
    p.text(&format!(
        "{found}{}",
        " ".repeat(inner_w.saturating_sub(found.len()))
    ));
    if st.running && !st.is_stopped() {
        const SPIN: [char; 4] = ['-', '\\', '|', '/'];
        let ch = SPIN[(st.spin as usize) % SPIN.len()];
        p.goto(x + w - 2, y + h - 6);
        p.text(&ch.to_string());
    }
    let state = format!("  {}", find_results_state_text(st));
    p.goto(x + 1, y + h - 5);
    let stxt = truncate(&state, inner_w);
    p.text(&format!(
        "{stxt}{}",
        " ".repeat(inner_w.saturating_sub(stxt.chars().count()))
    ));
    paint_hline(p, x, y + h - 4, w, pal);
    let rows_spec = find_results_button_rows(st.is_stopped());
    for (ri, specs) in rows_spec.iter().enumerate() {
        let (pad, _) = center_button_row(inner_w, specs);
        let mut cx = x + 1 + pad as u16;
        let by = y + h - 3 + ri as u16;
        p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
        p.goto(x + 1, by);
        p.text(&" ".repeat(inner_w));
        for (f, lab) in specs {
            let focused = st.focus == *f;
            p.set_fg_bg(
                if focused {
                    pal.dfocus_fg
                } else {
                    pal.buttonbar_button_fg
                },
                if focused {
                    pal.dfocus_bg
                } else {
                    pal.buttonbar_button_bg
                },
            );
            p.goto(cx, by);
            p.text(lab);
            cx = cx.saturating_add(lab.len() as u16 + 1);
        }
    }
    if y.saturating_add(h) < rows {
        p.set_fg_bg(pal.shadow_fg, pal.shadow_bg);
        p.hline(
            x + 1,
            y + h,
            w.saturating_sub(1),
            ' ',
            pal.shadow_fg,
            pal.shadow_bg,
        );
    }
    if x.saturating_add(w) < cols {
        p.vline(x + w, y + 1, h, ' ', pal.shadow_fg, pal.shadow_bg);
    }
}

fn scrollbar_glyph(row: u16, list_h: u16, scroll_top: usize, total: usize) -> char {
    let list_h = list_h as usize;
    if list_h == 0 || total <= list_h {
        return ' ';
    }
    let last = list_h.saturating_sub(1);
    let r = row as usize;
    if r == 0 && scroll_top > 0 {
        return '^';
    }
    if r == last && scroll_top + list_h < total {
        return 'v';
    }
    let thumb = if total <= 1 {
        0
    } else {
        let max_top = total.saturating_sub(list_h);
        (scroll_top * last).checked_div(max_top).unwrap_or(0)
    };
    if r == thumb
        .max(if scroll_top > 0 { 1 } else { 0 })
        .min(last.saturating_sub(1))
    {
        '*'
    } else {
        ' '
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
    use rmc_core::find::{find_results_origin, FindDialogState};
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
        draw_state_at(st, 80, 24)
    }

    fn results_state() -> FindDialogState {
        let mut st = FindDialogState::new(PathBuf::from("/tmp/mcr-fixture"));
        st.phase = FindDialogPhase::Results;
        st.focus = F::ResultsList;
        st.results.paths = vec![
            PathBuf::from("/tmp/mcr-fixture/file1.txt"),
            PathBuf::from("/tmp/mcr-fixture/file2.txt"),
        ];
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
    fn raster_results_matches_live_gnu_4_8_30_cells() {
        let st = results_state();
        let grid = draw_state_at(&st, 80, 24);
        let text = screen_text(&grid);
        let (x, y) = find_results_origin(80, 24);
        assert_eq!((x, y), (9, 3), "62×18 WPOS on 80×24");
        assert_eq!(rmc_core::find::find_results_width(80), 62);
        assert_eq!(rmc_core::find::find_results_height(24), 18);
        let row = |dy: u16| grid[y as usize + dy as usize].as_str();
        assert!(row(0).contains("Find File: \"*\""), "{text}");
        assert!(row(1).contains(" /tmp/mcr-fixture"), "{text}");
        assert!(row(2).contains("    file1.txt"), "{text}");
        assert!(row(12).contains("Found: 2"), "{text}");
        assert!(row(13).contains("Finished"), "{text}");
        assert!(row(15).contains("[< Chdir >]"), "{text}");
        assert!(row(15).contains("[ Again ]"), "{text}");
        assert!(row(15).contains("[ Suspend ]"), "{text}");
        assert!(row(15).contains("[ Quit ]"), "{text}");
        assert_eq!(row(15).find("[< Chdir >]"), Some(19));
        assert_eq!(row(15).find("[ Again ]"), Some(31));
        assert_eq!(row(15).find("[ Suspend ]"), Some(41));
        assert_eq!(row(15).find("[ Quit ]"), Some(53));
        assert!(row(16).contains("[ Panelize ]"), "{text}");
        assert!(row(16).contains("[ View - F3 ]"), "{text}");
        assert!(row(16).contains("[ Edit - F4 ]"), "{text}");
        assert_eq!(row(16).find("[ Panelize ]"), Some(20));
        assert_eq!(row(16).find("[ View - F3 ]"), Some(33));
        assert_eq!(row(16).find("[ Edit - F4 ]"), Some(47));
        assert!(
            !text.contains("[ OK ]")
                && !text.contains("[ Stop ]")
                && !text.contains("[ Start ]")
                && !text.contains("[ Cancel ]"),
            "results must not paint the setup/old action row:\n{text}"
        );
    }

    #[test]
    fn raster_results_grows_on_100x30() {
        let st = results_state();
        let grid = draw_state_at(&st, 100, 30);
        let (x, y) = find_results_origin(100, 30);
        assert_eq!((x, y), (9, 3));
        assert_eq!(rmc_core::find::find_results_width(100), 82);
        assert_eq!(rmc_core::find::find_results_height(30), 24);
        let row = |dy: u16| grid[y as usize + dy as usize].as_str();
        assert!(row(0).contains("Find File: \"*\""));
        assert_eq!(row(21).find("[< Chdir >]"), Some(29));
        assert_eq!(row(22).find("[ Panelize ]"), Some(30));
        assert!(row(21).contains("[ Suspend ]"));
        assert!(row(22).contains("[ Edit - F4 ]"));
    }

    #[test]
    fn raster_stopped_suspend_slot_is_continue() {
        let mut st = results_state();
        st.stopped = true;
        st.focus = F::ButtonSuspend;
        let grid = draw_state(&st);
        let text = screen_text(&grid);
        let (_x, y) = find_results_origin(80, 24);
        let row = |dy: u16| grid[y as usize + dy as usize].as_str();
        assert!(row(13).contains("Stopped"), "{text}");
        assert!(row(15).contains("[ Continue ]"), "{text}");
        assert_eq!(row(15).find("[< Chdir >]"), Some(18));
        assert_eq!(row(15).find("[ Continue ]"), Some(40));
        assert!(
            !text.contains("[ Suspend ]")
                && !text.contains("[ Stop ]")
                && !text.contains("[ Start ]"),
            "stopped pair must say Continue:\n{text}"
        );
    }

    #[test]
    fn action_button_hit_chdir_and_view() {
        let st = results_state();
        let (_x, y) = find_results_origin(80, 24);
        let h = rmc_core::find::find_results_height(24);
        let row1 = y + h - 3;
        let row2 = y + h - 2;
        let chdir =
            (0..80u16).find_map(|mx| find_action_button_at(80, 24, mx, row1, st.focus, false));
        assert_eq!(chdir, Some(F::ButtonChdir));
        let view =
            (0..80u16).find_map(|mx| find_action_button_at(80, 24, mx, row2, st.focus, false));
        assert_eq!(view, Some(F::ButtonPanelize));
        let edit = find_action_button_at(80, 24, 47, row2, st.focus, false);
        assert_eq!(edit, Some(F::ButtonEdit));
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
