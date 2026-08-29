//! GNU Midnight Commander hint-line texts and idle rotation.
//!
//! Wording is locked against live GNU mc 4.8.30 on 80×24 (`Hint: ` prefix,
//! rest-state line). Cadence follows public mc-devel notes that the hint
//! refreshes after one minute; live 4.8.30 on this box kept the rest-state
//! line through 75s of idle (no PTY bytes), so the timer is an idle tick,
//! not a full-screen refresh.

use std::time::{Duration, Instant};

/// Live GNU 4.8.30 rest-state hint on 80×24 (every launch, `/tmp/mcr-fixture`).
pub const GNU_REST_HINT: &str = "Hint: Tab changes your current panel.";

/// Public mc-devel (2002): the hint line refreshes after one minute.
pub const HINT_ROTATION: Duration = Duration::from_secs(60);

/// English hint-line texts as they appear on GNU mc (prefix included).
/// Index 0 is the live rest-state line; later entries rotate on [`HINT_ROTATION`].
pub const HINTS: &[&str] = &[
    GNU_REST_HINT,
    "Hint: Use C-x t to copy tagged file names to the command line.",
    "Hint: Use C-x p to copy the current pathname to the command line.",
    "Hint: Completion: use M-Tab (or Esc+Tab). Type it twice to get a list.",
    "Hint: Use M-p and M-n to access the command history.",
    "Hint: Need to quote a character? Use Control-q and the character.",
    "Hint: Tired of these messages? Turn them off from the Options/Layout menu.",
    "Hint: Selecting directories: add a slash to the end of the matching pattern.",
    "Hint: If your terminal lacks functions keys, use the ESC+number sequence.",
    "Hint: VFS coolness: tap enter on a tar file to examine its contents.",
    "Hint: We also have a nice manual page.",
    "Hint: Do you want Lynx-style navigation? Set it in the Configuration dialog.",
    "Hint: % macros work even on the command line.",
    "Hint: M-! will allow you to execute programs and see the output in the viewer.",
    "Hint: The file listing format can be customized; do \"man mc\" for details.",
    "Hint: %D/%T expands to the tagged files in the opposite directory.",
    "Hint: Want your plain shell? Press C-o, and get back to MC with C-o again.",
    "Hint: Setting the CDPATH variable can save you keystrokes in cd commands.",
    "Hint: If you want to see your .* files, say so in the Configuration dialog.",
    "Hint: Want to see your *~ backup files? Set it in the Configuration dialog.",
    "Hint: Completion works on all input lines in all dialogs. Just press M-Tab.",
    "Hint: On slow terminals the -s flag may help.",
    "Hint: Find File: you can work on the files found using the Panelize button.",
    "Hint: Want to do complex searches? Use the External Panelize command.",
    "Hint: To change directory halfway through typing a command, use M-c (quick cd).",
    "Hint: Shell commands will not work when you are on a non-local file system.",
    "Hint: Bring text back from the dead with C-y.",
    "Hint: Are some of your keys not working? Look at Options/Learn keys.",
    "Hint: To look at the output of a command in the viewer, use M-!",
    "Hint: F13 (or Shift-F3) invokes the viewer in raw mode.",
    "Hint: You may specify the editor for F4 with the shell variable EDITOR.",
    "Hint: You may specify the external viewer with the shell vars VIEWER or PAGER.",
    "Hint: You can disable all requests for confirmation in Options/Confirmation.",
    "Hint: Leap to frequently used directories in a single bound with C-\\.",
    "Hint: You can do anonymous FTP with mc by typing 'cd ftp://machine.edu'",
    "Hint: FTP is built in the Midnight Commander, check the File/FTP link menu.",
    "Hint: M-t changes quickly the listing mode.",
    "Hint: You can specify the username when doing ftps: 'cd ftp://user@machine.edu'",
    "Hint: You can browse RPM files by tapping enter on top of an rpm file.",
    "Hint: To mark directories on the select dialog box, append a slash.",
    "Hint: To use the mouse cut and paste may require holding the shift key",
    "Hint: Key frequently visited ftp sites in the hotlist: type C-\\.",
];

/// Idle hint-line rotator. Only the hint row should repaint on a tick.
#[derive(Clone, Debug)]
pub struct HintBarState {
    index: usize,
    last_changed: Instant,
}

impl Default for HintBarState {
    fn default() -> Self {
        Self::at(Instant::now())
    }
}

impl HintBarState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn at(now: Instant) -> Self {
        Self {
            index: 0,
            last_changed: now,
        }
    }

    pub fn index(&self) -> usize {
        self.index
    }

    pub fn text(&self) -> &'static str {
        HINTS[self.index % HINTS.len()]
    }

    /// Advance when `now` is at least [`HINT_ROTATION`] after the last change.
    /// Returns true only when the visible text actually changed.
    pub fn maybe_rotate(&mut self, now: Instant) -> bool {
        if now.saturating_duration_since(self.last_changed) < HINT_ROTATION {
            return false;
        }
        let prev = self.text();
        self.index = (self.index + 1) % HINTS.len();
        self.last_changed = now;
        self.text() != prev
    }
}

/// 80-column rest-state row: live GNU paints the text in hintbar
/// (`default;default`) and leaves the remainder as the screen-clear fill.
pub fn gnu_80col_rest_hint_row() -> String {
    let mut row = GNU_REST_HINT.to_string();
    row.push_str(&" ".repeat(80usize.saturating_sub(GNU_REST_HINT.chars().count())));
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rest_state_matches_live_gnu_480() {
        assert_eq!(HINTS[0], GNU_REST_HINT);
        assert_eq!(
            HintBarState::at(Instant::now()).text(),
            "Hint: Tab changes your current panel."
        );
        assert!(
            GNU_REST_HINT.starts_with("Hint: "),
            "live GNU prefix is `Hint: `"
        );
        let row = gnu_80col_rest_hint_row();
        assert_eq!(row.chars().count(), 80);
        assert_eq!(&row[..GNU_REST_HINT.len()], GNU_REST_HINT);
        assert!(row[GNU_REST_HINT.len()..].chars().all(|c| c == ' '));
    }

    #[test]
    fn idle_under_one_minute_does_not_rotate() {
        let t0 = Instant::now();
        let mut hint = HintBarState::at(t0);
        assert!(!hint.maybe_rotate(t0));
        assert!(!hint.maybe_rotate(t0 + Duration::from_millis(2_500)));
        assert!(!hint.maybe_rotate(t0 + Duration::from_secs(59)));
        assert_eq!(hint.text(), GNU_REST_HINT);
        assert_eq!(hint.index(), 0);
    }

    #[test]
    fn one_minute_idle_advances_to_the_next_hint() {
        let t0 = Instant::now();
        let mut hint = HintBarState::at(t0);
        assert!(hint.maybe_rotate(t0 + HINT_ROTATION));
        assert_eq!(hint.index(), 1);
        assert_eq!(hint.text(), HINTS[1]);
        assert_ne!(hint.text(), GNU_REST_HINT);
        assert!(!hint.maybe_rotate(t0 + HINT_ROTATION + Duration::from_secs(30)));
        assert!(hint.maybe_rotate(t0 + HINT_ROTATION + HINT_ROTATION));
        assert_eq!(hint.index(), 2);
    }

    #[test]
    fn rotation_wraps_the_catalog() {
        let t0 = Instant::now();
        let mut hint = HintBarState::at(t0);
        for i in 0..HINTS.len() {
            assert_eq!(hint.text(), HINTS[i]);
            assert!(hint.maybe_rotate(t0 + HINT_ROTATION * (i as u32 + 1)));
        }
        assert_eq!(hint.text(), GNU_REST_HINT);
        assert_eq!(hint.index(), 0);
    }

    #[test]
    fn catalog_uses_hint_prefix_and_fits_80_cols() {
        for (i, h) in HINTS.iter().enumerate() {
            assert!(
                h.starts_with("Hint: "),
                "hint {i} must use the live GNU prefix: {h}"
            );
            assert!(h.chars().count() <= 80, "hint {i} overflows 80 cols: {h}");
        }
    }
}
