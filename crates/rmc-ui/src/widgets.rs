use crossterm::cursor::MoveTo;
use crossterm::style::{Color, Print};
use crossterm::QueueableCommand;
use std::io::Write;

pub struct Painter<'a> {
    pub out: &'a mut dyn Write,
}

/// GNU/S-Lang 16-color foreground SGR (30–37 / 90–97).
///
/// Crossterm `SetColors` goes through `Colored::Display`, which becomes empty
/// under `NO_COLOR` and then formats as `\x1b[m` (full reset). That leaves the
/// previous pair — selected/statusbar black;cyan — on the header row. Emit
/// classic 16-color codes ourselves so header yellow;blue actually hits the
/// pixels. Skin name `blue` is stored as `Color::Blue` but GNU blue is navy
/// (34/44), not 256-color index 12.
pub(crate) fn ansi_sgr_fg(color: Color) -> u8 {
    match color {
        Color::Black => 30,
        Color::DarkRed => 31,
        Color::DarkGreen => 32,
        Color::DarkYellow => 33,
        Color::DarkBlue | Color::Blue => 34,
        Color::DarkMagenta => 35,
        Color::DarkCyan | Color::Cyan => 36,
        Color::Grey => 37,
        Color::DarkGrey => 90,
        Color::Red => 91,
        Color::Green => 92,
        Color::Yellow => 93,
        Color::Magenta => 95,
        Color::White => 97,
        Color::Reset => 39,
        Color::AnsiValue(v) => match v {
            0..=7 => 30 + v,
            8..=15 => 90 + (v - 8),
            _ => 37,
        },
        Color::Rgb { .. } => 37,
    }
}

pub(crate) fn ansi_sgr_bg(color: Color) -> u8 {
    match ansi_sgr_fg(color) {
        39 => 49,
        fg @ 30..=37 => fg + 10,
        fg @ 90..=97 => fg + 10,
        _ => 49,
    }
}

impl<'a> Painter<'a> {
    pub fn set_fg_bg(&mut self, fg: Color, bg: Color) {
        let _ = write!(self.out, "\x1b[{};{}m", ansi_sgr_fg(fg), ansi_sgr_bg(bg));
    }
    pub fn goto(&mut self, x: u16, y: u16) {
        let _ = self.out.queue(MoveTo(x, y));
    }
    pub fn text(&mut self, s: &str) {
        let _ = self.out.queue(Print(s));
    }
    pub fn fill_line(&mut self, y: u16, width: u16, bg: Color, fg: Color) {
        self.set_fg_bg(fg, bg);
        self.goto(0, y);
        let s = " ".repeat(width as usize);
        self.text(&s);
    }
    pub fn fill_rect(&mut self, x: u16, y: u16, w: u16, h: u16, fg: Color, bg: Color) {
        for row in 0..h {
            self.hline(x, y + row, w, ' ', fg, bg);
        }
    }
    pub fn hline(&mut self, x: u16, y: u16, w: u16, ch: char, fg: Color, bg: Color) {
        self.set_fg_bg(fg, bg);
        self.goto(x, y);
        self.text(&ch.to_string().repeat(w as usize));
    }
    pub fn vline(&mut self, x: u16, y: u16, h: u16, ch: char, fg: Color, bg: Color) {
        self.set_fg_bg(fg, bg);
        for i in 0..h {
            self.goto(x, y + i);
            self.text(&ch.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_fg_bg_emits_gnu_16color_header_not_sgr_reset() {
        let mut buf = Vec::new();
        {
            let mut p = Painter { out: &mut buf };
            p.set_fg_bg(Color::Yellow, Color::Blue);
            p.goto(1, 1);
            p.text("Name");
        }
        let s = String::from_utf8_lossy(&buf);
        assert!(
            s.contains("\x1b[93;44m"),
            "header pair must be yellow;blue 93;44, got {s:?}"
        );
        assert!(
            !s.contains("\x1b[mName") && !s.contains("\x1b[0mName"),
            "must not reset before Name (leftover cyan): {s:?}"
        );
        assert!(!s.contains("38;5;"), "must not use 256-color SGR: {s:?}");
    }

    #[test]
    fn set_fg_bg_selected_is_black_on_cyan() {
        let mut buf = Vec::new();
        {
            let mut p = Painter { out: &mut buf };
            p.set_fg_bg(Color::Black, Color::Cyan);
        }
        let s = String::from_utf8_lossy(&buf);
        assert_eq!(s, "\x1b[30;46m");
        assert_ne!(ansi_sgr_bg(Color::Cyan), ansi_sgr_bg(Color::Blue));
        assert_eq!(ansi_sgr_fg(Color::Yellow), 93);
        assert_eq!(ansi_sgr_bg(Color::Blue), 44);
    }

    #[test]
    fn set_fg_bg_menu_white_cyan_not_reverse() {
        let mut buf = Vec::new();
        {
            let mut p = Painter { out: &mut buf };
            p.set_fg_bg(Color::White, Color::Cyan);
        }
        assert_eq!(String::from_utf8_lossy(&buf), "\x1b[97;46m");
        let mut sel = Vec::new();
        {
            let mut p = Painter { out: &mut sel };
            p.set_fg_bg(Color::White, Color::Black);
        }
        assert_eq!(String::from_utf8_lossy(&sel), "\x1b[97;40m");
    }

    #[test]
    fn set_fg_bg_dialog_and_error_pairs() {
        let mut dlg = Vec::new();
        {
            let mut p = Painter { out: &mut dlg };
            p.set_fg_bg(Color::Black, Color::Grey);
        }
        assert_eq!(String::from_utf8_lossy(&dlg), "\x1b[30;47m");
        let mut title = Vec::new();
        {
            let mut p = Painter { out: &mut title };
            p.set_fg_bg(Color::Blue, Color::Grey);
        }
        assert_eq!(String::from_utf8_lossy(&title), "\x1b[34;47m");
        let mut err = Vec::new();
        {
            let mut p = Painter { out: &mut err };
            p.set_fg_bg(Color::White, Color::Red);
        }
        assert_eq!(String::from_utf8_lossy(&err), "\x1b[97;101m");
        let mut hot = Vec::new();
        {
            let mut p = Painter { out: &mut hot };
            p.set_fg_bg(Color::Yellow, Color::Black);
        }
        assert_eq!(String::from_utf8_lossy(&hot), "\x1b[93;40m");
    }
}
