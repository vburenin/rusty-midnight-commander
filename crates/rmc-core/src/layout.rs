#[derive(Clone, Copy, Debug)]
pub struct ChromeGeom {
    pub panel_top: u16,
    pub content_bottom: u16,
    pub gauge_row: Option<u16>,
    pub hint_row: Option<u16>,
    pub cmd_row: Option<u16>,
    pub fbar_row: Option<u16>,
}

/// Rectangle of one directory panel inside the chrome content area.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PanelRect {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

impl ChromeGeom {
    pub fn mid_col(cols: u16) -> u16 {
        panel_split(cols, 0.5)
    }
}

/// GNU mc(1) menu bar titles. First and last become ` Above ` / ` Below `
/// when the horizontal panel split is chosen (Left and Right Menus).
pub fn menu_bar_titles(horizontal_split: bool) -> [&'static str; 5] {
    if horizontal_split {
        [" Above ", " File ", " Command ", " Options ", " Below "]
    } else {
        [" Left ", " File ", " Command ", " Options ", " Right "]
    }
}

/// Dual-pane rectangles inside the chrome content area.
/// Vertical (default): side-by-side columns (Left | Right).
/// Horizontal: stacked rows (Above / Below), split by [`panel_split`] on height.
pub fn dual_panel_rects(
    cols: u16,
    chrome: &ChromeGeom,
    opt: &crate::app::LayoutOptions,
) -> (PanelRect, PanelRect) {
    let y = chrome.panel_top;
    let h = chrome.content_bottom.saturating_sub(chrome.panel_top);
    if opt.horizontal_split {
        let top_h = panel_split(h, opt.panel_ratio);
        let bot_h = h.saturating_sub(top_h);
        (
            PanelRect {
                x: 0,
                y,
                w: cols,
                h: top_h,
            },
            PanelRect {
                x: 0,
                y: y.saturating_add(top_h),
                w: cols,
                h: bot_h,
            },
        )
    } else {
        let mid = panel_split(cols, opt.panel_ratio);
        (
            PanelRect { x: 0, y, w: mid, h },
            PanelRect {
                x: mid,
                y,
                w: cols.saturating_sub(mid),
                h,
            },
        )
    }
}

/// First-panel size in columns (vertical split) or rows (horizontal split).
/// `ratio` is the first panel's share; 0.5 is equal (`total / 2`).
pub fn panel_split(total: u16, ratio: f32) -> u16 {
    if total <= 1 {
        return 0;
    }
    if (ratio - 0.5).abs() <= f32::EPSILON {
        return total / 2;
    }
    let ratio = ratio.clamp(0.2, 0.8);
    let split = ((total as f32) * ratio).round() as u16;
    split.clamp(1, total.saturating_sub(1))
}

/// Compute positions of UI chrome rows based on terminal size and LayoutOptions.
/// Packing rules:
/// - Optional top menubar at row 0 when enabled
/// - Panels occupy from panel_top down to content_bottom (inclusive bottom border)
/// - Bottom chrome packed from the last row upwards in order: fbar, cmd, hint
/// - GNU "Show free space" is painted in each panel's bottom frame, not a chrome row
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
    let hint_row = if opt.hintbar_visible {
        Some(next)
    } else {
        None
    };
    // Free space lives in the panel bottom frame (mc(1) "bottom frame of panel").
    let gauge_row = None;
    // content_bottom is the row just above the nearest bottom chrome (hint/cmd/fbar).
    let content_bottom = {
        let mut first_bottom: Option<u16> = None;
        for y in [gauge_row, hint_row, cmd_row, fbar_row]
            .into_iter()
            .flatten()
        {
            first_bottom = Some(match first_bottom {
                Some(cur) => cur.min(y),
                None => y,
            });
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

#[cfg(test)]
mod tests {
    use super::{compute_chrome_geom, dual_panel_rects, menu_bar_titles, panel_split};
    use crate::app::LayoutOptions;

    #[test]
    fn equal_ratio_matches_integer_half() {
        assert_eq!(panel_split(80, 0.5), 40);
        assert_eq!(panel_split(81, 0.5), 40);
    }

    #[test]
    fn unequal_ratio_moves_the_split() {
        assert_eq!(panel_split(80, 0.8), 64);
        assert_eq!(panel_split(80, 0.2), 16);
    }

    #[test]
    fn menu_bar_titles_left_right_when_vertical() {
        assert_eq!(
            menu_bar_titles(false),
            [" Left ", " File ", " Command ", " Options ", " Right "]
        );
    }

    #[test]
    fn menu_bar_titles_above_below_when_horizontal() {
        assert_eq!(
            menu_bar_titles(true),
            [" Above ", " File ", " Command ", " Options ", " Below "]
        );
    }

    #[test]
    fn default_layout_is_vertical_side_by_side() {
        let opt = LayoutOptions::default();
        assert!(!opt.horizontal_split);
        let chrome = compute_chrome_geom(80, 24, &opt);
        let (left, right) = dual_panel_rects(80, &chrome, &opt);
        let content_h = chrome.content_bottom.saturating_sub(chrome.panel_top);
        assert_eq!(left.w, panel_split(80, opt.panel_ratio));
        assert_eq!(right.w, 80 - left.w);
        assert_eq!(left.h, content_h);
        assert_eq!(right.h, content_h);
        assert_eq!(left.y, right.y);
        assert_eq!(left.x, 0);
        assert_eq!(right.x, left.w);
    }

    #[test]
    fn horizontal_split_uses_panel_split_for_rows() {
        let opt = LayoutOptions {
            horizontal_split: true,
            panel_ratio: 0.5,
            ..LayoutOptions::default()
        };
        let chrome = compute_chrome_geom(80, 24, &opt);
        let content_h = chrome.content_bottom.saturating_sub(chrome.panel_top);
        let (above, below) = dual_panel_rects(80, &chrome, &opt);
        assert_eq!(above.h, panel_split(content_h, 0.5));
        assert_eq!(below.h, content_h - above.h);
        assert_eq!(above.y, chrome.panel_top);
        assert_eq!(below.y, chrome.panel_top + above.h);
        assert_eq!(above.w, 80);
        assert_eq!(below.w, 80);
        assert_eq!(above.x, 0);
        assert_eq!(below.x, 0);
    }

    #[test]
    fn horizontal_unequal_ratio_moves_the_row_split() {
        let opt = LayoutOptions {
            horizontal_split: true,
            panel_ratio: 0.8,
            equal_split: false,
            ..LayoutOptions::default()
        };
        let chrome = compute_chrome_geom(80, 24, &opt);
        let content_h = chrome.content_bottom.saturating_sub(chrome.panel_top);
        let (above, below) = dual_panel_rects(80, &chrome, &opt);
        assert_eq!(above.h, panel_split(content_h, 0.8));
        assert_eq!(below.h, content_h - above.h);
        assert!(
            above.h > below.h,
            "0.8 ratio should give the top pane more rows"
        );
    }

    #[test]
    fn show_free_space_does_not_reserve_a_chrome_row() {
        let on = LayoutOptions {
            show_free_space: true,
            ..LayoutOptions::default()
        };
        let off = LayoutOptions {
            show_free_space: false,
            ..LayoutOptions::default()
        };
        let a = compute_chrome_geom(80, 24, &on);
        let b = compute_chrome_geom(80, 24, &off);
        assert_eq!(a.content_bottom, b.content_bottom);
        assert_eq!(a.gauge_row, None);
        assert_eq!(b.gauge_row, None);
        assert!(a.hint_row.is_some(), "hint bar stays below the panels");
    }
}
