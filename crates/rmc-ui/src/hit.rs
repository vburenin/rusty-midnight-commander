//! Hit-testing for GNU-style dialog buttons and the F9 dropdown.
//!
//! Geometry must stay in lockstep with the matching `render` draw functions.
//! Clicks land on the painted label, not a guessed cell next to it.

use rmc_core::app::CopyDialogFocus;
use rmc_core::fileop::FileOpProgressView;

use crate::render::top_menu_items;

/// Gap of two spaces between labels in `paint_dialog_button_cluster`.
const BUTTON_GAP: u16 = 2;

/// Which label in a centered `[ OK ]  [ Cancel ]` row contains `(mx, my)`.
pub(crate) fn button_cluster_at(
    bx: u16,
    by: u16,
    labels: &[&str],
    mx: u16,
    my: u16,
) -> Option<usize> {
    if my != by {
        return None;
    }
    let mut cx = bx;
    for (i, lab) in labels.iter().enumerate() {
        let end = cx.saturating_add(lab.len() as u16);
        if mx >= cx && mx < end {
            return Some(i);
        }
        cx = end;
        if i + 1 < labels.len() {
            cx = cx.saturating_add(BUTTON_GAP);
        }
    }
    None
}

fn dialog_box_rect(cols: u16, rows: u16) -> Option<(u16, u16, u16, u16)> {
    let w = (cols as usize).min(60) as u16;
    let h = 7u16;
    if cols < w || rows < h {
        return None;
    }
    let x = (cols - w) / 2;
    let y = (rows - h) / 2;
    Some((x, y, w, h))
}

/// `draw_dialog_box` / Delete / Confirm / PromptInput button row.
pub(crate) fn dialog_box_button_at(
    cols: u16,
    rows: u16,
    mx: u16,
    my: u16,
    labels: &[&str],
) -> Option<usize> {
    let (x, y, w, h) = dialog_box_rect(cols, rows)?;
    let btns_w = labels.iter().map(|s| s.len()).sum::<usize>()
        + (BUTTON_GAP as usize) * labels.len().saturating_sub(1);
    let bx = x + (w.saturating_sub(btns_w as u16)) / 2;
    button_cluster_at(bx, y + h - 2, labels, mx, my)
}

/// Delete Yes/No. `true` is Yes (OK).
pub(crate) fn delete_button_at(
    cols: u16,
    rows: u16,
    mx: u16,
    my: u16,
    _focus_ok: bool,
) -> Option<bool> {
    let w = 21u16.min(cols.saturating_sub(2)).max(17);
    let h = 6u16;
    if cols < w || rows < h {
        return None;
    }
    let x = (cols - w) / 2;
    let y = (rows - h) / 2;
    let items = ["[ Yes ]", "[ No ]"];
    let btns_w = items.iter().map(|s| s.len()).sum::<usize>() + BUTTON_GAP as usize;
    let bx = x + (w.saturating_sub(btns_w as u16)) / 2;
    button_cluster_at(bx, y + h - 2, &items, mx, my).map(|i| i == 0)
}

/// DialogConfirm / PromptInput: `true` is OK, `false` is Cancel.
pub(crate) fn confirm_button_at(cols: u16, rows: u16, mx: u16, my: u16) -> Option<bool> {
    dialog_box_button_at(cols, rows, mx, my, &["< OK >", "Cancel"]).map(|i| i == 0)
}

/// Mkdir OK/Cancel. `true` is OK.
pub(crate) fn mkdir_button_at(
    cols: u16,
    rows: u16,
    mx: u16,
    my: u16,
    focus_ok: bool,
) -> Option<bool> {
    let w = (cols as usize).min(44) as u16;
    let h = 6u16;
    if cols < w || rows < h {
        return None;
    }
    let x = (cols - w) / 2;
    let y = (rows - h) / 2;
    let ok = if focus_ok { "[< OK >]" } else { "[ OK ]" };
    let cancel = if focus_ok {
        "[ Cancel ]"
    } else {
        "[< Cancel >]"
    };
    let items = [ok, cancel];
    let btns_w = items.iter().map(|s| s.len()).sum::<usize>() + BUTTON_GAP as usize;
    let bx = x + (w.saturating_sub(btns_w as u16)) / 2;
    button_cluster_at(bx, y + h - 2, &items, mx, my).map(|i| i == 0)
}

/// Input / Quick cd OK/Cancel. `true` is OK.
pub(crate) fn input_dialog_button_at(
    cols: u16,
    rows: u16,
    mx: u16,
    my: u16,
    focus_ok: bool,
) -> Option<bool> {
    let w = (cols as usize).min(66) as u16;
    let h = 9u16;
    if cols < w || rows < h {
        return None;
    }
    let x = (cols - w) / 2;
    let y = (rows - h) / 2;
    let ok = if focus_ok { "< OK >" } else { "[ OK ]" };
    let cancel = if focus_ok { " Cancel " } else { "[ Cancel ]" };
    let items = [ok, cancel];
    let btns_w = items.iter().map(|s| s.len()).sum::<usize>() + BUTTON_GAP as usize;
    let bx = x + (w.saturating_sub(btns_w as u16)) / 2;
    button_cluster_at(bx, y + h - 2, &items, mx, my).map(|i| i == 0)
}

fn copy_button_label(focus: CopyDialogFocus, which: CopyDialogFocus, txt: &str) -> String {
    if focus == which {
        format!("[< {txt} >]")
    } else {
        format!("[ {txt} ]")
    }
}

/// Copy/Move mask, destination, checkboxes, and OK / Background / Cancel.
pub(crate) fn copy_move_hit_at(
    cols: u16,
    rows: u16,
    mx: u16,
    my: u16,
    focus: CopyDialogFocus,
) -> Option<CopyDialogFocus> {
    let w = (cols as usize).min(66) as u16;
    let h = 12u16.min(rows.saturating_sub(1)).max(7);
    if cols < w || rows < h {
        return None;
    }
    let x = (cols - w) / 2;
    let y = (rows - h) / 2;
    if my == y + 2 && mx >= x + 2 && mx < x + w.saturating_sub(2) {
        return Some(CopyDialogFocus::Mask);
    }
    if my == y + 5 && mx >= x + 2 && mx < x + w.saturating_sub(2) {
        return Some(CopyDialogFocus::To);
    }
    let shell = "[x] Using shell patterns";
    let shell_x = x + w.saturating_sub(2 + shell.len() as u16);
    if my == y + 3 && mx >= shell_x && mx < shell_x.saturating_add(shell.len() as u16) {
        return Some(CopyDialogFocus::Checkbox1);
    }
    const LEFT: [(CopyDialogFocus, &str, u16); 2] = [
        (CopyDialogFocus::Checkbox2, "[ ] Follow links", 7),
        (CopyDialogFocus::Checkbox3, "[ ] Preserve attributes", 8),
    ];
    const RIGHT: [(CopyDialogFocus, &str, u16); 2] = [
        (
            CopyDialogFocus::Checkbox4,
            "[ ] Dive into subdir if exists",
            7,
        ),
        (CopyDialogFocus::Checkbox5, "[ ] Stable symlinks", 8),
    ];
    for (f, text, row) in LEFT {
        if my == y + row {
            let start = x + 2;
            if mx >= start && mx < start.saturating_add(text.len() as u16) {
                return Some(f);
            }
        }
    }
    for (f, text, row) in RIGHT {
        if my == y + row {
            let start = x + 35;
            if mx >= start && mx < start.saturating_add(text.len() as u16) {
                return Some(f);
            }
        }
    }
    if my == y + h - 2 {
        let ok = copy_button_label(focus, CopyDialogFocus::Ok, "OK");
        let bg = copy_button_label(focus, CopyDialogFocus::Background, "Background");
        let cancel = copy_button_label(focus, CopyDialogFocus::Cancel, "Cancel");
        let labels = [ok.as_str(), bg.as_str(), cancel.as_str()];
        let btns_w = labels.iter().map(|s| s.len()).sum::<usize>() + labels.len().saturating_sub(1);
        let bx = x + (w.saturating_sub(btns_w as u16)) / 2;
        return match button_cluster_at_gap(bx, my, &labels, mx, my, 1) {
            Some(0) => Some(CopyDialogFocus::Ok),
            Some(1) => Some(CopyDialogFocus::Background),
            Some(2) => Some(CopyDialogFocus::Cancel),
            _ => None,
        };
    }
    None
}

fn button_cluster_at_gap(
    bx: u16,
    by: u16,
    labels: &[&str],
    mx: u16,
    my: u16,
    gap: u16,
) -> Option<usize> {
    if my != by {
        return None;
    }
    let mut cx = bx;
    for (i, lab) in labels.iter().enumerate() {
        let end = cx.saturating_add(lab.len() as u16);
        if mx >= cx && mx < end {
            return Some(i);
        }
        cx = end;
        if i + 1 < labels.len() {
            cx = cx.saturating_add(gap);
        }
    }
    None
}

/// File-operations dialog `[ Abort ]` (drawn as `< Abort >`).
pub(crate) fn file_op_abort_at(
    cols: u16,
    rows: u16,
    mx: u16,
    my: u16,
    view: &FileOpProgressView,
) -> bool {
    let mut n = 2usize;
    if view.source_name.is_some() {
        n += 2;
    }
    if view.file_bar.is_some() {
        n += 2;
    }
    if view.total_bar.is_some() {
        n += 2;
    }
    n += 1;
    if view.total_bytes.is_some() {
        n += 1;
    }
    let w = (cols as usize).min(74) as u16;
    let h = (n as u16)
        .saturating_add(4)
        .min(rows.saturating_sub(2))
        .max(7);
    let x = cols.saturating_sub(w) / 2;
    let y = rows.saturating_sub(h) / 2;
    let abort = "< Abort >";
    let bx = x + (w.saturating_sub(abort.len() as u16)) / 2;
    my == y + h - 2 && mx >= bx && mx < bx.saturating_add(abort.len() as u16)
}

/// Dropdown row under the open F9 menu. `None` if the click is not on an item.
pub(crate) fn menu_dropdown_item_at(
    mx: u16,
    my: u16,
    top_index: usize,
    horizontal_split: bool,
) -> Option<usize> {
    let items = top_menu_items(top_index);
    let x = rmc_core::layout::menu_bar_item_start(top_index, horizontal_split);
    let y = 1u16;
    let w = (items.iter().map(|s| s.len()).max().unwrap_or(8) + 4) as u16;
    let h = items.len() as u16 + 2;
    if mx < x || mx >= x.saturating_add(w) {
        return None;
    }
    if my <= y || my >= y.saturating_add(h.saturating_sub(1)) {
        return None;
    }
    let i = (my - y - 1) as usize;
    (i < items.len()).then_some(i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmc_core::app::CopyDialogFocus as F;

    #[test]
    fn delete_yes_and_no_are_distinct_hits() {
        let yes = (0..80u16)
            .find(|&x| delete_button_at(80, 24, x, (24 - 6) / 2 + 4, true) == Some(true))
            .expect("Yes");
        let no = (0..80u16)
            .find(|&x| delete_button_at(80, 24, x, (24 - 6) / 2 + 4, true) == Some(false))
            .expect("No");
        assert_ne!(yes, no);
        assert!(delete_button_at(80, 24, yes, 0, true).is_none());
    }

    #[test]
    fn copy_ok_background_cancel_are_distinct() {
        let by = (24u16 - 12) / 2 + 10;
        let ok = (0..80u16)
            .find(|&x| copy_move_hit_at(80, 24, x, by, F::To) == Some(F::Ok))
            .expect("OK");
        let bg = (0..80u16)
            .find(|&x| copy_move_hit_at(80, 24, x, by, F::To) == Some(F::Background))
            .expect("Background");
        let cancel = (0..80u16)
            .find(|&x| copy_move_hit_at(80, 24, x, by, F::To) == Some(F::Cancel))
            .expect("Cancel");
        assert!(ok < bg && bg < cancel);
    }

    #[test]
    fn menu_file_copy_row_is_hittable() {
        // File menu is top_index 1; Copy is the fourth item (index 3), row y=1+1+3=5.
        assert_eq!(crate::render::FILE_MENU_ITEMS[3], "Copy");
        assert_eq!(
            menu_dropdown_item_at(12, 5, 1, false),
            Some(3),
            "click on File→Copy row"
        );
        assert!(menu_dropdown_item_at(12, 0, 1, false).is_none());
    }
}
