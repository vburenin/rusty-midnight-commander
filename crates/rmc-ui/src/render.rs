use crate::dirtree::draw_directory_tree_dialog;
use crate::filehighlight::{listing_name_color, name_span_in_line};
use crate::find::draw_find_dialog;
use crate::help::{initial_topic_or_contents, HelpIndex, HelpItem};
use crate::mc_colors::McPalette;
use crate::panel_preview::{info_lines_for_panel, preview_source_entry, quick_view_directory_line};
use crate::panelize::draw_external_panelize_dialog;
use crate::widgets::Painter;
use anyhow::Result;
use crossterm::style::Color;
use crossterm::terminal::{self, Clear, ClearType};
use crossterm::QueueableCommand;
use rmc_core::app::{App, EditorMenu, LayoutFocus, LayoutOptions};
use rmc_core::layout::{compute_chrome_geom, dual_panel_rects, menu_bar_titles};
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

    pub fn set_palette(&mut self, palette: McPalette) {
        self.palette = palette;
    }

    pub fn draw(&mut self, app: &App) -> Result<()> {
        let (cols, rows) = terminal::size()?;
        let mut painter = Painter { out: &mut self.out };
        // After a waited external: do not clear/redraw panels so the program's
        // output stays visible until the user presses a key.
        if matches!(app.ui_mode, rmc_core::app::UiMode::PauseAfterRun) {
            draw_pause_after_run_prompt(&mut painter, cols, rows, self.palette);
            painter.out.flush()?;
            return Ok(());
        }
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
            save_as_dialog,
            search_dialog,
            replace_dialog,
            pipe_dialog,
            goto_dialog,
            tab_spacing_dialog,
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
                save_as_dialog.as_deref(),
                search_dialog.as_deref(),
                replace_dialog.as_deref(),
                pipe_dialog.as_ref(),
                goto_dialog.as_deref(),
                tab_spacing_dialog.as_deref(),
                confirm_exit.as_ref(),
                app.shadows,
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
            format_nroff,
            parsed,
            sel_anchor,
            sel_cursor,
            viewer_menu,
            search_dialog,
            display_dialog,
            status_msg,
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
                *format_nroff,
                *parsed,
                *sel_anchor,
                *sel_cursor,
                *viewer_menu,
                search_dialog.as_deref(),
                display_dialog.as_deref(),
                status_msg.as_deref(),
                goto_prompt,
                app.shadows,
            )?;
            painter.out.flush()?;
            return Ok(());
        }
        // Clear any lingering viewer state when not in viewer mode
        crate::terminal::viewer_clear_state();
        // Full-screen diff viewer short-circuit
        if let rmc_core::app::UiMode::Diff(state) = &app.ui_mode {
            draw_diff(&mut painter, cols, rows, self.palette, state, app.shadows)?;
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
        // XTerm window title (OSC 0) from active CWD when enabled (write raw; do not move cursor)
        if app.layout.xterm_title {
            let title = format!("{}", app.active_panel().cwd.display());
            let osc = format!("\x1b]0;{title}\x07");
            let _ = std::io::Write::write_all(&mut painter.out, osc.as_bytes());
        }
        // Menu bar
        if app.layout.menubar_visible {
            let selected_top = match &app.ui_mode {
                rmc_core::app::UiMode::Menu { top_index, .. } => Some(*top_index),
                _ => None,
            };
            draw_menu_bar(
                &mut painter,
                cols,
                self.palette,
                selected_top,
                app.layout.horizontal_split,
            );
        }
        // Panels area layout (shared geometry with terminal.rs for mouse hit-tests)
        // rows: [menu?] + 1 frame top + content + frame bottom + [gauge?] + [hint?] + [cmd?] + [fbar?]
        let geom = compute_chrome_geom(cols, rows, &app.layout);
        let (left_rect, right_rect) = dual_panel_rects(cols, &geom, &app.layout);
        draw_panel(
            &mut painter,
            left_rect.x,
            left_rect.y,
            left_rect.w,
            left_rect.h,
            true,
            app,
            true,
            self.palette,
        )?;
        draw_panel(
            &mut painter,
            right_rect.x,
            right_rect.y,
            right_rect.w,
            right_rect.h,
            false,
            app,
            false,
            self.palette,
        )?;
        // Gauge/status line between panels
        if let Some(y) = geom.gauge_row {
            draw_gauge(&mut painter, y, cols, self.palette, app);
        }
        if let Some(y) = geom.hint_row {
            draw_hint(&mut painter, y, cols, self.palette);
        }
        if let Some(y) = geom.cmd_row {
            draw_cmdline(&mut painter, y, cols, self.palette, app);
        }
        if let Some(y) = geom.fbar_row {
            draw_fbar(&mut painter, y, cols, self.palette);
        }
        // Overlays (dialogs)
        draw_overlays(&mut painter, app, cols, rows, self.palette)?;
        painter.out.flush()?;
        Ok(())
    }
}

fn draw_menu_bar(
    p: &mut Painter,
    cols: u16,
    pal: McPalette,
    selected: Option<usize>,
    horizontal_split: bool,
) {
    p.set_fg_bg(pal.menu_fg, pal.menu_bg);
    p.goto(0, 0);
    let items = menu_bar_titles(horizontal_split);
    let mut x = 0u16;
    for (i, it) in items.iter().enumerate() {
        if selected == Some(i) {
            p.set_fg_bg(pal.menusel_fg, pal.menusel_bg);
        } else {
            p.set_fg_bg(pal.menu_fg, pal.menu_bg);
        }
        p.goto(x, 0);
        p.text(it);
        x += it.len() as u16;
    }
    // Fill rest
    p.set_fg_bg(pal.menu_fg, pal.menu_bg);
    p.goto(x, 0);
    let rest = " ".repeat(cols.saturating_sub(x) as usize);
    p.text(&rest);
}

fn draw_pause_after_run_prompt(p: &mut Painter, cols: u16, rows: u16, pal: McPalette) {
    let y = rows.saturating_sub(1);
    p.fill_line(y, cols, pal.dialog_default_bg, pal.dialog_default_fg);
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(0, y);
    let msg = crate::terminal::PAUSE_AFTER_RUN_PROMPT;
    p.text(&truncate(msg, cols as usize));
}

fn draw_overlays(p: &mut Painter, app: &App, cols: u16, rows: u16, pal: McPalette) -> Result<()> {
    match &app.ui_mode {
        rmc_core::app::UiMode::DialogConfirm { title, message, .. } => {
            draw_dialog_box(
                p,
                cols,
                rows,
                pal,
                title,
                message,
                &["< OK >", "Cancel"],
                app.shadows,
            );
        }
        rmc_core::app::UiMode::ConfirmationsDialog { draft, focus } => {
            draw_confirmations_dialog(p, cols, rows, pal, draft, *focus, app.shadows);
        }
        rmc_core::app::UiMode::ConfigurationDialog { draft, focus } => {
            draw_configuration_dialog(p, cols, rows, pal, draft, *focus, app.shadows);
        }
        rmc_core::app::UiMode::VfsOptionsDialog { draft, focus } => {
            draw_vfs_options_dialog(p, cols, rows, pal, draft, *focus, app.shadows);
        }
        rmc_core::app::UiMode::PanelOptionsDialog { draft, focus } => {
            draw_panel_options_dialog(p, cols, rows, pal, draft, *focus, app.shadows);
        }
        rmc_core::app::UiMode::OverwriteDialog {
            op,
            src_path: _,
            dst_path,
            focus,
        } => {
            draw_overwrite_dialog(p, cols, rows, pal, *op, dst_path, *focus, app.shadows);
        }
        rmc_core::app::UiMode::InputDialog {
            title,
            prompt,
            value,
            focus_ok,
            ..
        } => {
            draw_input_dialog(
                p,
                cols,
                rows,
                pal,
                title,
                prompt,
                value,
                *focus_ok,
                app.shadows,
            );
        }
        rmc_core::app::UiMode::FtpConnectDialog {
            scheme,
            host,
            port,
            user,
            password,
            directory,
            anonymous,
            focus_index,
            focus_ok,
        } => {
            draw_ftp_connect_dialog(
                p,
                cols,
                rows,
                pal,
                scheme,
                host,
                port,
                user,
                password,
                directory,
                *anonymous,
                *focus_index,
                *focus_ok,
                app.shadows,
            );
        }
        rmc_core::app::UiMode::LayoutDialog { draft, focus } => {
            draw_layout_dialog(p, cols, rows, pal, draft, *focus, app.shadows);
        }
        rmc_core::app::UiMode::AppearanceDialog {
            draft_skin,
            draft_shadows,
            skins,
            selected,
            focus,
        } => {
            draw_appearance_dialog(
                p,
                cols,
                rows,
                pal,
                draft_skin,
                *draft_shadows,
                skins,
                *selected,
                *focus,
                app.shadows,
            );
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
                app.shadows,
            );
        }
        rmc_core::app::UiMode::ListingModeDialog {
            side,
            listing,
            user_format,
            focus,
        } => {
            draw_listing_mode_dialog(
                p,
                cols,
                rows,
                pal,
                *side,
                *listing,
                user_format,
                *focus,
                app.shadows,
            );
        }
        rmc_core::app::UiMode::FilterDialog {
            side,
            pattern,
            regular_expression,
            files_only,
            case_sensitive,
            focus,
        } => {
            draw_filter_dialog(
                p,
                cols,
                rows,
                pal,
                *side,
                pattern,
                *regular_expression,
                *files_only,
                *case_sensitive,
                *focus,
                app.shadows,
            );
        }
        rmc_core::app::UiMode::SelectGroupDialog {
            select,
            pattern,
            files_only,
            case_sensitive,
            regular_expression,
            focus,
        } => {
            draw_select_group_dialog(
                p,
                cols,
                rows,
                pal,
                *select,
                pattern,
                *files_only,
                *case_sensitive,
                *regular_expression,
                *focus,
                app.shadows,
            );
        }
        rmc_core::app::UiMode::FindDialog(state) => {
            draw_find_dialog(p, cols, rows, pal, state);
        }
        rmc_core::app::UiMode::HotlistDialog(state) => {
            crate::hotlist::draw_hotlist_dialog(p, cols, rows, pal, state);
        }
        rmc_core::app::UiMode::ExternalPanelizeDialog(state) => {
            draw_external_panelize_dialog(p, cols, rows, pal, state);
        }
        rmc_core::app::UiMode::DirectoryTree(state) => {
            draw_directory_tree_dialog(p, cols, rows, pal, state);
        }
        rmc_core::app::UiMode::HistoryDialog {
            selected_index,
            scroll_top,
            focus,
            confirm_clean,
        } => {
            draw_history_dialog(
                p,
                cols,
                rows,
                pal,
                app,
                *selected_index,
                *scroll_top,
                *focus,
                *confirm_clean,
                app.shadows,
            );
        }
        rmc_core::app::UiMode::PromptInput { title, value, .. } => {
            let msg = value.to_string();
            draw_dialog_box(
                p,
                cols,
                rows,
                pal,
                title,
                &msg,
                &["< OK >", "Cancel"],
                app.shadows,
            );
        }
        rmc_core::app::UiMode::MkdirDialog { value, focus_ok } => {
            draw_mkdir_dialog(p, cols, rows, pal, value, *focus_ok, app.shadows);
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
                app.shadows,
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
                app.shadows,
            );
        }
        rmc_core::app::UiMode::FileOpProgress { state, .. } => {
            let si = app.panel_opts.kilobyte_si;
            let w = (cols as usize).min(74);
            let bar_width = w.saturating_sub(6).max(16);
            let view = state.view(bar_width, si);
            draw_file_op_progress_dialog(p, cols, rows, pal, &view, app.shadows);
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
            allow_recursive,
            focus,
            ..
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
                *allow_recursive,
                *focus,
                app.shadows,
            );
        }
        rmc_core::app::UiMode::ChownDialog {
            owner,
            group,
            recursive,
            allow_recursive,
            focus,
            ..
        } => {
            draw_chown_dialog(
                p,
                cols,
                rows,
                pal,
                owner,
                group,
                *recursive,
                *allow_recursive,
                *focus,
                app.shadows,
            );
        }
        rmc_core::app::UiMode::LinkDialog {
            kind, value, focus, ..
        } => {
            use rmc_core::app::LinkDialogFocus as F;
            draw_input_dialog(
                p,
                cols,
                rows,
                pal,
                kind.title(),
                kind.prompt(),
                value,
                matches!(*focus, F::Ok),
                app.shadows,
            );
            if matches!(*focus, F::Cancel | F::Name) {
                let w = (cols as usize).min(66) as u16;
                let h = 9u16;
                let x = (cols - w) / 2;
                let y = (rows - h) / 2;
                p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
                let btns = if matches!(*focus, F::Cancel) {
                    "[ OK ]  < Cancel >"
                } else {
                    "[ OK ]  [ Cancel ]"
                };
                let bx = x + (w.saturating_sub(btns.len() as u16)) / 2;
                p.goto(bx, y + h - 2);
                p.text(btns);
            }
        }
        rmc_core::app::UiMode::Menu {
            top_index,
            selected_index,
            dropped,
        } => {
            if *dropped {
                draw_menu_dropdown(
                    p,
                    pal,
                    *top_index,
                    *selected_index,
                    app.layout.horizontal_split,
                );
            }
        }
        rmc_core::app::UiMode::JobsDialog {
            selected_index,
            focus,
        } => {
            draw_jobs_dialog(
                p,
                cols,
                rows,
                pal,
                app,
                *selected_index,
                *focus,
                app.shadows,
            );
        }
        rmc_core::app::UiMode::CompareDirsDialog { mode, focus } => {
            draw_compare_dirs_dialog(p, cols, rows, pal, *mode, *focus, app.shadows);
        }
        rmc_core::app::UiMode::LearnKeysDialog {
            draft,
            selected,
            capturing,
            focus_ok,
        } => {
            draw_learn_keys_dialog(
                p,
                cols,
                rows,
                pal,
                draft,
                *selected,
                *capturing,
                *focus_ok,
                app.shadows,
            );
        }
        rmc_core::app::UiMode::ScreenList {
            selected,
            scroll_top,
            focus,
            ..
        } => {
            draw_screen_list_dialog(
                p,
                cols,
                rows,
                pal,
                app,
                *selected,
                *scroll_top,
                *focus,
                app.shadows,
            );
        }
        rmc_core::app::UiMode::CompletionList {
            items,
            selected,
            scroll_top,
            ..
        } => {
            draw_completion_list(
                p,
                cols,
                rows,
                pal,
                items,
                *selected,
                *scroll_top,
                app.shadows,
            );
        }
        _ => {}
    }
    Ok(())
}

fn draw_panel_options_dialog(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    draft: &rmc_core::app::PanelOptions,
    focus: rmc_core::app::PanelOptionsFocus,
    show_shadow: bool,
) {
    let title = "Panel options";
    let w = 54u16.min(cols.saturating_sub(2));
    let h = 18u16.min(rows.saturating_sub(2)).max(14);
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
    // Options (checkboxes)
    use rmc_core::app::PanelOptionsFocus as F;
    let items: [(&str, bool, F); 10] = [
        ("Show hidden files", draft.show_hidden, F::ShowHidden),
        ("Mix all files", draft.mix_all_files, F::MixAllFiles),
        ("Mark moves down", draft.mark_moves_down, F::MarkMovesDown),
        (
            "Show mini-status",
            draft.show_mini_status,
            F::ShowMiniStatus,
        ),
        ("Use SI size units", draft.kilobyte_si, F::UseSiUnits),
        ("Fast directory reload", draft.fast_reload, F::FastReload),
        (
            "Reverse files only",
            draft.reverse_files_only,
            F::ReverseFilesOnly,
        ),
        ("Simple swap", draft.simple_swap, F::SimpleSwap),
        ("Auto save setup", draft.auto_save_setup, F::AutoSaveSetup),
        ("Lynx-like motion", draft.lynx_like, F::LynxLikeMotion),
    ];
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    for (i, (label, on, lf)) in items.iter().enumerate() {
        let row_y = y + 2 + i as u16;
        if focus == *lf {
            p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
        } else {
            p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
        }
        p.goto(x + 2, row_y);
        p.text(&format!("[{}] {}", if *on { 'x' } else { ' ' }, label));
    }
    // Buttons: focused `< txt >`, unfocused `[ txt ]`
    let ok_sel = matches!(focus, F::Ok);
    let cancel_sel = matches!(focus, F::Cancel);
    p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
    let ok_txt = if ok_sel { "< OK >" } else { "[ OK ]" };
    let cancel_txt = if cancel_sel {
        "< Cancel >"
    } else {
        "[ Cancel ]"
    };
    let btns = format!("{ok_txt}  {cancel_txt}");
    let bx = x + (w.saturating_sub(btns.len() as u16)) / 2;
    p.goto(bx, y + h - 2);
    p.text(&btns);
    // Shadow
    if show_shadow {
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
    show_shadow: bool,
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
    if show_shadow {
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
}

fn draw_confirmations_dialog(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    draft: &rmc_core::app::ConfirmOptions,
    focus: rmc_core::app::ConfirmationsFocus,
    show_shadow: bool,
) {
    let title = "Confirmation";
    let w = 54u16.min(cols.saturating_sub(2));
    let h = 14u16.min(rows.saturating_sub(2)).max(10);
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
    // Options (checkboxes)
    use rmc_core::app::ConfirmationsFocus as F;
    let items: [(&str, bool, F); 6] = [
        ("Delete", draft.delete, F::Delete),
        ("Overwrite", draft.overwrite, F::Overwrite),
        ("Execute", draft.execute, F::Execute),
        ("Exit", draft.exit, F::Exit),
        (
            "Directory hotlist",
            draft.directory_hotlist,
            F::DirectoryHotlist,
        ),
        ("History cleanup", draft.history_cleanup, F::HistoryCleanup),
    ];
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    for (i, (label, on, lf)) in items.iter().enumerate() {
        let row_y = y + 2 + i as u16;
        if focus == *lf {
            p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
        } else {
            p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
        }
        p.goto(x + 2, row_y);
        p.text(&format!("[{}] {}", if *on { 'x' } else { ' ' }, label));
    }
    // Buttons
    let ok_sel = matches!(focus, F::Ok);
    let cancel_sel = matches!(focus, F::Cancel);
    p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
    let ok_txt = if ok_sel { "< OK >" } else { "  OK  " };
    let cancel_txt = if cancel_sel {
        "[ Cancel ]"
    } else {
        "  Cancel  "
    };
    let btns = format!("{ok_txt}  {cancel_txt}");
    let bx = x + (w.saturating_sub(btns.len() as u16)) / 2;
    p.goto(bx, y + h - 2);
    p.text(&btns);
    // Shadow
    if show_shadow {
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
}

fn draw_configuration_dialog(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    draft: &rmc_core::app::ConfigOptions,
    focus: rmc_core::app::ConfigOptionsFocus,
    show_shadow: bool,
) {
    let title = "Configuration";
    // Width based on longest label; 13 options + 2 rows for buttons/title
    let w = 60u16.min(cols.saturating_sub(2)).max(40);
    let h = 19u16.min(rows.saturating_sub(2)).max(15);
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
    // Options (checkboxes)
    use rmc_core::app::ConfigOptionsFocus as F;
    let items: [(&str, bool, F); 13] = [
        ("Verbose operation", draft.verbose, F::Verbose),
        ("Compute totals", draft.compute_totals, F::ComputeTotals),
        (
            "Classic progressbar",
            draft.classic_progressbar,
            F::ClassicProgressbar,
        ),
        (
            "Preallocate space",
            draft.preallocate_space,
            F::PreallocateSpace,
        ),
        (
            "Use COW file cloning",
            draft.use_cow_file_cloning,
            F::UseCowFileCloning,
        ),
        (
            "Use internal viewer",
            draft.use_internal_view,
            F::UseInternalViewer,
        ),
        (
            "Use internal editor",
            draft.use_internal_edit,
            F::UseInternalEditor,
        ),
        ("Pause after run", draft.pause_after_run, F::PauseAfterRun),
        ("Shell patterns", draft.shell_patterns, F::ShellPatterns),
        ("Auto menus", draft.auto_menus, F::AutoMenus),
        ("Drop down menus", draft.drop_menus, F::DropMenus),
        ("Mkdir autoname", draft.mkdir_autoname, F::MkdirAutoname),
        (
            "Complete: show all",
            draft.complete_show_all,
            F::CompleteShowAll,
        ),
    ];
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    for (i, (label, on, lf)) in items.iter().enumerate() {
        let row_y = y + 2 + i as u16;
        if focus == *lf {
            p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
        } else {
            p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
        }
        p.goto(x + 2, row_y);
        p.text(&format!("[{}] {}", if *on { 'x' } else { ' ' }, label));
    }
    // Buttons
    let ok_sel = matches!(focus, F::Ok);
    let cancel_sel = matches!(focus, F::Cancel);
    p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
    let ok_txt = if ok_sel { "< OK >" } else { "  OK  " };
    let cancel_txt = if cancel_sel {
        "[ Cancel ]"
    } else {
        "  Cancel  "
    };
    let btns = format!("{ok_txt}  {cancel_txt}");
    let bx = x + (w.saturating_sub(btns.len() as u16)) / 2;
    p.goto(bx, y + h - 2);
    p.text(&btns);
    // Shadow
    if show_shadow {
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
}

fn draw_vfs_options_dialog(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    draft: &rmc_core::app::VfsOptions,
    focus: rmc_core::app::VfsOptionsFocus,
    show_shadow: bool,
) {
    let title = "Virtual FS";
    let w = 64u16.min(cols.saturating_sub(2)).max(48);
    let h = 14u16.min(rows.saturating_sub(2)).max(12);
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
    // Rows: two checkboxes + three inputs
    use rmc_core::app::VfsOptionsFocus as F;
    let row0 = y + 2;
    // Always use ftp proxy [x]
    if matches!(focus, F::AlwaysUseFtpProxy) {
        p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
    } else {
        p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    }
    p.goto(x + 2, row0);
    p.text(&format!(
        "[{}] Always use ftp proxy",
        if draft.always_use_ftp_proxy { 'x' } else { ' ' }
    ));
    // FTP proxy host: value
    if matches!(focus, F::FtpProxyHost) {
        p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
    } else {
        p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    }
    p.goto(x + 2, row0 + 1);
    let host_label = "FTP proxy host:";
    p.text(host_label);
    let max_host = w.saturating_sub(4 + host_label.len() as u16);
    let shown = truncate(&draft.ftp_proxy_host, max_host as usize);
    p.goto(x + 2 + host_label.len() as u16 + 1, row0 + 1);
    p.text(&shown);
    // Use ~/.netrc
    if matches!(focus, F::UseNetrc) {
        p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
    } else {
        p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    }
    p.goto(x + 2, row0 + 2);
    p.text(&format!(
        "[{}] Use ~/.netrc",
        if draft.use_netrc { 'x' } else { ' ' }
    ));
    // FTP anonymous password:
    if matches!(focus, F::FtpAnonPassword) {
        p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
    } else {
        p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    }
    p.goto(x + 2, row0 + 3);
    let anon_label = "FTP anonymous password:";
    p.text(anon_label);
    let max_pass = w.saturating_sub(4 + anon_label.len() as u16);
    let shown_pass = truncate(&draft.ftp_anon_password, max_pass as usize);
    p.goto(x + 2 + anon_label.len() as u16 + 1, row0 + 3);
    p.text(&shown_pass);
    // Directory cache timeout:
    if matches!(focus, F::DirCacheTimeout) {
        p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
    } else {
        p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    }
    p.goto(x + 2, row0 + 4);
    let ttl_label = "Directory cache timeout (sec):";
    p.text(ttl_label);
    let ttl_str = draft.dir_cache_timeout_secs.to_string();
    let max_ttl = w.saturating_sub(4 + ttl_label.len() as u16);
    let shown_ttl = truncate(&ttl_str, max_ttl as usize);
    p.goto(x + 2 + ttl_label.len() as u16 + 1, row0 + 4);
    p.text(&shown_ttl);
    // Buttons
    let ok_sel = matches!(focus, F::Ok);
    let cancel_sel = matches!(focus, F::Cancel);
    p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
    let ok_txt = if ok_sel { "< OK >" } else { "  OK  " };
    let cancel_txt = if cancel_sel {
        "[ Cancel ]"
    } else {
        "  Cancel  "
    };
    let btns = format!("{ok_txt}  {cancel_txt}");
    let bx = x + (w.saturating_sub(btns.len() as u16)) / 2;
    p.goto(bx, y + h - 2);
    p.text(&btns);
    if show_shadow {
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
}
#[allow(clippy::too_many_arguments)]
fn draw_learn_keys_dialog(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    draft: &[(rmc_core::actions::Action, crossterm::event::KeyEvent)],
    selected: usize,
    capturing: bool,
    focus_ok: bool,
    show_shadow: bool,
) {
    let title = "Learn keys";
    // Labels matching the draft order
    let labels: [&str; 15] = [
        "Help",
        "User menu",
        "View",
        "Edit",
        "Copy",
        "Rename/Move",
        "Make directory",
        "Delete",
        "Pull down",
        "Quit",
        "Select",
        "Subshell",
        "Hidden files",
        "Swap panels",
        "Refresh",
    ];
    // Helper to format keys similar to config writer
    let fmt_key = |ev: &crossterm::event::KeyEvent| -> String {
        use crossterm::event::{KeyCode, KeyModifiers};
        let mut out = String::new();
        if ev.modifiers.contains(KeyModifiers::CONTROL) {
            out.push_str("C-");
        }
        if ev.modifiers.contains(KeyModifiers::ALT) {
            out.push_str("Alt-");
        }
        match ev.code {
            KeyCode::Up => out.push_str("Up"),
            KeyCode::Down => out.push_str("Down"),
            KeyCode::Left => out.push_str("Left"),
            KeyCode::Right => out.push_str("Right"),
            KeyCode::Home => out.push_str("Home"),
            KeyCode::End => out.push_str("End"),
            KeyCode::PageUp => out.push_str("PageUp"),
            KeyCode::PageDown => out.push_str("PageDown"),
            KeyCode::Tab => out.push_str("Tab"),
            KeyCode::Enter => out.push_str("Enter"),
            KeyCode::Backspace => out.push_str("Backspace"),
            KeyCode::Insert => out.push_str("Insert"),
            KeyCode::Char(' ') => out.push_str("Space"),
            KeyCode::Char(ch) => out.push(ch),
            KeyCode::F(n) => out.push_str(&format!("F{n}")),
            _ => out.push('?'),
        }
        out
    };
    let rows_len = draft.len().min(labels.len());
    // Compute width based on longest "label    key"
    let mut max_line = 0usize;
    for i in 0..rows_len {
        let key = fmt_key(&draft[i].1);
        max_line = max_line.max(labels[i].len() + 2 + key.len());
    }
    let w = (max_line + 8).clamp(40, 72) as u16;
    let h = (rows_len as u16 + 6).min(rows.saturating_sub(2)).max(10);
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
    let ttl = format!(" {title} ");
    let tx = x + (w.saturating_sub(ttl.len() as u16)) / 2;
    p.goto(tx, y);
    p.text(&ttl);
    // Hint while capturing
    if capturing {
        p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
        p.goto(x + 2, y + 1);
        p.text("Press the wanted key or Esc to skip");
    }
    // Rows
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    let list_top = y + 2;
    for i in 0..rows_len {
        let row_y = list_top + i as u16;
        if selected == i {
            p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
        } else {
            p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
        }
        p.goto(x + 2, row_y);
        let key = fmt_key(&draft[i].1);
        let mut line = format!("{}  {}", labels[i], key);
        while line.len() < (w - 4) as usize {
            line.push(' ');
        }
        p.text(&line);
    }
    // Buttons
    let btn_row = y + h - 2;
    let ok_txt = if selected == rows_len && focus_ok {
        "< OK >"
    } else {
        "  OK  "
    };
    let cancel_txt = if selected == rows_len && !focus_ok {
        "[ Cancel ]"
    } else {
        "  Cancel  "
    };
    p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
    let btns = format!("{ok_txt}  {cancel_txt}");
    let bx = x + (w.saturating_sub(btns.len() as u16)) / 2;
    p.goto(bx, btn_row);
    p.text(&btns);
    // Shadow
    if show_shadow {
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
}

fn draw_compare_dirs_dialog(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    mode: rmc_core::app::CompareDirsMode,
    focus: rmc_core::app::CompareDirsFocus,
    show_shadow: bool,
) {
    let title = "Compare directories";
    let w = 50u16.min(cols.saturating_sub(2));
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
    // Radios
    let radios = [
        (
            "Quick (size OR mtime differ)",
            rmc_core::app::CompareDirsMode::Quick,
        ),
        (
            "Size only (size differ)",
            rmc_core::app::CompareDirsMode::SizeOnly,
        ),
        (
            "Thorough (byte contents)",
            rmc_core::app::CompareDirsMode::Thorough,
        ),
    ];
    for (i, (label, kind)) in radios.iter().enumerate() {
        let row_y = y + 2 + i as u16;
        let sel = if *kind == mode { 'x' } else { ' ' };
        let f = match i {
            0 => rmc_core::app::CompareDirsFocus::RadioQuick,
            1 => rmc_core::app::CompareDirsFocus::RadioSizeOnly,
            _ => rmc_core::app::CompareDirsFocus::RadioThorough,
        };
        if focus == f {
            p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
        } else {
            p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
        }
        p.goto(x + 2, row_y);
        p.text(&format!("({sel}) {label}"));
    }
    // Buttons
    let ok_txt = if matches!(focus, rmc_core::app::CompareDirsFocus::Ok) {
        "< OK >"
    } else {
        "  OK  "
    };
    let cancel_txt = if matches!(focus, rmc_core::app::CompareDirsFocus::Cancel) {
        "[ Cancel ]"
    } else {
        "  Cancel  "
    };
    p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
    let btns = format!("{ok_txt}  {cancel_txt}");
    let bx = x + (w.saturating_sub(btns.len() as u16)) / 2;
    p.goto(bx, y + h - 2);
    p.text(&btns);
    // Shadow
    if show_shadow {
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

fn draw_layout_dialog(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    draft: &LayoutOptions,
    focus: LayoutFocus,
    show_shadow: bool,
) {
    let title = "Layout";
    let w = 54u16.min(cols.saturating_sub(2));
    let h = 17u16.min(rows.saturating_sub(2)).max(12);
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
    // Panel split radios + Equal split (GNU mc Layout "Panel split" group)
    let radios: [(&str, bool, LayoutFocus); 2] = [
        (
            "Vertical",
            !draft.horizontal_split,
            LayoutFocus::SplitVertical,
        ),
        (
            "Horizontal",
            draft.horizontal_split,
            LayoutFocus::SplitHorizontal,
        ),
    ];
    for (i, (label, on, lf)) in radios.iter().enumerate() {
        let row_y = y + 2 + i as u16;
        if focus == *lf {
            p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
        } else {
            p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
        }
        p.goto(x + 2, row_y);
        p.text(&format!("({}) {}", if *on { 'x' } else { ' ' }, label));
    }
    {
        let row_y = y + 4;
        if focus == LayoutFocus::EqualSplit {
            p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
        } else {
            p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
        }
        p.goto(x + 2, row_y);
        p.text(&format!(
            "[{}] Equal split",
            if draft.equal_split { 'x' } else { ' ' }
        ));
    }
    // Other options (existing six checkboxes; do not restaff)
    let items: [(&str, bool, LayoutFocus); 6] = [
        (
            "Menu bar visible",
            draft.menubar_visible,
            LayoutFocus::MenuBar,
        ),
        (
            "Command prompt",
            draft.command_prompt,
            LayoutFocus::CommandPrompt,
        ),
        ("Keybar visible", draft.keybar_visible, LayoutFocus::KeyBar),
        (
            "Hintbar visible",
            draft.hintbar_visible,
            LayoutFocus::HintBar,
        ),
        (
            "XTerm window title",
            draft.xterm_title,
            LayoutFocus::XtermTitle,
        ),
        (
            "Show free space",
            draft.show_free_space,
            LayoutFocus::ShowFreeSpace,
        ),
    ];
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    for (i, (label, on, lf)) in items.iter().enumerate() {
        let row_y = y + 6 + i as u16;
        if focus == *lf {
            p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
        } else {
            p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
        }
        p.goto(x + 2, row_y);
        p.text(&format!("[{}] {}", if *on { 'x' } else { ' ' }, label));
    }
    // Buttons: focused `< txt >`, unfocused `[ txt ]`
    let ok_sel = matches!(focus, LayoutFocus::Ok);
    let cancel_sel = matches!(focus, LayoutFocus::Cancel);
    p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
    let ok_txt = if ok_sel { "< OK >" } else { "[ OK ]" };
    let cancel_txt = if cancel_sel {
        "< Cancel >"
    } else {
        "[ Cancel ]"
    };
    let btns = format!("{ok_txt}  {cancel_txt}");
    let bx = x + (w.saturating_sub(btns.len() as u16)) / 2;
    p.goto(bx, y + h - 2);
    p.text(&btns);
    // Shadow
    if show_shadow {
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
}

#[allow(clippy::too_many_arguments)]
fn draw_appearance_dialog(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    draft_skin: &str,
    draft_shadows: bool,
    skins: &[String],
    selected: usize,
    focus: rmc_core::app::AppearanceFocus,
    show_shadow: bool,
) {
    let title = "Appearance";
    // Width based on longest skin name + padding
    let mut max_name = skins.iter().map(|s| s.len()).max().unwrap_or(7);
    max_name = max_name.max("default".len());
    let list_w = (max_name + 6).clamp(20, 60) as u16;
    let w = list_w;
    // rows: frame top + title + list + shadows + buttons + frame bottom
    let list_h = skins.len() as u16;
    let base_h = 2 /*title*/ + list_h + 3 /*shadows+space*/ + 2 /*buttons+space*/;
    let h = base_h.min(rows.saturating_sub(2)).max(10);
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
    let ttl = format!(" {title} ");
    let tx = x + (w.saturating_sub(ttl.len() as u16)) / 2;
    p.goto(tx, y);
    p.text(&ttl);
    // Skin list
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    let list_top = y + 2;
    for (i, name) in skins.iter().enumerate() {
        let row = list_top + i as u16;
        if i == selected && matches!(focus, rmc_core::app::AppearanceFocus::SkinList) {
            p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
        } else {
            p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
        }
        p.goto(x + 2, row);
        let mark = if name == draft_skin { '*' } else { ' ' };
        let mut line = format!("{mark} {name}");
        while line.len() < (w - 4) as usize {
            line.push(' ');
        }
        p.text(&line);
    }
    // Shadows checkbox (one row below list)
    let shadows_row = list_top + list_h + 1;
    if matches!(focus, rmc_core::app::AppearanceFocus::Shadows) {
        p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
    } else {
        p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    }
    p.goto(x + 2, shadows_row);
    p.text(&format!(
        "[{}] {}",
        if draft_shadows { 'x' } else { ' ' },
        "Shadows"
    ));
    // Buttons row at bottom-2
    let btn_row = y + h - 2;
    p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
    let ok_txt = if matches!(focus, rmc_core::app::AppearanceFocus::Ok) {
        "< OK >"
    } else {
        "  OK  "
    };
    let cancel_txt = if matches!(focus, rmc_core::app::AppearanceFocus::Cancel) {
        "[ Cancel ]"
    } else {
        "  Cancel  "
    };
    let btns = format!("{ok_txt}  {cancel_txt}");
    let bx = x + (w.saturating_sub(btns.len() as u16)) / 2;
    p.goto(bx, btn_row);
    p.text(&btns);
    // Shadow
    if show_shadow {
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
}

/// Map a syntax token (and selection overlay) onto public `[editor]` palette pairs.
/// `editmarked` always wins over keyword/comment/string colors.
fn editor_cell_style(kind: rmc_edit::TokenKind, selected: bool, pal: McPalette) -> (Color, Color) {
    if selected {
        return (pal.edit_marked_fg, pal.edit_marked_bg);
    }
    use rmc_edit::TokenKind as K;
    match kind {
        K::Keyword | K::Type | K::Preproc | K::Heading => (pal.edit_bold_fg, pal.edit_bold_bg),
        K::Comment => {
            let fg = pal.edit_whitespace_fg;
            let bg = pal.edit_normal_bg;
            if fg == bg {
                (pal.edit_linestate_fg, pal.edit_normal_bg)
            } else {
                (fg, bg)
            }
        }
        K::String | K::Number | K::Code | K::Link | K::Emphasis => {
            (pal.edit_linestate_fg, pal.edit_linestate_bg)
        }
        _ => (pal.edit_normal_fg, pal.edit_normal_bg),
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_editor(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    buf: &rmc_edit::EditorBuffer,
    show_menu: Option<EditorMenu>,
    status_msg: Option<&str>,
    _search_input: Option<&str>,
    save_as_dialog: Option<&rmc_core::app::EditorSaveAsDialog>,
    search_dialog: Option<&rmc_core::app::EditorSearchDialog>,
    replace_dialog: Option<&rmc_core::app::EditorReplaceDialog>,
    pipe_dialog: Option<&rmc_core::app::EditorPipeDialog>,
    goto_dialog: Option<&rmc_core::app::EditorGotoDialog>,
    tab_spacing_dialog: Option<&rmc_core::app::EditorTabSpacingDialog>,
    confirm: Option<&rmc_core::app::YncDialog>,
    show_shadow: bool,
) {
    // Background (editor [editor] _default_ pair)
    p.set_fg_bg(pal.edit_normal_fg, pal.edit_normal_bg);
    for y in 0..rows {
        p.goto(0, y);
        p.text(&" ".repeat(cols as usize));
    }
    // Top bar (mcedit menu bar)
    draw_editor_menu_bar(p, cols, pal, show_menu);
    // Status line (bottom-2) and F-bar (bottom-1)
    let status_row = rows.saturating_sub(2);
    let fbar_row = rows.saturating_sub(1);
    // Editor content box between menu and status
    let content_top = 1u16;
    let content_h = status_row.saturating_sub(content_top);
    // Render buffer window
    // We can't mutate buf here; assume viewport was adjusted by the event loop.
    // Draw content lines
    p.set_fg_bg(pal.edit_normal_fg, pal.edit_normal_bg);
    for i in 0..content_h {
        p.goto(0, content_top + i);
        p.text(&" ".repeat(cols as usize));
    }
    // Spans for syntax coloring (visible window only)
    let view_spans = buf.render_window_spans(cols as usize, content_h as usize);
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
            let t = truncate(&tok.text, cols as usize - drawn_cols);
            let tok_len = t.chars().count();
            if tok_len == 0 {
                continue;
            }
            match sel {
                None => {
                    let (fg, bg) = editor_cell_style(kind, false, pal);
                    p.set_fg_bg(fg, bg);
                    p.text(&t);
                    drawn_cols += tok_len;
                }
                Some((sa, sb)) => {
                    // Non-overlapping entirely before selection
                    if drawn_cols + tok_len <= sa || drawn_cols >= sb {
                        let (fg, bg) = editor_cell_style(kind, false, pal);
                        p.set_fg_bg(fg, bg);
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
                        let (fg, bg) = editor_cell_style(kind, false, pal);
                        p.set_fg_bg(fg, bg);
                        let left: String = t.chars().take(left_len).collect();
                        p.text(&left);
                    }
                    // Selection part: editmarked wins over syntax colors
                    if sel_end > sel_start {
                        let (fg, bg) = editor_cell_style(kind, true, pal);
                        p.set_fg_bg(fg, bg);
                        let sel_txt: String = t
                            .chars()
                            .skip(sel_start)
                            .take(sel_end - sel_start)
                            .collect();
                        p.text(&sel_txt);
                    }
                    // Right part
                    if right_len > 0 {
                        let (fg, bg) = editor_cell_style(kind, false, pal);
                        p.set_fg_bg(fg, bg);
                        let right: String = t.chars().skip(sel_end).collect();
                        p.text(&right);
                    }
                    drawn_cols += tok_len;
                }
            }
        }
        // If we drew less than full width, pad the rest
        if drawn_cols < cols as usize {
            p.set_fg_bg(pal.edit_normal_fg, pal.edit_normal_bg);
            p.text(&" ".repeat(cols as usize - drawn_cols));
        }
        p.set_fg_bg(pal.edit_normal_fg, pal.edit_normal_bg);
    }
    // Cursor indicator (soft, we don't move real terminal cursor here)
    // Draw a small inverse cell where the logical cursor is on screen
    let cur_y = buf.row.saturating_sub(buf.view_row) as u16 + content_top;
    let cur_x = buf.cursor_visual_col().saturating_sub(buf.view_col) as u16;
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
        p.set_fg_bg(pal.edit_normal_bg, pal.edit_normal_fg);
        p.text(&ch.to_string());
        // Restore default for safety
        p.set_fg_bg(pal.edit_normal_fg, pal.edit_normal_bg);
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
    // If show_menu, draw the GNU mcedit drop-down under the active title
    if let Some(menu) = show_menu {
        draw_editor_menu_dropdown(p, pal, menu);
    }
    if let Some(dlg) = save_as_dialog {
        draw_editor_save_as_dialog(p, cols, rows, pal, dlg, show_shadow);
    }
    if let Some(dlg) = search_dialog {
        draw_editor_search_dialog(p, cols, rows, pal, dlg, show_shadow);
    }
    if let Some(dlg) = replace_dialog {
        draw_editor_replace_dialog(p, cols, rows, pal, dlg, show_shadow);
    }
    if let Some(dlg) = pipe_dialog {
        draw_editor_pipe_dialog(p, cols, rows, pal, dlg, show_shadow);
    }
    if let Some(dlg) = goto_dialog {
        draw_editor_goto_dialog(p, cols, rows, pal, dlg, show_shadow);
    }
    if let Some(dlg) = tab_spacing_dialog {
        draw_editor_tab_spacing_dialog(p, cols, rows, pal, dlg, show_shadow);
    }
    if let Some(c) = confirm {
        draw_dialog_ync(
            p,
            cols,
            rows,
            pal,
            &c.title,
            &c.message,
            c.focus,
            show_shadow,
        );
    }
}

fn draw_editor_menu_bar(p: &mut Painter, cols: u16, pal: McPalette, show_menu: Option<EditorMenu>) {
    p.set_fg_bg(pal.menu_fg, pal.menu_bg);
    p.goto(0, 0);
    let selected = show_menu.map(EditorMenu::index);
    let mut x = 0u16;
    for (i, it) in EditorMenu::TITLES.iter().enumerate() {
        draw_menu_hotkey_label(p, x, 0, it, selected == Some(i), pal, it.len());
        x += it.len() as u16;
    }
    // Fill rest
    if x < cols {
        p.set_fg_bg(pal.menu_fg, pal.menu_bg);
        p.goto(x, 0);
        p.text(&" ".repeat(cols.saturating_sub(x) as usize));
    }
}

/// Menu chrome: default white;cyan, selected white;black, hotkey yellow;cyan
/// (same pairs as the panel menu bar). First non-space letter is the hotkey.
fn draw_menu_hotkey_label(
    p: &mut Painter,
    x: u16,
    y: u16,
    text: &str,
    selected: bool,
    pal: McPalette,
    width: usize,
) {
    p.goto(x, y);
    let mut line = text.to_string();
    while line.chars().count() < width {
        line.push(' ');
    }
    if selected {
        p.set_fg_bg(pal.menusel_fg, pal.menusel_bg);
        p.text(&line.chars().take(width).collect::<String>());
        return;
    }
    let mut hotkey_done = false;
    let mut drawn = 0usize;
    for ch in line.chars().take(width) {
        if !hotkey_done && !ch.is_whitespace() {
            p.set_fg_bg(pal.menuhot_fg, pal.menuhot_bg);
            hotkey_done = true;
        } else {
            p.set_fg_bg(pal.menu_fg, pal.menu_bg);
        }
        p.text(&ch.to_string());
        drawn += 1;
    }
    if drawn < width {
        p.set_fg_bg(pal.menu_fg, pal.menu_bg);
        p.text(&" ".repeat(width - drawn));
    }
}

fn draw_editor_menu_dropdown(p: &mut Painter, pal: McPalette, menu: EditorMenu) {
    let items = menu.items();
    if items.is_empty() {
        return;
    }
    let mut x = 0u16;
    for title in EditorMenu::TITLES.iter().take(menu.index()) {
        x += title.len() as u16;
    }
    let y = 1u16;
    let inner = items.iter().map(|s| s.len()).max().unwrap_or(8) + 2;
    let w = (inner + 2) as u16;
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
    let selected = menu.selected();
    for (i, it) in items.iter().enumerate() {
        let row = y + 1 + i as u16;
        let mut label = String::from(" ");
        label.push_str(it);
        draw_menu_hotkey_label(p, x + 1, row, &label, i == selected, pal, inner);
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

fn draw_editor_search_dialog(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    dlg: &rmc_core::app::EditorSearchDialog,
    show_shadow: bool,
) {
    use rmc_core::app::EditorSearchFocus as F;
    let w = (cols as usize).min(66) as u16;
    let h = 10u16;
    if cols < w || rows < h {
        return;
    }
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
    // Title — GNU mcedit wording
    p.set_fg_bg(pal.dtitle_fg, pal.dtitle_bg);
    let title = " Search ";
    let tx = x + (w.saturating_sub(title.len() as u16)) / 2;
    p.goto(tx, y);
    p.text(title);
    // Inner fill
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    for i in 1..h - 1 {
        p.goto(x + 1, y + i);
        p.text(&" ".repeat((w - 2) as usize));
    }
    let inner_w = (w - 4) as usize;
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(x + 2, y + 1);
    p.text(&truncate("Enter search string:", inner_w));
    if matches!(dlg.focus, F::Search) {
        p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
    } else {
        p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    }
    p.goto(x + 2, y + 2);
    let st = truncate(&dlg.search, inner_w);
    p.text(&format!(
        "{st}{}",
        " ".repeat(inner_w.saturating_sub(st.len()))
    ));
    let checks: [(F, &str, bool); 4] = [
        (F::CaseSensitive, "Case sensitive", dlg.case_sensitive),
        (F::Backwards, "Backwards", dlg.backwards),
        (F::WholeWords, "Whole words", dlg.whole_words),
        (
            F::RegularExpression,
            "Regular expression",
            dlg.regular_expression,
        ),
    ];
    for (i, (focus, label, on)) in checks.iter().enumerate() {
        if dlg.focus == *focus {
            p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
        } else {
            p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
        }
        p.goto(x + 2, y + 3 + i as u16);
        p.text(&truncate(
            &format!("[{}] {}", if *on { 'x' } else { ' ' }, label),
            inner_w,
        ));
    }
    // Buttons: focused `< OK >`, unfocused `[ Cancel ]` (GNU mc / History / Replace / Pipe / Goto)
    let focus = dlg.focus;
    let sel_btn = |want, txt: &str| {
        if focus == want {
            format!("< {txt} >")
        } else {
            format!("[ {txt} ]")
        }
    };
    p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
    let btns = format!("{}  {}", sel_btn(F::Ok, "OK"), sel_btn(F::Cancel, "Cancel"));
    let bx = x + (w.saturating_sub(btns.len() as u16)) / 2;
    p.goto(bx, y + h - 2);
    p.text(&btns);
    if show_shadow {
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
}

fn draw_viewer_display_dialog(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    dlg: &rmc_core::app::ViewerDisplayDialog,
    show_shadow: bool,
) {
    use rmc_core::app::ViewerDisplayFocus as F;
    let w = (cols as usize).min(66) as u16;
    let h = 8u16;
    if cols < w || rows < h {
        return;
    }
    let x = (cols - w) / 2;
    let y = (rows - h) / 2;
    // Frame — same chrome as editor Search / Save as
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
    // Title — PARITY.md / mc(1) Internal File Viewer display options
    p.set_fg_bg(pal.dtitle_fg, pal.dtitle_bg);
    let title = " Display options ";
    let tx = x + (w.saturating_sub(title.len() as u16)) / 2;
    p.goto(tx, y);
    p.text(title);
    // Inner fill
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    for i in 1..h - 1 {
        p.goto(x + 1, y + i);
        p.text(&" ".repeat((w - 2) as usize));
    }
    let inner_w = (w - 4) as usize;
    // Labels from mc(1) Internal File Viewer (wrap/hex) and PARITY display options
    let checks: [(F, &str, bool); 4] = [
        (
            F::ShowLineNumbers,
            "Show line numbers",
            dlg.show_line_numbers,
        ),
        (F::ShowCr, "Show CR as ^M", dlg.show_cr),
        (F::WrapMode, "Wrap mode", dlg.wrap),
        (F::HexMode, "Hex mode", dlg.hex),
    ];
    for (i, (focus, label, on)) in checks.iter().enumerate() {
        if dlg.focus == *focus {
            p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
        } else {
            p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
        }
        p.goto(x + 2, y + 1 + i as u16);
        p.text(&truncate(
            &format!("[{}] {}", if *on { 'x' } else { ' ' }, label),
            inner_w,
        ));
    }
    // Buttons: focused `< OK >`, unfocused `[ Cancel ]` (GNU mc / History / Search / Save as)
    let focus = dlg.focus;
    let sel_btn = |want, txt: &str| {
        if focus == want {
            format!("< {txt} >")
        } else {
            format!("[ {txt} ]")
        }
    };
    p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
    let btns = format!("{}  {}", sel_btn(F::Ok, "OK"), sel_btn(F::Cancel, "Cancel"));
    let bx = x + (w.saturating_sub(btns.len() as u16)) / 2;
    p.goto(bx, y + h - 2);
    p.text(&btns);
    if show_shadow {
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
}

fn draw_editor_replace_dialog(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    dlg: &rmc_core::app::EditorReplaceDialog,
    show_shadow: bool,
) {
    use rmc_core::app::EditorReplaceFocus as F;
    let w = (cols as usize).min(66) as u16;
    let h = 12u16;
    if cols < w || rows < h {
        return;
    }
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
    // Title — GNU mcedit wording
    p.set_fg_bg(pal.dtitle_fg, pal.dtitle_bg);
    let title = " Replace ";
    let tx = x + (w.saturating_sub(title.len() as u16)) / 2;
    p.goto(tx, y);
    p.text(title);
    // Inner fill
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    for i in 1..h - 1 {
        p.goto(x + 1, y + i);
        p.text(&" ".repeat((w - 2) as usize));
    }
    let inner_w = (w - 4) as usize;
    // Search field
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(x + 2, y + 1);
    p.text(&truncate("Enter search string:", inner_w));
    if matches!(dlg.focus, F::Search) {
        p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
    } else {
        p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    }
    p.goto(x + 2, y + 2);
    let st = truncate(&dlg.search, inner_w);
    p.text(&format!(
        "{st}{}",
        " ".repeat(inner_w.saturating_sub(st.len()))
    ));
    // Replacement field
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(x + 2, y + 3);
    p.text(&truncate("Enter replacement string:", inner_w));
    if matches!(dlg.focus, F::Replacement) {
        p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
    } else {
        p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    }
    p.goto(x + 2, y + 4);
    let rt = truncate(&dlg.replacement, inner_w);
    p.text(&format!(
        "{rt}{}",
        " ".repeat(inner_w.saturating_sub(rt.len()))
    ));
    let checks: [(F, &str, bool); 4] = [
        (F::CaseSensitive, "Case sensitive", dlg.case_sensitive),
        (F::Backwards, "Backwards", dlg.backwards),
        (F::WholeWords, "Whole words", dlg.whole_words),
        (
            F::RegularExpression,
            "Regular expression",
            dlg.regular_expression,
        ),
    ];
    for (i, (focus, label, on)) in checks.iter().enumerate() {
        if dlg.focus == *focus {
            p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
        } else {
            p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
        }
        p.goto(x + 2, y + 5 + i as u16);
        p.text(&truncate(
            &format!("[{}] {}", if *on { 'x' } else { ' ' }, label),
            inner_w,
        ));
    }
    // Buttons: focused `< Replace >`, unfocused `[ All ]` (GNU mc / Search / History)
    let focus = dlg.focus;
    let sel_btn = |want, txt: &str| {
        if focus == want {
            format!("< {txt} >")
        } else {
            format!("[ {txt} ]")
        }
    };
    p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
    let btns = format!(
        "{}  {}  {}  {}",
        sel_btn(F::Replace, "Replace"),
        sel_btn(F::All, "All"),
        sel_btn(F::Skip, "Skip"),
        sel_btn(F::Cancel, "Cancel")
    );
    let bx = x + (w.saturating_sub(btns.len() as u16)) / 2;
    p.goto(bx, y + h - 2);
    p.text(&btns);
    if show_shadow {
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
}

fn draw_editor_pipe_dialog(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    dlg: &rmc_core::app::EditorPipeDialog,
    show_shadow: bool,
) {
    use rmc_core::app::EditorPipeFocus as F;
    let w = (cols as usize).min(66) as u16;
    let h = 8u16;
    if cols < w || rows < h {
        return;
    }
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
    // Title — GNU mcedit wording
    p.set_fg_bg(pal.dtitle_fg, pal.dtitle_bg);
    let title = " Pipe ";
    let tx = x + (w.saturating_sub(title.len() as u16)) / 2;
    p.goto(tx, y);
    p.text(title);
    // Inner fill
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    for i in 1..h - 1 {
        p.goto(x + 1, y + i);
        p.text(&" ".repeat((w - 2) as usize));
    }
    let inner_w = (w - 4) as usize;
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(x + 2, y + 1);
    p.text(&truncate("Enter pipe command:", inner_w));
    if matches!(dlg.focus, F::Command) {
        p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
    } else {
        p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    }
    p.goto(x + 2, y + 2);
    let ct = truncate(&dlg.command, inner_w);
    p.text(&format!(
        "{ct}{}",
        " ".repeat(inner_w.saturating_sub(ct.len()))
    ));
    // Buttons: focused `< OK >`, unfocused `[ Cancel ]` (GNU mc / History / Replace)
    let focus = dlg.focus;
    let sel_btn = |want, txt: &str| {
        if focus == want {
            format!("< {txt} >")
        } else {
            format!("[ {txt} ]")
        }
    };
    p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
    let btns = format!("{}  {}", sel_btn(F::Ok, "OK"), sel_btn(F::Cancel, "Cancel"));
    let bx = x + (w.saturating_sub(btns.len() as u16)) / 2;
    p.goto(bx, y + h - 2);
    p.text(&btns);
    if show_shadow {
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
}

fn draw_editor_goto_dialog(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    dlg: &rmc_core::app::EditorGotoDialog,
    show_shadow: bool,
) {
    use rmc_core::app::EditorGotoFocus as F;
    let w = (cols as usize).min(66) as u16;
    let h = 8u16;
    if cols < w || rows < h {
        return;
    }
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
    // Title — GNU mcedit wording
    p.set_fg_bg(pal.dtitle_fg, pal.dtitle_bg);
    let title = " Goto line ";
    let tx = x + (w.saturating_sub(title.len() as u16)) / 2;
    p.goto(tx, y);
    p.text(title);
    // Inner fill
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    for i in 1..h - 1 {
        p.goto(x + 1, y + i);
        p.text(&" ".repeat((w - 2) as usize));
    }
    let inner_w = (w - 4) as usize;
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(x + 2, y + 1);
    p.text(&truncate("Enter line:", inner_w));
    if matches!(dlg.focus, F::Line) {
        p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
    } else {
        p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    }
    p.goto(x + 2, y + 2);
    let lt = truncate(&dlg.line, inner_w);
    p.text(&format!(
        "{lt}{}",
        " ".repeat(inner_w.saturating_sub(lt.len()))
    ));
    // Buttons: focused `< OK >`, unfocused `[ Cancel ]` (GNU mc / History / Replace / Pipe)
    let focus = dlg.focus;
    let sel_btn = |want, txt: &str| {
        if focus == want {
            format!("< {txt} >")
        } else {
            format!("[ {txt} ]")
        }
    };
    p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
    let btns = format!("{}  {}", sel_btn(F::Ok, "OK"), sel_btn(F::Cancel, "Cancel"));
    let bx = x + (w.saturating_sub(btns.len() as u16)) / 2;
    p.goto(bx, y + h - 2);
    p.text(&btns);
    if show_shadow {
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
}

fn draw_editor_tab_spacing_dialog(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    dlg: &rmc_core::app::EditorTabSpacingDialog,
    show_shadow: bool,
) {
    use rmc_core::app::EditorTabSpacingFocus as F;
    let w = (cols as usize).min(66) as u16;
    let h = 8u16;
    if cols < w || rows < h {
        return;
    }
    let x = (cols - w) / 2;
    let y = (rows - h) / 2;
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
    // Title — mcedit(1) `editor_tab_spacing`
    p.set_fg_bg(pal.dtitle_fg, pal.dtitle_bg);
    let title = " Tab spacing ";
    let tx = x + (w.saturating_sub(title.len() as u16)) / 2;
    p.goto(tx, y);
    p.text(title);
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    for i in 1..h - 1 {
        p.goto(x + 1, y + i);
        p.text(&" ".repeat((w - 2) as usize));
    }
    let inner_w = (w - 4) as usize;
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(x + 2, y + 1);
    p.text(&truncate("Enter tab spacing:", inner_w));
    if matches!(dlg.focus, F::Width) {
        p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
    } else {
        p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    }
    p.goto(x + 2, y + 2);
    let lt = truncate(&dlg.width, inner_w);
    p.text(&format!(
        "{lt}{}",
        " ".repeat(inner_w.saturating_sub(lt.len()))
    ));
    let focus = dlg.focus;
    let sel_btn = |want, txt: &str| {
        if focus == want {
            format!("< {txt} >")
        } else {
            format!("[ {txt} ]")
        }
    };
    p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
    let btns = format!("{}  {}", sel_btn(F::Ok, "OK"), sel_btn(F::Cancel, "Cancel"));
    let bx = x + (w.saturating_sub(btns.len() as u16)) / 2;
    p.goto(bx, y + h - 2);
    p.text(&btns);
    if show_shadow {
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
}

fn draw_editor_save_as_dialog(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    dlg: &rmc_core::app::EditorSaveAsDialog,
    show_shadow: bool,
) {
    if let Some(c) = &dlg.overwrite {
        draw_dialog_ync(
            p,
            cols,
            rows,
            pal,
            &c.title,
            &c.message,
            c.focus,
            show_shadow,
        );
        return;
    }
    use rmc_core::app::EditorSaveAsFocus as F;
    let w = (cols as usize).min(66) as u16;
    let h = 8u16;
    if cols < w || rows < h {
        return;
    }
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
    // Title — GNU mcedit File menu Save as
    p.set_fg_bg(pal.dtitle_fg, pal.dtitle_bg);
    let title = " Save as ";
    let tx = x + (w.saturating_sub(title.len() as u16)) / 2;
    p.goto(tx, y);
    p.text(title);
    // Inner fill
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    for i in 1..h - 1 {
        p.goto(x + 1, y + i);
        p.text(&" ".repeat((w - 2) as usize));
    }
    let inner_w = (w - 4) as usize;
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(x + 2, y + 1);
    p.text(&truncate("Enter file name:", inner_w));
    if matches!(dlg.focus, F::Filename) {
        p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
    } else {
        p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    }
    p.goto(x + 2, y + 2);
    let ft = truncate(&dlg.filename, inner_w);
    p.text(&format!(
        "{ft}{}",
        " ".repeat(inner_w.saturating_sub(ft.len()))
    ));
    // Buttons: focused `< OK >`, unfocused `[ Cancel ]` (GNU mc / History / Search / Goto)
    let focus = dlg.focus;
    let sel_btn = |want, txt: &str| {
        if focus == want {
            format!("< {txt} >")
        } else {
            format!("[ {txt} ]")
        }
    };
    p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
    let btns = format!("{}  {}", sel_btn(F::Ok, "OK"), sel_btn(F::Cancel, "Cancel"));
    let bx = x + (w.saturating_sub(btns.len() as u16)) / 2;
    p.goto(bx, y + h - 2);
    p.text(&btns);
    if show_shadow {
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
}

#[allow(clippy::too_many_arguments)]
fn draw_dialog_ync(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    title: &str,
    message: &str,
    focus: rmc_core::app::YncFocus,
    show_shadow: bool,
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
    if show_shadow {
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
}
#[allow(clippy::too_many_arguments)]
fn draw_dialog_box(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    title: &str,
    message: &str,
    buttons: &[&str],
    show_shadow: bool,
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
    if show_shadow {
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
    show_shadow: bool,
) {
    let _ = side; // implied by Left/Right menu; title remains generic
    let title = "Sort order";
    let w = 54u16.min(cols.saturating_sub(2));
    let h = 13u16.min(rows.saturating_sub(2)).max(8);
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
        ("Access time", rmc_core::panel::SortBy::Atime),
        ("Change time", rmc_core::panel::SortBy::Ctime),
        ("Size", rmc_core::panel::SortBy::Size),
        ("Inode", rmc_core::panel::SortBy::Inode),
        ("Unsorted", rmc_core::panel::SortBy::Unsorted),
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
    // Checkboxes on the right of the first two radio rows (existing dirs_first wiring)
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
    // Buttons: focused `< txt >`, unfocused `[ txt ]` (History/Replace)
    p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
    let ok_focus = focus_index == radios.len() + 2;
    let cancel_focus = focus_index == radios.len() + 3;
    let ok_txt = if ok_focus { "< OK >" } else { "[ OK ]" };
    let cancel_txt = if cancel_focus {
        "< Cancel >"
    } else {
        "[ Cancel ]"
    };
    let btns = format!("{ok_txt}  {cancel_txt}");
    let bx = x + (w.saturating_sub(btns.len() as u16)) / 2;
    p.goto(bx, y + h - 2);
    p.text(&btns);
    // Shadow
    if show_shadow {
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
    format_nroff: bool,
    parsed: bool,
    sel_anchor: Option<u64>,
    sel_cursor: u64,
    viewer_menu: Option<rmc_core::app::ViewerMenu>,
    search_dialog: Option<&rmc_core::app::ViewerSearchDialog>,
    display_dialog: Option<&rmc_core::app::ViewerDisplayDialog>,
    status_msg: Option<&str>,
    goto_prompt: &Option<String>,
    show_shadow: bool,
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
        rmc_view::ViewOptions {
            hex,
            wrap,
            show_cr,
            format: format_nroff,
        },
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
    let sel_range = sel_anchor.map(|a| {
        if a <= sel_cursor {
            (a, sel_cursor)
        } else {
            (sel_cursor, a)
        }
    });
    for (i, line) in rr.lines.into_iter().enumerate() {
        let row_y = 1 + i as u16;
        p.goto(1, row_y);
        if ln_enabled {
            // Draw gray-ish line number gutter
            p.set_fg_bg(pal.frame_fg, pal.core_default_bg);
            let label = format!("{:>6} ", start_ln + i as u64);
            p.text(&label);
            p.goto(1 + ln_gutter, row_y);
        }
        let t = truncate(&line, content_cols as usize);
        let line_sel = sel_range
            .zip(rr.line_byte_ranges.get(i).copied())
            .is_some_and(|((sel_lo, sel_hi), (line_lo, line_hi))| {
                sel_lo < sel_hi && sel_lo < line_hi && sel_hi > line_lo
            });
        let (fg, bg) = viewer_line_style(line_sel, pal);
        p.set_fg_bg(fg, bg);
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
    if let Some(msg) = status_msg {
        status.push_str("  ");
        status.push_str(msg);
    }
    let st = truncate(&status, cols as usize);
    p.text(&st);
    // Viewer F-bar (GNU mcview 10-key: Help / Wrap / Quit / Hex / Goto / … / Quit)
    draw_viewer_fbar(
        p,
        rows.saturating_sub(1),
        cols,
        pal,
        wrap,
        hex,
        parsed,
        format_nroff,
    );
    // GNU mcview F7 Search dialog (same chrome as mcedit Search)
    if let Some(dlg) = search_dialog {
        draw_editor_search_dialog(p, cols, rows, pal, dlg, show_shadow);
    }
    // mcview display-options dialog (Options → Display options; same chrome as Search)
    if let Some(dlg) = display_dialog {
        draw_viewer_display_dialog(p, cols, rows, pal, dlg, show_shadow);
    }
    if let Some(menu) = viewer_menu {
        draw_viewer_menu(p, cols, pal, menu);
    }
    // Goto prompt overlay (MC-style input dialog)
    if let Some(current) = goto_prompt {
        draw_dialog_box(
            p,
            cols,
            rows,
            pal,
            "Goto",
            current,
            &["< OK >", "Cancel"],
            show_shadow,
        );
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
                // Selected file on the other (listing) panel — not this panel's
                // leftover cursor, and not a full UiMode::Viewer (F3) screen.
                if let Some(ent) = preview_source_entry(app, is_left) {
                    if let Some(msg) = quick_view_directory_line(ent) {
                        p.goto(x + 1, content_top);
                        p.text(&truncate(&msg, (w - 2) as usize));
                    } else {
                        let offset = if panel.preview_path.as_ref() == Some(&ent.path) {
                            panel.preview_offset
                        } else {
                            0
                        };
                        let rr = rmc_view::render_window(
                            &ent.path,
                            rmc_view::ViewOptions {
                                hex: false,
                                wrap: true,
                                show_cr: false,
                                format: false,
                            },
                            offset,
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
                    }
                }
            }
            PanelMode::Info => {
                let lines = info_lines_for_panel(app, is_left);
                for (i, line) in lines.iter().enumerate() {
                    if (i as u16) >= content_h {
                        break;
                    }
                    p.goto(x + 1, content_top + i as u16);
                    p.text(&truncate(line, (w - 2) as usize));
                }
            }
            PanelMode::Tree => {
                if let Some(tree) = &panel.tree {
                    let is_active_panel = (is_left
                        && matches!(app.active, rmc_core::actions::PaneSide::Left))
                        || (!is_left && matches!(app.active, rmc_core::actions::PaneSide::Right));
                    let status = rmc_core::panel::tree_panel_mini_status(
                        tree,
                        app.panel_opts.show_mini_status,
                        is_active_panel,
                    );
                    let list_h = if status.is_some() {
                        content_h.saturating_sub(1)
                    } else {
                        content_h
                    };
                    let figure = &tree.figure;
                    for i in 0..list_h as usize {
                        let idx = figure.scroll_top + i;
                        if let Some(ent) = figure.entries.get(idx) {
                            let row_y = content_top + i as u16;
                            let is_cursor = idx == figure.selected_index;
                            let (fg, bg) = if is_cursor && is_active_panel {
                                (pal.selected_fg, pal.selected_bg)
                            } else {
                                (pal.core_default_fg, pal.core_default_bg)
                            };
                            p.set_fg_bg(fg, bg);
                            p.goto(x + 1, row_y);
                            let name = ent.path.file_name().and_then(|s| s.to_str()).unwrap_or("/");
                            let indent = "  ".repeat(ent.depth);
                            let display = if ent.path.as_os_str() == "/" {
                                "/".to_string()
                            } else {
                                format!("{indent}{name}/")
                            };
                            p.text(&truncate(&display, (w - 2) as usize));
                        }
                    }
                    if let Some(text) = status {
                        let status_y = y + h - 2;
                        p.set_fg_bg(pal.statusbar_fg, pal.statusbar_bg);
                        p.goto(x + 1, status_y);
                        let s = truncate(&text, (w - 2) as usize);
                        p.text(&s);
                        let inner = (w - 2) as usize;
                        let used = s.chars().count();
                        if used < inner {
                            p.text(&" ".repeat(inner - used));
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
    let user_tokens = if matches!(panel.listing, rmc_core::panel::ListingFormat::User) {
        rmc_core::panel::parse_user_listing_format(&panel.user_format)
    } else {
        Vec::new()
    };
    // Empty / unrecognized user_format falls back to Long layout.
    let listing = if matches!(panel.listing, rmc_core::panel::ListingFormat::User)
        && user_tokens.is_empty()
    {
        rmc_core::panel::ListingFormat::Long
    } else {
        panel.listing
    };
    match listing {
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
        rmc_core::panel::ListingFormat::User => {
            p.goto(x + 1, y + 1);
            let header =
                rmc_core::panel::format_user_listing_header(&user_tokens, (w - 2) as usize);
            p.text(&truncate(&header, (w - 2) as usize));
        }
        rmc_core::panel::ListingFormat::Long => {
            // Column-aligned like ls -l: perm, nlink, owner, group, size, mtime
            let perms_col = x + 1;
            let nlink_col = perms_col + 11; // 10 perms + 1 space
            let owner_col = nlink_col + 5; // nlink 4 + 1 space
            let group_col = owner_col + 9; // owner 8 + 1 space
            let size_col = group_col + 9; // group 8 + 1 space
            let time_col = size_col + 9; // size 8 + 1 space
            p.goto(perms_col, y + 1);
            p.text("Perms");
            p.goto(nlink_col, y + 1);
            p.text("Nl");
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
    // Mini-status occupies the row above the bottom frame. When it is off, listing
    // uses that row so the frame stays closed (no empty gap). Quick search still
    // borrows the same row on the active panel.
    let is_active_panel = (is_left && matches!(app.active, rmc_core::actions::PaneSide::Left))
        || (!is_left && matches!(app.active, rmc_core::actions::PaneSide::Right));
    let reserve_status = rmc_core::panel::reserve_panel_mini_status(
        app.panel_opts.show_mini_status,
        is_active_panel,
        app.quick_search.is_some(),
    );
    let content_h = rmc_core::panel::panel_listing_content_rows(h, reserve_status);
    let _panel = if is_left { &app.left } else { &app.right };
    // Viewport uses panel.scroll_top, updated by the event loop per visible capacity
    let panel = if is_left { &app.left } else { &app.right };
    match listing {
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
                    // Name (filehighlight fg when not selected/marked)
                    let display_name = format_entry_name(ent);
                    p.goto(x + 1, row_y);
                    let name_width = (w - 2).saturating_sub(26);
                    let name_trunc = truncate(&display_name, name_width as usize);
                    let name_fg =
                        listing_name_color(ent, &pal, is_cursor, is_active_panel, selected);
                    p.set_fg_bg(name_fg, bg);
                    p.text(&name_trunc);
                    // Size / time stay row colors (not filehighlight)
                    p.set_fg_bg(fg, bg);
                    p.goto(size_col, row_y);
                    p.text(&format_size(ent, app.panel_opts.kilobyte_si));
                    // Time
                    p.goto(x + w - 15, row_y);
                    p.text(&format_time(ent));
                }
            }
        }
        rmc_core::panel::ListingFormat::Brief => {
            // Pack names into 1–9 columns (GNU Brief; default 2).
            let cols_n = rmc_core::panel::clamp_brief_columns(panel.brief_columns);
            let per_col_width = rmc_core::panel::brief_column_width(w, cols_n);
            for i in 0..content_h as usize {
                let row_y = content_top + i as u16;
                p.set_fg_bg(pal.core_default_fg, pal.core_default_bg);
                p.goto(x + 1, row_y);
                p.text(&" ".repeat((w - 2) as usize));
                for col in 0..cols_n as usize {
                    let idx = rmc_core::panel::brief_entry_index(
                        panel.scroll_top,
                        i,
                        col,
                        content_h as usize,
                    );
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
                        let col_x = x + 1 + col as u16 * (per_col_width + 1);
                        p.goto(col_x, row_y);
                        let display_name = format_entry_name(ent);
                        let name_trunc = truncate(&display_name, per_col_width as usize);
                        let name_fg =
                            listing_name_color(ent, &pal, is_cursor, is_active_panel, selected);
                        p.set_fg_bg(name_fg, bg);
                        p.text(&name_trunc);
                        p.set_fg_bg(fg, bg);
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
                    let prefix = rmc_core::panel::format_long_listing_prefix(
                        ent,
                        app.panel_opts.kilobyte_si,
                    );
                    let width = (w - 2) as usize;
                    p.goto(x + 1, row_y);
                    let prefix_len = prefix.chars().count();
                    if prefix_len >= width {
                        p.text(&truncate(&prefix, width));
                    } else {
                        p.text(&prefix);
                        let name_trunc = truncate(&ent.name, width - prefix_len);
                        let name_fg =
                            listing_name_color(ent, &pal, is_cursor, is_active_panel, selected);
                        p.set_fg_bg(name_fg, bg);
                        p.text(&name_trunc);
                        p.set_fg_bg(fg, bg);
                    }
                }
            }
        }
        rmc_core::panel::ListingFormat::User => {
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
                    let mut line = rmc_core::panel::format_user_listing_line(
                        ent,
                        &user_tokens,
                        (w - 2) as usize,
                        app.panel_opts.kilobyte_si,
                    );
                    line = truncate(&line, (w - 2) as usize);
                    p.goto(x + 1, row_y);
                    let name_fg =
                        listing_name_color(ent, &pal, is_cursor, is_active_panel, selected);
                    paint_line_with_name_color(p, &line, &ent.name, name_fg, fg, bg);
                }
            }
        }
    }
    // Mini status
    if let Some(text) = rmc_core::panel::panel_mini_status_line(
        app.panel_opts.show_mini_status,
        is_active_panel,
        app.quick_search.as_ref().map(|qs| qs.pattern.as_str()),
        panel.current_entry(),
        app.panel_opts.kilobyte_si,
    ) {
        let status_y = y + h - 2;
        p.set_fg_bg(pal.statusbar_fg, pal.statusbar_bg);
        p.goto(x + 1, status_y);
        let s = truncate(&text, (w - 2) as usize);
        p.text(&s);
        let inner = (w - 2) as usize;
        let used = s.chars().count();
        if used < inner {
            p.text(&" ".repeat(inner - used));
        }
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
/// GNU mc(1) default panel F-bar labels (F1…F10). F2 is **Menu**, not “User menu”.
pub(crate) fn panel_fbar_labels() -> [&'static str; 10] {
    [
        "Help", "Menu", "View", "Edit", "Copy", "RenMov", "Mkdir", "Delete", "PullDn", "Quit",
    ]
}

fn draw_fbar(p: &mut Painter, y: u16, cols: u16, pal: McPalette) {
    let labels = panel_fbar_labels();
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
        rows.saturating_sub(3),
        '│',
        pal.frame_fg,
        pal.dialog_default_bg,
    );
    p.vline(
        cols - 1,
        1,
        rows.saturating_sub(3),
        '│',
        pal.frame_fg,
        pal.dialog_default_bg,
    );
    let frame_bottom = rows.saturating_sub(2);
    p.goto(0, frame_bottom);
    p.text("└");
    p.hline(
        1,
        frame_bottom,
        cols.saturating_sub(2),
        '─',
        pal.frame_fg,
        pal.dialog_default_bg,
    );
    p.goto(cols - 1, frame_bottom);
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
    // Content area inside the frame; last row is the GNU-ish help F-bar.
    let content_top = 1u16;
    let content_h = rows.saturating_sub(3);
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
    // Bottom F-bar (GNU help: Help / Index / Prev / Next / Quit — not the panel bar)
    draw_help_fbar(p, rows.saturating_sub(1), cols, pal);
}

/// GNU mc help viewer F-bar. Focused labels (not panel Help/Menu/View/Edit).
pub(crate) fn help_fbar_labels() -> [&'static str; 10] {
    [
        "Help",  // F1 — this key list
        "Index", // F2 — Contents
        "Prev",  // F3 — history back
        "Next",  // F4 — follow selected link
        "", "", "", "", "", "Quit",
    ]
}

fn draw_help_fbar(p: &mut Painter, y: u16, cols: u16, pal: McPalette) {
    let labels = help_fbar_labels();
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

#[allow(clippy::too_many_arguments)]
fn draw_viewer_fbar(
    p: &mut Painter,
    y: u16,
    cols: u16,
    pal: McPalette,
    wrap: bool,
    hex: bool,
    parsed: bool,
    format: bool,
) {
    let labels = viewer_fbar_labels(wrap, hex, parsed, format);
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

/// GNU mcview 10-key bar. Button text is the mode you *enter* (mc(1): F8 Raw/Parsed,
/// F9 format/unformat). Public labels: Help, Wrap/UnWrap, Quit, Hex/Ascii, Goto,
/// Search, Parse/Raw, Format/Unform, Quit.
pub(crate) fn viewer_fbar_labels(
    wrap: bool,
    hex: bool,
    parsed: bool,
    format: bool,
) -> [&'static str; 10] {
    [
        "Help",
        if wrap { "UnWrap" } else { "Wrap" },
        "Quit",
        if hex { "Ascii" } else { "Hex" },
        "Goto",
        "",
        "Search",
        if parsed { "Raw" } else { "Parse" },
        if format { "Unform" } else { "Format" },
        "Quit",
    ]
}

/// Viewer selection uses `[viewer] selected` (yellow;cyan), not panel `selected`
/// (black;cyan).
pub(crate) fn viewer_line_style(selected: bool, pal: McPalette) -> (Color, Color) {
    if selected {
        (pal.viewer_selected_fg, pal.viewer_selected_bg)
    } else {
        (pal.core_default_fg, pal.core_default_bg)
    }
}

/// Hit-test GNU viewer menu titles on the top line: File / Command / Options.
pub(crate) fn viewer_menu_from_x(x: u16) -> rmc_core::app::ViewerMenu {
    // Packed: " File  Command  Options "
    if x < 6 {
        rmc_core::app::ViewerMenu::File { selected: 0 }
    } else if x < 16 {
        rmc_core::app::ViewerMenu::Command { selected: 0 }
    } else {
        rmc_core::app::ViewerMenu::Options { selected: 0 }
    }
}

fn draw_viewer_menu(p: &mut Painter, cols: u16, pal: McPalette, menu: rmc_core::app::ViewerMenu) {
    p.set_fg_bg(pal.menu_fg, pal.menu_bg);
    p.goto(0, 0);
    let titles = [" File ", " Command ", " Options "];
    let mut x = 0u16;
    for (i, t) in titles.iter().enumerate() {
        let active = matches!(
            (i, menu),
            (0, rmc_core::app::ViewerMenu::File { .. })
                | (1, rmc_core::app::ViewerMenu::Command { .. })
                | (2, rmc_core::app::ViewerMenu::Options { .. })
        );
        if active {
            p.set_fg_bg(pal.menusel_fg, pal.menusel_bg);
        } else {
            p.set_fg_bg(pal.menu_fg, pal.menu_bg);
        }
        p.goto(x, 0);
        p.text(t);
        x += t.len() as u16;
    }
    if x < cols {
        p.set_fg_bg(pal.menu_fg, pal.menu_bg);
        p.goto(x, 0);
        p.text(&" ".repeat(cols.saturating_sub(x) as usize));
    }
    let items = menu.items();
    let drop_x = match menu {
        rmc_core::app::ViewerMenu::File { .. } => 0,
        rmc_core::app::ViewerMenu::Command { .. } => 6,
        rmc_core::app::ViewerMenu::Options { .. } => 16,
    };
    let w = (items.iter().map(|s| s.len()).max().unwrap_or(4) + 4) as u16;
    let h = items.len() as u16 + 2;
    p.set_fg_bg(pal.menu_fg, pal.menu_bg);
    p.goto(drop_x, 1);
    p.text("┌");
    p.hline(
        drop_x + 1,
        1,
        w.saturating_sub(2),
        '─',
        pal.menu_fg,
        pal.menu_bg,
    );
    p.goto(drop_x + w.saturating_sub(1), 1);
    p.text("┐");
    for (i, item) in items.iter().enumerate() {
        let y = 2 + i as u16;
        if i == menu.selected() {
            p.set_fg_bg(pal.menusel_fg, pal.menusel_bg);
        } else {
            p.set_fg_bg(pal.menu_fg, pal.menu_bg);
        }
        p.goto(drop_x, y);
        p.text("│");
        p.text(&format!(" {item} "));
        let pad = w.saturating_sub(3 + item.len() as u16);
        if pad > 0 {
            p.text(&" ".repeat(pad as usize));
        }
        p.goto(drop_x + w.saturating_sub(1), y);
        p.text("│");
    }
    p.set_fg_bg(pal.menu_fg, pal.menu_bg);
    p.goto(drop_x, 1 + h - 1);
    p.text("└");
    p.hline(
        drop_x + 1,
        1 + h - 1,
        w.saturating_sub(2),
        '─',
        pal.menu_fg,
        pal.menu_bg,
    );
    p.goto(drop_x + w.saturating_sub(1), 1 + h - 1);
    p.text("┘");
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
    show_shadow: bool,
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
        draw_dialog_box(
            p,
            cols,
            rows,
            pal,
            "Search",
            current,
            &["< OK >", "Cancel"],
            show_shadow,
        );
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
            show_shadow,
        );
    }
    if let Some(c) = &state.confirm_exit {
        draw_dialog_ync(
            p,
            cols,
            rows,
            pal,
            &c.title,
            &c.message,
            c.focus,
            show_shadow,
        );
    }
    Ok(())
}

fn draw_diff_fbar(p: &mut Painter, y: u16, cols: u16, pal: McPalette) {
    // F1 Help, F2 Save, F4 Edit left, F5 Merge, F7 Search, F10 Quit.
    // F14 (Shift-F4) edits the right file (GNU mc(1) Internal Diff Viewer).
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
/// Paint a listing line, coloring only the filename span, then restore row fg/bg.
fn paint_line_with_name_color(
    p: &mut Painter,
    line: &str,
    name: &str,
    name_fg: Color,
    row_fg: Color,
    row_bg: Color,
) {
    if name_fg == row_fg {
        p.set_fg_bg(row_fg, row_bg);
        p.text(line);
        return;
    }
    if let Some((start, end)) = name_span_in_line(line, name) {
        p.set_fg_bg(row_fg, row_bg);
        p.text(&line[..start]);
        p.set_fg_bg(name_fg, row_bg);
        p.text(&line[start..end]);
        p.set_fg_bg(row_fg, row_bg);
        p.text(&line[end..]);
    } else {
        p.set_fg_bg(row_fg, row_bg);
        p.text(line);
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

fn format_size(ent: &FileEntry, si: bool) -> String {
    if ent.name == ".." {
        "UP--DIR".to_string()
    } else if ent.is_dir {
        "        ".to_string()
    } else {
        format!("{:>8}", rmc_core::panel::format_byte_size(ent.size, si))
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
    show_shadow: bool,
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
    if show_shadow {
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
    show_shadow: bool,
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
    if show_shadow {
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
}

#[allow(clippy::too_many_arguments)]
fn draw_ftp_connect_dialog(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    scheme: &str,
    host: &str,
    port: &str,
    user: &str,
    password: &str,
    directory: &str,
    anonymous: bool,
    focus_index: usize,
    focus_ok: bool,
    show_shadow: bool,
) {
    let w = (cols as usize).min(66) as u16;
    let h = 12u16;
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
    let ttl = if scheme == "sftp" {
        " SFTP to machine "
    } else {
        " FTP to machine "
    };
    let tx = x + (w.saturating_sub(ttl.len() as u16)) / 2;
    p.goto(tx, y);
    p.text(ttl);
    // Labels and fields
    let lab_x = x + 2;
    let fld_x = x + 14;
    // Host
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(lab_x, y + 2);
    p.text("Host name:");
    if focus_index == 0 {
        p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
    } else {
        p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    }
    p.goto(fld_x, y + 2);
    let hv = truncate(host, (w - (fld_x - x) - 2) as usize);
    p.text(&format!(
        "{hv}{}",
        " ".repeat((w - (fld_x - x) - 2) as usize - hv.len())
    ));
    // Port
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(lab_x, y + 3);
    p.text("Port:");
    if focus_index == 1 {
        p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
    } else {
        p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    }
    p.goto(fld_x, y + 3);
    let pv = truncate(port, (w - (fld_x - x) - 2) as usize);
    p.text(&format!(
        "{pv}{}",
        " ".repeat((w - (fld_x - x) - 2) as usize - pv.len())
    ));
    // User
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(lab_x, y + 4);
    p.text("User name:");
    if focus_index == 2 {
        p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
    } else {
        p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    }
    p.goto(fld_x, y + 4);
    let uv = truncate(user, (w - (fld_x - x) - 2) as usize);
    p.text(&format!(
        "{uv}{}",
        " ".repeat((w - (fld_x - x) - 2) as usize - uv.len())
    ));
    // Password (draw as asterisks)
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(lab_x, y + 5);
    p.text("Password:");
    if focus_index == 3 {
        p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
    } else {
        p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    }
    p.goto(fld_x, y + 5);
    let stars = "*".repeat(password.chars().count());
    let pwv = truncate(&stars, (w - (fld_x - x) - 2) as usize);
    p.text(&format!(
        "{pwv}{}",
        " ".repeat((w - (fld_x - x) - 2) as usize - pwv.len())
    ));
    // Directory
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(lab_x, y + 6);
    p.text("Directory:");
    if focus_index == 4 {
        p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
    } else {
        p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    }
    p.goto(fld_x, y + 6);
    let dv = truncate(directory, (w - (fld_x - x) - 2) as usize);
    p.text(&format!(
        "{dv}{}",
        " ".repeat((w - (fld_x - x) - 2) as usize - dv.len())
    ));
    // Anonymous checkbox
    if focus_index == 5 {
        p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
    } else {
        p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    }
    p.goto(x + 2, y + 8);
    p.text(&format!(
        "[{}] {}",
        if anonymous { 'x' } else { ' ' },
        "Anonymous"
    ));
    // Buttons
    p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
    let ok = if focus_ok { "< OK >" } else { "[ OK ]" };
    let cancel = if focus_ok { " Cancel " } else { "[ Cancel ]" };
    let btns = format!("{ok}  {cancel}");
    let bx = x + (w.saturating_sub(btns.len() as u16)) / 2;
    p.goto(bx, y + h - 2);
    p.text(&btns);
    // Shadow
    if show_shadow {
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
}

#[allow(clippy::too_many_arguments)]
fn draw_jobs_dialog(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    app: &App,
    selected: usize,
    focus: rmc_core::app::JobsDialogFocus,
    show_shadow: bool,
) {
    let jobs = app.jobs.snapshot();
    // Size: width up to 80, height based on number of jobs + chrome
    let w = (cols as usize).min(80) as u16;
    let min_h = 7u16;
    let list_h = (jobs.len() as u16).clamp(1, rows.saturating_sub(6));
    let h = (list_h + 4).max(min_h).min(rows.saturating_sub(2));
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
    let ttl = " Background jobs ";
    let tx = x + (w.saturating_sub(ttl.len() as u16)) / 2;
    p.goto(tx, y);
    p.text(ttl);
    // Column headers (light)
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    let header =
        "Kind  Source                     → Destination                          Progress  Status";
    p.goto(x + 2, y + 1);
    p.text(&truncate(header, (w - 4) as usize));
    // List rows
    let list_top = y + 2;
    for i in 0..(h.saturating_sub(4) as usize) {
        let row_y = list_top + i as u16;
        p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
        p.goto(x + 1, row_y);
        p.text(&" ".repeat((w - 2) as usize));
        if let Some(job) = jobs.get(i) {
            let is_sel = i == selected;
            if is_sel && matches!(focus, rmc_core::app::JobsDialogFocus::List) {
                p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
            } else {
                p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
            }
            p.goto(x + 2, row_y);
            // Kind
            let kind = match job.kind {
                rmc_core::jobs::JobKind::Copy => "Copy",
                rmc_core::jobs::JobKind::Move => "Move",
            };
            p.text(kind);
            // Source basename
            p.goto(x + 8, row_y);
            let src_base_owned: String = job
                .src
                .file_name()
                .and_then(|s| s.to_str().map(|s| s.to_string()))
                .unwrap_or_else(|| job.src.to_string_lossy().into_owned());
            let src_txt = truncate(&src_base_owned, 24);
            p.text(&src_txt);
            // Arrow and destination
            p.goto(x + 8 + 26, row_y);
            p.text("→ ");
            let dst_txt = truncate(&job.dst.display().to_string(), 30);
            p.text(&dst_txt);
            // Progress
            p.goto(x + w.saturating_sub(24), row_y);
            let pct_txt = if job.bytes_total > 0 {
                let pct = (job.bytes_done as f64 / job.bytes_total as f64 * 100.0).round() as u64;
                format!("{:>3}%", pct.min(100))
            } else {
                "  …%".to_string()
            };
            p.text(&pct_txt);
            // Status
            p.goto(x + w.saturating_sub(12), row_y);
            let status = match job.status {
                rmc_core::jobs::JobStatus::Queued => "Queued",
                rmc_core::jobs::JobStatus::Running => "Running",
                rmc_core::jobs::JobStatus::Done => "Done",
                rmc_core::jobs::JobStatus::Failed => "Failed",
                rmc_core::jobs::JobStatus::Cancelled => "Cancelled",
            };
            let st = truncate(status, 10);
            p.text(&st);
        }
    }
    // Buttons
    let sel_btn = |want: rmc_core::app::JobsDialogFocus, txt: &str| {
        if focus == want {
            format!("< {txt} >")
        } else {
            format!("[ {txt} ]")
        }
    };
    p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
    let btns = format!(
        "{}  {}  {}",
        sel_btn(rmc_core::app::JobsDialogFocus::Cancel, "Cancel"),
        sel_btn(rmc_core::app::JobsDialogFocus::Cleanup, "Clean up"),
        sel_btn(rmc_core::app::JobsDialogFocus::Ok, "OK")
    );
    let bx = x + (w.saturating_sub(btns.len() as u16)) / 2;
    p.goto(bx, y + h - 2);
    p.text(&btns);
    // Shadow
    if show_shadow {
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
}

#[allow(clippy::too_many_arguments)]
fn draw_screen_list_dialog(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    app: &App,
    selected: usize,
    scroll_top: usize,
    focus: rmc_core::app::ScreenListFocus,
    show_shadow: bool,
) {
    use rmc_core::app::ScreenListFocus as F;
    let entries = app.screen_list_labels();
    let w = (cols as usize).min(56) as u16;
    let min_h = 7u16;
    let list_h = (entries.len() as u16).clamp(1, rows.saturating_sub(6));
    let h = (list_h + 4).max(min_h).min(rows.saturating_sub(2));
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
    let ttl = " Screen list ";
    let tx = x + w.saturating_sub(ttl.len() as u16) / 2;
    p.goto(tx, y);
    p.text(ttl);
    let list_top = y + 1;
    let visible = h.saturating_sub(4) as usize;
    let mut start = scroll_top;
    if selected < start {
        start = selected;
    }
    if visible > 0 && selected >= start + visible {
        start = selected.saturating_add(1).saturating_sub(visible);
    }
    for i in 0..visible {
        let row_y = list_top + i as u16;
        p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
        p.goto(x + 1, row_y);
        p.text(&" ".repeat(w.saturating_sub(2) as usize));
        let idx = start + i;
        if let Some(line) = entries.get(idx) {
            if idx == selected && matches!(focus, F::List) {
                p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
            } else {
                p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
            }
            let t = truncate(line, w.saturating_sub(4) as usize);
            p.goto(x + 2, row_y);
            p.text(&t);
        }
    }
    let sel_btn = |want: F, txt: &str| {
        if focus == want {
            format!("< {txt} >")
        } else {
            format!("[ {txt} ]")
        }
    };
    p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
    let btns = format!("{}  {}", sel_btn(F::Ok, "OK"), sel_btn(F::Cancel, "Cancel"));
    let bx = x + w.saturating_sub(btns.len() as u16) / 2;
    p.goto(bx, y + h.saturating_sub(2));
    p.text(&btns);
    if show_shadow {
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
}

#[allow(clippy::too_many_arguments)]
fn draw_history_dialog(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    app: &App,
    selected: usize,
    scroll_top: usize,
    focus: rmc_core::app::HistoryDialogFocus,
    confirm_clean: bool,
    show_shadow: bool,
) {
    use rmc_core::app::HistoryDialogFocus as HF;
    let entries = app.subshell.history();
    let w = (cols as usize).min(64) as u16;
    let min_h = 7u16;
    let list_h = (entries.len() as u16).clamp(1, rows.saturating_sub(6));
    let h = (list_h + 4).max(min_h).min(rows.saturating_sub(2));
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
    let ttl = format!(" {} ", crate::terminal::HISTORY_DIALOG_TITLE);
    let tx = x + w.saturating_sub(ttl.len() as u16) / 2;
    p.goto(tx, y);
    p.text(&ttl);
    let list_top = y + 1;
    let visible = h.saturating_sub(4) as usize;
    let mut start = scroll_top;
    if selected < start {
        start = selected;
    }
    if visible > 0 && selected >= start + visible {
        start = selected.saturating_add(1).saturating_sub(visible);
    }
    for i in 0..visible {
        let row_y = list_top + i as u16;
        p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
        p.goto(x + 1, row_y);
        p.text(&" ".repeat(w.saturating_sub(2) as usize));
        let idx = start + i;
        if let Some(line) = entries.get(idx) {
            if idx == selected && matches!(focus, HF::List) {
                p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
            } else {
                p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
            }
            let t = truncate(line, w.saturating_sub(4) as usize);
            p.goto(x + 2, row_y);
            p.text(&t);
        }
    }
    let sel_btn = |want: HF, txt: &str| {
        if focus == want {
            format!("< {txt} >")
        } else {
            format!("[ {txt} ]")
        }
    };
    p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
    let btns = format!(
        "{}  {}  {}",
        sel_btn(HF::Ok, "OK"),
        sel_btn(HF::Cancel, "Cancel"),
        sel_btn(HF::Clear, "Clear")
    );
    let bx = x + w.saturating_sub(btns.len() as u16) / 2;
    p.goto(bx, y + h.saturating_sub(2));
    p.text(&btns);
    if show_shadow {
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
    if confirm_clean {
        draw_dialog_box(
            p,
            cols,
            rows,
            pal,
            crate::terminal::HISTORY_CLEAN_TITLE,
            crate::terminal::HISTORY_CLEAN_MESSAGE,
            &["< Yes >", "No"],
            show_shadow,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_completion_list(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    items: &[rmc_core::complete::CompletionItem],
    selected: usize,
    scroll_top: usize,
    show_shadow: bool,
) {
    let w = (cols as usize).clamp(20, 48) as u16;
    let min_h = 5u16;
    let list_h = (items.len() as u16).clamp(1, rows.saturating_sub(4));
    let h = (list_h + 2).max(min_h).min(rows.saturating_sub(2));
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
    let ttl = format!(" {} ", crate::terminal::COMPLETION_LIST_TITLE);
    let tx = x + w.saturating_sub(ttl.len() as u16) / 2;
    p.goto(tx, y);
    p.text(&ttl);
    let list_top = y + 1;
    let visible = h.saturating_sub(2) as usize;
    let mut start = scroll_top;
    if selected < start {
        start = selected;
    }
    if visible > 0 && selected >= start + visible {
        start = selected.saturating_add(1).saturating_sub(visible);
    }
    for i in 0..visible {
        let row_y = list_top + i as u16;
        p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
        p.goto(x + 1, row_y);
        p.text(&" ".repeat(w.saturating_sub(2) as usize));
        let idx = start + i;
        if let Some(item) = items.get(idx) {
            if idx == selected {
                p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
            } else {
                p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
            }
            let t = truncate(&item.display, w.saturating_sub(4) as usize);
            p.goto(x + 2, row_y);
            p.text(&t);
        }
    }
    if show_shadow {
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
    show_shadow: bool,
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
    if show_shadow {
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
}

fn draw_file_op_progress_dialog(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    view: &rmc_core::fileop::FileOpProgressView,
    show_shadow: bool,
) {
    let mut body: Vec<String> = Vec::new();
    if let Some(name) = &view.source_name {
        body.push("Source".to_string());
        body.push(name.clone());
    }
    body.push("Target".to_string());
    body.push(view.target_path.clone());
    if let Some(bar) = &view.classic_bar {
        body.push(String::new());
        body.push(bar.clone());
    }
    if let Some(bar) = &view.file_bar {
        body.push("File".to_string());
        body.push(bar.clone());
    }
    if let Some(bar) = &view.total_bar {
        body.push("Total".to_string());
        body.push(bar.clone());
    }
    body.push(view.files_processed.clone());
    if let Some(t) = &view.total_bytes {
        body.push(t.clone());
    }

    let w = (cols as usize).min(74) as u16;
    let h = (body.len() as u16)
        .saturating_add(4)
        .min(rows.saturating_sub(2))
        .max(7);
    let x = cols.saturating_sub(w) / 2;
    let y = rows.saturating_sub(h) / 2;
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
    let ttl = format!(" {} ", view.title);
    let tx = x + (w.saturating_sub(ttl.len() as u16)) / 2;
    p.goto(tx, y);
    p.text(&ttl);
    // Body
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    let inner = (w - 4) as usize;
    for (i, line) in body.iter().enumerate() {
        let row_y = y + 2 + i as u16;
        if row_y >= y + h - 2 {
            break;
        }
        p.goto(x + 2, row_y);
        p.text(&truncate(line, inner));
    }
    // Abort (GNU mc file-operations dialog)
    p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
    let abort = "< Abort >";
    let bx = x + (w.saturating_sub(abort.len() as u16)) / 2;
    p.goto(bx, y + h - 2);
    p.text(abort);
    if show_shadow {
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
    allow_recursive: bool,
    focus: rmc_core::app::ChmodDialogFocus,
    show_shadow: bool,
) {
    use rmc_core::app::ChmodDialogFocus as F;
    let w = (cols as usize).min(66) as u16;
    let h = 14u16;
    let x = (cols - w) / 2;
    let y = (rows - h) / 2;
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
    p.set_fg_bg(pal.dtitle_fg, pal.dtitle_bg);
    let ttl = " Chmod ";
    let tx = x + (w.saturating_sub(ttl.len() as u16)) / 2;
    p.goto(tx, y);
    p.text(ttl);
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(x + 2, y + 2);
    p.text(&truncate(&format!("File: {}", name), (w - 4) as usize));
    p.goto(x + 2, y + 3);
    p.text(&format!("Octal: {:04o}", mode & 0o7777));
    let labels = ["read", "write", "execute"];
    let groups = ["User", "Group", "Other"];
    let vals = [u, g, o];
    let bit_focus = [
        [F::UserRead, F::UserWrite, F::UserExec],
        [F::GroupRead, F::GroupWrite, F::GroupExec],
        [F::OtherRead, F::OtherWrite, F::OtherExec],
    ];
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
            let focused = focus == bit_focus[gi][li];
            if focused {
                p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
            } else {
                p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
            }
            p.text(&format!("[{}] {}", if on { 'x' } else { ' ' }, lab));
            p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
            p.text("  ");
        }
    }
    let specials = [
        ("Set UID", special.0, F::SetUid),
        ("Set GID", special.1, F::SetGid),
        ("Sticky", special.2, F::Sticky),
    ];
    for (i, (lab, on, f)) in specials.iter().enumerate() {
        if *f == focus {
            p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
        } else {
            p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
        }
        p.goto(x + 2 + (i as u16) * 20, y + 8);
        p.text(&format!("[{}] {}", if *on { 'x' } else { ' ' }, lab));
    }
    if allow_recursive {
        if matches!(focus, F::Recursive) {
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
    }
    let set = if matches!(focus, F::Set) {
        "< Set >"
    } else {
        "[ Set ]"
    };
    let cancel = if matches!(focus, F::Cancel) {
        "< Cancel >"
    } else {
        "[ Cancel ]"
    };
    p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
    let btns = format!("{set}  {cancel}");
    let bx = x + (w.saturating_sub(btns.len() as u16)) / 2;
    p.goto(bx, y + h - 2);
    p.text(&btns);
    if show_shadow {
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
    allow_recursive: bool,
    focus: rmc_core::app::ChownDialogFocus,
    show_shadow: bool,
) {
    use rmc_core::app::ChownDialogFocus as F;
    let w = (cols as usize).min(66) as u16;
    let h = 10u16;
    let x = (cols - w) / 2;
    let y = (rows - h) / 2;
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
    p.set_fg_bg(pal.dtitle_fg, pal.dtitle_bg);
    let ttl = " Chown ";
    let tx = x + (w.saturating_sub(ttl.len() as u16)) / 2;
    p.goto(tx, y);
    p.text(ttl);
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(x + 2, y + 2);
    p.text("Owner:");
    let own_focus = matches!(focus, F::Owner);
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
    let grp_focus = matches!(focus, F::Group);
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
    if allow_recursive {
        let rec_focus = matches!(focus, F::Recursive);
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
    }
    p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
    let ok = if matches!(focus, F::Ok) {
        "< OK >"
    } else {
        "[ OK ]"
    };
    let cancel = if matches!(focus, F::Cancel) {
        "< Cancel >"
    } else {
        "[ Cancel ]"
    };
    let btns = format!("{ok}  {cancel}");
    let bx = x + (w.saturating_sub(btns.len() as u16)) / 2;
    p.goto(bx, y + h - 2);
    p.text(&btns);
    if show_shadow {
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
}

/// GNU mc(1) Command menu labels. Shared with `terminal.rs` so both copies match.
pub(crate) const COMMAND_MENU_ITEMS: &[&str] = &[
    "User menu",
    "Directory tree",
    "Find file",
    "Swap panels",
    "Switch panels on/off",
    "Compare dirs",
    "External panelize",
    "Command history",
    "Directory hotlist",
    "Edit extension file",
    "Edit menu file",
    "Screen list",
];

/// GNU mc(1) Left/Right menu labels. Shared with `terminal.rs`.
pub(crate) const LEFT_RIGHT_MENU_ITEMS: &[&str] = &[
    "Copy",
    "Move",
    "Mkdir",
    "Delete",
    "FTP link",
    "Shell link",
    "SFTP link",
    "SMB link",
    "Listing mode...",
    "Sort order...",
    "Tree",
    "Filter",
    "Reread",
    "Equal panel size",
];

/// GNU mc(1) File menu labels. Shared with `terminal.rs`.
pub(crate) const FILE_MENU_ITEMS: &[&str] = &[
    "Help",
    "View",
    "Edit",
    "Copy",
    "Move",
    "Mkdir",
    "Delete",
    "Quick cd",
    "Select group",
    "Unselect group",
    "Invert selection",
    "Chmod",
    "Chown",
    "Hard link",
    "SymLink",
    "Relative symlink",
    "Quit",
];

fn draw_menu_dropdown(
    p: &mut Painter,
    pal: McPalette,
    top_index: usize,
    selected: usize,
    horizontal_split: bool,
) {
    // Real top menus and stub items
    let menus: [&[&str]; 5] = [
        LEFT_RIGHT_MENU_ITEMS,
        FILE_MENU_ITEMS,
        COMMAND_MENU_ITEMS,
        &[
            "Configuration",
            "Layout",
            "Panels",
            "Confirmations",
            "Appearance",
            "Virtual FS...",
            "Learn keys",
            "Save setup",
        ],
        LEFT_RIGHT_MENU_ITEMS,
    ];
    let titles = menu_bar_titles(horizontal_split);
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

#[allow(clippy::too_many_arguments)]
fn draw_listing_mode_dialog(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    side: rmc_core::actions::PaneSide,
    listing: rmc_core::panel::ListingFormat,
    user_format: &str,
    focus: rmc_core::app::ListingModeFocus,
    show_shadow: bool,
) {
    let _ = side; // implied by Left/Right menu; dialog title remains generic
    let title = "Listing mode";
    let w = 60u16.min(cols.saturating_sub(2));
    let h = 12u16;
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
    // Radios: Full / Brief / Long / User defined
    let radios = [
        ("Full file list", rmc_core::panel::ListingFormat::Full),
        ("Brief", rmc_core::panel::ListingFormat::Brief),
        ("Long file list", rmc_core::panel::ListingFormat::Long),
        ("User defined", rmc_core::panel::ListingFormat::User),
    ];
    for (i, (label, kind)) in radios.iter().enumerate() {
        let row_y = y + 2 + i as u16;
        let sel = if *kind == listing { 'x' } else { ' ' };
        let focused = matches!(
            (i, focus),
            (0, rmc_core::app::ListingModeFocus::RadioFull)
                | (1, rmc_core::app::ListingModeFocus::RadioBrief)
                | (2, rmc_core::app::ListingModeFocus::RadioLong)
                | (3, rmc_core::app::ListingModeFocus::RadioUser)
        );
        if focused {
            p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
        } else {
            p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
        }
        p.goto(x + 2, row_y);
        p.text(&format!("({sel}) {label}"));
    }
    // One-line format field (editable only when User defined is selected)
    let input_row = y + 7;
    let is_input_focus = matches!(focus, rmc_core::app::ListingModeFocus::Input);
    if is_input_focus {
        p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
    } else {
        p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    }
    let prompt = "Format:";
    p.goto(x + 2, input_row);
    p.text(prompt);
    let max_len = w.saturating_sub(4 + prompt.len() as u16);
    let shown = truncate(user_format, max_len as usize);
    p.goto(x + 2 + prompt.len() as u16 + 1, input_row);
    p.text(&shown);
    // Buttons
    p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
    let ok_focus = matches!(focus, rmc_core::app::ListingModeFocus::Ok);
    let cancel_focus = matches!(focus, rmc_core::app::ListingModeFocus::Cancel);
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
    if show_shadow {
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
}

#[allow(clippy::too_many_arguments)]
fn draw_filter_dialog(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    side: rmc_core::actions::PaneSide,
    pattern: &str,
    regular_expression: bool,
    files_only: bool,
    case_sensitive: bool,
    focus: rmc_core::app::FilterDialogFocus,
    show_shadow: bool,
) {
    let _ = side;
    use rmc_core::app::FilterDialogFocus as F;
    let w = 52u16.min(cols.saturating_sub(2));
    let h = 10u16.min(rows.saturating_sub(2)).max(9);
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
    let ttl = " Filter ";
    let tx = x + w.saturating_sub(ttl.len() as u16) / 2;
    p.goto(tx, y);
    p.text(ttl);

    let inner_w = w.saturating_sub(4);
    if matches!(focus, F::Pattern) {
        p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
    } else {
        p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    }
    p.goto(x + 2, y + 2);
    let shown = truncate(pattern, inner_w as usize);
    let mut field = shown;
    while field.chars().count() < inner_w as usize {
        field.push(' ');
    }
    p.text(&field);

    let boxes = [
        (
            "Regular expression",
            regular_expression,
            F::RegularExpression,
        ),
        ("Files only", files_only, F::FilesOnly),
        ("Case sensitive", case_sensitive, F::CaseSensitive),
    ];
    for (i, (label, on, lf)) in boxes.iter().enumerate() {
        let row_y = y + 3 + i as u16;
        if focus == *lf {
            p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
        } else {
            p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
        }
        p.goto(x + 2, row_y);
        p.text(&format!("[{}] {}", if *on { 'x' } else { ' ' }, label));
    }

    let sel_btn = |want: F, txt: &str| {
        if focus == want {
            format!("< {txt} >")
        } else {
            format!("[ {txt} ]")
        }
    };
    p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
    let btns = format!("{}  {}", sel_btn(F::Ok, "OK"), sel_btn(F::Cancel, "Cancel"));
    let bx = x + w.saturating_sub(btns.len() as u16) / 2;
    p.goto(bx, y + h.saturating_sub(2));
    p.text(&btns);
    if show_shadow {
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
}

#[allow(clippy::too_many_arguments)]
fn draw_select_group_dialog(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    select: bool,
    pattern: &str,
    files_only: bool,
    case_sensitive: bool,
    regular_expression: bool,
    focus: rmc_core::app::SelectGroupDialogFocus,
    show_shadow: bool,
) {
    use rmc_core::app::SelectGroupDialogFocus as F;
    let w = 52u16.min(cols.saturating_sub(2));
    let h = 10u16.min(rows.saturating_sub(2)).max(9);
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
    let ttl = if select { " Select " } else { " Unselect " };
    let tx = x + w.saturating_sub(ttl.len() as u16) / 2;
    p.goto(tx, y);
    p.text(ttl);

    let inner_w = w.saturating_sub(4);
    if matches!(focus, F::Pattern) {
        p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
    } else {
        p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    }
    p.goto(x + 2, y + 2);
    let shown = truncate(pattern, inner_w as usize);
    let mut field = shown;
    while field.chars().count() < inner_w as usize {
        field.push(' ');
    }
    p.text(&field);

    let boxes = [
        ("Files only", files_only, F::FilesOnly),
        ("Case sensitive", case_sensitive, F::CaseSensitive),
        (
            "Regular expression",
            regular_expression,
            F::RegularExpression,
        ),
    ];
    for (i, (label, on, lf)) in boxes.iter().enumerate() {
        let row_y = y + 3 + i as u16;
        if focus == *lf {
            p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
        } else {
            p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
        }
        p.goto(x + 2, row_y);
        p.text(&format!("[{}] {}", if *on { 'x' } else { ' ' }, label));
    }

    let sel_btn = |want: F, txt: &str| {
        if focus == want {
            format!("< {txt} >")
        } else {
            format!("[ {txt} ]")
        }
    };
    p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
    let btns = format!("{}  {}", sel_btn(F::Ok, "OK"), sel_btn(F::Cancel, "Cancel"));
    let bx = x + w.saturating_sub(btns.len() as u16) / 2;
    p.goto(bx, y + h.saturating_sub(2));
    p.text(&btns);
    if show_shadow {
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
}

#[cfg(test)]
mod viewer_fbar_and_selection_style_tests {
    use super::{help_fbar_labels, panel_fbar_labels, viewer_fbar_labels, viewer_line_style};
    use crate::mc_colors::McPalette;
    use crossterm::style::Color;

    #[test]
    fn gnu_panel_fbar_labels_match_mc_defaults() {
        assert_eq!(
            panel_fbar_labels(),
            [
                "Help", "Menu", "View", "Edit", "Copy", "RenMov", "Mkdir", "Delete", "PullDn",
                "Quit"
            ]
        );
        assert_eq!(
            panel_fbar_labels()[1],
            "Menu",
            "GNU F2 label is Menu, not User menu"
        );
    }

    #[test]
    fn gnu_help_fbar_is_not_panel_bar() {
        assert_eq!(
            help_fbar_labels(),
            ["Help", "Index", "Prev", "Next", "", "", "", "", "", "Quit"]
        );
        assert_ne!(
            help_fbar_labels()[1],
            "Menu",
            "help F-bar must not reuse the panel Menu/View/Edit labels"
        );
        assert_eq!(help_fbar_labels()[9], "Quit");
    }

    #[test]
    fn gnu_mcview_fbar_labels_default_text_mode() {
        assert_eq!(
            viewer_fbar_labels(false, false, true, false),
            ["Help", "Wrap", "Quit", "Hex", "Goto", "", "Search", "Raw", "Format", "Quit"]
        );
    }

    #[test]
    fn gnu_mcview_fbar_labels_toggle_modes() {
        assert_eq!(viewer_fbar_labels(true, false, true, false)[1], "UnWrap");
        assert_eq!(viewer_fbar_labels(false, true, true, false)[3], "Ascii");
        assert_eq!(viewer_fbar_labels(false, false, false, false)[7], "Parse");
        assert_eq!(viewer_fbar_labels(false, false, true, true)[8], "Unform");
    }

    #[test]
    fn viewer_selected_is_yellow_on_cyan_not_panel_bar() {
        let pal = McPalette::default();
        assert_eq!(pal.selected_fg, Color::Black);
        assert_eq!(pal.selected_bg, Color::Cyan);
        assert_eq!(pal.viewer_selected_fg, Color::Yellow);
        assert_eq!(pal.viewer_selected_bg, Color::Cyan);
        let (fg, bg) = viewer_line_style(true, pal);
        assert_eq!(fg, Color::Yellow);
        assert_eq!(bg, Color::Cyan);
        assert_ne!(fg, pal.selected_fg);
        let (nfg, nbg) = viewer_line_style(false, pal);
        assert_eq!(nfg, pal.core_default_fg);
        assert_eq!(nbg, pal.core_default_bg);
    }
}

#[cfg(test)]
mod editor_syntax_style_tests {
    use super::editor_cell_style;
    use crate::mc_colors::McPalette;
    use crossterm::style::Color;
    use rmc_edit::{EditorBuffer, TokenKind};
    use std::path::PathBuf;

    #[test]
    fn rust_keywords_use_editbold_txt_stays_unhighlighted() {
        let pal = McPalette::default();
        let rs = EditorBuffer::from_bytes(b"fn let name", Some(PathBuf::from("main.rs")));
        let spans = &rs.render_window_spans(40, 1)[0];
        let mut saw_kw = false;
        let mut saw_id = false;
        for sp in spans {
            let (fg, bg) = editor_cell_style(sp.kind, false, pal);
            if sp.text == "fn" || sp.text == "let" {
                assert_eq!(fg, pal.edit_bold_fg);
                assert_eq!(bg, pal.edit_bold_bg);
                assert_eq!((fg, bg), (Color::Yellow, Color::Green));
                saw_kw = true;
            }
            if sp.text == "name" {
                assert_eq!(fg, pal.edit_normal_fg);
                assert_eq!(bg, pal.edit_normal_bg);
                assert_ne!((fg, bg), (pal.edit_bold_fg, pal.edit_bold_bg));
                saw_id = true;
            }
        }
        assert!(saw_kw && saw_id, "{spans:?}");

        let txt = EditorBuffer::from_bytes(b"fn let name", Some(PathBuf::from("notes.txt")));
        for sp in &txt.render_window_spans(40, 1)[0] {
            let (fg, bg) = editor_cell_style(sp.kind, false, pal);
            assert_eq!((fg, bg), (pal.edit_normal_fg, pal.edit_normal_bg));
            assert_ne!(sp.kind, TokenKind::Keyword);
        }
    }

    #[test]
    fn selection_uses_editmarked_not_keyword_color() {
        let pal = McPalette::default();
        let (kw_fg, kw_bg) = editor_cell_style(TokenKind::Keyword, false, pal);
        let (sel_fg, sel_bg) = editor_cell_style(TokenKind::Keyword, true, pal);
        assert_eq!(kw_fg, pal.edit_bold_fg);
        assert_eq!(kw_bg, pal.edit_bold_bg);
        assert_eq!(sel_fg, pal.edit_marked_fg);
        assert_eq!(sel_bg, pal.edit_marked_bg);
        assert_eq!((sel_fg, sel_bg), (Color::Black, Color::Cyan));
        assert_ne!((sel_fg, sel_bg), (kw_fg, kw_bg));
        assert_ne!(
            sel_bg, pal.marked_bg,
            "must not use panel marked yellow;blue"
        );

        let mut buf = EditorBuffer::from_bytes(b"fn foo", Some(PathBuf::from("a.rs")));
        buf.row = 0;
        buf.col = 0;
        buf.mark_start();
        buf.col = 2;
        buf.mark_end();
        let sel = buf.selection_spans_for_view(0, 0, 1, 20);
        assert_eq!(sel[0], Some((0, 2)));
        let spans = &buf.render_window_spans(20, 1)[0];
        assert_eq!(spans[0].kind, TokenKind::Keyword);
        let (fg, bg) = editor_cell_style(spans[0].kind, true, pal);
        assert_eq!((fg, bg), (pal.edit_marked_fg, pal.edit_marked_bg));
    }

    #[test]
    fn comments_and_strings_use_editor_pairs() {
        let pal = McPalette::default();
        let (c_fg, c_bg) = editor_cell_style(TokenKind::Comment, false, pal);
        assert_eq!(c_fg, pal.edit_whitespace_fg);
        assert_eq!(c_bg, pal.edit_normal_bg);
        assert_ne!((c_fg, c_bg), (pal.edit_bold_fg, pal.edit_bold_bg));
        let (s_fg, s_bg) = editor_cell_style(TokenKind::String, false, pal);
        assert_eq!(s_fg, pal.edit_linestate_fg);
        assert_eq!(s_bg, pal.edit_linestate_bg);
        assert_ne!((s_fg, s_bg), (c_fg, c_bg));
        assert_ne!((s_fg, s_bg), (pal.edit_bold_fg, pal.edit_bold_bg));
    }
}
