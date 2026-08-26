#[derive(Clone, Copy, Debug)]
pub struct ChromeGeom {
    pub panel_top: u16,
    pub content_bottom: u16,
    pub gauge_row: Option<u16>,
    pub hint_row: Option<u16>,
    pub cmd_row: Option<u16>,
    pub fbar_row: Option<u16>,
}

impl ChromeGeom {
    pub fn mid_col(cols: u16) -> u16 {
        cols / 2
    }
}

/// Compute positions of UI chrome rows based on terminal size and LayoutOptions.
/// Packing rules:
/// - Optional top menubar at row 0 when enabled
/// - Panels occupy from panel_top down to content_bottom (inclusive bottom border)
/// - Bottom chrome packed from the last row upwards in order: fbar, cmd, hint, gauge
pub fn compute_chrome_geom(cols: u16, rows: u16, opt: &crate::app::LayoutOptions) -> ChromeGeom {
    let _ = cols;
    // Top area
    let panel_top: u16 = if opt.menubar_visible { 1 } else { 0 };
    // Bottom packing
    let mut next = rows.saturating_sub(1);
    let fbar_row = if opt.keybar_visible { Some(next) } else { None };
    if fbar_row.is_some() && next > 0 {
        next = next.saturating_sub(1);
    }
    let cmd_row = if opt.command_prompt { Some(next) } else { None };
    if cmd_row.is_some() && next > 0 {
        next = next.saturating_sub(1);
    }
    let hint_row = if opt.hintbar_visible { Some(next) } else { None };
    if hint_row.is_some() && next > 0 {
        next = next.saturating_sub(1);
    }
    let gauge_row = if opt.show_free_space { Some(next) } else { None };
    // content_bottom is the row just above the nearest bottom chrome (gauge/hint/cmd/fbar).
    let content_bottom = {
        let mut first_bottom = None;
        for r in [gauge_row, hint_row, cmd_row, fbar_row] {
            if let Some(y) = r {
                first_bottom = Some(first_bottom.map_or(y, |cur| cur.min(y)));
            }
        }
        if let Some(b) = first_bottom {
            b.saturating_sub(1)
        } else {
            rows.saturating_sub(1)
        }
    };
    ChromeGeom {
        panel_top,
        content_bottom,
        gauge_row,
        hint_row,
        cmd_row,
        fbar_row,
    }
}

