use crossterm::cursor::MoveTo;
use crossterm::style::{Color, Colors, Print, SetColors};
use crossterm::QueueableCommand;
use std::io::Write;

pub struct Painter<'a> {
    pub out: &'a mut dyn Write,
}

impl<'a> Painter<'a> {
    pub fn set_fg_bg(&mut self, fg: Color, bg: Color) {
        let _ = self.out.queue(SetColors(Colors::new(fg, bg)));
    }
    pub fn goto(&mut self, x: u16, y: u16) {
        let _ = self.out.queue(MoveTo(x, y));
    }
    pub fn text(&mut self, s: &str) {
        let _ = self.out.queue(Print(s));
    }
    pub fn fill_line(&mut self, y: u16, width: u16, bg: Color, fg: Color) {
        self.goto(0, y);
        self.set_fg_bg(fg, bg);
        let s = " ".repeat(width as usize);
        self.text(&s);
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
