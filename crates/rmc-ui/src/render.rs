use crate::dirtree::draw_directory_tree_dialog;
use crate::filehighlight::{listing_name_color, name_span_in_line};
use crate::find::draw_find_dialog;
use crate::help::{initial_topic_or_contents, HelpIndex, HelpItem};
use crate::mc_colors::McPalette;
use crate::panel_preview::{info_lines_for_panel, preview_source_entry, quick_view_directory_line};
use crate::panelize::draw_external_panelize_dialog;
use crate::widgets::Painter;
use anyhow::Result;
use crossterm::cursor::Hide;
use crossterm::style::Color;
use crossterm::terminal::{self, Clear, ClearType};
use crossterm::QueueableCommand;
use rmc_core::app::{App, EditorMenu, LayoutFocus, LayoutOptions};
use rmc_core::layout::{compute_chrome_geom, dual_panel_rects};
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
        // Crossterm Colored::Display is empty when NO_COLOR is set, which turns
        // SetColors into `\x1b[m`. Painter::set_fg_bg writes 16-color SGR itself;
        // still force color so any remaining crossterm style commands stay on.
        crossterm::style::force_color_output(true);
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
        // Clear to core default (lightgray;blue). Crossterm Clear uses the current
        // background; without this, the previous F-bar cyan floods unpainted cells.
        painter.set_fg_bg(self.palette.core_default_fg, self.palette.core_default_bg);
        painter.out.queue(Clear(ClearType::All))?;
        let _ = painter.out.queue(Hide);
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
        // Free space is in each panel's bottom frame (not a below-panels chrome row).
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
    // Live GNU 4.8: `  Left     File     Command     Options     Right`
    p.set_fg_bg(pal.menu_fg, pal.menu_bg);
    p.goto(0, 0);
    p.text(&" ".repeat(cols as usize));
    let items = rmc_core::layout::menu_bar_labels(horizontal_split);
    for (i, it) in items.iter().enumerate() {
        let x = rmc_core::layout::menu_bar_item_start(i, horizontal_split);
        if x >= cols {
            break;
        }
        let (fg, bg) = if selected == Some(i) {
            (pal.menusel_fg, pal.menusel_bg)
        } else {
            (pal.menu_fg, pal.menu_bg)
        };
        let start = x.saturating_sub(1);
        let padded = format!(" {it} ");
        paint_span(
            p,
            start,
            0,
            fg,
            bg,
            &truncate(&padded, (cols - start) as usize),
        );
    }
}

fn draw_pause_after_run_prompt(p: &mut Painter, cols: u16, rows: u16, pal: McPalette) {
    let y = rows.saturating_sub(1);
    p.fill_line(y, cols, pal.dialog_default_bg, pal.dialog_default_fg);
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(0, y);
    let msg = crate::terminal::PAUSE_AFTER_RUN_PROMPT;
    p.text(&truncate(msg, cols as usize));
}

/// Test-only: paint dialog overlays (Copy / Error / …) into a byte buffer.
/// Used to prove F5 cannot abort on the next frame.
#[cfg(test)]
pub(crate) fn paint_overlays_for_test(app: &App, cols: u16, rows: u16) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    {
        let mut p = Painter { out: &mut buf };
        draw_overlays(&mut p, app, cols, rows, McPalette::default())?;
    }
    Ok(buf)
}

fn draw_overlays(p: &mut Painter, app: &App, cols: u16, rows: u16, pal: McPalette) -> Result<()> {
    match &app.ui_mode {
        rmc_core::app::UiMode::DialogConfirm { title, message, .. } => {
            // Live GNU F5 on `..`: compact white;red, no buttons. Multi-line
            // same-file Error bodies stay on `draw_dialog_box` so a sibling
            // can wrap `"path"\nand\n"path"\nare the same file`.
            if dialog_is_error_title(title) && !message.contains('\n') {
                draw_error_dialog(p, cols, rows, pal, message, app.shadows);
            } else {
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
            src_path,
            dst_path,
            focus,
            skip_zero_length,
        } => {
            let src_meta = app.vfs.stat(src_path).ok();
            let dst_meta = app.vfs.stat(dst_path).ok();
            draw_overwrite_dialog(
                p,
                cols,
                rows,
                pal,
                *op,
                dst_path,
                src_meta.as_ref(),
                dst_meta.as_ref(),
                *focus,
                *skip_zero_length,
                app.panel_opts.kilobyte_si,
                app.shadows,
            );
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
        rmc_core::app::UiMode::MkdirDialog { value, focus } => {
            draw_mkdir_dialog(p, cols, rows, pal, value, *focus, app.shadows);
        }
        rmc_core::app::UiMode::DeleteDialog {
            name,
            paths,
            focus_ok,
            ..
        } => {
            if paths.len() > 1 {
                let yes = if *focus_ok { "< Yes >" } else { "  Yes  " };
                let no = if *focus_ok { "  No  " } else { "< No >" };
                let msg = format!("Delete {} files?", paths.len());
                draw_dialog_box(p, cols, rows, pal, "Delete", &msg, &[yes, no], app.shadows);
            } else {
                draw_delete_dialog(p, cols, rows, pal, name, *focus_ok, app.shadows);
            }
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
            let src_kind = app
                .active_panel()
                .current_entry()
                .map(|e| {
                    if e.is_dir && !e.is_parent_marker() {
                        "directory"
                    } else {
                        "file"
                    }
                })
                .unwrap_or("file");
            draw_copy_move_dialog(
                p,
                cols,
                rows,
                pal,
                title,
                src_name,
                src_kind,
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
            keys,
            selected,
            capturing,
            focus_save,
        } => {
            draw_learn_keys_dialog(
                p,
                cols,
                rows,
                pal,
                keys,
                *selected,
                *capturing,
                *focus_save,
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
        rmc_core::app::UiMode::SftpHostKeyDialog { prompt, focus, .. } => {
            draw_sftp_host_key_dialog(p, cols, rows, pal, prompt, *focus, app.shadows);
        }
        _ => {}
    }
    Ok(())
}

fn wrap_dialog_lines(text: &str, width: usize) -> Vec<String> {
    let width = width.max(8);
    let mut lines = Vec::new();
    for para in text.split('\n') {
        if para.is_empty() {
            lines.push(String::new());
            continue;
        }
        let mut cur = String::new();
        for word in para.split_whitespace() {
            if cur.is_empty() {
                cur = word.to_string();
            } else if cur.len() + 1 + word.len() <= width {
                cur.push(' ');
                cur.push_str(word);
            } else {
                lines.push(std::mem::take(&mut cur));
                cur = word.to_string();
            }
        }
        if !cur.is_empty() {
            lines.push(cur);
        }
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn draw_sftp_host_key_dialog(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    prompt: &rmc_fs::sftpfs::HostKeyPrompt,
    focus: rmc_core::app::SftpHostKeyFocus,
    show_shadow: bool,
) {
    let title = rmc_fs::sftpfs::HostKeyPrompt::dialog_title();
    let w = (cols as usize).clamp(40, 72) as u16;
    let inner = w.saturating_sub(4) as usize;
    let lines = wrap_dialog_lines(&prompt.dialog_message(), inner);
    let h = (lines.len() as u16 + 6).min(rows.saturating_sub(2)).max(8);
    let x = cols.saturating_sub(w) / 2;
    let y = rows.saturating_sub(h) / 2;
    paint_dialog_frame(p, x, y, w, h, title, pal, false);
    let (fg, bg) = dialog_chrome_pair(pal, false);
    p.set_fg_bg(fg, bg);
    let max_body = h.saturating_sub(5) as usize;
    for (i, line) in lines.iter().take(max_body).enumerate() {
        p.goto(x + 2, y + 2 + i as u16);
        p.text(&truncate(line, inner));
    }
    use rmc_core::app::SftpHostKeyFocus as F;
    let yes = if matches!(focus, F::Yes) {
        "< Yes >"
    } else {
        "[ Yes ]"
    };
    let ignore = if matches!(focus, F::Ignore) {
        "< Ignore >"
    } else {
        "[ Ignore ]"
    };
    let no = if matches!(focus, F::No) {
        "< No >"
    } else {
        "[ No ]"
    };
    let items = [
        (yes, matches!(focus, F::Yes)),
        (ignore, matches!(focus, F::Ignore)),
        (no, matches!(focus, F::No)),
    ];
    let btns_w =
        items.iter().map(|(s, _)| s.len()).sum::<usize>() + 2 * items.len().saturating_sub(1);
    let bx = x + w.saturating_sub(btns_w as u16) / 2;
    paint_dialog_button_cluster(p, bx, y + h - 2, pal, &items, false);
    if show_shadow {
        paint_dialog_shadow(p, x, y, w, h, pal);
    }
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
    src_meta: Option<&rmc_fs::Metadata>,
    dst_meta: Option<&rmc_fs::Metadata>,
    focus: rmc_core::app::OverwriteFocus,
    skip_zero_length: bool,
    si: bool,
    show_shadow: bool,
) {
    use rmc_core::app::{overwrite_button_rows, OverwriteFocus, DONT_OVERWRITE_ZERO_LENGTH_LABEL};
    let w = (cols as usize).min(70) as u16;
    let h = 13u16.min(rows.saturating_sub(2)).max(11);
    let x = (cols.saturating_sub(w)) / 2;
    let y = (rows.saturating_sub(h)) / 2;
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
    let ttl = match op {
        rmc_core::app::CopyMoveOp::Copy => " Copy ",
        rmc_core::app::CopyMoveOp::Move => " Move ",
    };
    let tx = x + (w.saturating_sub(ttl.len() as u16)) / 2;
    p.goto(tx, y);
    p.text(ttl);
    // Destination path
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(x + 2, y + 1);
    let d = truncate(&dst.display().to_string(), (w - 4) as usize);
    p.text(&d);
    // Dates and sizes of both files (GNU replace dialog).
    let fmt_info = |meta: Option<&rmc_fs::Metadata>| -> String {
        match meta {
            Some(m) => {
                let dt: OffsetDateTime = m.modified.into();
                let ts = dt
                    .format(&time::macros::format_description!(
                        "[month repr:short] [day padding:space] [hour]:[minute]"
                    ))
                    .unwrap_or_default();
                format!("{:>8}  {ts}", rmc_core::panel::format_byte_size(m.size, si))
            }
            None => "-".to_string(),
        }
    };
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(x + 2, y + 3);
    p.text(&truncate(
        &format!("New:      {}", fmt_info(src_meta)),
        (w - 4) as usize,
    ));
    p.goto(x + 2, y + 4);
    p.text(&truncate(
        &format!("Existing: {}", fmt_info(dst_meta)),
        (w - 4) as usize,
    ));
    // Checkbox
    if focus == OverwriteFocus::ZeroLength {
        p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
    } else {
        p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    }
    p.goto(x + 2, y + 6);
    p.text(&format!(
        "[{}] {}",
        if skip_zero_length { 'x' } else { ' ' },
        DONT_OVERWRITE_ZERO_LENGTH_LABEL
    ));
    let src_size = src_meta.map(|m| m.size).unwrap_or(0);
    let dst_size = dst_meta.map(|m| m.size).unwrap_or(0);
    let rows_btns = overwrite_button_rows(op, src_size, dst_size);
    let draw_row = |p: &mut Painter,
                    pal: McPalette,
                    x: u16,
                    y: u16,
                    total_w: u16,
                    btns: &[OverwriteFocus],
                    focus: OverwriteFocus| {
        let mut width = 0usize;
        for (i, k) in btns.iter().enumerate() {
            let t = if *k == focus {
                format!("< {} >", k.label())
            } else {
                format!("[ {} ]", k.label())
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
                format!("< {} >", k.label())
            } else {
                p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
                format!("[ {} ]", k.label())
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
    let n_rows = rows_btns.len() as u16;
    for (i, row) in rows_btns.iter().enumerate() {
        let row_y = y + h - 1 - n_rows + i as u16;
        draw_row(p, pal, x, row_y, w, row, focus);
    }
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
    // Width based on longest label; 14 options + 2 rows for buttons/title
    let w = 60u16.min(cols.saturating_sub(2)).max(40);
    let h = 20u16.min(rows.saturating_sub(2)).max(16);
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
    let ttl = format!(" {title} ");
    let tx = x + (w.saturating_sub(ttl.len() as u16)) / 2;
    p.goto(tx, y);
    p.text(&ttl);
    // Options (checkboxes)
    use rmc_core::app::ConfigOptionsFocus as F;
    let items: [(&str, bool, F); 14] = [
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
        ("Safe delete", draft.safe_delete, F::SafeDelete),
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
    keys: &[rmc_core::learn_keys::LearnKeyRow],
    selected: usize,
    capturing: bool,
    focus_save: bool,
    show_shadow: bool,
) {
    use rmc_core::learn_keys::grid_col_row;
    let title = "Learn keys";
    let col_w = 22usize;
    let nkeys = keys.len();
    let grid_rows = *rmc_core::learn_keys::COL_LENS.iter().max().unwrap_or(&12);
    let w = (col_w * 3 + 6).clamp(40, (cols.saturating_sub(2)) as usize) as u16;
    let h = (grid_rows as u16 + 5).min(rows.saturating_sub(2)).max(10);
    let x = (cols.saturating_sub(w)) / 2;
    let y = (rows.saturating_sub(h)) / 2;
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
    // Fill interior
    for r in 1..h.saturating_sub(1) {
        p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
        p.goto(x + 1, y + r);
        p.text(&" ".repeat((w.saturating_sub(2)) as usize));
    }
    let list_top = y + 1;
    for (i, row) in keys.iter().enumerate() {
        let Some((col, r)) = grid_col_row(i) else {
            continue;
        };
        let row_y = list_top + r as u16;
        if row_y >= y + h - 2 {
            continue;
        }
        let cell_x = x + 2 + (col as u16) * (col_w as u16);
        if selected == i {
            p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
        } else {
            p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
        }
        let ok = if row.ok { "OK" } else { "  " };
        let mut line = format!("{:<16} {ok}", row.key.label());
        while line.len() < col_w.saturating_sub(1) {
            line.push(' ');
        }
        if line.len() > col_w.saturating_sub(1) {
            line.truncate(col_w.saturating_sub(1));
        }
        p.goto(cell_x, row_y);
        p.text(&line);
    }
    let btn_row = y + h - 2;
    let save_txt = if selected == nkeys && focus_save {
        "< Save >"
    } else {
        "  Save  "
    };
    let cancel_txt = if selected == nkeys && !focus_save {
        "[ Cancel ]"
    } else {
        "  Cancel  "
    };
    p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
    let btns = format!("{save_txt}  {cancel_txt}");
    let bx = x + (w.saturating_sub(btns.len() as u16)) / 2;
    p.goto(bx, btn_row);
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
    if capturing {
        let label = keys
            .get(selected)
            .map(|r| r.key.label())
            .unwrap_or("that key");
        let msg = format!("Press {label}");
        draw_dialog_box(
            p,
            cols,
            rows,
            pal,
            "Learn keys",
            &msg,
            &["Esc aborts"],
            show_shadow,
        );
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
    // GNU mcedit: frameless row-0 status; F-bar on the last row; content in between.
    // The File/Edit/… menu replaces row 0 only while F9 is open.
    let fbar_row = rows.saturating_sub(1);
    let content_top = 1u16;
    let content_h = fbar_row.saturating_sub(content_top);
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
    // Frameless GNU row-0 status (black;cyan). Transient `status_msg` is not
    // mixed into the cell-accurate field layout (GNU keeps those as dialogs).
    let _ = status_msg;
    p.set_fg_bg(pal.statusbar_fg, pal.statusbar_bg);
    p.goto(0, 0);
    let status = buf.gnu_status_line(cols as usize);
    let t = truncate(&status, cols as usize);
    p.text(&t);
    if t.chars().count() < cols as usize {
        p.text(&" ".repeat(cols as usize - t.chars().count()));
    }
    draw_editor_fbar(p, fbar_row, cols, pal);
    // F9: menu bar replaces the status row; dropdown hangs under the active title.
    if let Some(menu) = show_menu {
        draw_editor_menu_bar(p, cols, pal, Some(menu));
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

/// Menu chrome: default white;cyan, selected white;black, hotkey yellow;cyan,
/// hotkey+selected yellow;black. `hotkey` marks that letter (GNU `&`); when
/// `None`, the first non-space letter is the hotkey.
fn draw_menu_hotkey_label(
    p: &mut Painter,
    x: u16,
    y: u16,
    text: &str,
    selected: bool,
    pal: McPalette,
    width: usize,
) {
    draw_menu_hotkey_label_at(p, x, y, text, None, selected, pal, width);
}

#[allow(clippy::too_many_arguments)]
fn draw_menu_hotkey_label_at(
    p: &mut Painter,
    x: u16,
    y: u16,
    text: &str,
    hotkey: Option<char>,
    selected: bool,
    pal: McPalette,
    width: usize,
) {
    p.goto(x, y);
    let mut line = text.to_string();
    while line.chars().count() < width {
        line.push(' ');
    }
    let (norm_fg, norm_bg, hot_fg, hot_bg) = if selected {
        (
            pal.menusel_fg,
            pal.menusel_bg,
            pal.menuhotsel_fg,
            pal.menuhotsel_bg,
        )
    } else {
        (pal.menu_fg, pal.menu_bg, pal.menuhot_fg, pal.menuhot_bg)
    };
    let mut hotkey_done = false;
    let mut drawn = 0usize;
    for ch in line.chars().take(width) {
        let is_hot = !hotkey_done
            && match hotkey {
                Some(h) => ch.eq_ignore_ascii_case(&h),
                None => !ch.is_whitespace(),
            };
        if is_hot {
            p.set_fg_bg(hot_fg, hot_bg);
            hotkey_done = true;
        } else {
            p.set_fg_bg(norm_fg, norm_bg);
        }
        p.text(&ch.to_string());
        drawn += 1;
    }
    if drawn < width {
        p.set_fg_bg(norm_fg, norm_bg);
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
    paint_mc_fbar(
        p,
        y,
        cols,
        pal,
        &[
            "Help", "Save", "Mark", "Replac", "Copy", "Move", "Search", "Delete", "PullDn", "Quit",
        ],
    );
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

fn draw_viewer_search_dialog(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    dlg: &rmc_core::app::ViewerSearchDialog,
    show_shadow: bool,
) {
    use rmc_core::app::ViewerSearchFocus as F;
    use rmc_core::app::ViewerSearchType;
    // GNU mcview quick dialog is 58 columns; two-column radios + checks.
    let w = (cols as usize).min(58) as u16;
    let h = 10u16;
    if cols < w || rows < h {
        return;
    }
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
    let title = rmc_core::app::ViewerSearchDialog::TITLE;
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
    p.text(&truncate(
        rmc_core::app::ViewerSearchDialog::PROMPT,
        inner_w,
    ));
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
    // GNU QUICK_SEPARATOR between the field and the two columns.
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(x + 1, y + 3);
    p.text(&"─".repeat((w - 2) as usize));
    let radios = ViewerSearchType::ALL;
    for (i, kind) in radios.iter().enumerate() {
        let selected = dlg.search_type == *kind;
        let mark = if selected { '*' } else { ' ' };
        let focused = matches!(dlg.focus, F::SearchType) && selected;
        if focused {
            p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
        } else {
            p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
        }
        p.goto(x + 2, y + 4 + i as u16);
        p.text(&truncate(&format!("({mark}) {}", kind.label()), 24));
    }
    let checks: [(F, &str, bool); 4] = [
        (
            F::CaseSensitive,
            rmc_core::app::ViewerSearchDialog::CHECK_LABELS[0],
            dlg.case_sensitive,
        ),
        (
            F::Backwards,
            rmc_core::app::ViewerSearchDialog::CHECK_LABELS[1],
            dlg.backwards,
        ),
        (
            F::WholeWords,
            rmc_core::app::ViewerSearchDialog::CHECK_LABELS[2],
            dlg.whole_words,
        ),
        (
            F::AllCharsets,
            rmc_core::app::ViewerSearchDialog::CHECK_LABELS[3],
            dlg.all_charsets,
        ),
    ];
    for (i, (focus, label, on)) in checks.iter().enumerate() {
        if dlg.focus == *focus {
            p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
        } else {
            p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
        }
        p.goto(x + 28, y + 4 + i as u16);
        p.text(&truncate(
            &format!("[{}] {}", if *on { 'x' } else { ' ' }, label),
            (w as usize).saturating_sub(30),
        ));
    }
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

fn dialog_is_error_title(title: &str) -> bool {
    title.eq_ignore_ascii_case("Error")
}

fn dialog_chrome_pair(pal: McPalette, error: bool) -> (Color, Color) {
    if error {
        (pal.error_default_fg, pal.error_default_bg)
    } else {
        (pal.dialog_default_fg, pal.dialog_default_bg)
    }
}

fn dialog_title_pair(pal: McPalette, error: bool) -> (Color, Color) {
    if error {
        (pal.error_default_fg, pal.error_default_bg)
    } else {
        (pal.dtitle_fg, pal.dtitle_bg)
    }
}

fn dialog_focus_pair(pal: McPalette, focused: bool, error: bool) -> (Color, Color) {
    if error {
        if focused {
            (pal.errdfocus_fg, pal.errdfocus_bg)
        } else {
            (pal.error_default_fg, pal.error_default_bg)
        }
    } else if focused {
        (pal.dfocus_fg, pal.dfocus_bg)
    } else {
        (pal.dialog_default_fg, pal.dialog_default_bg)
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_dialog_frame(
    p: &mut Painter,
    x: u16,
    y: u16,
    w: u16,
    h: u16,
    title: &str,
    pal: McPalette,
    error: bool,
) {
    let (fg, bg) = dialog_chrome_pair(pal, error);
    p.fill_rect(x, y, w, h, fg, bg);
    p.set_fg_bg(fg, bg);
    p.goto(x, y);
    p.text("┌");
    p.hline(x + 1, y, w.saturating_sub(2), '─', fg, bg);
    p.goto(x + w.saturating_sub(1), y);
    p.text("┐");
    p.vline(x, y + 1, h.saturating_sub(2), '│', fg, bg);
    p.vline(
        x + w.saturating_sub(1),
        y + 1,
        h.saturating_sub(2),
        '│',
        fg,
        bg,
    );
    p.goto(x, y + h.saturating_sub(1));
    p.text("└");
    p.hline(
        x + 1,
        y + h.saturating_sub(1),
        w.saturating_sub(2),
        '─',
        fg,
        bg,
    );
    p.goto(x + w.saturating_sub(1), y + h.saturating_sub(1));
    p.text("┘");
    let (tfg, tbg) = dialog_title_pair(pal, error);
    p.set_fg_bg(tfg, tbg);
    let title_str = format!(" {title} ");
    let tx = x + (w.saturating_sub(title_str.len() as u16)) / 2;
    p.goto(tx, y);
    p.text(&title_str);
}

fn paint_dialog_button_cluster(
    p: &mut Painter,
    x: u16,
    y: u16,
    pal: McPalette,
    buttons: &[(&str, bool)],
    error: bool,
) {
    let (gap_fg, gap_bg) = dialog_chrome_pair(pal, error);
    let mut cx = x;
    for (i, (label, focused)) in buttons.iter().enumerate() {
        if i > 0 {
            p.set_fg_bg(gap_fg, gap_bg);
            p.goto(cx, y);
            p.text("  ");
            cx += 2;
        }
        let (fg, bg) = dialog_focus_pair(pal, *focused, error);
        p.set_fg_bg(fg, bg);
        p.goto(cx, y);
        p.text(label);
        cx += label.len() as u16;
    }
}

fn paint_dialog_shadow(p: &mut Painter, x: u16, y: u16, w: u16, h: u16, pal: McPalette) {
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
    let w = (cols as usize).min(60) as u16;
    let h = 7u16;
    let x = (cols - w) / 2;
    let y = (rows - h) / 2;
    let error = dialog_is_error_title(title);
    paint_dialog_frame(p, x, y, w, h, title, pal, error);
    let (fg, bg) = dialog_chrome_pair(pal, error);
    p.set_fg_bg(fg, bg);
    p.goto(x + 2, y + 2);
    p.text(&truncate(message, (w - 4) as usize));
    let yes = if matches!(focus, rmc_core::app::YncFocus::Yes) {
        "< Yes >"
    } else {
        "[ Yes ]"
    };
    let no = if matches!(focus, rmc_core::app::YncFocus::No) {
        "< No >"
    } else {
        "[ No ]"
    };
    let cancel = if matches!(focus, rmc_core::app::YncFocus::Cancel) {
        "< Cancel >"
    } else {
        "[ Cancel ]"
    };
    let items = [
        (yes, matches!(focus, rmc_core::app::YncFocus::Yes)),
        (no, matches!(focus, rmc_core::app::YncFocus::No)),
        (cancel, matches!(focus, rmc_core::app::YncFocus::Cancel)),
    ];
    let btns_w = items.iter().map(|(s, _)| s.len()).sum::<usize>() + 2 * (items.len() - 1);
    let bx = x + (w.saturating_sub(btns_w as u16)) / 2;
    paint_dialog_button_cluster(p, bx, y + h - 2, pal, &items, error);
    if show_shadow {
        paint_dialog_shadow(p, x, y, w, h, pal);
    }
}
#[allow(clippy::too_many_arguments)]
fn draw_error_dialog(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    message: &str,
    show_shadow: bool,
) {
    // Live GNU 4.8.30 F5 on `..`: 27-wide (`msg + 4`), no buttons.
    let w = ((message.chars().count() + 4) as u16)
        .min(cols.saturating_sub(2))
        .max(17);
    let h = 5u16.min(rows.saturating_sub(1)).max(3);
    let x = gnu_dialog_left(cols, w);
    let y = gnu_dialog_top(rows, h);
    paint_dialog_frame(p, x, y, w, h, "Error", pal, true);
    let (fg, bg) = dialog_chrome_pair(pal, true);
    p.set_fg_bg(fg, bg);
    let msg = truncate(message, w.saturating_sub(4) as usize);
    let mx = x + (w.saturating_sub(msg.chars().count() as u16)) / 2;
    p.goto(mx, y + h / 2);
    p.text(&msg);
    if show_shadow {
        paint_dialog_shadow(p, x, y, w, h, pal);
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
    let max_w = (cols as usize).saturating_sub(2).max(8);
    let classic_w = 60.min(max_w);
    let longest = message
        .lines()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(0);
    let w = classic_w.max((longest + 4).min(max_w)) as u16;
    let inner = w.saturating_sub(4) as usize;
    let lines = dialog_body_lines(message, inner.max(1));
    let h = (6 + lines.len() as u16).max(7);
    let x = cols.saturating_sub(w) / 2;
    let y = rows.saturating_sub(h) / 2;
    let error = dialog_is_error_title(title);
    paint_dialog_frame(p, x, y, w, h, title, pal, error);
    let (fg, bg) = dialog_chrome_pair(pal, error);
    p.set_fg_bg(fg, bg);
    let btn_row = y.saturating_add(h.saturating_sub(2));
    for (i, line) in lines.iter().enumerate() {
        let row = y.saturating_add(2).saturating_add(i as u16);
        if row >= btn_row {
            break;
        }
        let shown = truncate(line, inner);
        let pad = inner.saturating_sub(shown.chars().count()) / 2;
        p.goto(x.saturating_add(2).saturating_add(pad as u16), row);
        p.text(&shown);
    }
    let items: Vec<(&str, bool)> = buttons
        .iter()
        .enumerate()
        .map(|(i, label)| (*label, i == 0))
        .collect();
    let btns_w =
        items.iter().map(|(s, _)| s.len()).sum::<usize>() + 2 * items.len().saturating_sub(1);
    let bx = x + (w.saturating_sub(btns_w as u16)) / 2;
    paint_dialog_button_cluster(p, bx, y + h - 2, pal, &items, error);
    if show_shadow {
        paint_dialog_shadow(p, x, y, w, h, pal);
    }
}

/// Split a GNU Error body (`"%s"\nand\n"%s"\nare the same file`) into
/// wrapped rows. Newlines are real line breaks; long paths wrap.
fn dialog_body_lines(message: &str, inner_width: usize) -> Vec<String> {
    let width = inner_width.max(1);
    let mut out = Vec::new();
    for raw in message.split('\n') {
        if raw.is_empty() {
            out.push(String::new());
            continue;
        }
        let mut rest = raw;
        while !rest.is_empty() {
            let n = rest.chars().count();
            if n <= width {
                out.push(rest.to_string());
                break;
            }
            let split_at = rest
                .char_indices()
                .nth(width)
                .map(|(i, _)| i)
                .unwrap_or(rest.len());
            out.push(rest[..split_at].to_string());
            rest = &rest[split_at..];
        }
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
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
    // mcview `[viewer] _default_` (lightgray;blue), not panel `[core] _default_`
    p.set_fg_bg(pal.viewer_default_fg, pal.viewer_default_bg);
    for y in 0..rows {
        p.goto(0, y);
        p.text(&" ".repeat(cols as usize));
    }
    // GNU mcview: frameless. Row 0 is the status line; last row is the F-bar.
    let content_rows = rows.saturating_sub(2);
    // Reserve space for optional line numbers (text mode only)
    let ln_enabled = show_line_numbers && !hex;
    // Compute line number gutter width conservatively (up to 7 digits + space)
    let ln_gutter: u16 = if ln_enabled { 8 } else { 0 };
    let content_cols = cols.saturating_sub(ln_gutter);
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
        p.goto(0, row_y);
        if ln_enabled {
            // Draw gray-ish line number gutter
            p.set_fg_bg(pal.frame_fg, pal.viewer_default_bg);
            let label = format!("{:>6} ", start_ln + i as u64);
            p.text(&label);
            p.goto(ln_gutter, row_y);
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
        if row_y >= rows.saturating_sub(2) {
            break;
        }
    }
    // Frameless GNU row-0 status: path + bytes/total (or hex offset) + percent.
    p.set_fg_bg(pal.statusbar_fg, pal.statusbar_bg);
    p.goto(0, 0);
    let total = rmc_view::file_len(&content_path).unwrap_or(0);
    let end_bytes = if hex {
        rr.offset
    } else {
        rr.next_screen_offset.min(total)
    };
    let _ = status_msg;
    let status = rmc_view::gnu_status_line(
        cols as usize,
        display_path,
        hex,
        rr.offset,
        end_bytes,
        total,
    );
    let st = truncate(&status, cols as usize);
    p.text(&st);
    if st.chars().count() < cols as usize {
        p.text(&" ".repeat(cols as usize - st.chars().count()));
    }
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
    // GNU mcview 4.8.33 F7 Search dialog (radios + All charsets)
    if let Some(dlg) = search_dialog {
        draw_viewer_search_dialog(p, cols, rows, pal, dlg, show_shadow);
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

/// GNU mc `|` listing-format token: box-drawing vertical, never ASCII `|`.
const LISTING_COL_BAR: char = '│'; // U+2502
const LISTING_COL_TEE_TOP: char = '┬'; // U+252C
const LISTING_COL_TEE_BOT: char = '┴'; // U+2534
const LISTING_SEP_LEFT: char = '├'; // U+251C
const LISTING_SEP_RIGHT: char = '┤'; // U+2524

/// Full listing: name grows; modest right-aligned size; fixed mtime on the right.
/// Bars sit in the 1-cell splits from GNU `half type name | size | mtime`.
#[derive(Clone, Copy, Debug)]
struct FullListingCols {
    size_bar: u16,
    size_x: u16,
    time_bar: u16,
    time_x: u16,
}

fn full_listing_cols(x: u16, w: u16) -> FullListingCols {
    let left = x.saturating_add(1);
    let inner_right = x.saturating_add(w.saturating_sub(2));
    if inner_right <= left {
        return FullListingCols {
            size_bar: left,
            size_x: left,
            time_bar: left,
            time_x: left,
        };
    }
    // Live GNU 4.8 Full listing on a 40-col panel: 7-char size, 12-char mtime,
    // bars at x+18 / x+26 (`│ Size  │Modify time │`).
    let time_x = x
        .saturating_add(w.saturating_sub(13))
        .clamp(left, inner_right);
    let time_bar = time_x.saturating_sub(1).clamp(left, inner_right);
    let size_x = time_bar
        .saturating_sub(7)
        .clamp(left.saturating_add(1).min(inner_right), inner_right);
    let size_bar = size_x.saturating_sub(1).clamp(left, inner_right);
    FullListingCols {
        size_bar,
        size_x,
        time_bar,
        time_x,
    }
}

fn full_listing_bar_xs(cols: FullListingCols, x: u16, w: u16) -> Vec<u16> {
    let lo = x.saturating_add(1);
    let hi = x.saturating_add(w.saturating_sub(2));
    let mut xs = Vec::new();
    if cols.size_bar >= lo && cols.size_bar <= hi && cols.size_bar < cols.time_bar {
        xs.push(cols.size_bar);
    }
    if cols.time_bar >= lo && cols.time_bar <= hi && cols.time_bar != cols.size_bar {
        xs.push(cols.time_bar);
    }
    xs
}

/// 1-cell gaps already reserved by `brief_column_width` (inner minus `n-1` seps).
fn brief_column_bar_xs(x: u16, w: u16, columns: u8) -> Vec<u16> {
    let n = rmc_core::panel::clamp_brief_columns(columns);
    if n <= 1 {
        return Vec::new();
    }
    let per = rmc_core::panel::brief_column_width(w, n);
    let lo = x.saturating_add(1);
    let hi = x.saturating_add(w.saturating_sub(2));
    (1..n)
        .map(|i| x.saturating_add(u16::from(i).saturating_mul(per.saturating_add(1))))
        .filter(|&bx| bx >= lo && bx <= hi)
        .collect()
}

fn listing_column_bar_xs(
    listing: rmc_core::panel::ListingFormat,
    x: u16,
    w: u16,
    brief_columns: u8,
) -> Vec<u16> {
    match listing {
        rmc_core::panel::ListingFormat::Full => full_listing_bar_xs(full_listing_cols(x, w), x, w),
        rmc_core::panel::ListingFormat::Brief => brief_column_bar_xs(x, w, brief_columns),
        rmc_core::panel::ListingFormat::Long | rmc_core::panel::ListingFormat::User => Vec::new(),
    }
}

fn paint_column_bars(p: &mut Painter, xs: &[u16], y: u16, fg: Color, bg: Color, ch: char) {
    let glyph = ch.to_string();
    for &bx in xs {
        paint_span(p, bx, y, fg, bg, &glyph);
    }
}

/// GNU mini-status split: `├─┴─┴─┤` (full-width ─, ┴ only at column bars).
fn paint_mini_status_split(
    p: &mut Painter,
    x: u16,
    y: u16,
    w: u16,
    _bar_xs: &[u16],
    fg: Color,
    bg: Color,
) {
    p.hline(x + 1, y, w.saturating_sub(2), '─', fg, bg);
    paint_span(p, x, y, fg, bg, &LISTING_SEP_LEFT.to_string());
    paint_span(
        p,
        x + w.saturating_sub(1),
        y,
        fg,
        bg,
        &LISTING_SEP_RIGHT.to_string(),
    );
    // Live GNU 4.8.30 mini-status split is a solid ├────────┤ (no ┴).
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
    let is_active_panel = (is_left && matches!(app.active, rmc_core::actions::PaneSide::Left))
        || (!is_left && matches!(app.active, rmc_core::actions::PaneSide::Right));
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
    // Leave cells for `<─` on the left and `.[^]>` on the right (GNU panel widgets).
    const TOP_WIDGET_LEFT: &str = "<";
    const TOP_WIDGET_RIGHT: &str = ".[^]>";
    let widget_left_w = TOP_WIDGET_LEFT.chars().count() as u16;
    let widget_right_w = TOP_WIDGET_RIGHT.chars().count() as u16;
    // Live GNU left-aligns ` /path ` after `<─`, then fills with ─ to `.[^]>`.
    let inner = w.saturating_sub(3 + widget_left_w + widget_right_w);
    let path_str_display = truncate(&path_str_display, inner as usize);
    let cap_x = x + 1 + widget_left_w + 1;
    // GNU: active panel path is selected (black;cyan); inactive stays on the frame.
    let (path_fg, path_bg) = panel_path_caption_colors(&pal, is_active_panel);
    paint_span(
        p,
        cap_x.min(x + w.saturating_sub(2)),
        y,
        path_fg,
        path_bg,
        &path_str_display,
    );
    paint_span(p, x + 1, y, frame_fg, frame_bg, TOP_WIDGET_LEFT);
    if w > 2 + widget_left_w + widget_right_w {
        paint_span(
            p,
            x + w - 1 - widget_right_w,
            y,
            frame_fg,
            frame_bg,
            TOP_WIDGET_RIGHT,
        );
    }

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
        paint_panel_frame_free_space(
            p,
            (x, y, w, h),
            path,
            app.layout.show_free_space,
            (frame_fg, frame_bg),
        );
        return Ok(());
    }

    // Headers: GNU `header` yellow;blue on a filled blue row — never selected/cyan.
    let (header_fg, header_bg) = panel_header_colors(&pal);
    let inner = w.saturating_sub(2);
    paint_span(
        p,
        x + 1,
        y + 1,
        header_fg,
        header_bg,
        &" ".repeat(inner as usize),
    );
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
    let full_cols = full_listing_cols(x, w);
    let bar_xs = listing_column_bar_xs(listing, x, w, panel.brief_columns);
    // Junctions share the listing-row split x. Path/widgets are painted after
    // so a long caption covers ┬ the way live GNU does.
    paint_column_bars(p, &bar_xs, y, frame_fg, frame_bg, LISTING_COL_TEE_TOP);
    paint_span(
        p,
        cap_x.min(x + w.saturating_sub(2)),
        y,
        path_fg,
        path_bg,
        &path_str_display,
    );
    paint_span(p, x + 1, y, frame_fg, frame_bg, TOP_WIDGET_LEFT);
    if w > 2 + widget_left_w + widget_right_w {
        paint_span(
            p,
            x + w - 1 - widget_right_w,
            y,
            frame_fg,
            frame_bg,
            TOP_WIDGET_RIGHT,
        );
    }
    // Live GNU never leaves ┬ on the top frame: after the path, fill ─ to `.[^]>`.
    let path_end = cap_x
        .min(x + w.saturating_sub(2))
        .saturating_add(path_str_display.chars().count() as u16);
    let right_x = x + w.saturating_sub(1 + widget_right_w);
    if path_end < right_x {
        p.hline(
            path_end,
            y,
            right_x.saturating_sub(path_end),
            '─',
            frame_fg,
            frame_bg,
        );
    }
    match listing {
        rmc_core::panel::ListingFormat::Full => {
            let ind = rmc_core::panel::full_listing_sort_indicator(panel.sort_by, panel.sort_dir);
            let ind_s: String = ind.iter().collect();
            paint_span(p, x + 1, y + 1, header_fg, header_bg, &ind_s);
            // Live GNU: `.n     Name      │ Size  │Modify time │` — Name is
            // centered in the leftover name field; Size is centered in 7 cells.
            let name_field = full_cols.size_bar.saturating_sub(x.saturating_add(1));
            let name_rest = name_field.saturating_sub(2);
            let name_x = x + 1 + 2 + name_rest.saturating_sub(4) / 2;
            paint_span(p, name_x, y + 1, header_fg, header_bg, "Name");
            let size_w = full_cols.time_bar.saturating_sub(full_cols.size_x);
            let size_x = full_cols.size_x + size_w.saturating_sub(4) / 2;
            paint_span(p, size_x, y + 1, header_fg, header_bg, "Size");
            paint_span(
                p,
                full_cols.time_x,
                y + 1,
                header_fg,
                header_bg,
                "Modify time",
            );
        }
        rmc_core::panel::ListingFormat::Brief => {
            paint_span(p, x + 1, y + 1, header_fg, header_bg, "Name");
        }
        rmc_core::panel::ListingFormat::User => {
            let header = rmc_core::panel::format_user_listing_header(&user_tokens, inner as usize);
            paint_span(
                p,
                x + 1,
                y + 1,
                header_fg,
                header_bg,
                &truncate(&header, inner as usize),
            );
        }
        rmc_core::panel::ListingFormat::Long => {
            // Column-aligned like ls -l: perm, nlink, owner, group, size, mtime
            let perms_col = x + 1;
            let nlink_col = perms_col + 11; // 10 perms + 1 space
            let owner_col = nlink_col + 5; // nlink 4 + 1 space
            let group_col = owner_col + 9; // owner 8 + 1 space
            let size_col = group_col + 9; // group 8 + 1 space
            let time_col = size_col + 9; // size 8 + 1 space
            paint_span(p, perms_col, y + 1, header_fg, header_bg, "Perms");
            paint_span(p, nlink_col, y + 1, header_fg, header_bg, "Nl");
            paint_span(p, owner_col, y + 1, header_fg, header_bg, "Owner");
            paint_span(p, group_col, y + 1, header_fg, header_bg, "Group");
            paint_span(p, size_col, y + 1, header_fg, header_bg, "Size");
            paint_span(p, time_col, y + 1, header_fg, header_bg, "Modify time");
        }
    }
    // Header `|` token is yellow;blue — do not skip this row.
    paint_column_bars(p, &bar_xs, y + 1, header_fg, header_bg, LISTING_COL_BAR);

    // Content rows
    let content_top = y + 2;
    // Mini-status occupies the row above the bottom frame. When it is off, listing
    // uses that row so the frame stays closed (no empty gap). Quick search still
    // borrows the same row on the active panel.
    let reserve_status = rmc_core::panel::reserve_panel_mini_status(
        app.panel_opts.show_mini_status,
        is_active_panel,
        app.quick_search.is_some(),
    );
    let column_split_sep =
        rmc_core::panel::listing_has_column_split_sep(listing, panel.brief_columns)
            && !bar_xs.is_empty();
    let content_h =
        rmc_core::panel::panel_listing_content_rows(h, reserve_status, column_split_sep);
    let _panel = if is_left { &app.left } else { &app.right };
    // Viewport uses panel.scroll_top, updated by the event loop per visible capacity
    let panel = if is_left { &app.left } else { &app.right };
    match listing {
        rmc_core::panel::ListingFormat::Full => {
            let name_width = full_cols.size_bar.saturating_sub(x.saturating_add(1));
            let size_width = full_cols.time_bar.saturating_sub(full_cols.size_x);
            let time_width = (x + w).saturating_sub(1).saturating_sub(full_cols.time_x);
            for i in 0..content_h as usize {
                let row_y = content_top + i as u16;
                // Clear row
                p.set_fg_bg(pal.core_default_fg, pal.core_default_bg);
                p.goto(x + 1, row_y);
                p.text(&" ".repeat((w - 2) as usize));
                let idx = panel.scroll_top + i;
                let mut bar_bg = pal.frame_bg;
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
                    // Fill so the `│` bar sits on the row bg (cyan when selected).
                    paint_span(p, x + 1, row_y, fg, bg, &" ".repeat((w - 2) as usize));
                    let display_name = format_entry_name(ent);
                    let name_trunc = truncate(&display_name, name_width as usize);
                    let name_fg =
                        listing_name_color(ent, &pal, is_cursor, is_active_panel, selected);
                    paint_span(p, x + 1, row_y, name_fg, bg, &name_trunc);
                    let size_text =
                        rmc_core::panel::format_full_listing_size(ent, app.panel_opts.kilobyte_si);
                    let size = if ent.is_parent_marker() {
                        truncate(&size_text, size_width as usize)
                    } else {
                        fit_right_cell(&size_text, size_width as usize)
                    };
                    paint_span(p, full_cols.size_x, row_y, fg, bg, &size);
                    let time = truncate(&format_time(ent), time_width as usize);
                    paint_span(p, full_cols.time_x, row_y, fg, bg, &time);
                    bar_bg = bg;
                }
                // Frame pair on empty rows; selected/marked rows keep frame glyph on row bg.
                paint_column_bars(p, &bar_xs, row_y, pal.frame_fg, bar_bg, LISTING_COL_BAR);
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
                paint_column_bars(
                    p,
                    &bar_xs,
                    row_y,
                    pal.frame_fg,
                    pal.frame_bg,
                    LISTING_COL_BAR,
                );
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
                        selected,
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
    if let Some(mut text) = rmc_core::panel::panel_mini_status_line(
        app.panel_opts.show_mini_status,
        is_active_panel,
        app.quick_search.as_ref().map(|qs| qs.pattern.as_str()),
        panel.current_entry(),
        app.panel_opts.kilobyte_si,
    ) {
        if panel.current_entry().is_some_and(|e| e.is_parent_marker())
            && !text.starts_with(" Search:")
        {
            text = "UP--DIR".to_string();
        }
        let status_y = y + h - 2;
        let inner = (w - 2) as usize;
        let s = truncate(&text, inner);
        let mut line = s.clone();
        while line.chars().count() < inner {
            line.push(' ');
        }
        paint_span(
            p,
            x + 1,
            status_y,
            pal.statusbar_fg,
            pal.statusbar_bg,
            &line,
        );
    }
    if reserve_status && column_split_sep {
        // GNU: `├─┴─┴─┤` immediately above mini-status; status itself is one cell.
        paint_mini_status_split(
            p,
            x,
            y + h.saturating_sub(3),
            w,
            &bar_xs,
            frame_fg,
            frame_bg,
        );
    } else if column_split_sep {
        // Mini-status off: ┴ meets the bottom frame.
        paint_column_bars(
            p,
            &bar_xs,
            y + h.saturating_sub(1),
            frame_fg,
            frame_bg,
            LISTING_COL_TEE_BOT,
        );
    }
    paint_panel_frame_free_space(
        p,
        (x, y, w, h),
        path,
        app.layout.show_free_space,
        (frame_fg, frame_bg),
    );
    Ok(())
}

/// GNU mc(1): free/total on the bottom frame of each panel, right-aligned.
fn panel_free_space_label(cwd: &std::path::Path) -> String {
    match (fs2::available_space(cwd), fs2::total_space(cwd)) {
        (Ok(avail), Ok(total)) => {
            // Live GNU paints free/total (93%), not used/total.
            let pct = if total > 0 {
                (avail as f64 / total as f64) * 100.0
            } else {
                0.0
            };
            format!(
                " {} / {} ({:.0}%) ─",
                human_bytes(avail),
                human_bytes(total),
                pct
            )
        }
        _ => String::new(),
    }
}

fn paint_panel_frame_free_space(
    p: &mut Painter,
    (x, y, w, h): (u16, u16, u16, u16),
    cwd: &std::path::Path,
    show: bool,
    (frame_fg, frame_bg): (Color, Color),
) {
    if !show || w < 4 {
        return;
    }
    let label = panel_free_space_label(cwd);
    if label.is_empty() {
        return;
    }
    let max_w = w.saturating_sub(2) as usize;
    let text = truncate(&label, max_w);
    let n = text.chars().count() as u16;
    if n == 0 || n + 1 >= w {
        return;
    }
    let start = x + w - 1 - n;
    paint_span(p, start, y + h - 1, frame_fg, frame_bg, &text);
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

/// Live GNU 4.8 dialog left edge: ceil((cols − w) / 2) so odd leftovers
/// sit on the left (`Delete` 21-wide at x=30 on 80 cols, not 29).
pub(crate) fn gnu_dialog_left(cols: u16, w: u16) -> u16 {
    cols.saturating_add(1).saturating_sub(w) / 2
}

/// Live GNU 4.8 dialog top: center in the panel band (screen minus menu,
/// hint, prompt, F-bar) so a 12-row Copy sits on row 4 of 80×24.
pub(crate) fn gnu_dialog_top(rows: u16, h: u16) -> u16 {
    rows.saturating_sub(4).saturating_sub(h) / 2
}

/// Live GNU 4.8 F-bar slots on an 80-col screen: one leading space, eight
/// 8-col buttons, F9 squeezed to 7 so `9PullDn10Quit` has no gap.
pub(crate) fn fbar_slot_bounds(cols: u16) -> [(u16, u16); 10] {
    let slot = (cols / 10).max(1);
    let lead = u16::from(cols >= 80);
    let mut out = [(0u16, 0u16); 10];
    let mut x = lead;
    for (i, slot_bounds) in out.iter_mut().enumerate() {
        let width = if i == 8 && cols >= 80 { 7 } else { slot };
        let end = x.saturating_add(width).min(cols);
        *slot_bounds = (x, end);
        x = end;
    }
    out
}

fn paint_mc_fbar(p: &mut Painter, y: u16, cols: u16, pal: McPalette, labels: &[&str]) {
    p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
    p.goto(0, y);
    p.text(&" ".repeat(cols as usize));
    for (i, (start, end)) in fbar_slot_bounds(cols).iter().enumerate() {
        if *start >= cols || start >= end {
            break;
        }
        let width = (*end - *start) as usize;
        let num = if i == 9 {
            "10".to_string()
        } else {
            (i + 1).to_string()
        };
        let lab = labels.get(i).copied().unwrap_or("");
        let mut text = format!("{num}{lab}");
        while text.chars().count() < width {
            text.push(' ');
        }
        let text: String = text.chars().take(width).collect();
        let num_n = num.chars().count().min(width);
        let num_s: String = text.chars().take(num_n).collect();
        let rest: String = text.chars().skip(num_n).collect();
        paint_span(
            p,
            *start,
            y,
            pal.buttonbar_hotkey_fg,
            pal.buttonbar_hotkey_bg,
            &num_s,
        );
        if !rest.is_empty() {
            paint_span(
                p,
                start.saturating_add(num_n as u16),
                y,
                pal.buttonbar_button_fg,
                pal.buttonbar_button_bg,
                &rest,
            );
        }
    }
}

fn draw_fbar(p: &mut Painter, y: u16, cols: u16, pal: McPalette) {
    paint_mc_fbar(p, y, cols, pal, &panel_fbar_labels());
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
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(0, 0);
    p.text("┌");
    p.hline(
        1,
        0,
        cols.saturating_sub(2),
        '─',
        pal.dialog_default_fg,
        pal.dialog_default_bg,
    );
    p.goto(cols - 1, 0);
    p.text("┐");
    p.vline(
        0,
        1,
        rows.saturating_sub(3),
        '│',
        pal.dialog_default_fg,
        pal.dialog_default_bg,
    );
    p.vline(
        cols - 1,
        1,
        rows.saturating_sub(3),
        '│',
        pal.dialog_default_fg,
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
        pal.dialog_default_fg,
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
    paint_mc_fbar(p, y, cols, pal, &help_fbar_labels());
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
    paint_mc_fbar(p, y, cols, pal, &labels);
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

/// Viewer pairs from public `[viewer]`: unselected `_default_` (lightgray;blue),
/// selected/`viewselected` (yellow;cyan). Not panel `selected` (black;cyan)
/// and not `[core] _default_`.
pub(crate) fn viewer_line_style(selected: bool, pal: McPalette) -> (Color, Color) {
    if selected {
        (pal.viewer_selected_fg, pal.viewer_selected_bg)
    } else {
        (pal.viewer_default_fg, pal.viewer_default_bg)
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

/// Write `s` at (x,y) with an explicit pair. Always sets colors immediately
/// before the glyphs so a previous selected/statusbar cyan cannot leak.
fn paint_span(p: &mut Painter, x: u16, y: u16, fg: Color, bg: Color, s: &str) {
    p.set_fg_bg(fg, bg);
    p.goto(x, y);
    p.text(s);
}

/// GNU default skin: active panel path caption uses `selected` (black;cyan);
/// inactive path stays on the frame (lightgray;blue).
pub(crate) fn panel_path_caption_colors(pal: &McPalette, active: bool) -> (Color, Color) {
    if active {
        (pal.selected_fg, pal.selected_bg)
    } else {
        (pal.frame_fg, pal.frame_bg)
    }
}

/// Column titles (Name / Size / Modify time) use `header` (yellow;blue), never selected/cyan.
pub(crate) fn panel_header_colors(pal: &McPalette) -> (Color, Color) {
    (pal.header_fg, pal.header_bg)
}

fn format_entry_name(ent: &FileEntry) -> String {
    // GNU `type` is always one cell, including a leading space for regular
    // files so names line up with `/docs`, `/..`, and `*run.sh`.
    let mark = rmc_core::panel::listing_type_char(ent);
    format!("{mark}{}", ent.name)
}

fn fit_right_cell(s: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let n = s.chars().count();
    if n >= width {
        s.chars().skip(n - width).collect()
    } else {
        let mut out = " ".repeat(width - n);
        out.push_str(s);
        out
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

/// Truncate to `width` characters and right-pad with spaces.
///
/// Must use character count, not UTF-8 bytes: [`truncate`] may append `…`
/// (U+2026, 3 bytes) and a path can have more bytes than columns. The Copy
/// dialog used `width - s.len()` and aborted on F5 when the dest was long.
fn pad_field(s: &str, width: usize) -> String {
    let t = truncate(s, width);
    let n = t.chars().count();
    format!("{t}{}", " ".repeat(width.saturating_sub(n)))
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
    focus: rmc_core::app::MkdirDialogFocus,
    show_shadow: bool,
) {
    use rmc_core::app::MkdirDialogFocus as F;
    // Live GNU 4.8.30 F7: 38×6, prompt, field, section bar, `[< OK >] [ Cancel ]`.
    // OK is the default button (angle brackets) even when Cancel or the field
    // has focus; focus itself is dfocus color, not a bracket swap.
    let w = (cols as usize).min(38) as u16;
    let h = 6u16;
    let x = gnu_dialog_left(cols, w);
    let y = gnu_dialog_top(rows, h);
    paint_dialog_frame(p, x, y, w, h, "Create a new Directory", pal, false);
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(x + 2, y + 1);
    p.text("Enter directory name:");
    // Live GNU keeps the field in dfocus (black;cyan) after Tab to a button.
    p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
    p.goto(x + 2, y + 2);
    p.text(&pad_field(value, w.saturating_sub(4) as usize));
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(x, y + 3);
    p.text("├");
    p.hline(
        x + 1,
        y + 3,
        w.saturating_sub(2),
        '─',
        pal.dialog_default_fg,
        pal.dialog_default_bg,
    );
    p.goto(x + w - 1, y + 3);
    p.text("┤");
    let ok_focused = matches!(focus, F::Ok);
    let cancel_focused = matches!(focus, F::Cancel);
    // Live GNU uses one space between `[< OK >]` and `[ Cancel ]`.
    let items = [("[< OK >]", ok_focused), ("[ Cancel ]", cancel_focused)];
    let btns_w = items.iter().map(|(s, _)| s.len()).sum::<usize>() + 1;
    let bx = x + (w.saturating_sub(btns_w as u16)) / 2;
    let (gap_fg, gap_bg) = (pal.dialog_default_fg, pal.dialog_default_bg);
    let mut cx = bx;
    for (i, (label, focused)) in items.iter().enumerate() {
        if i > 0 {
            p.set_fg_bg(gap_fg, gap_bg);
            p.goto(cx, y + h - 2);
            p.text(" ");
            cx += 1;
        }
        let (fg, bg) = dialog_focus_pair(pal, *focused, false);
        p.set_fg_bg(fg, bg);
        p.goto(cx, y + h - 2);
        p.text(label);
        cx += label.len() as u16;
    }
    if show_shadow {
        paint_dialog_shadow(p, x, y, w, h, pal);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_delete_dialog(
    p: &mut Painter,
    cols: u16,
    rows: u16,
    pal: McPalette,
    name: &str,
    focus_ok: bool,
    show_shadow: bool,
) {
    // Live GNU 4.8.30 F8: compact box, two-line prompt, section bar, `[ Yes ]  [ No ]`.
    let w = 21u16.min(cols.saturating_sub(2)).max(17);
    let h = 6u16;
    let x = gnu_dialog_left(cols, w);
    let y = gnu_dialog_top(rows, h);
    paint_dialog_frame(p, x, y, w, h, "Delete", pal, false);
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(x + 4, y + 1);
    p.text("Delete file");
    let quoted = format!("\"{name}\"?");
    p.goto(x + 4, y + 2);
    p.text(&truncate(&quoted, w.saturating_sub(6) as usize));
    p.goto(x, y + 3);
    p.text("├");
    p.hline(
        x + 1,
        y + 3,
        w.saturating_sub(2),
        '─',
        pal.dialog_default_fg,
        pal.dialog_default_bg,
    );
    p.goto(x + w - 1, y + 3);
    p.text("┤");
    let items = [("[ Yes ]", focus_ok), ("[ No ]", !focus_ok)];
    let btns_w = items.iter().map(|(s, _)| s.len()).sum::<usize>() + 2;
    let bx = x + (w.saturating_sub(btns_w as u16)) / 2;
    paint_dialog_button_cluster(p, bx, y + h - 2, pal, &items, false);
    if show_shadow {
        paint_dialog_shadow(p, x, y, w, h, pal);
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
    paint_dialog_frame(p, x, y, w, h, title, pal, false);
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(x + 2, y + 2);
    p.text(&truncate(prompt, (w - 4) as usize));
    p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
    p.goto(x + 2, y + 4);
    let t = truncate(value, (w - 4) as usize);
    p.text(&format!("{t}{}", " ".repeat((w - 4) as usize - t.len())));
    let ok = if focus_ok { "< OK >" } else { "[ OK ]" };
    let cancel = if focus_ok { " Cancel " } else { "[ Cancel ]" };
    let items = [(ok, focus_ok), (cancel, !focus_ok)];
    let btns_w = items.iter().map(|(s, _)| s.len()).sum::<usize>() + 2;
    let bx = x + (w.saturating_sub(btns_w as u16)) / 2;
    paint_dialog_button_cluster(p, bx, y + h - 2, pal, &items, false);
    if show_shadow {
        paint_dialog_shadow(p, x, y, w, h, pal);
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
                rmc_core::jobs::JobStatus::Stopped => "Stopped",
                rmc_core::jobs::JobStatus::Done => "Done",
                rmc_core::jobs::JobStatus::Failed => "Failed",
                rmc_core::jobs::JobStatus::Cancelled => "Cancelled",
            };
            let st = truncate(status, 10);
            p.text(&st);
        }
    }
    // GNU mc Background jobs: Stop / Restart / Kill, plus Clean up and OK.
    let sel_btn = |want: rmc_core::app::JobsDialogFocus, txt: &str| {
        if focus == want {
            format!("< {txt} >")
        } else {
            format!("[ {txt} ]")
        }
    };
    p.set_fg_bg(pal.buttonbar_button_fg, pal.buttonbar_button_bg);
    let btns = rmc_core::app::JOBS_DIALOG_BUTTONS
        .iter()
        .map(|(f, txt)| sel_btn(*f, txt))
        .collect::<Vec<_>>()
        .join("  ");
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
    src_kind: &str,
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
    // Live GNU 4.8.30 F5: 66×12, two-column checks, section bars.
    let w = (cols as usize).min(66) as u16;
    let h = 12u16.min(rows.saturating_sub(1)).max(7);
    let x = gnu_dialog_left(cols, w);
    let y = gnu_dialog_top(rows, h);
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
    let ttl = format!(" {title} ");
    let tx = x + (w.saturating_sub(ttl.len() as u16)) / 2;
    p.goto(tx, y);
    p.text(&ttl);
    // Lines — live GNU 4.8.30 F5
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(x + 2, y + 1);
    p.text(&truncate(
        &format!("{title} {src_kind} \"{src_name}\" with source mask:"),
        w.saturating_sub(4) as usize,
    ));
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
    p.goto(x + 2, y + 2);
    p.text(&pad_field(mask, w.saturating_sub(4) as usize));
    // Using shell patterns is right-aligned on the next row.
    let shell = format!(
        "[{}] Using shell patterns",
        if using_shell_patterns { 'x' } else { ' ' }
    );
    // Live GNU 4.8.30: shell check and the right-hand checks share column 33.
    let shell_x = x + 33;
    if matches!(focus, F::Checkbox1) {
        p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
    } else {
        p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    }
    p.goto(shell_x, y + 3);
    p.text(&shell);
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(x + 2, y + 4);
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
    p.goto(x + 2, y + 5);
    p.text(&pad_field(to, w.saturating_sub(4) as usize));
    // Section bar, two-column checks, section bar.
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(x, y + 6);
    p.text("├");
    p.hline(
        x + 1,
        y + 6,
        w.saturating_sub(2),
        '─',
        pal.dialog_default_fg,
        pal.dialog_default_bg,
    );
    p.goto(x + w - 1, y + 6);
    p.text("┤");
    let paint_check = |p: &mut Painter, xx: u16, yy: u16, on: bool, label: &str, focused: bool| {
        if focused {
            p.set_fg_bg(pal.dfocus_fg, pal.dfocus_bg);
        } else {
            p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
        }
        p.goto(xx, yy);
        p.text(&format!("[{}] {label}", if on { 'x' } else { ' ' }));
    };
    paint_check(
        p,
        x + 2,
        y + 7,
        follow_links,
        "Follow links",
        matches!(focus, F::Checkbox2),
    );
    paint_check(
        p,
        x + 33,
        y + 7,
        dive_into_subdir,
        "Dive into subdir if exists",
        matches!(focus, F::Checkbox4),
    );
    paint_check(
        p,
        x + 2,
        y + 8,
        preserve_attrs,
        "Preserve attributes",
        matches!(focus, F::Checkbox3),
    );
    paint_check(
        p,
        x + 33,
        y + 8,
        stable_symlinks,
        "Stable symlinks",
        matches!(focus, F::Checkbox5),
    );
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
    p.goto(x, y + 9);
    p.text("├");
    p.hline(
        x + 1,
        y + 9,
        w.saturating_sub(2),
        '─',
        pal.dialog_default_fg,
        pal.dialog_default_bg,
    );
    p.goto(x + w - 1, y + 9);
    p.text("┤");
    let sel = |f: F, txt: &str| {
        // GNU marks the default action `[< OK >]` while focus stays on `to:`.
        let marked = match f {
            F::Ok => !matches!(focus, F::Background | F::Cancel),
            _ => f == focus,
        };
        if marked {
            format!("[< {txt} >]")
        } else {
            format!("[ {txt} ]")
        }
    };
    let btns = format!(
        "{} {} {}",
        sel(F::Ok, "OK"),
        sel(F::Background, "Background"),
        sel(F::Cancel, "Cancel")
    );
    let bx = x + (w.saturating_sub(btns.len() as u16)) / 2;
    p.set_fg_bg(pal.dialog_default_fg, pal.dialog_default_bg);
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

/// One drop-down row. Empty `label` is a GNU separator (`├─…─┤`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct MenuRow {
    pub label: &'static str,
    pub hotkey: Option<char>,
    pub shortcut: &'static str,
}

impl MenuRow {
    pub const fn is_separator(self) -> bool {
        self.label.is_empty()
    }
}

/// File menu rows keep this alias so #174 cell tests stay on `FILE_MENU`.
pub(crate) type FileMenuRow = MenuRow;

/// GNU mc 4.8.30 `create_command_menu` after `g_list_reverse`, plus default
/// shortcut column (`midnight_get_shortcut` / `mc.default.keymap`).
/// Items without an existing handler are omitted (see leftover list).
pub(crate) const COMMAND_MENU: &[MenuRow] = &[
    MenuRow {
        label: "User menu",
        hotkey: Some('U'),
        shortcut: "F2",
    },
    MenuRow {
        label: "Directory tree",
        hotkey: Some('D'),
        shortcut: "",
    },
    MenuRow {
        label: "Find file",
        hotkey: Some('F'),
        shortcut: "M-?",
    },
    MenuRow {
        label: "Swap panels",
        hotkey: Some('w'),
        shortcut: "C-u",
    },
    MenuRow {
        label: "Switch panels on/off",
        hotkey: Some('p'),
        shortcut: "C-o",
    },
    MenuRow {
        label: "Compare directories",
        hotkey: Some('C'),
        shortcut: "C-x d",
    },
    MenuRow {
        label: "Compare files",
        hotkey: Some('o'),
        shortcut: "C-x C-d",
    },
    MenuRow {
        label: "External panelize",
        hotkey: Some('x'),
        shortcut: "C-x !",
    },
    MenuRow {
        label: "",
        hotkey: None,
        shortcut: "",
    },
    MenuRow {
        label: "Command history",
        hotkey: Some('h'),
        shortcut: "M-h",
    },
    MenuRow {
        label: "Directory hotlist",
        hotkey: Some('r'),
        shortcut: "C-\\",
    },
    MenuRow {
        label: "Background jobs",
        hotkey: Some('B'),
        shortcut: "C-x j",
    },
    MenuRow {
        label: "Screen list",
        hotkey: Some('t'),
        shortcut: "M-`",
    },
    MenuRow {
        label: "",
        hotkey: None,
        shortcut: "",
    },
    MenuRow {
        label: "Edit extension file",
        hotkey: Some('e'),
        shortcut: "",
    },
    MenuRow {
        label: "Edit menu file",
        hotkey: Some('m'),
        shortcut: "",
    },
    MenuRow {
        label: "Edit highlighting group file",
        hotkey: Some('g'),
        shortcut: "",
    },
];

/// Labels including `""` separators so dropdown row indices match `COMMAND_MENU`.
pub(crate) const COMMAND_MENU_ITEMS: &[&str] = &[
    "User menu",
    "Directory tree",
    "Find file",
    "Swap panels",
    "Switch panels on/off",
    "Compare directories",
    "Compare files",
    "External panelize",
    "",
    "Command history",
    "Directory hotlist",
    "Background jobs",
    "Screen list",
    "",
    "Edit extension file",
    "Edit menu file",
    "Edit highlighting group file",
];

/// GNU `menu_arrange`: box = max_hotkey_len + max_shortcut_len + 5.
pub(crate) const COMMAND_MENU_BOX_WIDTH: u16 = 41;
/// Inner column where the shortcut column starts (`max_hotkey_len + 3`).
pub(crate) const COMMAND_MENU_SHORTCUT_COL: usize = 32;

/// GNU mc 4.8.30 `create_panel_menu` after `g_list_reverse` (FTP/FISH/SFTP
/// typical 4.8 build). Encoding... / Panelize omitted (no existing handler).
pub(crate) const LEFT_RIGHT_MENU: &[MenuRow] = &[
    MenuRow {
        label: "File listing",
        hotkey: Some('g'),
        shortcut: "",
    },
    MenuRow {
        label: "Quick view",
        hotkey: Some('Q'),
        shortcut: "C-x q",
    },
    MenuRow {
        label: "Info",
        hotkey: Some('I'),
        shortcut: "C-x i",
    },
    MenuRow {
        label: "Tree",
        hotkey: Some('T'),
        shortcut: "",
    },
    MenuRow {
        label: "",
        hotkey: None,
        shortcut: "",
    },
    MenuRow {
        label: "Listing format...",
        hotkey: Some('L'),
        shortcut: "",
    },
    MenuRow {
        label: "Sort order...",
        hotkey: Some('S'),
        shortcut: "",
    },
    MenuRow {
        label: "Filter...",
        hotkey: Some('F'),
        shortcut: "",
    },
    MenuRow {
        label: "",
        hotkey: None,
        shortcut: "",
    },
    MenuRow {
        label: "FTP link...",
        hotkey: Some('P'),
        shortcut: "",
    },
    MenuRow {
        label: "Shell link...",
        hotkey: Some('h'),
        shortcut: "",
    },
    MenuRow {
        label: "SFTP link...",
        hotkey: Some('n'),
        shortcut: "",
    },
    MenuRow {
        label: "",
        hotkey: None,
        shortcut: "",
    },
    MenuRow {
        label: "Rescan",
        hotkey: Some('R'),
        shortcut: "C-r",
    },
];

/// Labels including `""` separators so dropdown row indices match `LEFT_RIGHT_MENU`.
pub(crate) const LEFT_RIGHT_MENU_ITEMS: &[&str] = &[
    "File listing",
    "Quick view",
    "Info",
    "Tree",
    "",
    "Listing format...",
    "Sort order...",
    "Filter...",
    "",
    "FTP link...",
    "Shell link...",
    "SFTP link...",
    "",
    "Rescan",
];

pub(crate) const LEFT_RIGHT_MENU_BOX_WIDTH: u16 = 27;
pub(crate) const LEFT_RIGHT_MENU_SHORTCUT_COL: usize = 20;

/// GNU mc 4.8.30 `create_file_menu` after `g_list_reverse`, plus default
/// shortcut column (`midnight_get_shortcut` / `mc.default.keymap`).
/// Empty `label` is a separator. Shared with `terminal.rs` and hit-testing.
pub(crate) const FILE_MENU: &[FileMenuRow] = &[
    FileMenuRow {
        label: "View",
        hotkey: Some('V'),
        shortcut: "F3",
    },
    FileMenuRow {
        label: "View file...",
        hotkey: Some('w'),
        shortcut: "",
    },
    FileMenuRow {
        label: "Filtered view",
        hotkey: Some('F'),
        shortcut: "M-!",
    },
    FileMenuRow {
        label: "Edit",
        hotkey: Some('E'),
        shortcut: "F4",
    },
    FileMenuRow {
        label: "Copy",
        hotkey: Some('C'),
        shortcut: "F5",
    },
    FileMenuRow {
        label: "Chmod",
        hotkey: Some('h'),
        shortcut: "C-x c",
    },
    FileMenuRow {
        label: "Link",
        hotkey: Some('L'),
        shortcut: "C-x l",
    },
    FileMenuRow {
        label: "Symlink",
        hotkey: Some('S'),
        shortcut: "C-x s",
    },
    FileMenuRow {
        label: "Relative symlink",
        hotkey: Some('k'),
        shortcut: "C-x v",
    },
    FileMenuRow {
        label: "Edit symlink",
        hotkey: Some('y'),
        shortcut: "C-x C-s",
    },
    FileMenuRow {
        label: "Chown",
        hotkey: Some('o'),
        shortcut: "C-x o",
    },
    FileMenuRow {
        label: "Advanced chown",
        hotkey: Some('A'),
        shortcut: "",
    },
    FileMenuRow {
        label: "Chattr",
        hotkey: Some('t'),
        shortcut: "C-x e",
    },
    FileMenuRow {
        label: "Rename/Move",
        hotkey: Some('R'),
        shortcut: "F6",
    },
    FileMenuRow {
        label: "Mkdir",
        hotkey: Some('M'),
        shortcut: "F7",
    },
    FileMenuRow {
        label: "Delete",
        hotkey: Some('D'),
        shortcut: "F8",
    },
    FileMenuRow {
        label: "Quick cd",
        hotkey: Some('Q'),
        shortcut: "M-c",
    },
    FileMenuRow {
        label: "",
        hotkey: None,
        shortcut: "",
    },
    FileMenuRow {
        label: "Select group",
        hotkey: Some('g'),
        shortcut: "+",
    },
    FileMenuRow {
        label: "Unselect group",
        hotkey: Some('n'),
        shortcut: "-",
    },
    FileMenuRow {
        label: "Invert selection",
        hotkey: Some('I'),
        shortcut: "*",
    },
    FileMenuRow {
        label: "",
        hotkey: None,
        shortcut: "",
    },
    FileMenuRow {
        label: "Exit",
        hotkey: Some('x'),
        shortcut: "F10",
    },
];

/// Labels including `""` separators so dropdown row indices match `FILE_MENU`.
pub(crate) const FILE_MENU_ITEMS: &[&str] = &[
    "View",
    "View file...",
    "Filtered view",
    "Edit",
    "Copy",
    "Chmod",
    "Link",
    "Symlink",
    "Relative symlink",
    "Edit symlink",
    "Chown",
    "Advanced chown",
    "Chattr",
    "Rename/Move",
    "Mkdir",
    "Delete",
    "Quick cd",
    "",
    "Select group",
    "Unselect group",
    "Invert selection",
    "",
    "Exit",
];

/// Live GNU 4.8.30 File drop-down: 26 inner cells + 2 borders.
pub(crate) const FILE_MENU_BOX_WIDTH: u16 = 28;
/// Inner column where the shortcut column starts (`max_hotkey_len + 3`).
pub(crate) const FILE_MENU_SHORTCUT_COL: usize = 19;

/// GNU mc 4.8.30 `create_options_menu` after `g_list_reverse`.
/// Display bits... omitted (no existing handler).
pub(crate) const OPTIONS_MENU: &[MenuRow] = &[
    MenuRow {
        label: "Configuration...",
        hotkey: Some('C'),
        shortcut: "",
    },
    MenuRow {
        label: "Layout...",
        hotkey: Some('L'),
        shortcut: "",
    },
    MenuRow {
        label: "Panel options...",
        hotkey: Some('P'),
        shortcut: "",
    },
    MenuRow {
        label: "Confirmation...",
        hotkey: Some('o'),
        shortcut: "",
    },
    MenuRow {
        label: "Appearance...",
        hotkey: Some('A'),
        shortcut: "",
    },
    MenuRow {
        label: "Learn keys...",
        hotkey: Some('k'),
        shortcut: "",
    },
    MenuRow {
        label: "Virtual FS...",
        hotkey: Some('V'),
        shortcut: "",
    },
    MenuRow {
        label: "",
        hotkey: None,
        shortcut: "",
    },
    MenuRow {
        label: "Save setup",
        hotkey: Some('S'),
        shortcut: "",
    },
];

/// Labels including `""` separators so dropdown row indices match `OPTIONS_MENU`.
pub(crate) const OPTIONS_MENU_ITEMS: &[&str] = &[
    "Configuration...",
    "Layout...",
    "Panel options...",
    "Confirmation...",
    "Appearance...",
    "Learn keys...",
    "Virtual FS...",
    "",
    "Save setup",
];

pub(crate) const OPTIONS_MENU_BOX_WIDTH: u16 = 21;
pub(crate) const OPTIONS_MENU_SHORTCUT_COL: usize = 19;

/// Dropdown entries for F9 top index 0..=4 (Left, File, Command, Options, Right).
pub(crate) fn top_menu_items(top_index: usize) -> &'static [&'static str] {
    match top_index {
        1 => FILE_MENU_ITEMS,
        2 => COMMAND_MENU_ITEMS,
        3 => OPTIONS_MENU_ITEMS,
        _ => LEFT_RIGHT_MENU_ITEMS,
    }
}

pub(crate) fn top_menu_rows(top_index: usize) -> &'static [MenuRow] {
    match top_index {
        1 => FILE_MENU,
        2 => COMMAND_MENU,
        3 => OPTIONS_MENU,
        _ => LEFT_RIGHT_MENU,
    }
}

/// GNU `menubar_draw_drop`: box at `start_x - 1` where `start_x` is the
/// leading space before the title word (`menu_bar_item_start - 1`).
pub(crate) fn dropdown_origin_x(top_index: usize, horizontal_split: bool) -> u16 {
    rmc_core::layout::menu_bar_item_start(top_index, horizontal_split).saturating_sub(2)
}

pub(crate) fn dropdown_box_size(top_index: usize) -> (u16, u16) {
    let items = top_menu_items(top_index);
    let h = items.len() as u16 + 2;
    let w = match top_index {
        1 => FILE_MENU_BOX_WIDTH,
        2 => COMMAND_MENU_BOX_WIDTH,
        3 => OPTIONS_MENU_BOX_WIDTH,
        _ => LEFT_RIGHT_MENU_BOX_WIDTH,
    };
    (w, h)
}

pub(crate) fn menu_shortcut_col(top_index: usize) -> usize {
    match top_index {
        1 => FILE_MENU_SHORTCUT_COL,
        2 => COMMAND_MENU_SHORTCUT_COL,
        3 => OPTIONS_MENU_SHORTCUT_COL,
        _ => LEFT_RIGHT_MENU_SHORTCUT_COL,
    }
}

pub(crate) fn menu_row_is_separator(top_index: usize, idx: usize) -> bool {
    top_menu_rows(top_index)
        .get(idx)
        .is_some_and(|r| r.is_separator())
}

pub(crate) fn menu_item_hotkey(top_index: usize, idx: usize) -> Option<char> {
    top_menu_rows(top_index).get(idx).and_then(|r| r.hotkey)
}

/// GNU menubar: Up/Down stop at the ends and skip separator rows.
pub(crate) fn step_menu_index(top_index: usize, selected: usize, delta: isize) -> usize {
    let n = top_menu_items(top_index).len();
    if n == 0 {
        return 0;
    }
    let mut i = selected as isize;
    for _ in 0..n {
        i += delta;
        if i < 0 || i >= n as isize {
            return selected;
        }
        let u = i as usize;
        if !menu_row_is_separator(top_index, u) {
            return u;
        }
    }
    selected
}

fn draw_menu_dropdown(
    p: &mut Painter,
    pal: McPalette,
    top_index: usize,
    selected: usize,
    horizontal_split: bool,
) {
    let items = top_menu_items(top_index);
    let x = dropdown_origin_x(top_index, horizontal_split);
    let y = 1u16;
    let (w, h) = dropdown_box_size(top_index);
    p.fill_rect(x, y, w, h, pal.menu_fg, pal.menu_bg);
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
    let inner = (w - 2) as usize;
    let shortcut_col = menu_shortcut_col(top_index);
    let rows = top_menu_rows(top_index);
    for (i, it) in items.iter().enumerate() {
        let row = y + 1 + i as u16;
        if menu_row_is_separator(top_index, i) {
            p.set_fg_bg(pal.menu_fg, pal.menu_bg);
            p.goto(x, row);
            p.text("├");
            p.hline(x + 1, row, w - 2, '─', pal.menu_fg, pal.menu_bg);
            p.goto(x + w - 1, row);
            p.text("┤");
            continue;
        }
        let hotkey = menu_item_hotkey(top_index, i);
        let shortcut = rows.get(i).map(|r| r.shortcut).unwrap_or("");
        draw_dropdown_menu_row(
            p,
            x,
            row,
            it,
            hotkey,
            shortcut,
            shortcut_col,
            i == selected,
            pal,
            inner,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_dropdown_menu_row(
    p: &mut Painter,
    x: u16,
    row: u16,
    label: &str,
    hotkey: Option<char>,
    shortcut: &str,
    shortcut_col: usize,
    selected: bool,
    pal: McPalette,
    inner: usize,
) {
    // GNU `menubar_paint_idx`: leading space, label, shortcut at max_hotkey_len+3.
    let mut line = String::from(" ");
    line.push_str(label);
    while line.chars().count() < shortcut_col {
        line.push(' ');
    }
    line.push_str(shortcut);
    while line.chars().count() < inner {
        line.push(' ');
    }
    draw_menu_hotkey_label_at(p, x + 1, row, &line, hotkey, selected, pal, inner);
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
        assert_eq!(pal.viewer_default_fg, Color::Grey);
        assert_eq!(pal.viewer_default_bg, Color::Blue);
        assert_eq!(pal.viewer_selected_fg, Color::Yellow);
        assert_eq!(pal.viewer_selected_bg, Color::Cyan);
        let (fg, bg) = viewer_line_style(true, pal);
        assert_eq!(fg, Color::Yellow);
        assert_eq!(bg, Color::Cyan);
        assert_ne!(fg, pal.selected_fg);
        let (nfg, nbg) = viewer_line_style(false, pal);
        assert_eq!((nfg, nbg), (Color::Grey, Color::Blue));
        assert_eq!(nfg, pal.viewer_default_fg);
        assert_eq!(nbg, pal.viewer_default_bg);
    }

    #[test]
    fn viewer_unselected_uses_viewer_default_not_core_pair() {
        let mut pal = McPalette::default();
        pal.core_default_fg = Color::White;
        pal.core_default_bg = Color::Red;
        pal.viewer_default_fg = Color::Grey;
        pal.viewer_default_bg = Color::Blue;
        let (nfg, nbg) = viewer_line_style(false, pal);
        assert_eq!((nfg, nbg), (Color::Grey, Color::Blue));
        assert_ne!((nfg, nbg), (pal.core_default_fg, pal.core_default_bg));
        let (sfg, sbg) = viewer_line_style(true, pal);
        assert_eq!((sfg, sbg), (Color::Yellow, Color::Cyan));
    }

    #[test]
    fn draw_viewer_paints_lightgray_blue_not_core_white_red() {
        let path = std::env::temp_dir().join(format!(
            "rmc-viewer-default-colors-{}-{}.txt",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, "hello viewer\n").expect("write sample");
        let mut pal = McPalette::default();
        pal.core_default_fg = Color::White;
        pal.core_default_bg = Color::Red;
        let goto_prompt: Option<String> = None;
        let mut buf = Vec::new();
        {
            let mut p = crate::widgets::Painter { out: &mut buf };
            super::draw_viewer(
                &mut p,
                40,
                12,
                pal,
                &path,
                false,
                false,
                0,
                false,
                false,
                false,
                true,
                None,
                0,
                None,
                None,
                None,
                None,
                &goto_prompt,
                false,
            )
            .expect("draw viewer");
        }
        let _ = std::fs::remove_file(&path);
        let s = String::from_utf8_lossy(&buf);
        assert!(
            s.contains("hello viewer"),
            "viewer must show file text: {s:?}"
        );
        assert!(
            s.contains("\x1b[37;44m"),
            "viewer _default_ is lightgray;blue 37;44, got {s:?}"
        );
        assert!(
            !s.contains("\x1b[97;101m"),
            "viewer fill must not use [core] white;red: {s:?}"
        );
    }

    #[test]
    fn viewer_search_dialog_paints_gnu_4_8_33_radios_and_checks() {
        let path = std::env::temp_dir().join(format!(
            "rmc-viewer-search-dlg-{}-{}.txt",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, "hello\n").expect("write sample");
        let dlg = rmc_core::app::ViewerSearchDialog::from_last_search(b"");
        let goto_prompt: Option<String> = None;
        let mut buf = Vec::new();
        {
            let mut p = crate::widgets::Painter { out: &mut buf };
            super::draw_viewer(
                &mut p,
                80,
                24,
                McPalette::default(),
                &path,
                false,
                false,
                0,
                false,
                false,
                false,
                true,
                None,
                0,
                None,
                Some(&dlg),
                None,
                None,
                &goto_prompt,
                false,
            )
            .expect("draw viewer search");
        }
        let _ = std::fs::remove_file(&path);
        let s = String::from_utf8_lossy(&buf);
        for needle in [
            " Search ",
            "Enter search string:",
            "(*) Normal",
            "( ) Regular expression",
            "( ) Hexadecimal",
            "( ) Wildcard search",
            "[ ] Case sensitive",
            "[ ] Backwards",
            "[ ] Whole words",
            "[ ] All charsets",
        ] {
            assert!(
                s.contains(needle),
                "missing GNU wording {needle:?} in {s:?}"
            );
        }
        assert!(
            !s.contains("In selection"),
            "viewer Search must not invent editor-only In selection: {s:?}"
        );
        assert!(
            !s.contains("Across lines"),
            "viewer Search must not invent Across lines (not in GNU 4.8.33): {s:?}"
        );
        assert!(
            !s.contains("Find all"),
            "viewer Search must not invent editor Find all: {s:?}"
        );
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

#[cfg(test)]
mod gnu_default_chrome_colors_tests {
    use super::{panel_header_colors, panel_path_caption_colors};
    use crate::mc_colors::McPalette;
    use crate::skin::load_from_file;
    use crate::widgets::Painter;
    use crossterm::style::Color;
    use rmc_core::actions::PaneSide;
    use rmc_core::app::App;
    use rmc_core::config::KeyMap;
    use rmc_core::panel::{FileEntry, ListingFormat};
    use rmc_fs::local::LocalFs;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, SystemTime};

    #[test]
    fn active_path_uses_selected_inactive_uses_frame() {
        let pal = McPalette::default();
        let (afg, abg) = panel_path_caption_colors(&pal, true);
        assert_eq!((afg, abg), (pal.selected_fg, pal.selected_bg));
        assert_eq!((afg, abg), (Color::Black, Color::Cyan));
        let (ifg, ibg) = panel_path_caption_colors(&pal, false);
        assert_eq!((ifg, ibg), (pal.frame_fg, pal.frame_bg));
        assert_eq!(ibg, Color::Blue);
        assert_ne!(
            abg, ibg,
            "active path caption must differ from inactive (cyan vs blue frame)"
        );
        assert_ne!(
            afg,
            Color::White,
            "active path is black on cyan, not light-on-cyan"
        );
        assert_ne!(
            afg,
            Color::Grey,
            "active path must not use frame lightgray on cyan"
        );
    }

    #[test]
    fn column_headers_use_header_yellow_blue_not_cyan() {
        let pal = McPalette::default();
        let (fg, bg) = panel_header_colors(&pal);
        assert_eq!((fg, bg), (Color::Yellow, Color::Blue));
        assert_eq!((fg, bg), (pal.header_fg, pal.header_bg));
        assert_ne!(bg, pal.selected_bg, "headers must not use selected cyan");
        assert_ne!(bg, pal.menu_bg, "headers must not use menu cyan");
        assert_ne!(bg, pal.statusbar_bg, "headers must not use statusbar cyan");
    }

    #[test]
    fn default_ini_palette_matches_gnu_header_and_selected() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/skins/default.ini");
        let pal = load_from_file(&path).expect("load default.ini");
        assert_eq!(pal.header_fg, Color::Yellow);
        assert_eq!(pal.header_bg, Color::Blue);
        assert_eq!(pal.selected_fg, Color::Black);
        assert_eq!(pal.selected_bg, Color::Cyan);
        let (hfg, hbg) = panel_header_colors(&pal);
        assert_eq!((hfg, hbg), (Color::Yellow, Color::Blue));
        let (afg, abg) = panel_path_caption_colors(&pal, true);
        assert_eq!((afg, abg), (Color::Black, Color::Cyan));
        let (ifg, ibg) = panel_path_caption_colors(&pal, false);
        assert_eq!((ifg, ibg), (pal.frame_fg, pal.frame_bg));
    }

    fn ansi16(n: u8) -> Color {
        match n {
            0 => Color::Black,
            1 => Color::DarkRed,
            2 => Color::DarkGreen,
            3 => Color::DarkYellow,
            4 => Color::Blue,
            5 => Color::DarkMagenta,
            6 => Color::Cyan,
            7 => Color::Grey,
            8 => Color::DarkGrey,
            9 => Color::Red,
            10 => Color::Green,
            11 => Color::Yellow,
            12 => Color::Blue,
            13 => Color::Magenta,
            14 => Color::Cyan,
            15 => Color::White,
            other => Color::AnsiValue(other),
        }
    }

    fn ansi256(n: u8) -> Color {
        ansi16(n)
    }

    #[derive(Clone, Copy, Debug)]
    struct Cell {
        ch: char,
        fg: Color,
        bg: Color,
    }

    fn rasterize(bytes: &[u8], cols: u16, rows: u16) -> Vec<Vec<Cell>> {
        let mut grid = vec![
            vec![
                Cell {
                    ch: ' ',
                    fg: Color::Reset,
                    bg: Color::Reset,
                };
                cols as usize
            ];
            rows as usize
        ];
        let mut fg = Color::Reset;
        let mut bg = Color::Reset;
        let mut x: usize = 0;
        let mut y: usize = 0;
        let mut i = 0;
        let s = bytes;
        while i < s.len() {
            if s[i] == 0x1b && i + 1 < s.len() && s[i + 1] == b'[' {
                i += 2;
                let start = i;
                while i < s.len() && (s[i].is_ascii_digit() || s[i] == b';' || s[i] == b'?') {
                    i += 1;
                }
                if i >= s.len() {
                    break;
                }
                let cmd = s[i];
                let params = std::str::from_utf8(&s[start..i]).unwrap_or("");
                i += 1;
                match cmd {
                    b'H' | b'f' => {
                        let mut it = params.split(';');
                        let row = it.next().unwrap_or("1").parse::<usize>().unwrap_or(1);
                        let col = it.next().unwrap_or("1").parse::<usize>().unwrap_or(1);
                        y = row.saturating_sub(1);
                        x = col.saturating_sub(1);
                    }
                    b'm' => {
                        let nums: Vec<u8> = if params.is_empty() {
                            vec![0]
                        } else {
                            params.split(';').filter_map(|p| p.parse().ok()).collect()
                        };
                        let mut k = 0;
                        while k < nums.len() {
                            match nums[k] {
                                0 => {
                                    fg = Color::Reset;
                                    bg = Color::Reset;
                                }
                                n @ 30..=37 => fg = ansi16(n - 30),
                                n @ 40..=47 => bg = ansi16(n - 40),
                                n @ 90..=97 => fg = ansi16(n - 90 + 8),
                                n @ 100..=107 => bg = ansi16(n - 100 + 8),
                                39 => fg = Color::Reset,
                                49 => bg = Color::Reset,
                                38 if k + 2 < nums.len() && nums[k + 1] == 5 => {
                                    fg = ansi256(nums[k + 2]);
                                    k += 2;
                                }
                                48 if k + 2 < nums.len() && nums[k + 1] == 5 => {
                                    bg = ansi256(nums[k + 2]);
                                    k += 2;
                                }
                                _ => {}
                            }
                            k += 1;
                        }
                    }
                    _ => {}
                }
                continue;
            }
            let rest = &s[i..];
            let Ok(ch) = std::str::from_utf8(rest).map(|t| t.chars().next()) else {
                i += 1;
                continue;
            };
            let Some(ch) = ch else {
                break;
            };
            i += ch.len_utf8();
            if ch == '\n' {
                y += 1;
                x = 0;
                continue;
            }
            if y < rows as usize && x < cols as usize {
                grid[y][x] = Cell { ch, fg, bg };
            }
            x += 1;
        }
        grid
    }

    fn epoch_parent() -> FileEntry {
        FileEntry {
            name: "..".into(),
            path: PathBuf::from(".."),
            is_dir: true,
            is_symlink: false,
            symlink_target: None,
            is_exe: false,
            size: 0,
            modified: SystemTime::UNIX_EPOCH,
            accessed: SystemTime::UNIX_EPOCH,
            changed: SystemTime::UNIX_EPOCH,
            permissions: 0,
            owner: None,
            group: None,
            nlink: 1,
            inode: 0,
            uid: 0,
            gid: 0,
            is_stale_symlink: false,
        }
    }

    #[test]
    fn draw_panel_header_row_is_yellow_on_blue_not_selected_cyan() {
        let mut app = App::new(Box::new(LocalFs::new()), KeyMap::mc_defaults()).unwrap();
        app.active = PaneSide::Left;
        app.left.listing = ListingFormat::Full;
        app.right.listing = ListingFormat::Full;
        app.left.entries = vec![epoch_parent()];
        app.right.entries = vec![epoch_parent()];
        app.left.cursor = 0;
        app.right.cursor = 0;
        app.panel_opts.show_mini_status = true;

        let pal = McPalette::default();
        assert_eq!((pal.header_fg, pal.header_bg), (Color::Yellow, Color::Blue));
        assert_eq!(
            (pal.selected_fg, pal.selected_bg),
            (Color::Black, Color::Cyan)
        );

        let mut buf = Vec::new();
        let mut painter = Painter { out: &mut buf };
        super::draw_panel(&mut painter, 0, 0, 40, 12, true, &app, true, pal).unwrap();
        super::draw_panel(&mut painter, 40, 0, 40, 12, false, &app, false, pal).unwrap();

        let raw = String::from_utf8_lossy(&buf);
        let idx = raw.find("Name").expect("Name in stream");
        let around = &raw[idx.saturating_sub(24)..(idx + 8).min(raw.len())];
        assert!(
            around.contains("\x1b[93;44m"),
            "header SGR before Name must be yellow;blue 93;44, not reset/cyan: {around:?}"
        );
        assert!(
            !around.contains("\x1b[30;46m"),
            "header must not use selected black;cyan: {around:?}"
        );

        let grid = rasterize(&buf, 80, 12);
        // Sort indicator `.n` at the left; "Name" follows after a space (not a cyan bar).
        let sort_ind: String = grid[1][1..3].iter().map(|c| c.ch).collect();
        assert_eq!(sort_ind, ".n", "sort-indicator column, got {:?}", {
            grid[1].iter().map(|c| c.ch).collect::<String>()
        });
        let name: String = grid[1][8..12].iter().map(|c| c.ch).collect();
        assert_eq!(name, "Name", "header label, got {:?}", {
            grid[1].iter().map(|c| c.ch).collect::<String>()
        });
        for cell in grid[1][1..8].iter() {
            assert_eq!(
                (cell.fg, cell.bg),
                (Color::Yellow, Color::Blue),
                "header `.n Name` must be yellow;blue, not selected {:?}/{:?}",
                pal.selected_fg,
                pal.selected_bg
            );
            assert_ne!(cell.bg, pal.selected_bg, "header row must not be cyan");
            assert_ne!(
                cell.bg, pal.statusbar_bg,
                "header row must not be statusbar"
            );
        }
        // Rest of the left header inner row is filled header blue, not leftover cyan.
        for cell in &grid[1][1..39] {
            assert_eq!(
                cell.bg,
                Color::Blue,
                "header row fill must be blue, got {:?} at {:?}",
                cell.bg,
                cell.ch
            );
        }

        // Active path uses selected; inactive uses frame.
        let top: String = grid[0].iter().map(|c| c.ch).collect();
        assert!(top.contains('/'), "path caption on top frame: {top:?}");
        assert_eq!(grid[0][1].ch, '<', "left widget `<-`: {top:?}");
        let left_right_w: String = grid[0][34..39].iter().map(|c| c.ch).collect();
        assert_eq!(left_right_w, ".[^]>", "right widgets: {top:?}");
        assert_eq!(grid[0][41].ch, '<');
        let right_right_w: String = grid[0][74..79].iter().map(|c| c.ch).collect();
        assert_eq!(right_right_w, ".[^]>", "inactive panel widgets: {top:?}");
        let mut saw_active_path = false;
        let mut saw_inactive_path = false;
        for (x, cell) in grid[0].iter().enumerate() {
            if cell.ch == '/' || cell.ch.is_ascii_alphanumeric() || cell.ch == '-' {
                if x < 40 {
                    if (cell.fg, cell.bg) == (Color::Black, Color::Cyan) {
                        saw_active_path = true;
                    }
                } else if (cell.fg, cell.bg) == (pal.frame_fg, pal.frame_bg) {
                    saw_inactive_path = true;
                }
            }
        }
        assert!(
            saw_active_path,
            "active path must use selected black;cyan on row 0: {top:?}"
        );
        assert!(
            saw_inactive_path,
            "inactive path must stay on the blue frame: {top:?}"
        );

        // Mini-status for `..` is UP--DIR, not epoch-zero perms.
        let status: String = grid[10].iter().map(|c| c.ch).collect();
        assert!(status.contains("UP--DIR"), "parent mini-status: {status:?}");
        assert!(
            !status.contains("d---------") && !status.contains("Jan"),
            "parent mini-status must not be Unix-epoch zeros: {status:?}"
        );
    }

    fn symlink_entry(name: &str, target: &str, is_dir: bool) -> FileEntry {
        FileEntry {
            name: name.into(),
            path: PathBuf::from(name),
            is_dir,
            is_symlink: true,
            symlink_target: Some(target.into()),
            is_exe: false,
            size: 11,
            modified: SystemTime::UNIX_EPOCH,
            accessed: SystemTime::UNIX_EPOCH,
            changed: SystemTime::UNIX_EPOCH,
            permissions: 0o777,
            owner: Some("alice".into()),
            group: Some("staff".into()),
            nlink: 1,
            inode: 0,
            uid: 0,
            gid: 0,
            is_stale_symlink: false,
        }
    }

    #[test]
    fn draw_panel_mini_status_shows_symlink_target() {
        let mut app = App::new(Box::new(LocalFs::new()), KeyMap::mc_defaults()).unwrap();
        app.active = PaneSide::Left;
        app.left.listing = ListingFormat::Full;
        app.right.listing = ListingFormat::Full;
        app.left.entries = vec![
            epoch_parent(),
            symlink_entry("thelink", "readme.txt", false),
            symlink_entry("dirlink", "../other", true),
        ];
        app.right.entries = vec![epoch_parent()];
        app.left.cursor = 1;
        app.right.cursor = 0;
        app.panel_opts.show_mini_status = true;

        let pal = McPalette::default();
        let mut buf = Vec::new();
        let mut painter = Painter { out: &mut buf };
        super::draw_panel(&mut painter, 0, 0, 40, 12, true, &app, true, pal).unwrap();
        let grid = rasterize(&buf, 40, 12);
        let status: String = grid[10].iter().map(|c| c.ch).collect();
        assert!(
            status.contains("-> readme.txt"),
            "file symlink mini-status: {status:?}"
        );
        assert!(
            !status.contains("alice") && !status.contains("rwx"),
            "symlink mini-status is the target, not perms: {status:?}"
        );

        app.left.cursor = 2;
        let mut buf = Vec::new();
        let mut painter = Painter { out: &mut buf };
        super::draw_panel(&mut painter, 0, 0, 40, 12, true, &app, true, pal).unwrap();
        let grid = rasterize(&buf, 40, 12);
        let status: String = grid[10].iter().map(|c| c.ch).collect();
        assert!(
            status.contains("-> ../other"),
            "directory symlink mini-status: {status:?}"
        );

        app.left.cursor = 0;
        let mut buf = Vec::new();
        let mut painter = Painter { out: &mut buf };
        super::draw_panel(&mut painter, 0, 0, 40, 12, true, &app, true, pal).unwrap();
        let grid = rasterize(&buf, 40, 12);
        let status: String = grid[10].iter().map(|c| c.ch).collect();
        assert!(status.contains("UP--DIR"), "parent mini-status: {status:?}");

        app.left.cursor = 1;
        app.panel_opts.show_mini_status = false;
        let mut buf = Vec::new();
        let mut painter = Painter { out: &mut buf };
        super::draw_panel(&mut painter, 0, 0, 40, 12, true, &app, true, pal).unwrap();
        let grid = rasterize(&buf, 40, 12);
        let status: String = grid[10].iter().map(|c| c.ch).collect();
        assert!(
            !status.contains("-> readme.txt"),
            "Show mini-status off omits the symlink target row: {status:?}"
        );
    }

    #[test]
    fn menu_bar_is_white_on_cyan_until_f9_selects() {
        let pal = McPalette::default();
        let mut buf = Vec::new();
        {
            let mut painter = Painter { out: &mut buf };
            super::draw_menu_bar(&mut painter, 40, pal, None, false);
        }
        let grid = rasterize(&buf, 40, 1);
        let left: String = grid[0][0..6].iter().map(|c| c.ch).collect();
        assert_eq!(left, "  Left");
        for cell in &grid[0][0..6] {
            assert_eq!(
                (cell.fg, cell.bg),
                (Color::White, Color::Cyan),
                "idle menu is white;cyan, not reverse menusel"
            );
            assert_ne!(cell.bg, Color::Black);
        }
        for cell in &grid[0] {
            assert_eq!(
                cell.bg,
                Color::Cyan,
                "menu bar fill is cyan, got {:?}",
                cell
            );
        }

        let mut sel = Vec::new();
        {
            let mut painter = Painter { out: &mut sel };
            super::draw_menu_bar(&mut painter, 40, pal, Some(0), false);
        }
        let grid = rasterize(&sel, 40, 1);
        for cell in &grid[0][1..7] {
            assert_eq!(
                (cell.fg, cell.bg),
                (Color::White, Color::Black),
                "F9-selected Left is white;black"
            );
        }
        assert_eq!(grid[0][0].bg, Color::Cyan, "lead space stays menu cyan");
        assert_eq!(
            grid[0][7].bg,
            Color::Cyan,
            "unselected rest stays menu cyan"
        );
    }

    #[test]
    fn menu_bar_and_fbar_match_live_gnu_80col_cells() {
        let pal = McPalette::default();
        let mut menu = Vec::new();
        {
            let mut p = Painter { out: &mut menu };
            super::draw_menu_bar(&mut p, 80, pal, None, false);
        }
        let menu_row = row_str(&rasterize(&menu, 80, 1), 0);
        assert_eq!(
            menu_row.trim_end(),
            "  Left     File     Command     Options     Right"
        );

        let mut fbar = Vec::new();
        {
            let mut p = Painter { out: &mut fbar };
            super::draw_fbar(&mut p, 0, 80, pal);
        }
        let fbar_row = row_str(&rasterize(&fbar, 80, 1), 0);
        assert_eq!(
            fbar_row,
            " 1Help   2Menu   3View   4Edit   5Copy   6RenMov 7Mkdir  8Delete 9PullDn10Quit  "
        );
        assert_eq!(super::fbar_slot_bounds(80)[0], (1, 9));
        assert_eq!(super::fbar_slot_bounds(80)[8], (65, 72));
        assert_eq!(super::fbar_slot_bounds(80)[9], (72, 80));
    }

    #[test]
    fn full_listing_40col_header_matches_live_gnu() {
        let app = panel_app(ListingFormat::Full);
        let pal = McPalette::default();
        let mut buf = Vec::new();
        {
            let mut p = Painter { out: &mut buf };
            super::draw_panel(&mut p, 0, 0, 40, 12, true, &app, true, pal).unwrap();
        }
        let header = row_str(&rasterize(&buf, 40, 12), 1);
        assert_eq!(header, "│.n     Name      │ Size  │Modify time │");
        let top = row_str(&rasterize(&buf, 40, 12), 0);
        assert!(
            !top.contains('┬'),
            "live GNU top frame has no ┬ under a short path: {top:?}"
        );
        assert!(
            top.starts_with("┌<─ /tmp "),
            "path left-aligned after <─: {top:?}"
        );
        assert!(top.contains(".[^]>"), "{top:?}");
    }

    fn row_str(grid: &[Vec<Cell>], y: usize) -> String {
        grid[y].iter().map(|c| c.ch).collect()
    }

    fn inner_bars(grid: &[Vec<Cell>], y: usize, x0: usize, w: usize) -> Vec<usize> {
        (x0 + 1..x0 + w - 1)
            .filter(|&x| grid[y][x].ch == '│')
            .collect()
    }

    fn epoch_file(name: &str, size: u64) -> FileEntry {
        FileEntry {
            name: name.into(),
            path: PathBuf::from(name),
            is_dir: false,
            is_symlink: false,
            symlink_target: None,
            is_exe: false,
            size,
            modified: SystemTime::UNIX_EPOCH,
            accessed: SystemTime::UNIX_EPOCH,
            changed: SystemTime::UNIX_EPOCH,
            permissions: 0o644,
            owner: None,
            group: None,
            nlink: 1,
            inode: 0,
            uid: 0,
            gid: 0,
            is_stale_symlink: false,
        }
    }

    fn panel_app(listing: ListingFormat) -> App {
        let mut app = App::new(Box::new(LocalFs::new()), KeyMap::mc_defaults()).unwrap();
        app.active = PaneSide::Left;
        app.left.listing = listing;
        app.right.listing = listing;
        app.left.entries = vec![epoch_parent(), epoch_file("readme.txt", 42)];
        app.right.entries = vec![epoch_parent()];
        app.left.cursor = 1;
        app.right.cursor = 0;
        app.left.cwd = PathBuf::from("/tmp");
        app.right.cwd = PathBuf::from("/tmp");
        app.panel_opts.show_mini_status = true;
        app.layout.show_free_space = false;
        app
    }

    #[test]
    fn format_entry_name_keeps_gnu_type_cell_including_space() {
        let parent = epoch_parent();
        assert_eq!(super::format_entry_name(&parent), "/..");
        let mut dir = epoch_file("docs", 4096);
        dir.is_dir = true;
        assert_eq!(super::format_entry_name(&dir), "/docs");
        let mut exe = epoch_file("run.sh", 128);
        exe.is_exe = true;
        assert_eq!(super::format_entry_name(&exe), "*run.sh");
        let mut link = epoch_file("alias", 7);
        link.is_symlink = true;
        assert_eq!(super::format_entry_name(&link), "@alias");
        let file = epoch_file("Cargo.lock", 72688);
        assert_eq!(super::format_entry_name(&file), " Cargo.lock");
        assert_eq!(
            super::format_entry_name(&epoch_file("Cargo.toml", 871)),
            " Cargo.toml"
        );
        assert_eq!(
            super::format_entry_name(&epoch_file(".gitignore", 684)),
            " .gitignore"
        );
        let mut crates = epoch_file("crates", 4096);
        crates.is_dir = true;
        assert_eq!(super::format_entry_name(&crates), "/crates");
        let mut color_sh = epoch_file("run-mcr-color.sh", 128);
        color_sh.is_exe = true;
        assert_eq!(super::format_entry_name(&color_sh), "*run-mcr-color.sh");
        let mut sock = epoch_file("sock", 0);
        sock.permissions = 0o140_000 | 0o666;
        assert_eq!(super::format_entry_name(&sock), "=sock");
        let mut fifo = epoch_file("pipe", 0);
        fifo.permissions = 0o010_000 | 0o666;
        assert_eq!(super::format_entry_name(&fifo), "|pipe");
        let mut chr = epoch_file("tty", 0);
        chr.permissions = 0o020_000 | 0o666;
        assert_eq!(super::format_entry_name(&chr), "-tty");
        let mut blk = epoch_file("sda", 0);
        blk.permissions = 0o060_000 | 0o666;
        assert_eq!(super::format_entry_name(&blk), "+sda");
        let mut stale = epoch_file("gone", 1);
        stale.is_symlink = true;
        stale.is_stale_symlink = true;
        assert_eq!(super::format_entry_name(&stale), "!gone");
        let mut linkdir = epoch_file("linked-dir", 1);
        linkdir.is_dir = true;
        linkdir.is_symlink = true;
        assert_eq!(super::format_entry_name(&linkdir), "~linked-dir");
    }

    #[test]
    fn user_listing_paints_format_tokens_and_mark() {
        let mut app = panel_app(ListingFormat::User);
        app.left.user_format = "type name mark | size | bsize perm mode".into();
        app.left.selection.select(1);
        {
            let ent = &mut app.left.entries[1];
            ent.is_exe = true;
            ent.permissions = 0o755;
        }

        let pal = McPalette::default();
        let mut buf = Vec::new();
        let mut painter = Painter { out: &mut buf };
        super::draw_panel(&mut painter, 0, 0, 80, 12, true, &app, true, pal).unwrap();
        let grid = rasterize(&buf, 80, 12);

        let header = row_str(&grid, 1);
        assert!(header.contains("Name"), "header={header:?}");
        assert!(header.contains("Size"), "header={header:?}");
        assert!(header.contains("Perm"), "header={header:?}");
        assert!(header.contains("Mode"), "header={header:?}");

        let parent = row_str(&grid, 2);
        assert!(parent.contains("UP--DIR"), "parent size/bsize={parent:?}");

        let file = row_str(&grid, 3);
        assert!(file.contains("readme.txt"), "file={file:?}");
        assert!(
            file.contains('*'),
            "exe type and/or mark asterisk: {file:?}"
        );
        assert!(file.contains("42"), "size={file:?}");
        assert!(file.contains("0755"), "octal mode={file:?}");
        assert!(file.contains("rwx"), "perm={file:?}");
        assert!(file.contains('|'), "column gap={file:?}");
    }

    #[test]
    fn full_listing_cols_pack_size_and_mtime_on_the_right() {
        let c = super::full_listing_cols(0, 40);
        assert_eq!(c.time_x, 27, "mtime stays at x+w-13 (12-char field)");
        assert_eq!(c.time_bar, 26);
        assert_eq!(c.size_x, 19);
        assert_eq!(c.size_bar, 18);
        assert!(c.size_bar > 1, "bar is inside the frame");
        assert!(c.size_bar < c.size_x);
        assert!(
            c.size_x + 6 < c.time_bar,
            "7-char size fits before time bar"
        );
        let bars = super::full_listing_bar_xs(c, 0, 40);
        assert_eq!(bars, vec![18, 26]);
    }

    #[test]
    fn full_listing_draws_box_column_bars_not_ascii_pipe() {
        let app = panel_app(ListingFormat::Full);
        let pal = McPalette::default();
        let mut buf = Vec::new();
        let mut painter = Painter { out: &mut buf };
        super::draw_panel(&mut painter, 0, 0, 40, 12, true, &app, true, pal).unwrap();

        let grid = rasterize(&buf, 40, 12);
        let header = row_str(&grid, 1);
        assert!(header.contains("Name"), "header={header:?}");
        assert!(header.contains("Size"), "header={header:?}");
        assert!(header.contains("Modify time"), "header={header:?}");
        assert!(
            !header.contains('|'),
            "must not use ASCII pipe in Full header: {header:?}"
        );

        let name_at = header.find("Name").expect("Name");
        let size_at = header.find("Size").expect("Size");
        let time_at = header.find("Modify time").expect("Modify time");
        assert!(
            name_at < size_at && size_at < time_at,
            "Name | Size | Modify time order: {header:?}"
        );
        let between_name_size: String = header[name_at + 4..size_at].chars().collect();
        let between_size_time: String = header[size_at + 4..time_at].chars().collect();
        assert!(
            between_name_size.contains('│'),
            "│ between Name and Size: {header:?}"
        );
        assert!(
            between_size_time.contains('│'),
            "│ between Size and Modify time: {header:?}"
        );
        assert!(
            !between_name_size.contains('|') && !between_size_time.contains('|'),
            "splits must be U+2502 not ASCII: {header:?}"
        );

        let bars = inner_bars(&grid, 1, 0, 40);
        assert_eq!(bars.len(), 2, "exactly two Full splits: {header:?}");
        for &bx in &bars {
            assert_eq!(grid[1][bx].ch, '│');
            assert_ne!(grid[1][bx].ch, '|');
            assert_eq!(
                (grid[1][bx].fg, grid[1][bx].bg),
                (Color::Yellow, Color::Blue),
                "header bar is yellow;blue"
            );
            assert_eq!(
                grid[0][bx].ch,
                '─',
                "live GNU top frame is ─ (path/fill covers ┬) at {bx}: {}",
                row_str(&grid, 0)
            );
            // Live GNU 4.8.30 mini-status split is solid ─ (no ┴).
            assert_eq!(
                grid[9][bx].ch,
                '─',
                "mini-status split is solid ─ at {bx}: {}",
                row_str(&grid, 9)
            );
            assert_eq!(
                grid[11][bx].ch, '─',
                "bottom frame stays ─ when mini-status is on"
            );
            // Listing rows including the selected file row (cursor=1 → y=3).
            assert_eq!(grid[2][bx].ch, '│', "parent row bar");
            assert_eq!(grid[3][bx].ch, '│', "selected row must still show │");
            assert_eq!(
                grid[3][bx].bg,
                Color::Cyan,
                "selected-row │ sits on selected cyan, not a space"
            );
            assert_eq!(grid[3][bx].fg, pal.frame_fg);
            // Empty listing rows keep the bar (frame pair).
            assert_eq!(grid[4][bx].ch, '│');
            assert_eq!(grid[4][bx].bg, pal.frame_bg);
            // Mini-status is full-width, not three cells.
            assert_ne!(grid[10][bx].ch, '│');
            assert_ne!(grid[10][bx].ch, '┬');
            assert_ne!(grid[10][bx].ch, '┴');
        }
        let status = row_str(&grid, 10);
        assert!(
            status.contains("rw-r--r--") || status.contains("42"),
            "mini-status is the current-entry line, not three column cells: {status:?}"
        );
        assert_eq!(grid[0][0].ch, '┌');
        assert_eq!(grid[0][39].ch, '┐');
        assert_eq!(grid[9][0].ch, '├');
        assert_eq!(grid[9][39].ch, '┤');
        assert_eq!(grid[11][0].ch, '└');
        assert_eq!(grid[11][39].ch, '┘');
        assert_eq!(grid[1][0].ch, '│');
        assert_eq!(grid[1][39].ch, '│');
        let split = row_str(&grid, 9);
        assert!(
            split.starts_with('├') && split.ends_with('┤'),
            "GNU mini-status separator: {split:?}"
        );
    }

    #[test]
    fn brief_listing_paints_bar_in_packed_column_gap() {
        let mut app = panel_app(ListingFormat::Brief);
        app.left.brief_columns = 2;
        let pal = McPalette::default();
        let mut buf = Vec::new();
        let mut painter = Painter { out: &mut buf };
        super::draw_panel(&mut painter, 0, 0, 40, 12, true, &app, true, pal).unwrap();
        let grid = rasterize(&buf, 40, 12);
        let expected = super::brief_column_bar_xs(0, 40, 2);
        assert_eq!(expected.len(), 1, "default Brief is two name columns");
        let bx = expected[0] as usize;
        assert_eq!(
            grid[1][bx].ch,
            '│',
            "Brief header gap: {}",
            row_str(&grid, 1)
        );
        assert_ne!(grid[1][bx].ch, '|');
        assert_eq!(
            grid[0][bx].ch,
            '─',
            "Brief top frame fill is ─, not ┬: {}",
            row_str(&grid, 0)
        );
        assert_eq!(
            grid[9][bx].ch, '─',
            "Brief mini-status split is solid ─, not bottom ┴"
        );
        assert_eq!(grid[9][0].ch, '├');
        assert_eq!(grid[9][39].ch, '┤');
        assert_eq!(grid[11][bx].ch, '─');
        assert_eq!(grid[2][bx].ch, '│');
        let header = row_str(&grid, 1);
        assert!(header.contains("Name"), "{header:?}");
        assert!(!header.contains('|'), "no ASCII pipe: {header:?}");
    }

    #[test]
    fn long_listing_does_not_insert_column_bars() {
        let app = panel_app(ListingFormat::Long);
        let pal = McPalette::default();
        let mut buf = Vec::new();
        let mut painter = Painter { out: &mut buf };
        super::draw_panel(&mut painter, 0, 0, 40, 12, true, &app, true, pal).unwrap();
        let grid = rasterize(&buf, 40, 12);
        let top = row_str(&grid, 0);
        let header = row_str(&grid, 1);
        assert!(
            !(1..39).any(|x| grid[0][x].ch == '┬'),
            "Long has no ┬ junctions: {top:?}"
        );
        assert!(
            inner_bars(&grid, 1, 0, 40).is_empty(),
            "Long header is spaces not bars: {header:?}"
        );
        assert!(header.contains("Perms"), "{header:?}");
        assert!(!header.contains('|'), "{header:?}");
        assert_ne!(
            grid[9][0].ch,
            '├',
            "Long does not grow a column-bar split-line: {}",
            row_str(&grid, 9)
        );
    }

    #[test]
    fn full_listing_mini_status_off_puts_tee_on_bottom_frame() {
        let mut app = panel_app(ListingFormat::Full);
        app.panel_opts.show_mini_status = false;
        let pal = McPalette::default();
        let mut buf = Vec::new();
        let mut painter = Painter { out: &mut buf };
        super::draw_panel(&mut painter, 0, 0, 40, 12, true, &app, true, pal).unwrap();
        let grid = rasterize(&buf, 40, 12);
        let bars = inner_bars(&grid, 1, 0, 40);
        assert_eq!(bars.len(), 2);
        for &bx in &bars {
            assert_eq!(
                grid[0][bx].ch,
                '─',
                "top frame fill is ─ even with mini-status off: {}",
                row_str(&grid, 0)
            );
            assert_eq!(
                grid[11][bx].ch,
                '┴',
                "mini-status off: ┴ on bottom frame: {}",
                row_str(&grid, 11)
            );
            assert_eq!(grid[2][bx].ch, '│');
        }
        assert_eq!(grid[11][0].ch, '└');
        assert_eq!(grid[11][39].ch, '┘');
        assert_ne!(grid[9][0].ch, '├');
    }

    #[test]
    fn full_listing_gnu_type_size_mtime_widgets_and_free_space() {
        let mut app = panel_app(ListingFormat::Full);
        app.layout.show_free_space = true;
        let mtime = SystemTime::UNIX_EPOCH + Duration::from_secs(1_698_220_980); // not epoch
        let mut parent = epoch_parent();
        parent.modified = mtime;
        parent.size = 4096;
        let mut dir = epoch_file("docs", 4096);
        dir.is_dir = true;
        dir.permissions = 0o755;
        let mut exe = epoch_file("run.sh", 128);
        exe.is_exe = true;
        exe.permissions = 0o755;
        let mut link = epoch_file("alias", 7);
        link.is_symlink = true;
        let file = epoch_file("Cargo.lock", 72688);
        app.left.entries = vec![parent.clone(), dir, exe, link, file];
        app.left.cursor = 0;
        app.right.entries = vec![parent];
        app.right.cursor = 0;

        let pal = McPalette::default();
        let mut buf = Vec::new();
        let mut painter = Painter { out: &mut buf };
        super::draw_panel(&mut painter, 0, 0, 40, 12, true, &app, true, pal).unwrap();
        let grid = rasterize(&buf, 40, 12);
        let header = row_str(&grid, 1);
        let top = row_str(&grid, 0);
        let parent_row = row_str(&grid, 2);
        let dir_row = row_str(&grid, 3);
        let exe_row = row_str(&grid, 4);
        let link_row = row_str(&grid, 5);
        let file_row = row_str(&grid, 6);
        let bottom = row_str(&grid, 11);

        assert!(header.contains(".n"), "sort indicator: {header:?}");
        assert!(
            header.contains("Name") && header.contains("Size") && header.contains("Modify time")
        );
        assert!(
            header.find(".n").unwrap() < header.find("Name").unwrap(),
            ".n is left of Name: {header:?}"
        );
        for cell in &grid[1][1..39] {
            assert_ne!(
                cell.bg,
                Color::Cyan,
                "header is not a solid cyan bar: {header:?}"
            );
            assert_eq!(cell.bg, Color::Blue);
        }

        assert_eq!(grid[0][1].ch, '<', "left widget: {top:?}");
        assert!(top.contains(".[^]>"), "right widgets: {top:?}");

        assert!(parent_row.contains(".."), "{parent_row:?}");
        let parent_type_name: String = grid[2][1..4].iter().map(|c| c.ch).collect();
        assert_eq!(
            parent_type_name, "/..",
            "GNU parent Full listing is `/..`: {parent_row:?}"
        );
        assert!(
            parent_row.contains("UP--DIR"),
            "only `..` is UP--DIR: {parent_row:?}"
        );
        assert!(
            !parent_row.contains("Jan  1 00:00") && !parent_row.contains("Jan  1"),
            "parent mtime must not be Unix epoch: {parent_row:?}"
        );
        let parent_time = super::format_time(&app.left.entries[0]);
        assert!(!parent_time.is_empty());
        assert!(
            parent_row.contains(&parent_time),
            "{parent_row:?} vs {parent_time:?}"
        );

        assert!(dir_row.contains("/docs"), "dir type prefix: {dir_row:?}");
        assert_eq!(grid[3][1].ch, '/', "dir type cell: {dir_row:?}");
        assert!(dir_row.contains("4096"), "dir inode size: {dir_row:?}");
        assert!(
            !dir_row.contains("UP--DIR"),
            "dirs are not UP--DIR: {dir_row:?}"
        );

        assert!(exe_row.contains("*run.sh"), "exe type prefix: {exe_row:?}");
        assert_eq!(grid[4][1].ch, '*', "exe type cell: {exe_row:?}");
        assert!(
            link_row.contains("@alias"),
            "symlink type prefix: {link_row:?}"
        );
        assert_eq!(grid[5][1].ch, '@', "symlink type cell: {link_row:?}");
        assert!(file_row.contains("Cargo.lock"), "{file_row:?}");
        assert_eq!(
            grid[6][1].ch, ' ',
            "regular file type cell is a space, not omitted: {file_row:?}"
        );
        assert_eq!(
            grid[6][2].ch, 'C',
            "Cargo.lock starts after the type cell: {file_row:?}"
        );
        assert_eq!(grid[3][2].ch, 'd', "docs starts after `/`: {dir_row:?}");
        let docs_x = dir_row.find("/docs").expect("/docs in listing row");
        let lock_x = file_row
            .find("Cargo.lock")
            .expect("Cargo.lock in listing row");
        assert_eq!(
            lock_x,
            docs_x + 1,
            "Cargo.lock lines up one cell right of /docs (type cell): docs={docs_x} lock={lock_x} dir={dir_row:?} file={file_row:?}"
        );
        assert!(
            file_row.contains("72688"),
            "raw byte size, not human 70K: {file_row:?}"
        );
        assert!(!file_row.contains("70K") && !file_row.contains("71K"));

        let cwd = &app.left.cwd;
        let expect_free = matches!(
            (fs2::available_space(cwd), fs2::total_space(cwd)),
            (Ok(_), Ok(_))
        );
        if expect_free {
            assert!(
                bottom.contains(" / ") && bottom.contains('%'),
                "free-space in bottom frame: {bottom:?}"
            );
        }
        assert_eq!(bottom.chars().next(), Some('└'));
        assert_eq!(bottom.chars().last(), Some('┘'));
        assert_eq!(
            inner_bars(&grid, 1, 0, 40).len(),
            2,
            "still two Full `|` bars"
        );
    }

    #[test]
    fn full_listing_regular_file_type_cell_is_space_aligned_with_dir() {
        // GNU mc 4.8.33 Full listing at /workspace/rmc: `/crates` vs `Cargo.toml`.
        // Name-field cell 0 is the `type` mark; the filename starts in cell 1,
        // so `C` of Cargo.toml lines up with `c` of /crates (mc(1) `half type name`).
        let mut app = panel_app(ListingFormat::Full);
        let mut crates = epoch_file("crates", 4096);
        crates.is_dir = true;
        crates.permissions = 0o755;
        let toml = epoch_file("Cargo.toml", 871);
        let mut exe = epoch_file("run-mcr-color.sh", 128);
        exe.is_exe = true;
        exe.permissions = 0o755;
        let gitignore = epoch_file(".gitignore", 684);
        app.left.entries = vec![crates, toml, exe, gitignore];
        app.left.cursor = 0;
        app.left.scroll_top = 0;

        let pal = McPalette::default();
        let mut buf = Vec::new();
        let mut painter = Painter { out: &mut buf };
        super::draw_panel(&mut painter, 0, 0, 40, 12, true, &app, true, pal).unwrap();
        let grid = rasterize(&buf, 40, 12);
        let name0 = 1usize; // first inner cell after the frame (GNU type cell)
        let crates_y = 2usize;
        let toml_y = 3usize;
        let exe_y = 4usize;
        let git_y = 5usize;
        let crates_row = row_str(&grid, crates_y);
        let toml_row = row_str(&grid, toml_y);
        let exe_row = row_str(&grid, exe_y);
        let git_row = row_str(&grid, git_y);

        assert_eq!(
            grid[crates_y][name0].ch, '/',
            "/crates type: {crates_row:?}"
        );
        assert_eq!(
            grid[crates_y][name0 + 1].ch,
            'c',
            "letter after `/`: {crates_row:?}"
        );
        assert_eq!(
            grid[toml_y][name0].ch, ' ',
            "regular file first name-field cell is space: {toml_row:?}"
        );
        assert_eq!(
            grid[toml_y][name0 + 1].ch,
            'C',
            "Cargo.toml starts in the next cell: {toml_row:?}"
        );
        assert_eq!(grid[exe_y][name0].ch, '*', "exe type cell: {exe_row:?}");
        assert_eq!(grid[exe_y][name0 + 1].ch, 'r', "{exe_row:?}");
        assert_eq!(
            grid[git_y][name0].ch, ' ',
            ".gitignore keeps the type-cell space: {git_row:?}"
        );
        assert_eq!(grid[git_y][name0 + 1].ch, '.', "{git_row:?}");
        assert!(
            row_str(&grid, 1).contains(".n"),
            "do not drop `.n` sort indicator from Full listing chrome"
        );
    }

    fn find_text(grid: &[Vec<Cell>], needle: &str) -> (usize, usize) {
        let nchars: Vec<char> = needle.chars().collect();
        for (y, row) in grid.iter().enumerate() {
            if nchars.len() > row.len() {
                continue;
            }
            for x in 0..=row.len() - nchars.len() {
                if row[x..x + nchars.len()]
                    .iter()
                    .zip(nchars.iter())
                    .all(|(cell, ch)| cell.ch == *ch)
                {
                    return (x, y);
                }
            }
        }
        panic!(
            "missing {needle:?} in {}",
            grid.iter()
                .map(|row| row.iter().map(|c| c.ch).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    fn assert_span(grid: &[Vec<Cell>], x: usize, y: usize, text: &str, fg: Color, bg: Color) {
        for (i, ch) in text.chars().enumerate() {
            let cell = &grid[y][x + i];
            assert_eq!(cell.ch, ch, "glyph at ({},{})", x + i, y);
            assert_eq!(
                (cell.fg, cell.bg),
                (fg, bg),
                "pair for {ch:?} at ({},{})",
                x + i,
                y
            );
        }
    }

    #[test]
    fn dialog_chrome_is_black_on_lightgray_title_blue_focus_cyan() {
        let pal = McPalette::default();
        assert_eq!(
            (pal.dialog_default_fg, pal.dialog_default_bg),
            (Color::Black, Color::Grey)
        );
        assert_eq!((pal.dfocus_fg, pal.dfocus_bg), (Color::Black, Color::Cyan));
        assert_eq!((pal.dtitle_fg, pal.dtitle_bg), (Color::Blue, Color::Grey));

        let mut buf = Vec::new();
        {
            let mut p = Painter { out: &mut buf };
            super::draw_dialog_box(
                &mut p,
                80,
                24,
                pal,
                "Confirmation",
                "Delete this file?",
                &["< OK >", "Cancel"],
                false,
            );
        }
        let grid = rasterize(&buf, 80, 24);
        let w = 60usize;
        let h = 7usize;
        let x = (80 - w) / 2;
        let y = (24 - h) / 2;
        assert_eq!(grid[y][x].ch, '┌');
        assert_eq!(grid[y][x + w - 1].ch, '┐');
        assert_eq!(grid[y + h - 1][x].ch, '└');
        assert_eq!(grid[y + h - 1][x + w - 1].ch, '┘');
        for cell in [grid[y][x], grid[y][x + 1], grid[y + 1][x]] {
            assert_eq!(
                (cell.fg, cell.bg),
                (Color::Black, Color::Grey),
                "dialog chrome is black;lightgray, not panel frame {:?}/{:?}",
                pal.frame_fg,
                pal.frame_bg
            );
            assert_ne!(cell.fg, pal.frame_fg);
            assert_ne!(cell.bg, Color::Blue);
        }
        assert_eq!(
            (grid[y + 1][x + 2].fg, grid[y + 1][x + 2].bg),
            (Color::Black, Color::Grey),
            "dialog interior fill is lightgray, not leftover panel blue"
        );

        let (tx, ty) = find_text(&grid, " Confirmation ");
        assert_eq!(ty, y);
        assert_span(&grid, tx, ty, " Confirmation ", Color::Blue, Color::Grey);

        let (mx, my) = find_text(&grid, "Delete this file?");
        assert_span(
            &grid,
            mx,
            my,
            "Delete this file?",
            Color::Black,
            Color::Grey,
        );

        let (ox, oy) = find_text(&grid, "< OK >");
        assert_span(&grid, ox, oy, "< OK >", Color::Black, Color::Cyan);
        let (cx, cy) = find_text(&grid, "Cancel");
        assert_eq!(cy, oy);
        assert_span(&grid, cx, cy, "Cancel", Color::Black, Color::Grey);
        assert_ne!(
            grid[cy][cx].bg,
            Color::Cyan,
            "unfocused button must not use buttonbar cyan"
        );
    }

    #[test]
    fn error_box_is_white_on_red_with_black_on_lightgray_focus() {
        let pal = McPalette::default();
        assert_eq!(
            (pal.error_default_fg, pal.error_default_bg),
            (Color::White, Color::Red)
        );
        assert_eq!(
            (pal.errdfocus_fg, pal.errdfocus_bg),
            (Color::Black, Color::Grey)
        );

        let mut buf = Vec::new();
        {
            let mut p = Painter { out: &mut buf };
            super::draw_error_dialog(&mut p, 80, 24, pal, "Cannot operate on \"..\"!", false);
        }
        let grid = rasterize(&buf, 80, 24);
        let (tx, ty) = find_text(&grid, " Error ");
        let x = super::gnu_dialog_left(80, 27) as usize;
        let y = super::gnu_dialog_top(24, 5) as usize;
        assert_eq!(x, 27, "GNU Error x on 80-col");
        assert_eq!(y, 7, "GNU Error y on 24-row");
        assert_eq!(ty, y);
        let title: String = grid[y][x..x + 27].iter().map(|c| c.ch).collect();
        assert_eq!(title, "┌───────── Error ─────────┐");
        assert!(
            (0..80).any(|x| grid[ty][x].ch == '┌'),
            "compact error frame on the title row"
        );
        assert_eq!(
            (grid[ty][tx].fg, grid[ty][tx].bg),
            (Color::White, Color::Red),
            "error title is white;red, not dialog lightgray"
        );
        assert_ne!(grid[ty][tx].bg, pal.dialog_default_bg);
        let (mx, my) = find_text(&grid, "Cannot operate on \"..\"!");
        assert_span(
            &grid,
            mx,
            my,
            "Cannot operate on \"..\"!",
            Color::White,
            Color::Red,
        );
        let all: String = grid.iter().flatten().map(|c| c.ch).collect();
        assert!(
            !all.contains("Cancel"),
            "live GNU parent-copy error has no Cancel: {all:?}"
        );
        assert!(
            !all.contains("< OK >"),
            "live GNU parent-copy error has no OK button: {all:?}"
        );
    }

    #[test]
    fn gnu_delete_dialog_cells_match_live_480() {
        let pal = McPalette::default();
        let mut buf = Vec::new();
        {
            let mut p = Painter { out: &mut buf };
            super::draw_delete_dialog(&mut p, 80, 24, pal, "notes.txt", true, false);
        }
        let grid = rasterize(&buf, 80, 24);
        let w = 21usize;
        let h = 6usize;
        let x = super::gnu_dialog_left(80, 21) as usize;
        let y = super::gnu_dialog_top(24, 6) as usize;
        assert_eq!(x, 30, "GNU Delete x on 80-col");
        assert_eq!(y, 7, "GNU Delete y on 24-row");
        assert_eq!(grid[y][x].ch, '┌');
        assert_eq!(grid[y][x + w - 1].ch, '┐');
        assert_eq!(grid[y + h - 1][x].ch, '└');
        let title_row: String = grid[y][x..x + w].iter().map(|c| c.ch).collect();
        assert_eq!(title_row, "┌───── Delete ──────┐");
        let prompt: String = grid[y + 1][x..x + w].iter().map(|c| c.ch).collect();
        assert_eq!(prompt, "│   Delete file     │");
        let quoted: String = grid[y + 2][x..x + w].iter().map(|c| c.ch).collect();
        assert_eq!(quoted, "│   \"notes.txt\"?    │");
        let body: String = grid.iter().flatten().map(|c| c.ch).collect();
        assert!(body.contains("[ Yes ]"), "{body:?}");
        assert!(body.contains("[ No ]"), "{body:?}");
        let bar: String = grid[y + 3].iter().map(|c| c.ch).collect();
        assert!(
            bar.contains('├') && bar.contains('┤'),
            "section bar: {bar:?}"
        );
    }

    #[test]
    fn gnu_mkdir_dialog_cells_match_live_480() {
        let pal = McPalette::default();
        let mut buf = Vec::new();
        {
            let mut p = Painter { out: &mut buf };
            super::draw_mkdir_dialog(
                &mut p,
                80,
                24,
                pal,
                "",
                rmc_core::app::MkdirDialogFocus::Input,
                false,
            );
        }
        let grid = rasterize(&buf, 80, 24);
        let w = 38usize;
        let h = 6usize;
        let x = super::gnu_dialog_left(80, 38) as usize;
        let y = super::gnu_dialog_top(24, 6) as usize;
        assert_eq!(x, 21, "GNU Mkdir x on 80-col");
        assert_eq!(grid[y][x].ch, '┌');
        assert_eq!(grid[y][x + w - 1].ch, '┐');
        let title: String = grid[y][x..x + w].iter().map(|c| c.ch).collect();
        assert_eq!(title, "┌────── Create a new Directory ──────┐");
        let prompt: String = grid[y + 1][x..x + w].iter().map(|c| c.ch).collect();
        assert!(prompt.contains("Enter directory name:"), "{prompt:?}");
        let btns: String = grid[y + h - 2][x..x + w].iter().map(|c| c.ch).collect();
        assert!(
            btns.contains("[< OK >] [ Cancel ]"),
            "one-space GNU buttons: {btns:?}"
        );
        assert!(
            !btns.contains("[< Cancel >]"),
            "Cancel is unmarked on open: {btns:?}"
        );
        let field: String = grid[y + 2][x..x + w].iter().map(|c| c.ch).collect();
        assert_eq!(
            (grid[y + 2][x + 2].fg, grid[y + 2][x + 2].bg),
            (pal.dfocus_fg, pal.dfocus_bg),
            "input stays dfocus on open: {field:?}"
        );
        let (ok_x, btn_y) = find_text(&grid, "[< OK >]");
        assert_eq!(btn_y, y + h - 2);
        assert_eq!(
            (grid[btn_y][ok_x].fg, grid[btn_y][ok_x].bg),
            (pal.dialog_default_fg, pal.dialog_default_bg),
            "OK is default geometry but not dfocus-marked on open"
        );
        let (ca_x, _) = find_text(&grid, "[ Cancel ]");
        assert_eq!(
            (grid[btn_y][ca_x].fg, grid[btn_y][ca_x].bg),
            (pal.dialog_default_fg, pal.dialog_default_bg),
            "Cancel is not marked/focused on open"
        );
        let bar: String = grid[y + 3].iter().map(|c| c.ch).collect();
        assert!(
            bar.contains('├') && bar.contains('┤'),
            "section bar: {bar:?}"
        );
    }

    #[test]
    fn gnu_mkdir_cancel_focus_keeps_ok_default_brackets() {
        let pal = McPalette::default();
        let mut buf = Vec::new();
        {
            let mut p = Painter { out: &mut buf };
            super::draw_mkdir_dialog(
                &mut p,
                80,
                24,
                pal,
                "",
                rmc_core::app::MkdirDialogFocus::Cancel,
                false,
            );
        }
        let grid = rasterize(&buf, 80, 24);
        let body: String = grid.iter().flatten().map(|c| c.ch).collect();
        assert!(
            body.contains("[< OK >] [ Cancel ]"),
            "Cancel focus does not steal default brackets: {body:?}"
        );
        assert!(!body.contains("[< Cancel >]"), "{body:?}");
        let (ok_x, btn_y) = find_text(&grid, "[< OK >]");
        assert_eq!(
            (grid[btn_y][ok_x].fg, grid[btn_y][ok_x].bg),
            (pal.dialog_default_fg, pal.dialog_default_bg)
        );
        let (ca_x, _) = find_text(&grid, "[ Cancel ]");
        assert_eq!(
            (grid[btn_y][ca_x].fg, grid[btn_y][ca_x].bg),
            (pal.dfocus_fg, pal.dfocus_bg),
            "Cancel focus is dfocus color"
        );
    }

    #[test]
    fn menu_dropdown_hotkeys_are_yellow_selected_hotkey_yellow_on_black() {
        let pal = McPalette::default();
        assert_eq!((pal.menu_fg, pal.menu_bg), (Color::White, Color::Cyan));
        assert_eq!(
            (pal.menusel_fg, pal.menusel_bg),
            (Color::White, Color::Black)
        );
        assert_eq!(
            (pal.menuhot_fg, pal.menuhot_bg),
            (Color::Yellow, Color::Cyan)
        );
        assert_eq!(
            (pal.menuhotsel_fg, pal.menuhotsel_bg),
            (Color::Yellow, Color::Black)
        );

        let mut buf = Vec::new();
        {
            let mut p = Painter { out: &mut buf };
            super::draw_menu_dropdown(&mut p, pal, 1, 0, false);
        }
        let grid = rasterize(&buf, 40, 20);
        let x = super::dropdown_origin_x(1, false) as usize;
        assert_eq!(grid[1][x].ch, '┌');
        assert_eq!(
            (grid[1][x].fg, grid[1][x].bg),
            (Color::White, Color::Cyan),
            "dropdown frame is menu white;cyan"
        );

        let (hx, hy) = find_text(&grid, " View");
        assert_eq!(hy, 2);
        assert_eq!(grid[hy][hx].ch, ' ');
        assert_eq!(
            (grid[hy][hx].fg, grid[hy][hx].bg),
            (Color::White, Color::Black)
        );
        assert_eq!(grid[hy][hx + 1].ch, 'V');
        assert_eq!(
            (grid[hy][hx + 1].fg, grid[hy][hx + 1].bg),
            (Color::Yellow, Color::Black),
            "selected hotkey is yellow;black"
        );
        assert_eq!(grid[hy][hx + 2].ch, 'i');
        assert_eq!(
            (grid[hy][hx + 2].fg, grid[hy][hx + 2].bg),
            (Color::White, Color::Black)
        );

        let (fx, fy) = find_text(&grid, " Filtered view");
        assert_eq!(grid[fy][fx].ch, ' ');
        assert_eq!(
            (grid[fy][fx].fg, grid[fy][fx].bg),
            (Color::White, Color::Cyan)
        );
        assert_eq!(grid[fy][fx + 1].ch, 'F');
        assert_eq!(
            (grid[fy][fx + 1].fg, grid[fy][fx + 1].bg),
            (Color::Yellow, Color::Cyan),
            "idle hotkey is yellow;cyan"
        );
        assert_eq!(grid[fy][fx + 2].ch, 'i');
        assert_eq!(
            (grid[fy][fx + 2].fg, grid[fy][fx + 2].bg),
            (Color::White, Color::Cyan)
        );
    }

    #[test]
    fn file_menu_matches_gnu_4_8_30_item_and_shortcut_columns() {
        let pal = McPalette::default();
        let mut buf = Vec::new();
        {
            let mut p = Painter { out: &mut buf };
            super::draw_menu_dropdown(&mut p, pal, 1, 0, false);
        }
        let grid = rasterize(&buf, 80, 30);
        let x = super::dropdown_origin_x(1, false) as usize;
        assert_eq!(x, 9, "live GNU File drop-down starts at column 9");
        assert_eq!(super::FILE_MENU_BOX_WIDTH, 28);
        assert_eq!(grid[1][x].ch, '┌');
        assert_eq!(grid[1][x + 27].ch, '┐');

        let expected = [
            " View              F3     ",
            " View file...             ",
            " Filtered view     M-!    ",
            " Edit              F4     ",
            " Copy              F5     ",
            " Chmod             C-x c  ",
            " Link              C-x l  ",
            " Symlink           C-x s  ",
            " Relative symlink  C-x v  ",
            " Edit symlink      C-x C-s",
            " Chown             C-x o  ",
            " Advanced chown           ",
            " Chattr            C-x e  ",
            " Rename/Move       F6     ",
            " Mkdir             F7     ",
            " Delete            F8     ",
            " Quick cd          M-c    ",
            "──────────────────────────",
            " Select group      +      ",
            " Unselect group    -      ",
            " Invert selection  *      ",
            "──────────────────────────",
            " Exit              F10    ",
        ];
        assert_eq!(expected.len(), super::FILE_MENU.len());
        for (i, want) in expected.iter().enumerate() {
            let y = 2 + i;
            let got: String = grid[y][x + 1..x + 27].iter().map(|c| c.ch).collect();
            assert_eq!(got, *want, "File menu row {i}");
            if want.chars().all(|c| c == '─') {
                assert_eq!(grid[y][x].ch, '├');
                assert_eq!(grid[y][x + 27].ch, '┤');
            }
        }
        assert_eq!(grid[2 + expected.len()][x].ch, '└');

        // GNU `&` hotkeys: Vie&w file..., C&hmod, E&xit — not the first letter.
        let (wx, wy) = find_text(&grid, "View file...");
        assert_eq!(grid[wy][wx + 3].ch, 'w');
        assert_eq!(
            (grid[wy][wx + 3].fg, grid[wy][wx + 3].bg),
            (Color::Yellow, Color::Cyan)
        );
        let (hx, hy) = find_text(&grid, "Chmod");
        assert_eq!(grid[hy][hx + 1].ch, 'h');
        assert_eq!(
            (grid[hy][hx + 1].fg, grid[hy][hx + 1].bg),
            (Color::Yellow, Color::Cyan)
        );
        let (xx, xy) = find_text(&grid, "Exit");
        assert_eq!(grid[xy][xx + 1].ch, 'x');
        assert_eq!(
            (grid[xy][xx + 1].fg, grid[xy][xx + 1].bg),
            (Color::Yellow, Color::Cyan)
        );
    }

    fn assert_dropdown_rows(top_index: usize, origin: usize, width: u16, expected: &[&str]) {
        let pal = McPalette::default();
        let mut buf = Vec::new();
        {
            let mut p = Painter { out: &mut buf };
            super::draw_menu_dropdown(&mut p, pal, top_index, 0, false);
        }
        let grid = rasterize(&buf, 80, 40);
        let x = super::dropdown_origin_x(top_index, false) as usize;
        assert_eq!(x, origin, "drop-down {top_index} origin");
        assert_eq!(grid[1][x].ch, '┌');
        assert_eq!(grid[1][x + width as usize - 1].ch, '┐');
        let inner = width as usize - 2;
        for (i, want) in expected.iter().enumerate() {
            let y = 2 + i;
            let got: String = grid[y][x + 1..x + 1 + inner].iter().map(|c| c.ch).collect();
            assert_eq!(got, *want, "menu {top_index} row {i}");
            if want.chars().all(|c| c == '─') {
                assert_eq!(grid[y][x].ch, '├');
                assert_eq!(grid[y][x + width as usize - 1].ch, '┤');
            }
        }
        assert_eq!(grid[2 + expected.len()][x].ch, '└');
    }

    #[test]
    fn left_menu_matches_gnu_4_8_30_item_and_shortcut_columns() {
        assert_eq!(super::LEFT_RIGHT_MENU_BOX_WIDTH, 27);
        assert_eq!(super::dropdown_origin_x(0, false), 0);
        let expected = [
            " File listing            ",
            " Quick view         C-x q",
            " Info               C-x i",
            " Tree                    ",
            "─────────────────────────",
            " Listing format...       ",
            " Sort order...           ",
            " Filter...               ",
            "─────────────────────────",
            " FTP link...             ",
            " Shell link...           ",
            " SFTP link...            ",
            "─────────────────────────",
            " Rescan             C-r  ",
        ];
        assert_dropdown_rows(0, 0, 27, &expected);
        let pal = McPalette::default();
        let mut buf = Vec::new();
        {
            let mut p = Painter { out: &mut buf };
            super::draw_menu_dropdown(&mut p, pal, 0, 0, false);
        }
        let grid = rasterize(&buf, 80, 40);
        let (gx, gy) = find_text(&grid, "File listing");
        assert_eq!(grid[gy][gx + 11].ch, 'g');
        assert_eq!(
            (grid[gy][gx + 11].fg, grid[gy][gx + 11].bg),
            (Color::Yellow, Color::Black),
            "selected File listing hotkey g is yellow;black"
        );
        let (qx, qy) = find_text(&grid, "Quick view");
        assert_eq!(grid[qy][qx].ch, 'Q');
        assert_eq!(
            (grid[qy][qx].fg, grid[qy][qx].bg),
            (Color::Yellow, Color::Cyan),
            "idle Quick view hotkey Q is yellow;cyan"
        );
        let (px, py) = find_text(&grid, "FTP link...");
        assert_eq!(grid[py][px + 2].ch, 'P');
        assert_eq!(
            (grid[py][px + 2].fg, grid[py][px + 2].bg),
            (Color::Yellow, Color::Cyan)
        );
    }

    #[test]
    fn command_menu_matches_gnu_4_8_30_item_and_shortcut_columns() {
        assert_eq!(super::COMMAND_MENU_BOX_WIDTH, 41);
        assert_eq!(super::dropdown_origin_x(2, false), 18);
        let expected = [
            " User menu                      F2     ",
            " Directory tree                        ",
            " Find file                      M-?    ",
            " Swap panels                    C-u    ",
            " Switch panels on/off           C-o    ",
            " Compare directories            C-x d  ",
            " Compare files                  C-x C-d",
            " External panelize              C-x !  ",
            "───────────────────────────────────────",
            " Command history                M-h    ",
            " Directory hotlist              C-\\    ",
            " Background jobs                C-x j  ",
            " Screen list                    M-`    ",
            "───────────────────────────────────────",
            " Edit extension file                   ",
            " Edit menu file                        ",
            " Edit highlighting group file          ",
        ];
        assert_dropdown_rows(2, 18, 41, &expected);
        let pal = McPalette::default();
        let mut buf = Vec::new();
        {
            let mut p = Painter { out: &mut buf };
            super::draw_menu_dropdown(&mut p, pal, 2, 0, false);
        }
        let grid = rasterize(&buf, 80, 40);
        let (wx, wy) = find_text(&grid, "Swap panels");
        assert_eq!(grid[wy][wx + 1].ch, 'w');
        assert_eq!(
            (grid[wy][wx + 1].fg, grid[wy][wx + 1].bg),
            (Color::Yellow, Color::Cyan)
        );
    }

    #[test]
    fn options_menu_matches_gnu_4_8_30_item_and_shortcut_columns() {
        assert_eq!(super::OPTIONS_MENU_BOX_WIDTH, 21);
        assert_eq!(super::dropdown_origin_x(3, false), 30);
        let expected = [
            " Configuration...  ",
            " Layout...         ",
            " Panel options...  ",
            " Confirmation...   ",
            " Appearance...     ",
            " Learn keys...     ",
            " Virtual FS...     ",
            "───────────────────",
            " Save setup        ",
        ];
        assert_dropdown_rows(3, 30, 21, &expected);
        let pal = McPalette::default();
        let mut buf = Vec::new();
        {
            let mut p = Painter { out: &mut buf };
            super::draw_menu_dropdown(&mut p, pal, 3, 0, false);
        }
        let grid = rasterize(&buf, 80, 30);
        let (ox, oy) = find_text(&grid, "Confirmation...");
        assert_eq!(grid[oy][ox + 1].ch, 'o');
        assert_eq!(
            (grid[oy][ox + 1].fg, grid[oy][ox + 1].bg),
            (Color::Yellow, Color::Cyan)
        );
        let (kx, ky) = find_text(&grid, "Learn keys...");
        assert_eq!(grid[ky][kx + 6].ch, 'k');
        assert_eq!(
            (grid[ky][kx + 6].fg, grid[ky][kx + 6].bg),
            (Color::Yellow, Color::Cyan)
        );
    }

    #[test]
    fn right_menu_origin_and_width_match_left_chrome() {
        assert_eq!(super::dropdown_origin_x(4, false), 42);
        assert_eq!(super::dropdown_box_size(4), (27, 16));
        assert_eq!(super::dropdown_box_size(0), (27, 16));
    }

    fn paint_editor(pal: McPalette, buf: &rmc_edit::EditorBuffer, cols: u16, rows: u16) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut p = Painter { out: &mut out };
            super::draw_editor(
                &mut p, cols, rows, pal, buf, None, None, None, None, None, None, None, None, None,
                None, false,
            );
        }
        out
    }

    #[test]
    fn editor_chrome_is_lightgray_on_blue_editbold_yellow_green_editmarked_black_cyan() {
        let pal = McPalette::default();
        assert_eq!(
            (pal.edit_normal_fg, pal.edit_normal_bg),
            (Color::Grey, Color::Blue)
        );
        assert_eq!(
            (pal.edit_bold_fg, pal.edit_bold_bg),
            (Color::Yellow, Color::Green)
        );
        assert_eq!(
            (pal.edit_marked_fg, pal.edit_marked_bg),
            (Color::Black, Color::Cyan)
        );
        assert_ne!(
            pal.edit_marked_bg, pal.marked_bg,
            "editmarked is not panel marked yellow;blue"
        );

        let mut buf = rmc_edit::EditorBuffer::from_bytes(b"fn hello", Some(PathBuf::from("a.rs")));
        // Cursor past the identifier so invert does not cover tokens.
        buf.col = 8;
        let out = paint_editor(pal, &buf, 40, 10);
        let raw = String::from_utf8_lossy(&out);
        assert!(
            raw.contains("\x1b[37;44m"),
            "editor default is lightgray;blue 37;44: {raw:?}"
        );
        assert!(
            raw.contains("\x1b[93;102m"),
            "editbold is yellow;green 93;102: {raw:?}"
        );

        let grid = rasterize(&out, 40, 10);
        let (fx, fy) = find_text(&grid, "fn");
        assert_eq!(fy, 1, "content starts under the row-0 status line");
        assert_span(&grid, fx, fy, "fn", Color::Yellow, Color::Green);
        let (hx, hy) = find_text(&grid, "hello");
        assert_eq!(hy, fy);
        assert_span(&grid, hx, hy, "hello", Color::Grey, Color::Blue);
        assert_eq!(
            (grid[fy][20].fg, grid[fy][20].bg),
            (Color::Grey, Color::Blue),
            "content padding stays editor default, not leftover panel cyan"
        );
        assert_ne!(grid[fy][20].bg, pal.selected_bg);

        buf.col = 0;
        buf.mark_start();
        buf.col = 2;
        buf.mark_end();
        buf.col = 8;
        let out = paint_editor(pal, &buf, 40, 10);
        let raw = String::from_utf8_lossy(&out);
        assert!(
            raw.contains("\x1b[30;46m"),
            "editmarked is black;cyan 30;46: {raw:?}"
        );
        let grid = rasterize(&out, 40, 10);
        let (fx, fy) = find_text(&grid, "fn");
        assert_span(&grid, fx, fy, "fn", Color::Black, Color::Cyan);
        let (hx, hy) = find_text(&grid, "hello");
        assert_span(&grid, hx, hy, "hello", Color::Grey, Color::Blue);
        assert_ne!(
            (grid[fy][fx].fg, grid[fy][fx].bg),
            (pal.edit_bold_fg, pal.edit_bold_bg),
            "selection wins over editbold"
        );
    }

    #[test]
    fn editor_pairs_come_from_skin_not_core_or_panel_selected() {
        let mut pal = McPalette::default();
        pal.edit_normal_fg = Color::White;
        pal.edit_normal_bg = Color::Red;
        pal.edit_bold_fg = Color::Black;
        pal.edit_bold_bg = Color::Yellow;
        pal.edit_marked_fg = Color::Green;
        pal.edit_marked_bg = Color::Magenta;

        let mut buf = rmc_edit::EditorBuffer::from_bytes(b"fn hello", Some(PathBuf::from("a.rs")));
        buf.col = 8;
        let grid = rasterize(&paint_editor(pal, &buf, 40, 10), 40, 10);
        let (fx, fy) = find_text(&grid, "fn");
        assert_span(&grid, fx, fy, "fn", Color::Black, Color::Yellow);
        let (hx, hy) = find_text(&grid, "hello");
        assert_span(&grid, hx, hy, "hello", Color::White, Color::Red);
        assert_ne!(
            grid[hy][hx].bg, pal.core_default_bg,
            "plain text must not fall back to core lightgray;blue"
        );
        assert_ne!(grid[fy][fx].bg, pal.selected_bg);

        buf.col = 0;
        buf.mark_start();
        buf.col = 2;
        buf.mark_end();
        buf.col = 8;
        let grid = rasterize(&paint_editor(pal, &buf, 40, 10), 40, 10);
        let (fx, fy) = find_text(&grid, "fn");
        assert_span(&grid, fx, fy, "fn", Color::Green, Color::Magenta);
        assert_ne!(grid[fy][fx].bg, pal.marked_bg);
    }

    fn paint_viewer(path: &Path, cols: u16, rows: u16, hex: bool) -> Vec<u8> {
        let pal = McPalette::default();
        let goto_prompt: Option<String> = None;
        let mut out = Vec::new();
        {
            let mut p = Painter { out: &mut out };
            super::draw_viewer(
                &mut p,
                cols,
                rows,
                pal,
                path,
                hex,
                false,
                0,
                false,
                false,
                false,
                true,
                None,
                0,
                None,
                None,
                None,
                None,
                &goto_prompt,
                false,
            )
            .expect("draw viewer");
        }
        out
    }

    #[test]
    fn viewer_is_frameless_with_gnu_row0_status() {
        let dir = std::path::Path::new("/tmp/mcr-fixture");
        let _ = std::fs::create_dir_all(dir);
        let path = dir.join("notes.txt");
        std::fs::write(&path, "hello from notes\n").expect("write notes");
        let grid = rasterize(&paint_viewer(&path, 80, 24, false), 80, 24);
        let row0: String = grid[0].iter().map(|c| c.ch).collect();
        assert_eq!(
            row0,
            "/tmp/mcr-fixture/notes.txt                             17/17                100%"
        );
        assert_eq!(
            (grid[0][0].fg, grid[0][0].bg),
            (Color::Black, Color::Cyan),
            "viewer status is statusbar black;cyan"
        );
        assert_ne!(grid[0][0].ch, '┌', "GNU mcview has no box frame");
        assert_eq!(grid[1][0].ch, 'h', "content starts at col 0 under status");
        let row1: String = grid[1].iter().map(|c| c.ch).collect();
        assert!(row1.starts_with("hello from notes"));
        assert_eq!(grid[1][79].ch, ' ', "no right-hand frame");
        let fbar: String = grid[23].iter().map(|c| c.ch).collect();
        assert!(
            fbar.contains("1Help"),
            "F-bar stays on the last row: {fbar:?}"
        );
        assert!(!fbar.contains("┌"));
    }

    #[test]
    fn viewer_hex_row0_uses_offset_not_bytes_counter() {
        let dir = std::path::Path::new("/tmp/mcr-fixture");
        let _ = std::fs::create_dir_all(dir);
        let path = dir.join("notes.txt");
        std::fs::write(&path, "hello from notes\n").expect("write notes");
        let grid = rasterize(&paint_viewer(&path, 80, 24, true), 80, 24);
        let row0: String = grid[0].iter().map(|c| c.ch).collect();
        assert_eq!(
            row0,
            "/tmp/mcr-fixture/notes.txt                      0x00000000                    0%"
        );
    }

    #[test]
    fn editor_idle_is_frameless_gnu_row0_status_not_menu() {
        let buf = rmc_edit::EditorBuffer::from_bytes(
            b"hello from notes\n",
            Some(PathBuf::from("/tmp/mcr-fixture/notes.txt")),
        );
        let pal = McPalette::default();
        let grid = rasterize(&paint_editor(pal, &buf, 80, 24), 80, 24);
        let row0: String = grid[0].iter().map(|c| c.ch).collect();
        assert_eq!(
            row0,
            "/tmp/mcr~otes.txt   [----]  0 L:[  1+ 0   1/  2] *(0   /  17b) 0104 0x068 [*][X]"
        );
        assert_eq!((grid[0][0].fg, grid[0][0].bg), (Color::Black, Color::Cyan));
        assert!(
            !row0.contains("File"),
            "idle editor must not show the F9 menu bar"
        );
        let row1: String = grid[1].iter().map(|c| c.ch).collect();
        assert!(row1.starts_with("hello from notes"));
        let fbar: String = grid[23].iter().map(|c| c.ch).collect();
        assert!(fbar.contains("1Help") && fbar.contains("2Save"));
    }

    #[test]
    fn editor_f9_replaces_row0_with_menu_bar() {
        let buf = rmc_edit::EditorBuffer::from_bytes(
            b"hello from notes\n",
            Some(PathBuf::from("/tmp/mcr-fixture/notes.txt")),
        );
        let pal = McPalette::default();
        let mut out = Vec::new();
        {
            let mut p = Painter { out: &mut out };
            super::draw_editor(
                &mut p,
                80,
                24,
                pal,
                &buf,
                Some(rmc_core::app::EditorMenu::default_open()),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                false,
            );
        }
        let grid = rasterize(&out, 80, 24);
        let row0: String = grid[0].iter().map(|c| c.ch).collect();
        assert!(
            row0.contains("File"),
            "F9 paints the menu on row 0: {row0:?}"
        );
        assert!(
            !row0.contains("[----]"),
            "menu replaces the status line while open"
        );
    }
}

/// F5 / File→Copy / F-bar `5Copy` paint the GNU Copy dialog. Padding used
/// `width - truncated.len()` (UTF-8 bytes). `truncate` may append `…` (3
/// bytes) or keep a unicode path whose byte length exceeds the field, so
/// `str::repeat` underflowed and aborted the process.
#[cfg(test)]
mod copy_dialog_paint_tests {
    use super::{draw_copy_move_dialog, draw_overlays};
    use crate::mc_colors::McPalette;
    use crate::widgets::Painter;
    use rmc_core::app::{App, CopyDialogFocus, UiMode};
    use rmc_core::config::KeyMap;
    use rmc_fs::local::LocalFs;

    #[test]
    fn gnu_mc_433_copy_dialog_fields_are_present() {
        // Live GNU mc 4.8.33 F5: title Copy; header with quoted name and
        // source mask; `*`; Using shell patterns; `to:`; Follow links /
        // Dive / Preserve attributes / Stable symlinks; OK / Background / Cancel.
        let buf = paint_copy("*", "/workspace/", 80, 24);
        let s = String::from_utf8_lossy(&buf);
        assert!(s.contains("Copy"), "{s:?}");
        assert!(
            s.contains("Copy file \"notes.txt\" with source mask:"),
            "GNU header, got {s:?}"
        );
        assert!(
            s.contains("[< OK >]"),
            "GNU default button stays [< OK >] while dest is focused, got {s:?}"
        );
        assert!(s.contains("Using shell patterns"), "{s:?}");
        assert!(s.contains("to:"), "{s:?}");
        assert!(s.contains("/workspace/"), "{s:?}");
        assert!(s.contains("Follow links"), "{s:?}");
        assert!(s.contains("Dive into subdir if exists"), "{s:?}");
        assert!(s.contains("Preserve attributes"), "{s:?}");
        assert!(s.contains("Stable symlinks"), "{s:?}");
        assert!(
            s.contains("OK") && s.contains("Background") && s.contains("Cancel"),
            "{s:?}"
        );
    }

    fn paint_copy(mask: &str, to: &str, cols: u16, rows: u16) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut p = Painter { out: &mut buf };
            draw_copy_move_dialog(
                &mut p,
                cols,
                rows,
                McPalette::default(),
                "Copy",
                "notes.txt",
                "file",
                mask,
                to,
                true,
                false,
                true,
                false,
                false,
                CopyDialogFocus::To,
                true,
            );
        }
        buf
    }

    #[test]
    fn copy_dialog_long_ascii_dest_does_not_panic() {
        // Field width is min(cols, 74) - 8 = 66 on an 80-col screen.
        let dest = format!("{}notes.txt", "/very/long/path/segment".repeat(8));
        assert!(
            dest.len() > 66,
            "precondition: dest must exceed the Copy 'to' field"
        );
        let buf = paint_copy("*", &dest, 80, 24);
        let s = String::from_utf8_lossy(&buf);
        assert!(s.contains("Copy"), "GNU Copy dialog title");
        assert!(s.contains("to:"), "GNU dest prompt");
        assert!(s.contains('…'), "long dest is truncated, not a panic");
    }

    #[test]
    fn copy_dialog_unicode_dest_shorter_in_chars_than_bytes_does_not_panic() {
        // 40 CJK chars / 120 UTF-8 bytes: old pad used .len() and underflowed
        // even though chars().count() fits in the 66-column field.
        let dest = "文件".repeat(20);
        assert!(dest.chars().count() < 66);
        assert!(dest.len() > 66);
        let buf = paint_copy("*", &dest, 80, 24);
        assert!(!buf.is_empty());
    }

    #[test]
    fn copy_dialog_fits_on_short_terminal() {
        let dest = format!("{}notes.txt", "/very/long/path/segment".repeat(8));
        let buf = paint_copy("*", &dest, 80, 10);
        assert!(!buf.is_empty(), "rows < dialog height must not abort");
    }

    #[test]
    fn copy_dialog_fits_on_narrow_terminal() {
        let dest = format!("{}notes.txt", "/very/long/path/segment".repeat(8));
        let buf = paint_copy("*", &dest, 8, 8);
        assert!(!buf.is_empty(), "cols < dialog width must not abort");
        let tiny = paint_copy("*", &dest, 3, 5);
        assert!(!tiny.is_empty(), "cols < 4 / rows < 15 must not abort");
    }

    #[test]
    fn error_dialog_shows_gnu_same_file_last_line() {
        let vfs = LocalFs::new();
        let mut app = App::new(Box::new(vfs), KeyMap::mc_defaults()).unwrap();
        let src = std::path::Path::new("/tmp/mcr-live/fixture/alpha/notes.txt");
        app.ui_mode = UiMode::DialogConfirm {
            title: "Error".into(),
            message: rmc_core::filemask::same_path_error_message(src, src, false),
            on_ok: Box::new(|_| Ok(())),
        };
        let mut buf = Vec::new();
        {
            let mut p = Painter { out: &mut buf };
            draw_overlays(&mut p, &app, 80, 24, McPalette::default()).unwrap();
        }
        let s = String::from_utf8_lossy(&buf);
        assert!(s.contains("Error"), "Error title must paint, got {s:?}");
        assert!(
            s.contains("are the same file"),
            "GNU same-file last line must be visible, got {s:?}"
        );
        assert!(
            s.contains("/tmp/mcr-live/fixture/alpha/notes.txt"),
            "quoted path must paint, got {s:?}"
        );
    }

    #[test]
    fn error_dialog_for_parent_copy_fits_on_small_terminal() {
        let vfs = LocalFs::new();
        let mut app = App::new(Box::new(vfs), KeyMap::mc_defaults()).unwrap();
        app.ui_mode = UiMode::DialogConfirm {
            title: "Error".into(),
            message: r#"Cannot operate on ".."!"#.into(),
            on_ok: Box::new(|_| Ok(())),
        };
        let mut buf = Vec::new();
        {
            let mut p = Painter { out: &mut buf };
            draw_overlays(&mut p, &app, 20, 6, McPalette::default()).unwrap();
        }
        let s = String::from_utf8_lossy(&buf);
        assert!(s.contains("Error"), "Error title must paint, got {s:?}");
        assert!(
            s.contains("Cannot") || s.contains(".."),
            "GNU parent-dir message must paint (possibly truncated): {s:?}"
        );
    }

    #[test]
    fn multiline_same_file_error_stays_on_dialog_box_not_compact() {
        // Sibling #166 wraps this body in `draw_dialog_box`. The compact
        // no-button `..` Error must not swallow newline messages.
        let vfs = LocalFs::new();
        let mut app = App::new(Box::new(vfs), KeyMap::mc_defaults()).unwrap();
        app.ui_mode = UiMode::DialogConfirm {
            title: "Error".into(),
            message: rmc_core::filemask::same_path_error_message(
                std::path::Path::new("/tmp/mcr-live/fixture/alpha/notes.txt"),
                std::path::Path::new("/tmp/mcr-live/fixture/alpha/notes.txt"),
                false,
            ),
            on_ok: Box::new(|_| Ok(())),
        };
        let mut buf = Vec::new();
        {
            let mut p = Painter { out: &mut buf };
            draw_overlays(&mut p, &app, 80, 24, McPalette::default()).unwrap();
        }
        let s = String::from_utf8_lossy(&buf);
        assert!(s.contains("Error"), "{s:?}");
        assert!(
            s.contains("< OK >") && s.contains("Cancel"),
            "multi-line Error keeps draw_dialog_box buttons, got {s:?}"
        );
    }

    #[test]
    fn overlay_paint_of_copy_dialog_with_long_dest_does_not_panic() {
        let dest = format!("{}notes.txt", "/very/long/path/segment".repeat(8));
        let vfs = LocalFs::new();
        let mut app = App::new(Box::new(vfs), KeyMap::mc_defaults()).unwrap();
        app.ui_mode = UiMode::CopyDialog {
            title: "Copy".into(),
            src_name: "notes.txt".into(),
            src_path: dest.clone().into(),
            src_paths: vec![dest.clone().into()],
            mask: "*".into(),
            to: dest,
            using_shell_patterns: true,
            follow_links: false,
            preserve_attrs: true,
            dive_into_subdir: false,
            stable_symlinks: false,
            focus: CopyDialogFocus::To,
        };
        let mut buf = Vec::new();
        {
            let mut p = Painter { out: &mut buf };
            draw_overlays(&mut p, &app, 80, 24, McPalette::default()).unwrap();
        }
        let s = String::from_utf8_lossy(&buf);
        assert!(s.contains("Copy"));
        assert!(s.contains("to:"));
    }
}
