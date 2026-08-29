//! GNU mc(1) Quick view / Info panel helpers. Pure enough for GHA (no TTY).

use rmc_core::app::App;
use rmc_core::panel::FileEntry;
use std::path::Path;

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
/// Paint uses the GNU "Cannot view" line; tests still assert this helper.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn quick_view_directory_line(ent: &FileEntry) -> Option<String> {
    if ent.is_dir {
        Some(format!("Directory: {}", ent.name))
    } else {
        None
    }
}

fn perm_string(ent: &FileEntry) -> String {
    let mut s = String::new();
    s.push(if ent.is_symlink {
        'l'
    } else if ent.is_dir {
        'd'
    } else {
        '-'
    });
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
        s.push(if ent.permissions & bit != 0 { ch } else { '-' });
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

fn listing_ts(ts: std::time::SystemTime) -> String {
    rmc_core::panel::format_listing_time(ts)
}

fn info_value_line(label: &str, value: &str) -> String {
    // Live GNU 4.8.30: two-space indent, values start at inner column 14.
    let mut line = format!("  {label}:");
    while line.chars().count() < 14 {
        line.push(' ');
    }
    line.push_str(value);
    line
}

fn hex_dev_ino(dev: u64, ino: u64) -> String {
    format!("{dev:X}h:{ino:X}h")
}

#[cfg(unix)]
fn unix_stat_bits(path: &Path) -> Option<(u64, u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let md = std::fs::symlink_metadata(path).ok()?;
    Some((md.dev(), md.ino(), md.blocks()))
}

#[cfg(not(unix))]
fn unix_stat_bits(_path: &Path) -> Option<(u64, u64, u64)> {
    None
}

#[cfg(unix)]
fn statvfs_free_nodes(path: &Path) -> Option<(u64, u64)> {
    use std::ffi::CString;
    let c = CString::new(path.to_string_lossy().as_bytes()).ok()?;
    let mut buf = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    let rc = unsafe { libc::statvfs(c.as_ptr(), buf.as_mut_ptr()) };
    if rc != 0 {
        return None;
    }
    let st = unsafe { buf.assume_init() };
    Some((st.f_ffree, st.f_files))
}

#[cfg(not(unix))]
fn statvfs_free_nodes(_path: &Path) -> Option<(u64, u64)> {
    None
}

fn mount_info_for(path: &Path) -> Option<(String, String, String)> {
    // (mountpoint, device, fstype) from /proc/mounts — public procfs, not GPL.
    let text = std::fs::read_to_string("/proc/mounts").ok()?;
    let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mut best: Option<(usize, String, String, String)> = None;
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let dev = it.next()?.to_string();
        let mp = it.next()?.to_string();
        let fstype = it.next()?.to_string();
        if canon.starts_with(&mp) || canon.to_string_lossy().starts_with(&mp) {
            let score = mp.len();
            if best.as_ref().is_none_or(|(s, ..)| score >= *s) {
                best = Some((score, mp, dev, fstype));
            }
        }
    }
    best.map(|(_, mp, dev, fstype)| (mp, dev, fstype))
}

/// Info panel facts for the currently selected file. No TTY.
///
/// Field labels and value column match live GNU 4.8.30 (` File:`, `  Location:`
/// at inner column 14). Banner / title are painted by the panel chrome.
pub(crate) fn info_lines_for_entry(
    ent: &FileEntry,
    _si: bool,
    show_free_space: bool,
) -> Vec<String> {
    let perms = perm_string(ent);
    let mode_oct = format!("{:04o}", ent.permissions & 0o7777);
    let owner = ent.owner.as_deref().unwrap_or("-");
    let group = ent.group.as_deref().unwrap_or("-");
    let (dev, ino, blocks) = unix_stat_bits(&ent.path).unwrap_or((0, ent.inode, 0));
    let mut lines = vec![
        format!(" File: {}", ent.name),
        info_value_line("Location", &hex_dev_ino(dev, ino)),
        info_value_line("Mode", &format!("{perms} ({mode_oct})")),
        info_value_line("Attributes", "unavailable"),
        info_value_line("Links", &ent.nlink.to_string()),
        info_value_line("Owner", &format!("{owner}/{group}")),
        info_value_line("Size", &format!("{} ({blocks} blocks)", ent.size)),
        info_value_line("Changed", &listing_ts(ent.changed)),
        info_value_line("Modified", &listing_ts(ent.modified)),
        info_value_line("Accessed", &listing_ts(ent.accessed)),
    ];
    if let Some((mp, device, fstype)) = mount_info_for(&ent.path) {
        lines.push(info_value_line("Filesystem", &mp));
        lines.push(info_value_line("Device", &device));
        lines.push(info_value_line("Type", &format!("{fstype} ({dev:X}h)")));
    }
    if show_free_space {
        if let (Ok(avail), Ok(total)) =
            (fs2::available_space(&ent.path), fs2::total_space(&ent.path))
        {
            let pct = if total > 0 {
                (avail as f64 / total as f64) * 100.0
            } else {
                0.0
            };
            lines.push(info_value_line(
                "Free space",
                &format!(
                    "{} / {} ({:.0}%)",
                    human_bytes(avail),
                    human_bytes(total),
                    pct
                ),
            ));
        } else if let Some(free) = filesystem_free_line(&ent.path) {
            lines.push(free);
        }
        if let Some((free, files)) = statvfs_free_nodes(&ent.path) {
            let pct = if files > 0 {
                (free as f64 / files as f64) * 100.0
            } else {
                0.0
            };
            lines.push(info_value_line(
                "Free nodes",
                &format!("{free} / {files} ({:.0}%)", pct),
            ));
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
