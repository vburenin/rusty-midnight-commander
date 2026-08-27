//! GNU mc(1) Quick view / Info panel helpers. Pure enough for GHA (no TTY).

use rmc_core::app::App;
use rmc_core::panel::FileEntry;
use std::path::Path;
use time::OffsetDateTime;

/// Listing panel that feeds a Quick view / Info panel (`preview_is_left`).
pub(crate) fn preview_source_panel(
    app: &App,
    preview_is_left: bool,
) -> &rmc_core::panel::PanelState {
    if preview_is_left {
        &app.right
    } else {
        &app.left
    }
}

/// Currently selected entry shown in a Quick view / Info panel.
pub(crate) fn preview_source_entry(app: &App, preview_is_left: bool) -> Option<&FileEntry> {
    preview_source_panel(app, preview_is_left).current_entry()
}

/// Short directory placeholder (no listing dump). `None` for regular files.
pub(crate) fn quick_view_directory_line(ent: &FileEntry) -> Option<String> {
    if ent.is_dir {
        Some(format!("Directory: {}", ent.name))
    } else {
        None
    }
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

/// Filesystem free/total when local `stat` allows.
pub(crate) fn filesystem_free_line(path: &Path) -> Option<String> {
    match (fs2::available_space(path), fs2::total_space(path)) {
        (Ok(avail), Ok(total)) => Some(format!(
            "Free: {} / {}",
            human_bytes(avail),
            human_bytes(total)
        )),
        _ => None,
    }
}

/// Info panel facts for the currently selected file. No TTY.
pub(crate) fn info_lines_for_entry(
    ent: &FileEntry,
    si: bool,
    show_free_space: bool,
) -> Vec<String> {
    let perms = perm_string(ent.permissions, ent.is_dir);
    let owner = ent.owner.as_deref().unwrap_or("-");
    let group = ent.group.as_deref().unwrap_or("-");
    let size_s = rmc_core::panel::format_byte_size(ent.size, si);
    let tm: OffsetDateTime = ent.modified.into();
    let ts = tm
        .format(&time::macros::format_description!(
            "[year]-[month repr:numerical]-[day] [hour]:[minute]"
        ))
        .unwrap_or_default();
    let mut lines = vec![
        format!("Name: {}", ent.name),
        format!("Path: {}", ent.path.display()),
        format!("Type: {}", if ent.is_dir { "Directory" } else { "File" }),
        format!("Size: {size_s}"),
        format!("Owner: {owner}  Group: {group}"),
        format!("Perms: {perms}"),
        format!("Modified: {ts}"),
        format!("Links: {}", ent.nlink),
    ];
    if ent.inode != 0 {
        lines.push(format!("Inode: {}", ent.inode));
    }
    if show_free_space {
        if let Some(free) = filesystem_free_line(&ent.path) {
            lines.push(free);
        }
    }
    lines
}

/// Info lines for a Quick view / Info panel, sourced from the other panel.
pub(crate) fn info_lines_for_panel(app: &App, preview_is_left: bool) -> Vec<String> {
    let Some(ent) = preview_source_entry(app, preview_is_left) else {
        return Vec::new();
    };
    info_lines_for_entry(ent, app.panel_opts.kilobyte_si, app.layout.show_free_space)
}
