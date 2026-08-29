//! Hit-tests for GNU-class file-op dialogs. Geometry matches `render.rs`.
//!
//! Used by mouse dispatch and by tests that prove OK / Cancel / Background /
//! overwrite Yes/No/All actually fire — not that a function merely exists.

use rmc_core::app::{CopyDialogFocus, OverwriteFocus};

/// Shared dialog frame: centered, clamped to the terminal.
pub(crate) fn centered_dialog(
    cols: u16,
    rows: u16,
    max_w: u16,
    max_h: u16,
) -> (u16, u16, u16, u16) {
    let w = (cols as usize).min(max_w as usize) as u16;
    let h = max_h.min(rows.saturating_sub(1)).max(1);
    let x = cols.saturating_sub(w) / 2;
    let y = rows.saturating_sub(h) / 2;
    (x, y, w, h)
}

/// Hit a `[ label ]` / `< label >` cluster on one row (`"  "` between items).
pub(crate) fn button_cluster_hit(
    mx: u16,
    my: u16,
    row_y: u16,
    start_x: u16,
    labels: &[&str],
) -> Option<usize> {
    if my != row_y {
        return None;
    }
    let mut cx = start_x;
    for (i, lab) in labels.iter().enumerate() {
        let end = cx.saturating_add(lab.len() as u16);
        if mx >= cx && mx < end {
            return Some(i);
        }
        cx = end.saturating_add(2);
    }
    None
}

fn focused_bracket(focused: bool, text: &str) -> String {
    if focused {
        format!("< {text} >")
    } else {
        format!("[ {text} ]")
    }
}

/// Copy/Move dialog: mask, dest, five checkboxes, OK / Background / Cancel.
pub(crate) fn copy_dialog_geom(cols: u16, rows: u16) -> (u16, u16, u16, u16) {
    centered_dialog(cols, rows, 74, 15)
}

pub(crate) fn copy_dialog_button_labels(focus: CopyDialogFocus) -> [String; 3] {
    [
        focused_bracket(matches!(focus, CopyDialogFocus::Ok), "OK"),
        focused_bracket(matches!(focus, CopyDialogFocus::Background), "Background"),
        focused_bracket(matches!(focus, CopyDialogFocus::Cancel), "Cancel"),
    ]
}

pub(crate) fn copy_dialog_hit(
    cols: u16,
    rows: u16,
    mx: u16,
    my: u16,
    focus: CopyDialogFocus,
) -> Option<CopyDialogFocus> {
    let (x, y, w, h) = copy_dialog_geom(cols, rows);
    if mx < x || mx >= x.saturating_add(w) || my < y || my >= y.saturating_add(h) {
        return None;
    }
    if my == y + 3 {
        return Some(CopyDialogFocus::Mask);
    }
    if my == y + 5 {
        return Some(CopyDialogFocus::To);
    }
    if my >= y + 7 && my <= y + 11 {
        return Some(match my - (y + 7) {
            0 => CopyDialogFocus::Checkbox1,
            1 => CopyDialogFocus::Checkbox2,
            2 => CopyDialogFocus::Checkbox3,
            3 => CopyDialogFocus::Checkbox4,
            _ => CopyDialogFocus::Checkbox5,
        });
    }
    let labels = copy_dialog_button_labels(focus);
    let lab_refs: [&str; 3] = [&labels[0], &labels[1], &labels[2]];
    let bar: String = lab_refs.join("  ");
    let bx = x + (w.saturating_sub(bar.len() as u16)) / 2;
    match button_cluster_hit(mx, my, y + h - 2, bx, &lab_refs) {
        Some(0) => Some(CopyDialogFocus::Ok),
        Some(1) => Some(CopyDialogFocus::Background),
        Some(2) => Some(CopyDialogFocus::Cancel),
        _ => None,
    }
}

/// Delete confirmation: Yes / No on the GNU dialog-box button row.
pub(crate) fn delete_dialog_geom(cols: u16, rows: u16) -> (u16, u16, u16, u16) {
    centered_dialog(cols, rows, 60, 7)
}

pub(crate) fn delete_dialog_button_labels(focus_ok: bool) -> [String; 2] {
    let yes = if focus_ok { "< Yes >" } else { "  Yes  " };
    let no = if focus_ok { "  No  " } else { "< No >" };
    [yes.to_string(), no.to_string()]
}

pub(crate) fn delete_dialog_hit(
    cols: u16,
    rows: u16,
    mx: u16,
    my: u16,
    focus_ok: bool,
) -> Option<bool> {
    let (x, y, w, h) = delete_dialog_geom(cols, rows);
    let labels = delete_dialog_button_labels(focus_ok);
    let lab_refs: [&str; 2] = [&labels[0], &labels[1]];
    let btns_w = labels.iter().map(|s| s.len()).sum::<usize>() + 2;
    let bx = x + (w.saturating_sub(btns_w as u16)) / 2;
    match button_cluster_hit(mx, my, y + h - 2, bx, &lab_refs) {
        Some(0) => Some(true),
        Some(1) => Some(false),
        _ => None,
    }
}

/// Mkdir: OK / Cancel on the Create-directory dialog.
pub(crate) fn mkdir_dialog_geom(cols: u16, rows: u16) -> (u16, u16, u16, u16) {
    centered_dialog(cols, rows, 60, 7)
}

pub(crate) fn mkdir_dialog_button_labels(focus_ok: bool) -> [String; 2] {
    let ok = if focus_ok { "< OK >" } else { "  OK  " };
    let cancel = if focus_ok { " Cancel " } else { "[ Cancel ]" };
    [ok.to_string(), cancel.to_string()]
}

pub(crate) fn mkdir_dialog_hit(
    cols: u16,
    rows: u16,
    mx: u16,
    my: u16,
    focus_ok: bool,
) -> Option<bool> {
    let (x, y, w, h) = mkdir_dialog_geom(cols, rows);
    let labels = mkdir_dialog_button_labels(focus_ok);
    let lab_refs: [&str; 2] = [&labels[0], &labels[1]];
    let btns_w = labels.iter().map(|s| s.len()).sum::<usize>() + 2;
    let bx = x + (w.saturating_sub(btns_w as u16)) / 2;
    match button_cluster_hit(mx, my, y + h - 2, bx, &lab_refs) {
        Some(0) => Some(true),
        Some(1) => Some(false),
        _ => None,
    }
}

/// Overwrite/Replace: Yes / No / All / … plus the zero-length checkbox.
pub(crate) fn overwrite_dialog_geom(cols: u16, rows: u16) -> (u16, u16, u16, u16) {
    let w = (cols as usize).min(70) as u16;
    let h = 13u16.min(rows.saturating_sub(2)).max(11.min(rows));
    let x = cols.saturating_sub(w) / 2;
    let y = rows.saturating_sub(h) / 2;
    (x, y, w, h)
}

fn overwrite_label(focus: OverwriteFocus, kind: OverwriteFocus) -> String {
    focused_bracket(focus == kind, kind.label())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn overwrite_dialog_hit(
    cols: u16,
    rows: u16,
    mx: u16,
    my: u16,
    op: rmc_core::app::CopyMoveOp,
    src_size: u64,
    dst_size: u64,
    focus: OverwriteFocus,
) -> Option<OverwriteFocus> {
    let (x, y, w, h) = overwrite_dialog_geom(cols, rows);
    if my == y + 6 && mx >= x + 2 && mx < x.saturating_add(w).saturating_sub(1) {
        return Some(OverwriteFocus::ZeroLength);
    }
    let rows_btns = rmc_core::app::overwrite_button_rows(op, src_size, dst_size);
    let n_rows = rows_btns.len() as u16;
    for (i, row) in rows_btns.iter().enumerate() {
        let row_y = y + h - 1 - n_rows + i as u16;
        let labels: Vec<String> = row.iter().map(|k| overwrite_label(focus, *k)).collect();
        let lab_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let mut width = 0usize;
        for (j, s) in lab_refs.iter().enumerate() {
            width += s.len();
            if j + 1 != lab_refs.len() {
                width += 2;
            }
        }
        let bx = x + (w.saturating_sub(width as u16)) / 2;
        if let Some(idx) = button_cluster_hit(mx, my, row_y, bx, &lab_refs) {
            return Some(row[idx]);
        }
    }
    None
}

/// Sample x on a button cluster for tests (first cell of `labels[index]`).
#[allow(dead_code)] // used from `dialog_hit` and `feature_matrix` tests
pub(crate) fn button_cluster_x(start_x: u16, labels: &[&str], index: usize) -> u16 {
    let mut cx = start_x;
    for (i, lab) in labels.iter().enumerate() {
        if i == index {
            return cx;
        }
        cx = cx.saturating_add(lab.len() as u16).saturating_add(2);
    }
    cx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_ok_cancel_background_hits_are_distinct() {
        let cols = 80u16;
        let rows = 24u16;
        let focus = CopyDialogFocus::To;
        let (x, y, w, h) = copy_dialog_geom(cols, rows);
        let labels = copy_dialog_button_labels(focus);
        let lab_refs: [&str; 3] = [&labels[0], &labels[1], &labels[2]];
        let bar: String = lab_refs.join("  ");
        let bx = x + (w.saturating_sub(bar.len() as u16)) / 2;
        let by = y + h - 2;
        assert_eq!(
            copy_dialog_hit(cols, rows, button_cluster_x(bx, &lab_refs, 0), by, focus),
            Some(CopyDialogFocus::Ok)
        );
        assert_eq!(
            copy_dialog_hit(cols, rows, button_cluster_x(bx, &lab_refs, 1), by, focus),
            Some(CopyDialogFocus::Background)
        );
        assert_eq!(
            copy_dialog_hit(cols, rows, button_cluster_x(bx, &lab_refs, 2), by, focus),
            Some(CopyDialogFocus::Cancel)
        );
        assert_eq!(
            copy_dialog_hit(cols, rows, x + 2, y + 3, focus),
            Some(CopyDialogFocus::Mask)
        );
    }

    #[test]
    fn delete_and_mkdir_yes_no_ok_cancel_hits() {
        let cols = 80u16;
        let rows = 24u16;
        let (x, y, w, h) = delete_dialog_geom(cols, rows);
        let labels = delete_dialog_button_labels(true);
        let lab_refs: [&str; 2] = [&labels[0], &labels[1]];
        let btns_w = labels.iter().map(|s| s.len()).sum::<usize>() + 2;
        let bx = x + (w.saturating_sub(btns_w as u16)) / 2;
        let by = y + h - 2;
        assert_eq!(
            delete_dialog_hit(cols, rows, button_cluster_x(bx, &lab_refs, 0), by, true),
            Some(true)
        );
        assert_eq!(
            delete_dialog_hit(cols, rows, button_cluster_x(bx, &lab_refs, 1), by, true),
            Some(false)
        );

        let (x, y, w, h) = mkdir_dialog_geom(cols, rows);
        let labels = mkdir_dialog_button_labels(false);
        let lab_refs: [&str; 2] = [&labels[0], &labels[1]];
        let btns_w = labels.iter().map(|s| s.len()).sum::<usize>() + 2;
        let bx = x + (w.saturating_sub(btns_w as u16)) / 2;
        let by = y + h - 2;
        assert_eq!(
            mkdir_dialog_hit(cols, rows, button_cluster_x(bx, &lab_refs, 0), by, false),
            Some(true)
        );
        assert_eq!(
            mkdir_dialog_hit(cols, rows, button_cluster_x(bx, &lab_refs, 1), by, false),
            Some(false)
        );
    }

    #[test]
    fn overwrite_yes_no_all_are_hittable() {
        let cols = 80u16;
        let rows = 24u16;
        let op = rmc_core::app::CopyMoveOp::Copy;
        let focus = OverwriteFocus::Yes;
        let (x, y, w, h) = overwrite_dialog_geom(cols, rows);
        let row = [
            OverwriteFocus::Yes,
            OverwriteFocus::No,
            OverwriteFocus::All,
            OverwriteFocus::Older,
        ];
        let labels: Vec<String> = row.iter().map(|k| overwrite_label(focus, *k)).collect();
        let lab_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let mut width = 0usize;
        for (j, s) in lab_refs.iter().enumerate() {
            width += s.len();
            if j + 1 != lab_refs.len() {
                width += 2;
            }
        }
        let n_rows = 3u16;
        let row_y = y + h - 1 - n_rows;
        let bx = x + (w.saturating_sub(width as u16)) / 2;
        assert_eq!(
            overwrite_dialog_hit(
                cols,
                rows,
                button_cluster_x(bx, &lab_refs, 0),
                row_y,
                op,
                100,
                40,
                focus
            ),
            Some(OverwriteFocus::Yes)
        );
        assert_eq!(
            overwrite_dialog_hit(
                cols,
                rows,
                button_cluster_x(bx, &lab_refs, 1),
                row_y,
                op,
                100,
                40,
                focus
            ),
            Some(OverwriteFocus::No)
        );
        assert_eq!(
            overwrite_dialog_hit(
                cols,
                rows,
                button_cluster_x(bx, &lab_refs, 2),
                row_y,
                op,
                100,
                40,
                focus
            ),
            Some(OverwriteFocus::All)
        );
    }
}
