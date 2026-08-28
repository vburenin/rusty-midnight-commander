use crate::mc_colors::McPalette;
use crate::widgets::Painter;
use rmc_core::panelize::{ExternalPanelizeDialogState, ExternalPanelizeFocus as F};

pub fn draw_external_panelize_dialog(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    st: &ExternalPanelizeDialogState,
) {
    let w = (cols as usize).min(64) as u16;
    let h = (rows as usize).clamp(12, 20) as u16;
    let x = (cols - w) / 2;
    let y = (rows - h) / 2;
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
    let ttl = " External panelize ";
    let tx = x + (w.saturating_sub(ttl.len() as u16)) / 2;
    p.goto(tx, y);
    p.text(ttl);

    let list_top = y + 2;
    let list_bottom = y + h - 5;
    let list_h = list_bottom.saturating_sub(list_top).saturating_add(1);
    for i in 0..list_h {
        let row_y = list_top + i;
        p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
        p.goto(x + 2, row_y);
        p.text(&" ".repeat((w - 4) as usize));
        let idx = st.scroll_top as u16 + i;
        let idx_usize = idx as usize;
        if let Some(entry) = st.commands.get(idx_usize) {
            if idx_usize == st.selected_index && matches!(st.focus, F::List) {
                p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
            } else {
                p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
            }
            let t = truncate(&entry.name, (w - 6) as usize);
            p.goto(x + 3, row_y);
            p.text(&t);
        }
    }

    let cmd_y = y + h - 4;
    let cmd_focus = matches!(st.focus, F::Command);
    p.set_fg_bg(
        if cmd_focus {
            pal.dfocus_fg
        } else {
            pal.dialog_default_fg
        },
        if cmd_focus {
            pal.dfocus_bg
        } else {
            pal.dialog_default_bg
        },
    );
    p.goto(x + 2, cmd_y);
    let field_w = (w - 4) as usize;
    let t = truncate(&st.command, field_w);
    p.text(&format!(
        "{t}{}",
        " ".repeat(field_w.saturating_sub(t.len()))
    ));

    let sel = |want: F, txt: &str| -> String {
        if st.focus == want {
            format!("< {txt} >")
        } else {
            format!("[ {txt} ]")
        }
    };
    p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
    let btns = format!(
        "{}  {}  {}  {}",
        sel(F::ButtonAddNew, "Add new"),
        sel(F::ButtonPanelize, "Panelize"),
        sel(F::ButtonRemove, "Remove"),
        sel(F::ButtonCancel, "Cancel")
    );
    let bx = x + (w.saturating_sub(btns.len() as u16)) / 2;
    p.goto(bx, y + h - 2);
    p.text(&btns);

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

    if let Some(name) = &st.name_prompt {
        draw_name_prompt(p, cols, rows, pal, name);
    }
}

fn draw_name_prompt(p: &mut Painter, cols: u16, rows: u16, pal: McPalette, value: &str) {
    let w = (cols as usize).min(50) as u16;
    let h = 7u16;
    let x = (cols - w) / 2;
    let y = (rows - h) / 2;
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
    let ttl = " Command name ";
    let tx = x + (w.saturating_sub(ttl.len() as u16)) / 2;
    p.goto(tx, y);
    p.text(ttl);
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(x + 2, y + 2);
    p.text("Enter command name:");
    p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
    p.goto(x + 2, y + 4);
    let field_w = (w - 4) as usize;
    let t = truncate(value, field_w);
    p.text(&format!(
        "{t}{}",
        " ".repeat(field_w.saturating_sub(t.len()))
    ));
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
