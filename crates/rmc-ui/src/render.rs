use crate::find::draw_find_dialog;
use crate::help::{initial_topic_or_contents, HelpIndex, HelpItem};
use crate::mc_colors::McPalette;
use crate::widgets::Painter;
use anyhow::Result;
use crossterm::style::Color;
use crossterm::terminal::{self, Clear, ClearType};
use crossterm::QueueableCommand;
use rmc_core::app::App;
use rmc_core::panel::{FileEntry, PanelMode};
use std::io::{stdout, Stdout};
use time::OffsetDateTime;

pub struct Renderer {
    palette: McPalette,
    out: Stdout,
    help_index: Option<HelpIndex>,
}

impl Renderer {
    pub fn new(palette: McPalette) -> Self {
        Self {
            palette,
            out: stdout(),
            help_index: None,
        }
    }

    pub fn draw(&mut self, app: &App) -> Result<()> {
        let (cols, rows) = terminal::size()?;
        let mut painter = Painter { out: &mut self.out };
        // Clear screen
        painter.out.queue(Clear(ClearType::All))?;
        // Full-screen subshell/output view short-circuit
        if app.subshell.show_output_screen {
            draw_subshell_fullscreen(&mut painter, cols, rows, self.palette, app)?;
            painter.out.flush()?;
            return Ok(());
        }
        // Full-screen modes: short-circuit to avoid drawing panels underneath.
        if let rmc_core::app::UiMode::Editor {
            buf,
            show_menu,
            status_msg,
            search_input,
            save_as_input,
            confirm_exit,
            ..
        } = &app.ui_mode
        {
            draw_editor(
                &mut painter,
                cols,
                rows,
                self.palette,
                buf,
                *show_menu,
                status_msg.as_deref(),
                search_input.as_deref(),
                save_as_input.as_deref(),
                confirm_exit.as_ref(),
            );
            painter.out.flush()?;
            return Ok(());
        }
        // Full-screen viewer mode short-circuit: draw only viewer chrome/content/status/fbar
        if let rmc_core::app::UiMode::Viewer {
            path,
            hex,
            wrap,
            offset,
            show_line_numbers,
            show_cr,
            search_prompt,
            goto_prompt,
            ..
        } = &app.ui_mode
        {
            draw_viewer(
                &mut painter,
                cols,
                rows,
                self.palette,
                path,
                *hex,
                *wrap,
                *offset,
                *show_line_numbers,
                *show_cr,
                search_prompt,
                goto_prompt,
            )?;
            painter.out.flush()?;
            return Ok(());
        }
        // Clear any lingering viewer state when not in viewer mode
        crate::terminal::viewer_clear_state();
        // Full-screen diff viewer short-circuit
        if let rmc_core::app::UiMode::Diff(state) = &app.ui_mode {
            draw_diff(&mut painter, cols, rows, self.palette, state)?;
            painter.out.flush()?;
            return Ok(());
        }
        // Full-screen Help short-circuit (after subshell/editor/viewer/diff)
        if let rmc_core::app::UiMode::Help { state, .. } = &app.ui_mode {
            if self.help_index.is_none() {
                self.help_index = HelpIndex::load_default().ok();
            }
            draw_help(
                &mut painter,
                cols,
                rows,
                self.palette,
                state,
                self.help_index.as_ref(),
            );
            painter.out.flush()?;
            return Ok(());
        }
        // Otherwise draw the normal dual-pane UI
        painter.fill_line(
            0,
            cols,
            self.palette.core_default_bg,
            self.palette.core_default_fg,
        );
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
        draw_gauge(&mut painter, gauge_row, cols, self.palette, app);
        draw_hint(&mut painter, hint_row, cols, self.palette);
        draw_cmdline(&mut painter, cmd_row, cols, self.palette, app);
        draw_fbar(&mut painter, fbar_row, cols, self.palette);
        // Overlays (dialogs)
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
        rmc_core::app::UiMode::OverwriteDialog {
            op,
            src_path: _,
            dst_path,
            focus,
        } => {
            draw_overwrite_dialog(p, cols, rows, pal, *op, dst_path, *focus);
        }
        rmc_core::app::UiMode::InputDialog {
            title,
            prompt,
            value,
            focus_ok,
            ..
        } => {
            draw_input_dialog(p, cols, rows, pal, title, prompt, value, *focus_ok);
        }
        rmc_core::app::UiMode::Help { .. } => {
            // Full-screen; nothing overlays
        }
        rmc_core::app::UiMode::UserMenu {
            title,
            entries,
            selected_index,
        } => {
            draw_user_menu_dialog(p, cols, rows, pal, title, entries, *selected_index);
        }
        rmc_core::app::UiMode::SortDialog {
            side,
            focus_index,
            by,
            reverse,
            dirs_first,
        } => {
            draw_sort_dialog(
                p,
                cols,
                rows,
                pal,
                *side,
                *focus_index,
                *by,
                *reverse,
                *dirs_first,
            );
        }
        rmc_core::app::UiMode::FindDialog(state) => {
            draw_find_dialog(p, cols, rows, pal, state);
        }
        rmc_core::app::UiMode::HotlistDialog(state) => {
            crate::hotlist::draw_hotlist_dialog(p, cols, rows, pal, state);
        }
        rmc_core::app::UiMode::PromptInput { title, value, .. } => {
            let msg = value.to_string();
            draw_dialog_box(p, cols, rows, pal, title, &msg, &["< OK >", "Cancel"]);
        }
        rmc_core::app::UiMode::MkdirDialog { value, focus_ok } => {
            draw_mkdir_dialog(p, cols, rows, pal, value, *focus_ok);
        }
        rmc_core::app::UiMode::DeleteDialog { name, .. } => {
            draw_dialog_box(
                p,
                cols,
                rows,
                pal,
                "Delete",
                &format!("Delete \"{name}\"?"),
                &["< Yes >", "No"],
            );
        }
        rmc_core::app::UiMode::CopyDialog {
            title,
            src_name,
            mask,
            to,
            using_shell_patterns,
            follow_links,
            preserve_attrs,
            dive_into_subdir,
            stable_symlinks,
            focus,
            ..
        } => {
            draw_copy_move_dialog(
                p,
                cols,
                rows,
                pal,
                title,
                src_name,
                mask,
                to,
                *using_shell_patterns,
                *follow_links,
                *preserve_attrs,
                *dive_into_subdir,
                *stable_symlinks,
                *focus,
            );
        }
        rmc_core::app::UiMode::ChmodDialog {
            name,
            mode,
            ur,
            uw,
            ux,
            gr,
            gw,
            gx,
            or_,
            ow,
            ox,
            suid,
            sgid,
            sticky,
            recursive,
            focus_index,
        } => {
            draw_chmod_dialog(
                p,
                cols,
                rows,
                pal,
                name,
                *mode,
                (*ur, *uw, *ux),
                (*gr, *gw, *gx),
                (*or_, *ow, *ox),
                (*suid, *sgid, *sticky),
                *recursive,
                *focus_index,
            );
        }
        rmc_core::app::UiMode::ChownDialog {
            owner,
            group,
            recursive,
            focus_index,
        } => {
            draw_chown_dialog(p, cols, rows, pal, owner, group, *recursive, *focus_index);
        }
        rmc_core::app::UiMode::Menu {
            top_index,
            selected_index,
        } => {
            draw_menu_dropdown(p, pal, *top_index, *selected_index);
        }
        _ => {}
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn draw_overwrite_dialog(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    op: rmc_core::app::CopyMoveOp,
    dst: &std::path::Path,
    focus: rmc_core::app::OverwriteFocus,
) {
    let w = (cols as usize).min(70) as u16;
    let h = 9u16;
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
    p.vline(
        x + w - 1,
        y + 1,
        h - 2,
        '│',
        pal.frame_fg,
        pal.dialog_default_bg,
    );
    p.goto(x, y + h - 1);
    p.text("└");
    p.hline(
        x + 1,
        y + h - 1,
        w - 2,
        '─',
        pal.frame_fg,
        pal.dialog_default_bg,
    );
    p.goto(x + w - 1, y + h - 1);
    p.text("┘");
    // Title
    p.set_fg_bg(pal.dtitle_fg, pal.dtitle_bg);
    let ttl = match op {
        rmc_core::app::CopyMoveOp::Copy => " Copy ",
        rmc_core::app::CopyMoveOp::Move => " Move ",
    };
    let tx = x + (w.saturating_sub(ttl.len() as u16)) / 2;
    p.goto(tx, y);
    p.text(ttl);
    // Destination path
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(x + 2, y + 2);
    let d = truncate(&dst.display().to_string(), (w - 4) as usize);
    p.text(&d);
    // Buttons (two rows)
    let row1 = [
        rmc_core::app::OverwriteFocus::Yes,
        rmc_core::app::OverwriteFocus::No,
        rmc_core::app::OverwriteFocus::All,
        rmc_core::app::OverwriteFocus::Older,
    ];
    let row2 = [
        rmc_core::app::OverwriteFocus::None,
        rmc_core::app::OverwriteFocus::Smaller,
        rmc_core::app::OverwriteFocus::SizeDiffers,
        rmc_core::app::OverwriteFocus::Append,
    ];
    let label = |k: rmc_core::app::OverwriteFocus| -> &'static str {
        match k {
            rmc_core::app::OverwriteFocus::Yes => "Yes",
            rmc_core::app::OverwriteFocus::No => "No",
            rmc_core::app::OverwriteFocus::All => "All",
            rmc_core::app::OverwriteFocus::Older => "Older",
            rmc_core::app::OverwriteFocus::None => "None",
            rmc_core::app::OverwriteFocus::Smaller => "Smaller",
            rmc_core::app::OverwriteFocus::SizeDiffers => "Size differs",
            rmc_core::app::OverwriteFocus::Append => "Append",
        }
    };
    let draw_row = |p: &mut Painter,
                    pal: McPalette,
                    x: u16,
                    y: u16,
                    total_w: u16,
                    btns: &[rmc_core::app::OverwriteFocus],
                    focus: rmc_core::app::OverwriteFocus| {
        // compute total width
        let mut width = 0usize;
        for (i, k) in btns.iter().enumerate() {
            let t = if *k == focus {
                format!("< {} >", label(*k))
            } else {
                format!("[ {} ]", label(*k))
            };
            width += t.len();
            if i + 1 != btns.len() {
                width += 2;
            }
        }
        let mut cx = x + (total_w.saturating_sub(width as u16)) / 2;
        for (i, k) in btns.iter().enumerate() {
            let t = if *k == focus {
                p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
                format!("< {} >", label(*k))
            } else {
                p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
                format!("[ {} ]", label(*k))
            };
            p.goto(cx, y);
            p.text(&t);
            cx = cx.saturating_add(t.len() as u16);
            if i + 1 != btns.len() {
                p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
                p.goto(cx, y);
                p.text("  ");
                cx += 2;
            }
        }
    };
    draw_row(p, pal, x, y + h - 3, w, &row1, focus);
    draw_row(p, pal, x, y + h - 2, w, &row2, focus);
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
}

fn draw_user_menu_dialog(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    title: &str,
    entries: &[rmc_core::user_menu::MenuEntry],
    selected: usize,
) {
    // Size according to longest title, max 60 cols, height entries+4
    let max_title = entries.iter().map(|e| e.title.len()).max().unwrap_or(10);
    let list_w = (max_title + 8).clamp(24, 60) as u16;
    let w = list_w;
    let h = (entries.len() as u16 + 4).clamp(7, rows.saturating_sub(4));
    let x = (cols.saturating_sub(w)) / 2;
    let y = (rows.saturating_sub(h)) / 2;
    // Frame
    p.set_fg_bg(pal.frame_fg, pal.dialog_default_bg);
    p.goto(x, y);
    p.text("┌");
    p.hline(x + 1, y, w - 2, '─', pal.frame_fg, pal.dialog_default_bg);
    p.goto(x + w - 1, y);
    p.text("┐");
    p.vline(x, y + 1, h - 2, '│', pal.frame_fg, pal.dialog_default_bg);
    p.vline(
        x + w - 1,
        y + 1,
        h - 2,
        '│',
        pal.frame_fg,
        pal.dialog_default_bg,
    );
    p.goto(x, y + h - 1);
    p.text("└");
    p.hline(
        x + 1,
        y + h - 1,
        w - 2,
        '─',
        pal.frame_fg,
        pal.dialog_default_bg,
    );
    p.goto(x + w - 1, y + h - 1);
    p.text("┘");
    // Title
    p.set_fg_bg(pal.dtitle_fg, pal.dtitle_bg);
    let t = format!(" {title} ");
    let tx = x + (w.saturating_sub(t.len() as u16)) / 2;
    p.goto(tx, y);
    p.text(&t);
    // List
    let list_top = y + 2;
    for (i, e) in entries.iter().enumerate() {
        let row = list_top + i as u16;
        if row >= y + h - 1 {
            break;
        }
        let label = if let Some(hk) = e.hotkey {
            format!(" {}. {}", hk, e.title)
        } else {
            format!("    {}", e.title)
        };
        let mut line = label;
        while line.len() < (w - 2) as usize {
            line.push(' ');
        }
        if i == selected {
            p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
        } else {
            p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
        }
        p.goto(x + 1, row);
        p.text(&line);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_editor(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    buf: &rmc_edit::EditorBuffer,
    show_menu: bool,
    status_msg: Option<&str>,
    search_input: Option<&str>,
    save_as_input: Option<&str>,
    confirm: Option<&rmc_core::app::YncDialog>,
) {
    // Background (editor core colors)
    p.set_fg_bg(pal.core_default_fg, pal.core_default_bg);
    for y in 0..rows {
        p.goto(0, y);
        p.text(&" ".repeat(cols as usize));
    }
    // Top bar (mcedit menu bar)
    draw_editor_menu_bar(p, cols, pal);
    // Status line (bottom-2) and F-bar (bottom-1)
    let status_row = rows.saturating_sub(2);
    let fbar_row = rows.saturating_sub(1);
    // Editor content box between menu and status
    let content_top = 1u16;
    let content_h = status_row.saturating_sub(content_top);
    // Render buffer window
    // We can't mutate buf here; assume viewport was adjusted by the event loop.
    // Draw content lines
    p.set_fg_bg(pal.core_default_fg, pal.core_default_bg);
    for i in 0..content_h {
        p.goto(0, content_top + i);
        p.text(&" ".repeat(cols as usize));
    }
    // Spans for syntax coloring
    let view_spans = buf.render_window_spans(cols as usize, content_h as usize);
    // Map token kind to foreground using existing MC palette (no custom theme)
    let token_fg = |kind: rmc_edit::TokenKind| -> Color {
        use rmc_edit::TokenKind as K;
        match kind {
            K::Keyword => pal.source_color,     // cyan
            K::String => pal.archive_color,     // magenta
            K::Comment => pal.symlink_color,    // gray
            K::Number => pal.exec_color,        // green
            K::Type => pal.dir_color,           // white
            K::Preproc => pal.header_fg,        // yellow
            K::Heading => pal.dtitle_fg,        // blue
            K::Emphasis => pal.menuhot_fg,      // yellow
            K::Link => pal.menusel_fg,          // white
            K::Code => pal.buttonbar_hotkey_fg, // white on black style fg
            _ => pal.core_default_fg,
        }
    };
    // Compute selection spans in viewport coordinates
    let spans = buf.selection_spans_for_view(
        buf.view_row,
        buf.view_col,
        content_h as usize,
        cols as usize,
    );
    for (i, line_spans) in view_spans.iter().enumerate() {
        p.goto(0, content_top + i as u16);
        // Selection range for this row (viewport columns)
        let sel = spans.get(i).and_then(|x| *x);
        // Draw tokens with selection overlay. Maintain running column count.
        let mut drawn_cols = 0usize;
        for tok in line_spans {
            let kind = tok.kind;
            let fg = token_fg(kind);
            let t = truncate(&tok.text, cols as usize - drawn_cols);
            let tok_len = t.chars().count();
            if tok_len == 0 {
                continue;
            }
            match sel {
                None => {
                    p.set_fg_bg(fg, pal.core_default_bg);
                    p.text(&t);
                    drawn_cols += tok_len;
                }
                Some((sa, sb)) => {
                    // Non-overlapping entirely before selection
                    if drawn_cols + tok_len <= sa || drawn_cols >= sb {
                        p.set_fg_bg(fg, pal.core_default_bg);
                        p.text(&t);
                        drawn_cols += tok_len;
                        continue;
                    }
                    // Split into left/sel/right relative to [sa,sb)
                    let left_len = sa.saturating_sub(drawn_cols).min(tok_len);
                    let sel_start = left_len;
                    let sel_end = (sb.saturating_sub(drawn_cols)).min(tok_len);
                    let right_len = tok_len.saturating_sub(sel_end);
                    // Left part
                    if left_len > 0 {
                        p.set_fg_bg(fg, pal.core_default_bg);
                        let left: String = t.chars().take(left_len).collect();
                        p.text(&left);
                    }
                    // Selection part
                    if sel_end > sel_start {
                        p.set_fg_bg(pal.marked_fg, pal.marked_bg);
                        let sel_txt: String = t
                            .chars()
                            .skip(sel_start)
                            .take(sel_end - sel_start)
                            .collect();
                        p.text(&sel_txt);
                    }
                    // Right part
                    if right_len > 0 {
                        p.set_fg_bg(fg, pal.core_default_bg);
                        let right: String = t.chars().skip(sel_end).collect();
                        p.text(&right);
                    }
                    drawn_cols += tok_len;
                }
            }
        }
        // If we drew less than full width, pad the rest
        if drawn_cols < cols as usize {
            p.set_fg_bg(pal.core_default_fg, pal.core_default_bg);
            p.text(&" ".repeat(cols as usize - drawn_cols));
        }
        // Restore default
        p.set_fg_bg(pal.core_default_fg, pal.core_default_bg);
    }
    // Cursor indicator (soft, we don't move real terminal cursor here)
    // Draw a small inverse cell where the logical cursor is on screen
    let cur_y = buf.row.saturating_sub(buf.view_row) as u16 + content_top;
    let cur_x = buf.col.saturating_sub(buf.view_col) as u16;
    if cur_y >= content_top && cur_y < content_top + content_h && cur_x < cols {
        p.goto(cur_x, cur_y);
        // Get glyph under cursor from rendered view
        let vr = (cur_y - content_top) as usize; // row index within content
                                                 // Reconstruct the displayed row text from spans for cursor sampling
        let row_text: String = view_spans
            .get(vr)
            .map(|v| v.iter().map(|s| s.text.as_str()).collect::<String>())
            .unwrap_or_default();
        let ch = row_text.chars().nth(cur_x as usize).unwrap_or(' ');
        // Invert colors for that glyph
        p.set_fg_bg(pal.core_default_bg, pal.core_default_fg);
        p.text(&ch.to_string());
        // Restore default for safety
        p.set_fg_bg(pal.core_default_fg, pal.core_default_bg);
    }
    // Status line
    p.set_fg_bg(pal.statusbar_fg, pal.statusbar_bg);
    p.goto(0, status_row);
    let mut status = buf.status_text();
    if let Some(msg) = status_msg {
        status.push_str("  ");
        status.push_str(msg);
    }
    let t = truncate(&status, cols as usize);
    p.text(&t);
    if t.len() < cols as usize {
        p.text(&" ".repeat(cols as usize - t.len()));
    }
    // Bottom F-key bar for editor (packed MC style)
    draw_editor_fbar(p, fbar_row, cols, pal);
    // If show_menu, draw a small stub dropdown
    if show_menu {
        draw_editor_menu_dropdown(p, pal);
    }
    // Inline prompts
    if let Some(q) = search_input {
        draw_inline_prompt(p, pal, rows, cols, "Find:", q);
    }
    if let Some(q) = save_as_input {
        draw_inline_prompt(p, pal, rows, cols, "Save as:", q);
    }
    if let Some(c) = confirm {
        draw_dialog_ync(p, cols, rows, pal, &c.title, &c.message, c.focus);
    }
}

fn draw_editor_menu_bar(p: &mut Painter, cols: u16, pal: McPalette) {
    p.set_fg_bg(pal.menu_fg, pal.menu_bg);
    p.goto(0, 0);
    let items = [
        " File ",
        " Edit ",
        " Search ",
        " Command ",
        " Options ",
        " Help ",
    ];
    let mut x = 0u16;
    for it in items.iter() {
        p.goto(x, 0);
        p.text(it);
        x += it.len() as u16;
    }
    // Fill rest
    if x < cols {
        p.goto(x, 0);
        p.text(&" ".repeat(cols.saturating_sub(x) as usize));
    }
}

fn draw_editor_menu_dropdown(p: &mut Painter, pal: McPalette) {
    // Simple stub dropdown under "File"
    let x = 0u16;
    let y = 1u16;
    // Include a placeholder for Command->Pipe to reflect available action
    let items = ["Pipe", "Save", "Save as", "Quit"];
    let w = (items.iter().map(|s| s.len()).max().unwrap_or(4) + 4) as u16;
    let h = items.len() as u16 + 2;
    p.set_fg_bg(pal.menu_fg, pal.menu_bg);
    p.goto(x, y);
    p.text("┌");
    p.hline(x + 1, y, w - 2, '─', pal.menu_fg, pal.menu_bg);
    p.goto(x + w - 1, y);
    p.text("┐");
    p.vline(x, y + 1, h - 2, '│', pal.menu_fg, pal.menu_bg);
    p.vline(x + w - 1, y + 1, h - 2, '│', pal.menu_fg, pal.menu_bg);
    p.goto(x, y + h - 1);
    p.text("└");
    p.hline(x + 1, y + h - 1, w - 2, '─', pal.menu_fg, pal.menu_bg);
    p.goto(x + w - 1, y + h - 1);
    p.text("┘");
    for (i, it) in items.iter().enumerate() {
        let row = y + 1 + i as u16;
        p.goto(x + 1, row);
        let mut line = String::from(" ");
        line.push_str(it);
        while line.len() < (w - 2) as usize {
            line.push(' ');
        }
        p.text(&line);
    }
}

fn draw_editor_fbar(p: &mut Painter, y: u16, cols: u16, pal: McPalette) {
    let labels = [
        "Help", "Save", "Mark", "Replac", "Copy", "Move", "Search", "Delete", "PullDn", "Quit",
    ];
    let mut x = 0u16;
    for (i, lab) in labels.iter().enumerate() {
        let num = if i == 9 { "10" } else { &(i + 1).to_string() };
        // number: white on black
        p.set_fg_bg(pal.buttonbar_hotkey_fg, pal.buttonbar_hotkey_bg);
        p.goto(x, y);
        p.text(num);
        x += num.len() as u16;
        // label: black on cyan
        p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
        p.goto(x, y);
        p.text(lab);
        x += lab.len() as u16;
        if x < cols {
            p.goto(x, y);
            p.text(" ");
            x += 1;
        } else {
            break;
        }
    }
    if x < cols {
        p.goto(x, y);
        p.text(&" ".repeat(cols.saturating_sub(x) as usize));
    }
}

fn draw_inline_prompt(
    p: &mut Painter,
    pal: McPalette,
    rows: u16,
    cols: u16,
    title: &str,
    val: &str,
) {
    // Use dialog style bar on last row
    let y = rows.saturating_sub(1);
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(0, y);
    let mut txt = format!(" {title} {val}");
    if txt.len() < cols as usize {
        txt.push_str(&" ".repeat(cols as usize - txt.len()));
    }
    let t = truncate(&txt, cols as usize);
    p.text(&t);
}

fn draw_dialog_ync(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    title: &str,
    message: &str,
    focus: rmc_core::app::YncFocus,
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
    p.vline(
        x + w - 1,
        y + 1,
        h - 2,
        '│',
        pal.frame_fg,
        pal.dialog_default_bg,
    );
    p.goto(x, y + h - 1);
    p.text("└");
    p.hline(
        x + 1,
        y + h - 1,
        w - 2,
        '─',
        pal.frame_fg,
        pal.dialog_default_bg,
    );
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
    // Buttons
    let sel = |want: rmc_core::app::YncFocus, txt: &str| {
        if want == focus {
            format!("< {txt} >")
        } else {
            format!("[ {txt} ]")
        }
    };
    let btns = format!(
        "{}  {}  {}",
        sel(rmc_core::app::YncFocus::Yes, "Yes"),
        sel(rmc_core::app::YncFocus::No, "No"),
        sel(rmc_core::app::YncFocus::Cancel, "Cancel")
    );
    let bx = x + (w.saturating_sub(btns.len() as u16)) / 2;
    p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
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
    p.vline(
        x + w - 1,
        y + 1,
        h - 2,
        '│',
        pal.frame_fg,
        pal.dialog_default_bg,
    );
    p.goto(x, y + h - 1);
    p.text("└");
    p.hline(
        x + 1,
        y + h - 1,
        w - 2,
        '─',
        pal.frame_fg,
        pal.dialog_default_bg,
    );
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

#[allow(clippy::too_many_arguments)]
fn draw_sort_dialog(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    side: rmc_core::actions::PaneSide,
    focus_index: usize,
    by: rmc_core::panel::SortBy,
    reverse: bool,
    dirs_first: bool,
) {
    let _ = side; // implied by Left/Right menu; title remains generic
    let title = "Sort order";
    let w = 50u16.min(cols.saturating_sub(2));
    let h = 10u16;
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
    p.vline(
        x + w - 1,
        y + 1,
        h - 2,
        '│',
        pal.frame_fg,
        pal.dialog_default_bg,
    );
    p.goto(x, y + h - 1);
    p.text("└");
    p.hline(
        x + 1,
        y + h - 1,
        w - 2,
        '─',
        pal.frame_fg,
        pal.dialog_default_bg,
    );
    p.goto(x + w - 1, y + h - 1);
    p.text("┘");
    // Title
    p.set_fg_bg(pal.dtitle_fg, pal.dtitle_bg);
    let ttl = format!(" {title} ");
    let tx = x + (w.saturating_sub(ttl.len() as u16)) / 2;
    p.goto(tx, y);
    p.text(&ttl);
    // Options: radios on the left, checkboxes on the right (same rows), then buttons
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    let radios = [
        ("Name", rmc_core::panel::SortBy::Name),
        ("Extension", rmc_core::panel::SortBy::Ext),
        ("Modify time", rmc_core::panel::SortBy::Time),
        ("Size", rmc_core::panel::SortBy::Size),
    ];
    for (i, (label, kind)) in radios.iter().enumerate() {
        let row_y = y + 2 + i as u16;
        let sel = if *kind == by { 'x' } else { ' ' };
        if focus_index == i {
            p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
        } else {
            p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
        }
        p.goto(x + 2, row_y);
        p.text(&format!("({sel}) {label}"));
    }
    // Checkboxes on rows y+2 and y+3, to the right of radios
    let checks = [("Reverse", reverse), ("Directories first", dirs_first)];
    for (j, (label, on)) in checks.iter().enumerate() {
        let idx = radios.len() + j;
        let row_y = y + 2 + j as u16;
        if focus_index == idx {
            p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
        } else {
            p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
        }
        p.goto(x + 28, row_y);
        p.text(&format!("[{}] {}", if *on { 'x' } else { ' ' }, label));
    }
    // Buttons with focus highlight: indices 6=OK, 7=Cancel
    p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
    let ok_txt = if focus_index == 6 { "< OK >" } else { "  OK  " };
    let cancel_txt = if focus_index == 7 {
        "[ Cancel ]"
    } else {
        "  Cancel  "
    };
    let btns = format!("{ok_txt}  {cancel_txt}");
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
}

#[allow(clippy::too_many_arguments)]
fn draw_viewer(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    display_path: &std::path::Path,
    hex: bool,
    wrap: bool,
    offset: u64,
    show_line_numbers: bool,
    show_cr: bool,
    search_prompt: &Option<String>,
    goto_prompt: &Option<String>,
) -> Result<()> {
    // MC-style viewer: blue background with frame and title
    p.set_fg_bg(pal.core_default_fg, pal.core_default_bg);
    for y in 0..rows {
        p.goto(0, y);
        p.text(&" ".repeat(cols as usize));
    }
    // Frame
    p.set_fg_bg(pal.frame_fg, pal.core_default_bg);
    p.goto(0, 0);
    p.text("┌");
    p.hline(
        1,
        0,
        cols.saturating_sub(2),
        '─',
        pal.frame_fg,
        pal.core_default_bg,
    );
    p.goto(cols - 1, 0);
    p.text("┐");
    p.vline(
        0,
        1,
        rows.saturating_sub(2),
        '│',
        pal.frame_fg,
        pal.core_default_bg,
    );
    p.vline(
        cols - 1,
        1,
        rows.saturating_sub(2),
        '│',
        pal.frame_fg,
        pal.core_default_bg,
    );
    p.goto(0, rows - 1);
    p.text("└");
    p.hline(
        1,
        rows - 1,
        cols.saturating_sub(2),
        '─',
        pal.frame_fg,
        pal.core_default_bg,
    );
    p.goto(cols - 1, rows - 1);
    p.text("┘");
    // Title (show original path selected in panels)
    let title = format!(" {} ", display_path.display());
    let tx = (cols.saturating_sub(title.len() as u16)) / 2;
    p.goto(tx, 0);
    p.text(&title);
    // Render content window using rmc-view (windowed)
    // Layout: full frame; content rows = rows - 3 (status + fbar)
    let content_rows = rows.saturating_sub(3);
    // Reserve space for optional line numbers (text mode only)
    let ln_enabled = show_line_numbers && !hex;
    // Compute line number gutter width conservatively (up to 7 digits + space)
    let ln_gutter: u16 = if ln_enabled { 8 } else { 0 };
    let content_cols = cols.saturating_sub(2 + ln_gutter);
    // Ensure a stable view for the selected path (may be a filtered temp view)
    let content_path = crate::terminal::viewer_ensure_view_for(display_path);
    let rr = rmc_view::render_window(
        &content_path,
        rmc_view::ViewOptions { hex, wrap, show_cr },
        offset,
        content_cols, // content width inside frame
        content_rows,
    )?;
    // If showing line numbers, compute starting line number at rr.offset
    let mut start_ln = 1u64;
    if ln_enabled {
        if let Ok(n) = rmc_view::line_number_at(&content_path, rr.offset) {
            start_ln = n;
        }
    }
    for (i, line) in rr.lines.into_iter().enumerate() {
        let row_y = 1 + i as u16;
        p.goto(1, row_y);
        if ln_enabled {
            // Draw gray-ish line number gutter
            p.set_fg_bg(pal.frame_fg, pal.core_default_bg);
            let label = format!("{:>6} ", start_ln + i as u64);
            p.text(&label);
            p.set_fg_bg(pal.core_default_fg, pal.core_default_bg);
            p.goto(1 + ln_gutter, row_y);
        }
        let t = truncate(&line, content_cols as usize);
        p.text(&t);
        if (1 + i as u16) >= rows.saturating_sub(2) {
            break;
        }
    }
    // Status line (MC-style: percent / offset / mode)
    p.set_fg_bg(pal.statusbar_fg, pal.statusbar_bg);
    p.goto(0, rows.saturating_sub(2));
    let mode = if hex {
        "[HEX]"
    } else if wrap {
        "[TEXT WRAP]"
    } else {
        "[TEXT]"
    };
    let total = rmc_view::file_len(&content_path).unwrap_or(0);
    let pct = rr
        .offset
        .saturating_mul(100)
        .checked_div(total)
        .unwrap_or(100);
    let mut status = format!(" {:>3}%  0x{:08X}  {}", pct, rr.offset, mode);
    if ln_enabled {
        if let Ok(cur_ln) = rmc_view::line_number_at(&content_path, rr.offset) {
            status.push_str(&format!("  Ln {}", cur_ln));
        }
    }
    let st = truncate(&status, cols as usize);
    p.text(&st);
    // Viewer F-bar (white-on-black numbers, black-on-cyan labels)
    draw_viewer_fbar(p, rows.saturating_sub(1), cols, pal);
    // Search prompt overlay (MC-style input dialog)
    if let Some(current) = search_prompt {
        draw_dialog_box(p, cols, rows, pal, "Search", current, &["< OK >", "Cancel"]);
    }
    // Goto prompt overlay (MC-style input dialog)
    if let Some(current) = goto_prompt {
        draw_dialog_box(p, cols, rows, pal, "Goto", current, &["< OK >", "Cancel"]);
    }
    Ok(())
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
    p.hline(
        x + 1,
        y + h - 1,
        w.saturating_sub(2),
        '─',
        frame_fg,
        frame_bg,
    );
    p.goto(x + w - 1, y + h - 1);
    p.text("┘");
    // sides
    p.vline(x, y + 1, h.saturating_sub(2), '│', frame_fg, frame_bg);
    p.vline(
        x + w - 1,
        y + 1,
        h.saturating_sub(2),
        '│',
        frame_fg,
        frame_bg,
    );
    // caption path in top frame (hide internal '#' anchor for archives)
    let path = if is_left {
        &app.left.cwd
    } else {
        &app.right.cwd
    };
    let path_str_display = {
        if let Some(ap) = rmc_fs::pathutil::parse_archive_path(path) {
            if ap.inner.as_os_str().is_empty() {
                format!(" {}{} ", ap.archive.display(), "/")
            } else {
                format!(" {}/{} ", ap.archive.display(), ap.inner.display())
            }
        } else if let Some(ax) = rmc_fs::pathutil::parse_anchor_any(path) {
            if ax.inner.as_os_str().is_empty() {
                format!(" {}{} ", ax.base.display(), "/")
            } else {
                format!(" {}/{} ", ax.base.display(), ax.inner.display())
            }
        } else {
            format!(" {} ", path.display())
        }
    };
    let cap_x = x + ((w.saturating_sub(path_str_display.len() as u16)) / 2);
    p.goto(cap_x.max(x + 1), y);
    p.text(&path_str_display);

    // Non-listing panel modes (QuickView/Info/Tree): override standard listing rendering.
    let panel = if is_left { &app.left } else { &app.right };
    if !matches!(panel.mode, PanelMode::Listing) {
        let content_top = y + 1;
        let content_h = h.saturating_sub(2);
        // Clear inner area
        for i in 0..content_h {
            p.set_fg_bg(pal.core_default_fg, pal.core_default_bg);
            p.goto(x + 1, content_top + i);
            p.text(&" ".repeat((w - 2) as usize));
        }
        match panel.mode {
            PanelMode::QuickView => {
                // Show selected file from ACTIVE panel
                if let Some(ent) = app.active_panel().current_entry() {
                    if !ent.is_dir {
                        let rr = rmc_view::render_window(
                            &ent.path,
                            rmc_view::ViewOptions {
                                hex: false,
                                wrap: true,
                                show_cr: false,
                            },
                            0,
                            w.saturating_sub(2),
                            content_h,
                        )?;
                        for (i, line) in rr.lines.into_iter().enumerate() {
                            if (i as u16) >= content_h {
                                break;
                            }
                            p.goto(x + 1, content_top + i as u16);
                            let t = truncate(&line, (w - 2) as usize);
                            p.text(&t);
                        }
                    } else {
                        p.goto(x + 1, content_top);
                        let msg = format!("Directory: {}", ent.name);
                        p.text(&truncate(&msg, (w - 2) as usize));
                    }
                }
            }
            PanelMode::Info => {
                if let Some(ent) = app.active_panel().current_entry() {
                    let perms = perm_string(ent.permissions, ent.is_dir);
                    let owner = ent.owner.as_deref().unwrap_or("-");
                    let group = ent.group.as_deref().unwrap_or("-");
                    let size = if ent.is_dir { 0 } else { ent.size };
                    let tm: OffsetDateTime = ent.modified.into();
                    let ts = tm
                        .format(&time::macros::format_description!(
                            "[year]-[month repr:numerical]-[day] [hour]:[minute]"
                        ))
                        .unwrap_or_default();
                    let lines = [
                        format!("Name: {}", ent.name),
                        format!("Path: {}", ent.path.display()),
                        format!("Type: {}", if ent.is_dir { "Directory" } else { "File" }),
                        format!("Size: {}", size),
                        format!("Owner: {owner}  Group: {group}"),
                        format!("Perms: {perms}"),
                        format!("Modified: {ts}"),
                    ];
                    for (i, line) in lines.iter().enumerate() {
                        if (i as u16) >= content_h {
                            break;
                        }
                        p.goto(x + 1, content_top + i as u16);
                        p.text(&truncate(line, (w - 2) as usize));
                    }
                }
            }
            PanelMode::Tree => {
                if let Some(tree) = &panel.tree {
                    for i in 0..content_h as usize {
                        let idx = tree.scroll_top + i;
                        if let Some(ent) = tree.entries.get(idx) {
                            let row_y = content_top + i as u16;
                            let is_active_panel = (is_left
                                && matches!(app.active, rmc_core::actions::PaneSide::Left))
                                || (!is_left
                                    && matches!(app.active, rmc_core::actions::PaneSide::Right));
                            let is_cursor = idx == tree.cursor;
                            let (fg, bg) = if is_cursor && is_active_panel {
                                (pal.selected_fg, pal.selected_bg)
                            } else {
                                (pal.core_default_fg, pal.core_default_bg)
                            };
                            p.set_fg_bg(fg, bg);
                            p.goto(x + 1, row_y);
                            let name = ent.path.file_name().and_then(|s| s.to_str()).unwrap_or("/");
                            let indent = "  ".repeat(ent.depth);
                            let display = format!("{indent}{name}/");
                            p.text(&truncate(&display, (w - 2) as usize));
                        }
                    }
                } else {
                    p.goto(x + 1, content_top);
                    p.text("No tree");
                }
            }
            PanelMode::Listing => {}
        }
        return Ok(());
    }

    // Headers
    let header_fg = pal.header_fg;
    let header_bg = pal.header_bg;
    p.set_fg_bg(header_fg, header_bg);
    let panel = if is_left { &app.left } else { &app.right };
    match panel.listing {
        rmc_core::panel::ListingFormat::Full => {
            p.goto(x + 1, y + 1);
            p.text("Name");
            p.goto(x + w / 2, y + 1);
            p.text("Size");
            p.goto(x + w - 15, y + 1);
            p.text("Modify time");
        }
        rmc_core::panel::ListingFormat::Brief => {
            p.goto(x + 1, y + 1);
            p.text("Name");
        }
        rmc_core::panel::ListingFormat::Long => {
            // Column-aligned like ls -l
            let perms_col = x + 1;
            let owner_col = perms_col + 12; // 10 perms + 2 spaces
            let group_col = owner_col + 9; // owner 8 + 1 space
            let size_col = group_col + 9; // group 8 + 1 space
            let time_col = size_col + 9; // size 8 + 1 space
            p.goto(perms_col, y + 1);
            p.text("Perms");
            p.goto(owner_col, y + 1);
            p.text("Owner");
            p.goto(group_col, y + 1);
            p.text("Group");
            p.goto(size_col, y + 1);
            p.text("Size");
            p.goto(time_col, y + 1);
            p.text("Modify time");
        }
    }

    // Content rows
    let content_top = y + 2;
    let content_h = h.saturating_sub(4);
    let _panel = if is_left { &app.left } else { &app.right };
    // Viewport uses panel.scroll_top, updated by the event loop per visible capacity
    let panel = if is_left { &app.left } else { &app.right };
    match panel.listing {
        rmc_core::panel::ListingFormat::Full => {
            let size_col = x + w / 2;
            for i in 0..content_h as usize {
                let row_y = content_top + i as u16;
                // Clear row
                p.set_fg_bg(pal.core_default_fg, pal.core_default_bg);
                p.goto(x + 1, row_y);
                p.text(&" ".repeat((w - 2) as usize));
                let idx = panel.scroll_top + i;
                if let Some(ent) = panel.entries.get(idx) {
                    // active row highlight
                    let is_active_panel = (is_left
                        && matches!(app.active, rmc_core::actions::PaneSide::Left))
                        || (!is_left && matches!(app.active, rmc_core::actions::PaneSide::Right));
                    let is_cursor = idx == panel.cursor;
                    let selected = panel.selection.is_selected(idx);
                    // Determine colors following MC rules
                    let (fg, bg) = if is_cursor && is_active_panel {
                        (pal.selected_fg, pal.selected_bg)
                    } else if selected && is_cursor {
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
        }
        rmc_core::panel::ListingFormat::Brief => {
            // Two columns if there is enough width
            let two_cols = w >= 30;
            let per_col_width = if two_cols {
                (w - 3) / 2 // 1 left pad + 1 space + 1 right pad
            } else {
                w - 2
            };
            for i in 0..content_h as usize {
                let row_y = content_top + i as u16;
                p.set_fg_bg(pal.core_default_fg, pal.core_default_bg);
                p.goto(x + 1, row_y);
                p.text(&" ".repeat((w - 2) as usize));
                let left_idx = panel.scroll_top + i;
                let right_idx = if two_cols {
                    panel.scroll_top + i + content_h as usize
                } else {
                    usize::MAX
                };
                for (j, idx) in [left_idx, right_idx].into_iter().enumerate() {
                    if let Some(ent) = panel.entries.get(idx) {
                        let is_active_panel = (is_left
                            && matches!(app.active, rmc_core::actions::PaneSide::Left))
                            || (!is_left
                                && matches!(app.active, rmc_core::actions::PaneSide::Right));
                        let is_cursor = idx == panel.cursor;
                        let selected = panel.selection.is_selected(idx);
                        let (fg, bg) = if is_cursor && is_active_panel {
                            (pal.selected_fg, pal.selected_bg)
                        } else if selected && is_cursor {
                            (pal.markselect_fg, pal.markselect_bg)
                        } else if selected {
                            (pal.marked_fg, pal.marked_bg)
                        } else {
                            (pal.core_default_fg, pal.core_default_bg)
                        };
                        p.set_fg_bg(fg, bg);
                        let col_x = if j == 0 { x + 1 } else { x + 2 + per_col_width };
                        p.goto(col_x, row_y);
                        let display_name = format_entry_name(ent);
                        let name_trunc = truncate(&display_name, per_col_width as usize);
                        p.text(&name_trunc);
                    }
                }
            }
        }
        rmc_core::panel::ListingFormat::Long => {
            for i in 0..content_h as usize {
                let row_y = content_top + i as u16;
                p.set_fg_bg(pal.core_default_fg, pal.core_default_bg);
                p.goto(x + 1, row_y);
                p.text(&" ".repeat((w - 2) as usize));
                let idx = panel.scroll_top + i;
                if let Some(ent) = panel.entries.get(idx) {
                    let is_active_panel = (is_left
                        && matches!(app.active, rmc_core::actions::PaneSide::Left))
                        || (!is_left && matches!(app.active, rmc_core::actions::PaneSide::Right));
                    let is_cursor = idx == panel.cursor;
                    let selected = panel.selection.is_selected(idx);
                    let (fg, bg) = if is_cursor && is_active_panel {
                        (pal.selected_fg, pal.selected_bg)
                    } else if selected && is_cursor {
                        (pal.markselect_fg, pal.markselect_bg)
                    } else if selected {
                        (pal.marked_fg, pal.marked_bg)
                    } else {
                        (pal.core_default_fg, pal.core_default_bg)
                    };
                    p.set_fg_bg(fg, bg);
                    let perms = perm_string(ent.permissions, ent.is_dir);
                    let owner = ent.owner.as_deref().unwrap_or("-");
                    let group = ent.group.as_deref().unwrap_or("-");
                    let size = if ent.is_dir { 0 } else { ent.size };
                    let tm = format_time(ent);
                    let mut line = format!(
                        "{perms}  {owner:>8} {group:>8} {size:>8} {tm}  {}",
                        ent.name
                    );
                    line = truncate(&line, (w - 2) as usize);
                    p.goto(x + 1, row_y);
                    p.text(&line);
                }
            }
        }
    }
    // Mini status
    let status_y = y + h - 2;
    p.set_fg_bg(pal.statusbar_fg, pal.statusbar_bg);
    p.goto(x + 1, status_y);
    // If quick search is active and this is the active panel, draw mini prompt instead
    let is_active_panel = (is_left && matches!(app.active, rmc_core::actions::PaneSide::Left))
        || (!is_left && matches!(app.active, rmc_core::actions::PaneSide::Right));
    if is_active_panel {
        if let Some(qs) = &app.quick_search {
            let mut prompt = String::from(" Search: ");
            prompt.push_str(&qs.pattern);
            let s = truncate(&prompt, (w - 2) as usize);
            p.text(&s);
            if s.len() < (w - 2) as usize {
                p.text(&" ".repeat((w - 2) as usize - s.len()));
            }
        } else if let Some(cur) = panel.current_entry() {
            let s = format_mini_status(cur);
            let s = truncate(&s, (w - 2) as usize);
            p.text(&s);
        } else {
            let s = " ".repeat((w - 2) as usize);
            p.text(&s);
        }
    } else if let Some(cur) = panel.current_entry() {
        let s = format_mini_status(cur);
        let s = truncate(&s, (w - 2) as usize);
        p.text(&s);
    } else {
        let s = " ".repeat((w - 2) as usize);
        p.text(&s);
    }
    Ok(())
}

fn draw_gauge(p: &mut Painter, y: u16, cols: u16, pal: McPalette, app: &App) {
    p.set_fg_bg(pal.core_default_fg, pal.core_default_bg);
    p.goto(0, y);
    let path = &app.active_panel().cwd;
    let text = match (fs2::available_space(path), fs2::total_space(path)) {
        (Ok(avail), Ok(total)) => {
            let used = total.saturating_sub(avail);
            let pct = if total > 0 {
                (used as f64 / total as f64) * 100.0
            } else {
                0.0
            };
            format!(
                "{} / {} ({:.0}%)",
                human_bytes(used),
                human_bytes(total),
                pct
            )
        }
        _ => "".to_string(),
    };
    let t = truncate(&text, cols as usize);
    p.text(&t);
    if t.len() < cols as usize {
        p.text(&" ".repeat(cols as usize - t.len()));
    }
}
fn draw_hint(p: &mut Painter, y: u16, cols: u16, pal: McPalette) {
    p.set_fg_bg(pal.core_default_fg, pal.core_default_bg);
    p.goto(0, y);
    let hint = "Hint: Use C-x t to copy tagged file names to the command line.";
    let t = truncate(hint, cols as usize);
    p.text(&t);
    if t.len() < cols as usize {
        p.text(&" ".repeat(cols as usize - t.len()));
    }
}
fn draw_cmdline(p: &mut Painter, y: u16, cols: u16, _pal: McPalette, app: &App) {
    p.set_fg_bg(Color::White, Color::Black);
    p.goto(0, y);
    let user = whoami::username();
    let host = hostname::get()
        .ok()
        .and_then(|s| s.into_string().ok())
        .unwrap_or_default();
    let cwd = app.active_panel().cwd.display().to_string();
    let mut s = format!("{user}@{host}:{cwd}$ ");
    s.push_str(&app.subshell.cmdline);
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
            p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
            p.goto(x, y);
            p.text(" ");
            x += 1;
        }
        if x >= cols {
            break;
        }
    }
    if x < cols {
        p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
        p.goto(x, y);
        p.text(&" ".repeat(cols.saturating_sub(x) as usize));
    }
}

fn draw_help(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    state: &rmc_core::app::HelpState,
    index_opt: Option<&HelpIndex>,
) {
    // Background (dialogs palette: black on lightgray)
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    for y in 0..rows {
        p.goto(0, y);
        p.text(&" ".repeat(cols as usize));
    }
    // Frame
    p.set_fg_bg(pal.frame_fg, pal.dialog_default_bg);
    p.goto(0, 0);
    p.text("┌");
    p.hline(
        1,
        0,
        cols.saturating_sub(2),
        '─',
        pal.frame_fg,
        pal.dialog_default_bg,
    );
    p.goto(cols - 1, 0);
    p.text("┐");
    p.vline(
        0,
        1,
        rows.saturating_sub(2),
        '│',
        pal.frame_fg,
        pal.dialog_default_bg,
    );
    p.vline(
        cols - 1,
        1,
        rows.saturating_sub(2),
        '│',
        pal.frame_fg,
        pal.dialog_default_bg,
    );
    p.goto(0, rows - 1);
    p.text("└");
    p.hline(
        1,
        rows - 1,
        cols.saturating_sub(2),
        '─',
        pal.frame_fg,
        pal.dialog_default_bg,
    );
    p.goto(cols - 1, rows - 1);
    p.text("┘");
    // Title
    let (title, items) = if let Some(index) = index_opt {
        let topic = initial_topic_or_contents(index, state);
        if let Some(node) = index.get(&topic) {
            (format!(" Help: {} ", node.title), node.items.clone())
        } else {
            (format!(" Help: {} ", topic), Vec::new())
        }
    } else {
        (" Help ".to_string(), Vec::new())
    };
    p.set_fg_bg(pal.dtitle_fg, pal.dtitle_bg);
    let tx = (cols.saturating_sub(title.len() as u16)) / 2;
    p.goto(tx, 0);
    p.text(&title);
    // Content area layout (inside frame)
    let content_top = 1u16;
    let content_h = rows.saturating_sub(2);
    // Build rendered lines from items, keeping link indices
    let mut rendered: Vec<(String, Option<usize>)> = Vec::new();
    let mut link_idx: usize = 0;
    for it in items {
        match it {
            HelpItem::Text(t) => {
                rendered.push((t, None));
            }
            HelpItem::Link { label, .. } => {
                let line = format!("  • {}", label);
                rendered.push((line, Some(link_idx)));
                link_idx += 1;
            }
        }
    }
    // Clip by scroll and height
    let start = state.scroll_top.min(rendered.len());
    let end = (start + content_h as usize).min(rendered.len());
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    for (i, (line, link_opt)) in rendered[start..end].iter().enumerate() {
        p.goto(1, content_top + i as u16);
        let t = truncate(line, cols.saturating_sub(2) as usize);
        // Highlight selected link
        if let Some(li) = link_opt {
            if *li == state.cursor {
                p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
                // Draw with focus colors
                p.text(&t);
                // Restore default for rest of rows
                p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
                continue;
            }
        }
        p.text(&t);
        // clear remainder of row
        if t.len() < cols.saturating_sub(2) as usize {
            let pad = " ".repeat(cols.saturating_sub(2) as usize - t.len());
            p.text(&pad);
        }
    }
    // Bottom F-bar (MC-style)
    draw_help_fbar(p, rows.saturating_sub(1), cols, pal);
}

fn draw_help_fbar(p: &mut Painter, y: u16, cols: u16, pal: McPalette) {
    let labels = [
        "Help",  // F1
        "Index", // F2
        "Prev",  // F3
        "Next",  // F4
        "",      // F5
        "",      // F6
        "",      // F7
        "",      // F8
        "",      // F9
        "Quit",  // F10
    ];
    let mut x = 0u16;
    for (i, lab) in labels.iter().enumerate() {
        let num = if i == 9 { "10" } else { &(i + 1).to_string() };
        // number: white on black
        p.set_fg_bg(pal.buttonbar_hotkey_fg, pal.buttonbar_hotkey_bg);
        p.goto(x, y);
        p.text(num);
        x += num.len() as u16;
        // label: black on cyan; keep spacing even when empty
        p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
        p.goto(x, y);
        let text = if lab.is_empty() { "     " } else { *lab };
        p.text(text);
        x += text.len() as u16;
        if x < cols {
            p.goto(x, y);
            p.text(" ");
            x += 1;
        } else {
            break;
        }
    }
    if x < cols {
        p.goto(x, y);
        p.text(&" ".repeat(cols.saturating_sub(x) as usize));
    }
}
fn draw_viewer_fbar(p: &mut Painter, y: u16, cols: u16, pal: McPalette) {
    // MC-like order: Help, Save, Quit, Hex, Goto, (Raw), Search, Wrap, Menu, Quit
    let labels = [
        "Help", "Save", "Quit", "Hex", "Goto", "Raw", "Search", "Wrap", "Menu", "Quit",
    ];
    let mut x = 0u16;
    for (i, lab) in labels.iter().enumerate() {
        let num = if i == 9 { "10" } else { &(i + 1).to_string() };
        p.set_fg_bg(pal.buttonbar_hotkey_fg, pal.buttonbar_hotkey_bg); // white on black
        p.goto(x, y);
        p.text(num);
        x += num.len() as u16;
        p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg); // black on cyan
        p.goto(x, y);
        p.text(lab);
        x += lab.len() as u16;
        if x < cols {
            p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
            p.goto(x, y);
            p.text(" ");
            x += 1;
        }
        if x >= cols {
            break;
        }
    }
    if x < cols {
        p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
        p.goto(x, y);
        p.text(&" ".repeat(cols.saturating_sub(x) as usize));
    }
}

fn draw_subshell_fullscreen(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    _pal: McPalette,
    app: &App,
) -> Result<()> {
    // MC C-o: draw captured output full-screen with default terminal colors; no frame/title/status.
    p.set_fg_bg(Color::Reset, Color::Reset);
    for y in 0..rows {
        p.goto(0, y);
        p.text(&" ".repeat(cols as usize));
    }
    let max_lines = rows as usize;
    let mut y = 0u16;
    for line in app.subshell.window(max_lines) {
        if y >= rows {
            break;
        }
        p.goto(0, y);
        let t = truncate(line, cols as usize);
        p.text(&t);
        y = y.saturating_add(1);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn draw_diff(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    state: &rmc_core::app::DiffState,
) -> Result<()> {
    // Background
    p.set_fg_bg(pal.core_default_fg, pal.core_default_bg);
    for y in 0..rows {
        p.goto(0, y);
        p.text(&" ".repeat(cols as usize));
    }
    // Outer frame
    p.set_fg_bg(pal.frame_fg, pal.core_default_bg);
    p.goto(0, 0);
    p.text("┌");
    p.hline(
        1,
        0,
        cols.saturating_sub(2),
        '─',
        pal.frame_fg,
        pal.core_default_bg,
    );
    p.goto(cols - 1, 0);
    p.text("┐");
    p.vline(
        0,
        1,
        rows.saturating_sub(2),
        '│',
        pal.frame_fg,
        pal.core_default_bg,
    );
    p.vline(
        cols - 1,
        1,
        rows.saturating_sub(2),
        '│',
        pal.frame_fg,
        pal.core_default_bg,
    );
    p.goto(0, rows - 1);
    p.text("└");
    p.hline(
        1,
        rows - 1,
        cols.saturating_sub(2),
        '─',
        pal.frame_fg,
        pal.core_default_bg,
    );
    p.goto(cols - 1, rows - 1);
    p.text("┘");
    // Title (paths)
    let title = format!(
        " {}  |  {} ",
        state.left_path.display(),
        state.right_path.display()
    );
    let tx = (cols.saturating_sub(title.len() as u16)) / 2;
    p.goto(tx, 0);
    p.text(&title);
    // Layout inside
    let status_row = rows.saturating_sub(2);
    let fbar_row = rows.saturating_sub(1);
    let content_top = 1u16;
    let content_h = status_row.saturating_sub(content_top);
    // Column split from ratio
    let total_inner = cols.saturating_sub(2);
    let left_w = ((total_inner as f32) * state.panel_ratio).round() as u16;
    let left_w = left_w.clamp(10, total_inner.saturating_sub(10));
    let right_w = total_inner.saturating_sub(left_w);
    // Divider
    let divider_x = 1 + left_w;
    p.vline(
        divider_x,
        content_top,
        content_h,
        '│',
        pal.frame_fg,
        pal.core_default_bg,
    );
    // Line number column width
    let lnw = if state.show_line_numbers { 6u16 } else { 0 };
    // Left pane
    for i in 0..content_h {
        let li = state.left_scroll + i as usize;
        p.set_fg_bg(pal.core_default_fg, pal.core_default_bg);
        p.goto(1, content_top + i);
        let l_content_w = left_w.saturating_sub(1);
        p.text(&" ".repeat(l_content_w as usize));
        if let Some(line) = state.left_lines.get(li) {
            let in_hunk = state
                .hunks
                .get(state.current_hunk.min(state.hunks.len().saturating_sub(1)))
                .is_some_and(|h| li >= h.left_start && li < h.left_start + h.left_len);
            if in_hunk {
                p.set_fg_bg(pal.selected_fg, pal.selected_bg);
            } else {
                p.set_fg_bg(pal.core_default_fg, pal.core_default_bg);
            }
            let mut x = 1u16;
            if lnw > 0 {
                p.goto(x, content_top + i);
                p.text(&format!("{:>width$} ", li + 1, width = (lnw - 1) as usize));
                x += lnw;
            }
            p.goto(x, content_top + i);
            let avail = l_content_w.saturating_sub(x.saturating_sub(1));
            let t = truncate(line, avail as usize);
            p.text(&t);
        }
    }
    // Right pane
    for i in 0..content_h {
        let ri = state.right_scroll + i as usize;
        let base_x = divider_x.saturating_add(1);
        p.set_fg_bg(pal.core_default_fg, pal.core_default_bg);
        p.goto(base_x, content_top + i);
        let r_content_w = right_w.saturating_sub(1);
        p.text(&" ".repeat(r_content_w as usize));
        if let Some(line) = state.right_lines.get(ri) {
            let in_hunk = state
                .hunks
                .get(state.current_hunk.min(state.hunks.len().saturating_sub(1)))
                .is_some_and(|h| ri >= h.right_start && ri < h.right_start + h.right_len);
            if in_hunk {
                p.set_fg_bg(pal.selected_fg, pal.selected_bg);
            } else {
                p.set_fg_bg(pal.core_default_fg, pal.core_default_bg);
            }
            let mut x = base_x;
            if lnw > 0 {
                p.goto(x, content_top + i);
                p.text(&format!("{:>width$} ", ri + 1, width = (lnw - 1) as usize));
                x += lnw;
            }
            p.goto(x, content_top + i);
            let avail = r_content_w.saturating_sub(x - base_x);
            let t = truncate(line, avail as usize);
            p.text(&t);
        }
    }
    // Status line
    p.set_fg_bg(pal.statusbar_fg, pal.statusbar_bg);
    p.goto(0, status_row);
    let total_hunks = state.hunks.len();
    let cur_idx = if total_hunks == 0 {
        0
    } else {
        state.current_hunk + 1
    };
    let mut status = format!(
        " Hunk {}/{}    {}{}",
        cur_idx,
        total_hunks,
        if state.left_modified { "[L*] " } else { "" },
        if state.right_modified { "[R*]" } else { "" }
    );
    if state.show_hunk_status {
        if let Some(h) = state
            .hunks
            .get(state.current_hunk.min(state.hunks.len().saturating_sub(1)))
        {
            status.push_str(&format!(
                "   L:{}+{}  R:{}+{}",
                h.left_start, h.left_len, h.right_start, h.right_len
            ));
        }
    }
    let st = truncate(&status, cols as usize);
    p.text(&st);
    if st.len() < cols as usize {
        p.text(&" ".repeat(cols as usize - st.len()));
    }
    // F-bar
    draw_diff_fbar(p, fbar_row, cols, pal);
    // Overlays: search / goto / confirm-exit on top of diff
    if let Some(current) = &state.search_prompt {
        draw_dialog_box(p, cols, rows, pal, "Search", current, &["< OK >", "Cancel"]);
    }
    if let Some(current) = &state.goto_prompt {
        draw_dialog_box(
            p,
            cols,
            rows,
            pal,
            "Goto line",
            current,
            &["< OK >", "Cancel"],
        );
    }
    if let Some(c) = &state.confirm_exit {
        draw_dialog_ync(p, cols, rows, pal, &c.title, &c.message, c.focus);
    }
    Ok(())
}

fn draw_diff_fbar(p: &mut Painter, y: u16, cols: u16, pal: McPalette) {
    // F1 Help, F2 Save, F4 Edit, F5 Merge, F7 Search, F10 Quit; keep Menu on F9
    let labels = [
        "Help", "Save", "", "Edit", "Merge", "", "Search", "", "Menu", "Quit",
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
        let ltxt = if lab.is_empty() { " " } else { lab };
        p.text(ltxt);
        x += ltxt.len() as u16;
        if x < cols {
            p.goto(x, y);
            p.text(" ");
            x += 1;
        } else {
            break;
        }
    }
    if x < cols {
        p.goto(x, y);
        p.text(&" ".repeat(cols.saturating_sub(x) as usize));
    }
}
fn format_entry_name(ent: &FileEntry) -> String {
    if ent.name == ".." {
        "..".to_string()
    } else if ent.is_dir {
        ent.name.clone()
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
        "        ".to_string()
    } else {
        format!("{:>8}", ent.size)
    }
}

fn format_time(ent: &FileEntry) -> String {
    let dt: OffsetDateTime = ent.modified.into();
    dt.format(&time::macros::format_description!(
        "[month repr:short] [day padding:space] [hour]:[minute]"
    ))
    .unwrap_or_default()
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

fn format_mini_status(ent: &FileEntry) -> String {
    let perms = perm_string(ent.permissions, ent.is_dir);
    let owner = ent.owner.as_deref().unwrap_or("-");
    let group = ent.group.as_deref().unwrap_or("-");
    let size = if ent.is_dir { 0 } else { ent.size };
    let tm: OffsetDateTime = ent.modified.into();
    let ts = tm
        .format(&time::macros::format_description!(
            "[month repr:short] [day padding:space] [hour]:[minute]"
        ))
        .unwrap_or_default();
    format!("{perms}  {owner:>8} {group:>8} {size:>8} {ts}")
}

fn perm_string(mode: u32, is_dir: bool) -> String {
    let mut s = String::new();
    s.push(if is_dir { 'd' } else { '-' });
    let bits = [
        (0o400, 'r'),
        (0o200, 'w'),
        (0o100, 'x'),
        (0o040, 'r'),
        (0o020, 'w'),
        (0o010, 'x'),
        (0o004, 'r'),
        (0o002, 'w'),
        (0o001, 'x'),
    ];
    for (bit, ch) in bits {
        s.push(if mode & bit != 0 { ch } else { '-' });
    }
    s
}

fn human_bytes(b: u64) -> String {
    const G: f64 = 1024.0 * 1024.0 * 1024.0;
    const M: f64 = 1024.0 * 1024.0;
    if b as f64 >= G {
        format!("{:.0}G", (b as f64) / G)
    } else if b as f64 >= M {
        format!("{:.0}M", (b as f64) / M)
    } else {
        format!("{b}B")
    }
}

fn draw_mkdir_dialog(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    value: &str,
    focus_ok: bool,
) {
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
    p.vline(
        x + w - 1,
        y + 1,
        h - 2,
        '│',
        pal.frame_fg,
        pal.dialog_default_bg,
    );
    p.goto(x, y + h - 1);
    p.text("└");
    p.hline(
        x + 1,
        y + h - 1,
        w - 2,
        '─',
        pal.frame_fg,
        pal.dialog_default_bg,
    );
    p.goto(x + w - 1, y + h - 1);
    p.text("┘");
    // Title
    p.set_fg_bg(pal.dtitle_fg, pal.dtitle_bg);
    let title = " Create a new Directory ";
    let tx = x + (w.saturating_sub(title.len() as u16)) / 2;
    p.goto(tx, y);
    p.text(title);
    // Input line
    p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
    p.goto(x + 2, y + 2);
    let t = truncate(value, (w - 4) as usize);
    p.text(&format!("{t}{}", " ".repeat((w - 4) as usize - t.len())));
    // Buttons
    p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
    let ok = if focus_ok { "< OK >" } else { "  OK  " };
    let cancel = if focus_ok { " Cancel " } else { "[ Cancel ]" };
    let btns = format!("{ok}  {cancel}");
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
}

#[allow(clippy::too_many_arguments)]
fn draw_input_dialog(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    title: &str,
    prompt: &str,
    value: &str,
    focus_ok: bool,
) {
    let w = (cols as usize).min(66) as u16;
    let h = 9u16;
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
    p.vline(
        x + w - 1,
        y + 1,
        h - 2,
        '│',
        pal.frame_fg,
        pal.dialog_default_bg,
    );
    p.goto(x, y + h - 1);
    p.text("└");
    p.hline(
        x + 1,
        y + h - 1,
        w - 2,
        '─',
        pal.frame_fg,
        pal.dialog_default_bg,
    );
    p.goto(x + w - 1, y + h - 1);
    p.text("┘");
    // Title
    p.set_fg_bg(pal.dtitle_fg, pal.dtitle_bg);
    let ttl = format!(" {title} ");
    let tx = x + (w.saturating_sub(ttl.len() as u16)) / 2;
    p.goto(tx, y);
    p.text(&ttl);
    // Prompt
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(x + 2, y + 2);
    p.text(&truncate(prompt, (w - 4) as usize));
    // Input line
    p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
    p.goto(x + 2, y + 4);
    let t = truncate(value, (w - 4) as usize);
    p.text(&format!("{t}{}", " ".repeat((w - 4) as usize - t.len())));
    // Buttons
    p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
    let ok = if focus_ok { "< OK >" } else { "[ OK ]" };
    let cancel = if focus_ok { " Cancel " } else { "[ Cancel ]" };
    let btns = format!("{ok}  {cancel}");
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
}

#[allow(clippy::too_many_arguments)]
fn draw_copy_move_dialog(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    title: &str,
    src_name: &str,
    mask: &str,
    to: &str,
    using_shell_patterns: bool,
    follow_links: bool,
    preserve_attrs: bool,
    dive_into_subdir: bool,
    stable_symlinks: bool,
    focus: rmc_core::app::CopyDialogFocus,
) {
    use rmc_core::app::CopyDialogFocus as F;
    let w = (cols as usize).min(74) as u16;
    let h = 15u16;
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
    p.vline(
        x + w - 1,
        y + 1,
        h - 2,
        '│',
        pal.frame_fg,
        pal.dialog_default_bg,
    );
    p.goto(x, y + h - 1);
    p.text("└");
    p.hline(
        x + 1,
        y + h - 1,
        w - 2,
        '─',
        pal.frame_fg,
        pal.dialog_default_bg,
    );
    p.goto(x + w - 1, y + h - 1);
    p.text("┘");
    // Title
    p.set_fg_bg(pal.dtitle_fg, pal.dtitle_bg);
    let ttl = format!(" {title} ");
    let tx = x + (w.saturating_sub(ttl.len() as u16)) / 2;
    p.goto(tx, y);
    p.text(&ttl);
    // Lines
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(x + 2, y + 2);
    p.text(&truncate(
        &format!("{title} file \"{src_name}\" with source mask:"),
        (w - 4) as usize,
    ));
    // mask field
    let mask_focus = matches!(focus, F::Mask);
    p.set_fg_bg(
        if mask_focus {
            pal.dfocus_fg
        } else {
            pal.dialog_default_fg
        },
        if mask_focus {
            pal.dfocus_bg
        } else {
            pal.dialog_default_bg
        },
    );
    p.goto(x + 2, y + 3);
    let m = truncate(mask, (w - 4) as usize);
    p.text(&format!("{m}{}", " ".repeat((w - 4) as usize - m.len())));
    // to:
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(x + 2, y + 5);
    p.text("to:");
    let to_focus = matches!(focus, F::To);
    p.set_fg_bg(
        if to_focus {
            pal.dfocus_fg
        } else {
            pal.dialog_default_fg
        },
        if to_focus {
            pal.dfocus_bg
        } else {
            pal.dialog_default_bg
        },
    );
    p.goto(x + 6, y + 5);
    let t = truncate(to, (w - 8) as usize);
    p.text(&format!("{t}{}", " ".repeat((w - 8) as usize - t.len())));
    // Checkboxes
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    let checks = [
        ("Using shell patterns", using_shell_patterns),
        ("Follow links", follow_links),
        ("Preserve attributes", preserve_attrs),
        ("Dive into subdir if exists", dive_into_subdir),
        ("Stable symlinks", stable_symlinks),
    ];
    for (i, (label, on)) in checks.iter().enumerate() {
        let row_y = y + 7 + i as u16;
        p.goto(x + 4, row_y);
        let focused = matches!(
            (i, focus),
            (0, F::Checkbox1)
                | (1, F::Checkbox2)
                | (2, F::Checkbox3)
                | (3, F::Checkbox4)
                | (4, F::Checkbox5)
        );
        if focused {
            p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
        } else {
            p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
        }
        p.text(&format!("[{}] {}", if *on { 'x' } else { ' ' }, label));
    }
    // Buttons
    p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
    let sel = |f: F, txt: &str| {
        if f == focus {
            format!("< {txt} >")
        } else {
            format!("[ {txt} ]")
        }
    };
    let btns = format!(
        "{}  {}  {}",
        sel(F::Ok, "OK"),
        sel(F::Background, "Background"),
        sel(F::Cancel, "Cancel")
    );
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
}

#[allow(clippy::too_many_arguments)]
fn draw_chmod_dialog(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    name: &str,
    mode: u32,
    u: (bool, bool, bool),
    g: (bool, bool, bool),
    o: (bool, bool, bool),
    special: (bool, bool, bool), // suid, sgid, sticky
    recursive: bool,
    focus_index: usize,
) {
    let w = (cols as usize).min(66) as u16;
    let h = 14u16;
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
    p.vline(
        x + w - 1,
        y + 1,
        h - 2,
        '│',
        pal.frame_fg,
        pal.dialog_default_bg,
    );
    p.goto(x, y + h - 1);
    p.text("└");
    p.hline(
        x + 1,
        y + h - 1,
        w - 2,
        '─',
        pal.frame_fg,
        pal.dialog_default_bg,
    );
    p.goto(x + w - 1, y + h - 1);
    p.text("┘");
    // Title
    p.set_fg_bg(pal.dtitle_fg, pal.dtitle_bg);
    let ttl = " Chmod command ";
    let tx = x + (w.saturating_sub(ttl.len() as u16)) / 2;
    p.goto(tx, y);
    p.text(ttl);
    // Filename and octal
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(x + 2, y + 2);
    p.text(&truncate(&format!("File: {}", name), (w - 4) as usize));
    p.goto(x + 2, y + 3);
    p.text(&format!("Octal: {:04o}", mode & 0o7777));
    // Checkboxes: user/group/other rwx
    let labels = ["Read", "Write", "Exec"];
    let groups = ["User", "Group", "Other"];
    let vals = [u, g, o];
    let mut idx = 0usize;
    for (gi, gname) in groups.iter().enumerate() {
        p.goto(x + 2, y + 5 + gi as u16);
        p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
        p.text(&format!("{gname}: "));
        for (li, lab) in labels.iter().enumerate() {
            let on = match li {
                0 => vals[gi].0,
                1 => vals[gi].1,
                _ => vals[gi].2,
            };
            let focused = focus_index == idx;
            if focused {
                p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
            } else {
                p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
            }
            p.text(&format!("[{}] {}", if on { 'x' } else { ' ' }, lab));
            p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
            p.text("  ");
            idx += 1;
        }
    }
    // Special bits and recursive
    let specials = [
        ("Set UID", special.0),
        ("Set GID", special.1),
        ("Sticky", special.2),
    ];
    for (i, (lab, on)) in specials.iter().enumerate() {
        let focused = focus_index == (9 + i);
        if focused {
            p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
        } else {
            p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
        }
        p.goto(x + 2 + (i as u16) * 20, y + 8);
        p.text(&format!("[{}] {}", if *on { 'x' } else { ' ' }, lab));
    }
    let focused_rec = focus_index == 12;
    if focused_rec {
        p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
    } else {
        p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    }
    p.goto(x + 2, y + 10);
    p.text(&format!(
        "[{}] {}",
        if recursive { 'x' } else { ' ' },
        "Recursive"
    ));
    // Buttons
    let ok_focus = focus_index == 13;
    let cancel_focus = focus_index == 14;
    let ok = if ok_focus { "< OK >" } else { "[ OK ]" };
    let cancel = if cancel_focus {
        "< Cancel >"
    } else {
        "[ Cancel ]"
    };
    p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
    let btns = format!("{ok}  {cancel}");
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
}

#[allow(clippy::too_many_arguments)]
fn draw_chown_dialog(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    owner: &str,
    group: &str,
    recursive: bool,
    focus_index: usize,
) {
    let w = (cols as usize).min(66) as u16;
    let h = 10u16;
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
    p.vline(
        x + w - 1,
        y + 1,
        h - 2,
        '│',
        pal.frame_fg,
        pal.dialog_default_bg,
    );
    p.goto(x, y + h - 1);
    p.text("└");
    p.hline(
        x + 1,
        y + h - 1,
        w - 2,
        '─',
        pal.frame_fg,
        pal.dialog_default_bg,
    );
    p.goto(x + w - 1, y + h - 1);
    p.text("┘");
    // Title
    p.set_fg_bg(pal.dtitle_fg, pal.dtitle_bg);
    let ttl = " Chown ";
    let tx = x + (w.saturating_sub(ttl.len() as u16)) / 2;
    p.goto(tx, y);
    p.text(ttl);
    // Owner/group fields
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(x + 2, y + 2);
    p.text("Owner:");
    let own_focus = focus_index == 0;
    p.set_fg_bg(
        if own_focus {
            pal.dfocus_fg
        } else {
            pal.dialog_default_fg
        },
        if own_focus {
            pal.dfocus_bg
        } else {
            pal.dialog_default_bg
        },
    );
    p.goto(x + 10, y + 2);
    let ov = truncate(owner, (w - 12) as usize);
    p.text(&format!("{ov}{}", " ".repeat((w - 12) as usize - ov.len())));
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(x + 2, y + 3);
    p.text("Group:");
    let grp_focus = focus_index == 1;
    p.set_fg_bg(
        if grp_focus {
            pal.dfocus_fg
        } else {
            pal.dialog_default_fg
        },
        if grp_focus {
            pal.dfocus_bg
        } else {
            pal.dialog_default_bg
        },
    );
    p.goto(x + 10, y + 3);
    let gv = truncate(group, (w - 12) as usize);
    p.text(&format!("{gv}{}", " ".repeat((w - 12) as usize - gv.len())));
    // Recursive
    let rec_focus = focus_index == 2;
    p.set_fg_bg(
        if rec_focus {
            pal.dfocus_fg
        } else {
            pal.dialog_default_fg
        },
        if rec_focus {
            pal.dfocus_bg
        } else {
            pal.dialog_default_bg
        },
    );
    p.goto(x + 2, y + 5);
    p.text(&format!(
        "[{}] {}",
        if recursive { 'x' } else { ' ' },
        "Recursive"
    ));
    // Buttons
    let ok_focus = focus_index == 3;
    let cancel_focus = focus_index == 4;
    p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
    let ok = if ok_focus { "< OK >" } else { "[ OK ]" };
    let cancel = if cancel_focus {
        "< Cancel >"
    } else {
        "[ Cancel ]"
    };
    let btns = format!("{ok}  {cancel}");
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
}

fn draw_menu_dropdown(p: &mut Painter, pal: McPalette, top_index: usize, selected: usize) {
    // Real top menus and stub items
    let menus: [&[&str]; 5] = [
        &[
            "Copy",
            "Move",
            "Mkdir",
            "Delete",
            "FTP link",
            "SFTP link",
            "Sort order...",
            "Tree",
            "Filter",
        ],
        &[
            "View",
            "Edit",
            "Copy",
            "Move",
            "Mkdir",
            "Delete",
            "Chmod",
            "Chown",
            "Hard link",
            "SymLink",
            "Relative symlink",
            "Quit",
        ],
        &[
            "User menu",
            "Find file",
            "Directory hotlist",
            "Compare dirs",
        ],
        &["Layout", "Panels", "Confirmations"],
        &[
            "Copy",
            "Move",
            "Mkdir",
            "Delete",
            "FTP link",
            "SFTP link",
            "Sort order...",
            "Tree",
            "Filter",
        ],
    ];
    let titles = [" Left ", " File ", " Command ", " Options ", " Right "];
    // Compute x position under the selected top title
    let mut x = 0u16;
    for title in titles.iter().take(top_index) {
        x += title.len() as u16;
    }
    let items = menus[top_index];
    let y = 1u16;
    let w = (items.iter().map(|s| s.len()).max().unwrap_or(8) + 4) as u16;
    let h = items.len() as u16 + 2;
    p.set_fg_bg(pal.menu_fg, pal.menu_bg);
    // Frame
    p.goto(x, y);
    p.text("┌");
    p.hline(x + 1, y, w - 2, '─', pal.menu_fg, pal.menu_bg);
    p.goto(x + w - 1, y);
    p.text("┐");
    p.vline(x, y + 1, h - 2, '│', pal.menu_fg, pal.menu_bg);
    p.vline(x + w - 1, y + 1, h - 2, '│', pal.menu_fg, pal.menu_bg);
    p.goto(x, y + h - 1);
    p.text("└");
    p.hline(x + 1, y + h - 1, w - 2, '─', pal.menu_fg, pal.menu_bg);
    p.goto(x + w - 1, y + h - 1);
    p.text("┘");
    // Items
    for (i, it) in items.iter().enumerate() {
        let row = y + 1 + i as u16;
        if i == selected {
            p.set_fg_bg(pal.menusel_fg, pal.menusel_bg);
        } else {
            p.set_fg_bg(pal.menu_fg, pal.menu_bg);
        }
        p.goto(x + 1, row);
        let mut line = String::new();
        line.push(' ');
        line.push_str(it);
        while line.len() < (w - 2) as usize {
            line.push(' ');
        }
        p.text(&line);
    }
}
