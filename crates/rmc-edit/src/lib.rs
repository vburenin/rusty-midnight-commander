use anyhow::Result;
use std::cmp::min;
use std::path::PathBuf;

/// Basic mcedit-like buffer that is binary-safe (stores raw bytes).
/// Rendering helpers expose text with non-printables replaced to avoid corrupting data.
#[derive(Debug, Clone)]
pub struct EditorBuffer {
    /// File path if this buffer is associated with a file on disk.
    pub path: Option<PathBuf>,
    /// Split into lines on '\n'. Each line stores raw bytes without trailing '\n'.
    lines: Vec<Vec<u8>>,
    /// Cursor row (line index) and column (byte offset within the line).
    pub row: usize,
    pub col: usize,
    /// Top-left viewport origin for horizontal/vertical scrolling.
    pub view_row: usize,
    pub view_col: usize,
    /// True when in overwrite mode; false when inserting.
    pub overwrite: bool,
    /// True when buffer has unsaved changes.
    pub dirty: bool,
    /// Last search term (raw bytes)
    pub last_search: Vec<u8>,
    /// Undo stack of snapshots; simple but reliable. Each entry is (lines, row, col).
    undo: Vec<(Vec<Vec<u8>>, usize, usize)>,
}

impl EditorBuffer {
    /// Create an empty buffer.
    pub fn new_empty() -> Self {
        Self {
            path: None,
            lines: vec![Vec::new()],
            row: 0,
            col: 0,
            view_row: 0,
            view_col: 0,
            overwrite: false,
            dirty: false,
            last_search: Vec::new(),
            undo: Vec::new(),
        }
    }

    /// Create a buffer from raw bytes. Splits on '\n' without dropping bytes.
    pub fn from_bytes(bytes: &[u8], path: Option<PathBuf>) -> Self {
        let mut lines = Vec::new();
        let mut cur = Vec::new();
        for &b in bytes {
            if b == b'\n' {
                lines.push(cur);
                cur = Vec::new();
            } else {
                cur.push(b);
            }
        }
        lines.push(cur);
        Self {
            path,
            lines,
            row: 0,
            col: 0,
            view_row: 0,
            view_col: 0,
            overwrite: false,
            dirty: false,
            last_search: Vec::new(),
            undo: Vec::new(),
        }
    }

    /// Convert buffer back to raw bytes with '\n' between stored lines.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for (i, line) in self.lines.iter().enumerate() {
            out.extend_from_slice(line);
            if i + 1 != self.lines.len() {
                out.push(b'\n');
            }
        }
        out
    }

    /// Snapshot current state for undo.
    fn push_undo(&mut self) {
        self.undo.push((self.lines.clone(), self.row, self.col));
        // Bound memory growth (keep last 256 edits)
        if self.undo.len() > 256 {
            let drain = self.undo.len() - 256;
            self.undo.drain(0..drain);
        }
    }

    /// Undo last edit, if any.
    pub fn undo(&mut self) -> bool {
        if let Some((lines, row, col)) = self.undo.pop() {
            self.lines = lines;
            self.row = row;
            self.col = col;
            self.clamp_cursor();
            self.dirty = true; // state changed, not necessarily back to saved
            true
        } else {
            false
        }
    }

    /// Toggle insert/overwrite mode.
    pub fn toggle_overwrite(&mut self) {
        self.overwrite = !self.overwrite;
    }

    /// Move cursor left.
    pub fn move_left(&mut self) {
        if self.col > 0 {
            self.col -= 1;
        } else if self.row > 0 {
            self.row -= 1;
            self.col = self.lines[self.row].len();
        }
        self.ensure_cursor_visible();
    }

    /// Move cursor right.
    pub fn move_right(&mut self) {
        let line_len = self.lines[self.row].len();
        if self.col < line_len {
            self.col += 1;
        } else if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = 0;
        }
        self.ensure_cursor_visible();
    }

    /// Move cursor up a line, preserving column as much as possible.
    pub fn move_up(&mut self) {
        if self.row > 0 {
            self.row -= 1;
            let len = self.lines[self.row].len();
            self.col = min(self.col, len);
        }
        self.ensure_cursor_visible();
    }

    /// Move cursor down a line, preserving column as much as possible.
    pub fn move_down(&mut self) {
        if self.row + 1 < self.lines.len() {
            self.row += 1;
            let len = self.lines[self.row].len();
            self.col = min(self.col, len);
        }
        self.ensure_cursor_visible();
    }

    /// Insert a UTF-8 character at the cursor. Multi-byte chars are inserted as bytes.
    pub fn insert_char(&mut self, ch: char) {
        let mut tmp = [0u8; 4];
        let s = ch.encode_utf8(&mut tmp);
        self.insert_bytes(s.as_bytes());
    }

    /// Insert raw bytes at cursor (binary safe).
    pub fn insert_bytes(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.push_undo();
        let line = &mut self.lines[self.row];
        if self.overwrite {
            // Overwrite up to length; if beyond, extend
            let end = min(self.col + bytes.len(), line.len());
            // Overwrite existing range
            let overwrite_len = end.saturating_sub(self.col);
            for i in 0..overwrite_len {
                line[self.col + i] = bytes[i];
            }
            // If more bytes remain, insert them
            if bytes.len() > overwrite_len {
                line.splice(
                    self.col + overwrite_len..self.col + overwrite_len,
                    bytes[overwrite_len..].iter().copied(),
                );
            }
            self.col += bytes.len();
        } else {
            line.splice(self.col..self.col, bytes.iter().copied());
            self.col += bytes.len();
        }
        self.dirty = true;
        self.ensure_cursor_visible();
    }

    /// Handle Enter: split the current line at cursor.
    pub fn insert_newline(&mut self) {
        self.push_undo();
        let tail = self.lines[self.row].split_off(self.col);
        self.row += 1;
        self.col = 0;
        self.lines.insert(self.row, tail);
        self.dirty = true;
        self.ensure_cursor_visible();
    }

    /// Backspace: delete byte before cursor; join with previous line when at col 0.
    pub fn backspace(&mut self) {
        if self.col > 0 {
            self.push_undo();
            let line = &mut self.lines[self.row];
            line.remove(self.col - 1);
            self.col -= 1;
            self.dirty = true;
        } else if self.row > 0 {
            self.push_undo();
            // Merge with previous line
            let cur = self.lines.remove(self.row);
            self.row -= 1;
            let prev_len = self.lines[self.row].len();
            self.lines[self.row].extend_from_slice(&cur);
            self.col = prev_len;
            self.dirty = true;
        }
        self.ensure_cursor_visible();
    }

    /// Delete byte at cursor (Del).
    pub fn delete(&mut self) {
        let line_len = self.lines[self.row].len();
        if self.col < line_len {
            self.push_undo();
            self.lines[self.row].remove(self.col);
            self.dirty = true;
        } else if self.row + 1 < self.lines.len() {
            self.push_undo();
            // Join with next line
            let next = self.lines.remove(self.row + 1);
            self.lines[self.row].extend_from_slice(&next);
            self.dirty = true;
        }
        self.ensure_cursor_visible();
    }

    /// Ensure cursor is within bounds.
    fn clamp_cursor(&mut self) {
        if self.row >= self.lines.len() {
            self.row = self.lines.len().saturating_sub(1);
        }
        let len = self.lines[self.row].len();
        if self.col > len {
            self.col = len;
        }
    }

    /// Adjust viewport to keep cursor visible given available width/height.
    pub fn adjust_viewport(&mut self, view_width: usize, view_height: usize) {
        // Vertical
        if self.row < self.view_row {
            self.view_row = self.row;
        } else if self.row >= self.view_row + view_height {
            self.view_row = self.row + 1 - view_height;
        }
        // Horizontal
        if self.col < self.view_col {
            self.view_col = self.col;
        } else if self.col >= self.view_col + view_width {
            self.view_col = self.col + 1 - view_width;
        }
    }

    fn ensure_cursor_visible(&mut self) {
        // Keep a soft margin by default when possible
        let margin = 4usize;
        self.view_row = self.row.saturating_sub(margin);
        self.view_col = self.col.saturating_sub(margin);
    }

    /// Render a window of lines for display. Non-printable bytes are shown as '.'.
    pub fn render_window(&self, width: usize, height: usize) -> Vec<String> {
        let mut out = Vec::new();
        for i in 0..height {
            let li = self.view_row + i;
            if let Some(line) = self.lines.get(li) {
                let mut s = String::with_capacity(width);
                // Determine visible slice
                let start = min(self.view_col, line.len());
                let mut j = start;
                while s.chars().count() < width && j < line.len() {
                    let b = line[j];
                    // Show ASCII printable; else dot
                    if (0x20..=0x7E).contains(&b) || b == b'\t' {
                        let ch = if b == b'\t' { ' ' } else { b as char };
                        s.push(ch);
                    } else {
                        s.push('.');
                    }
                    j += 1;
                }
                // Pad to width
                while s.chars().count() < width {
                    s.push(' ');
                }
                out.push(s);
            } else {
                out.push(" ".repeat(width));
            }
        }
        out
    }

    /// Find next occurrence of the given needle (raw bytes) starting from cursor (inclusive).
    /// Returns (row, col) if found and moves cursor there.
    pub fn search_forward(&mut self, needle: &[u8]) -> Option<(usize, usize)> {
        if needle.is_empty() {
            return None;
        }
        self.last_search = needle.to_vec();
        // Search current line from current col
        let mut r = self.row;
        let mut c = self.col;
        loop {
            if r >= self.lines.len() {
                return None;
            }
            if let Some(pos) = find_bytes(&self.lines[r], needle, c) {
                self.row = r;
                self.col = pos;
                self.ensure_cursor_visible();
                return Some((r, pos));
            }
            r += 1;
            c = 0;
        }
    }

    /// Repeat last search, if any.
    pub fn search_next(&mut self) -> Option<(usize, usize)> {
        let needle = self.last_search.clone();
        if needle.is_empty() {
            return None;
        }
        // Start after current cursor position to avoid finding same spot
        let mut start_col = self.col.saturating_add(1);
        let mut r = self.row;
        loop {
            if r >= self.lines.len() {
                return None;
            }
            if let Some(pos) = find_bytes(&self.lines[r], &needle, start_col) {
                self.row = r;
                self.col = pos;
                self.ensure_cursor_visible();
                return Some((r, pos));
            }
            r += 1;
            start_col = 0;
        }
    }

    /// Human-friendly status line: "path  Ln x, Col y  [INS|OVR]  [* if dirty]"
    pub fn status_text(&self) -> String {
        let name = self
            .path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "<untitled>".to_string());
        let ln = self.row + 1;
        let col = self.col + 1;
        let mode = if self.overwrite { "OVR" } else { "INS" };
        let dirty = if self.dirty { " *" } else { "" };
        format!("{name}  Ln {ln}, Col {col}  [{mode}]{dirty}")
    }
}

fn find_bytes(hay: &[u8], needle: &[u8], start: usize) -> Option<usize> {
    if needle.is_empty() || start >= hay.len() {
        return None;
    }
    hay.windows(needle.len())
        .enumerate()
        .skip(start)
        .find_map(|(i, w)| if w == needle { Some(i) } else { None })
}

/// Lightweight trait to describe an editor host integration.
/// Not a full UI; the UI layer should own event handling and drawing.
pub trait EditorHost {
    fn open(&mut self, _path: &std::path::Path) -> Result<()>;
    fn save(&mut self) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_empty_buffer() {
        let b = EditorBuffer::new_empty();
        assert_eq!(b.lines.len(), 1);
        assert_eq!(b.lines[0], Vec::<u8>::new());
        assert_eq!(b.row, 0);
        assert_eq!(b.col, 0);
    }

    #[test]
    fn load_and_roundtrip_bytes() {
        let data = b"abc\ndef\n\x00\x01\x02";
        let b = EditorBuffer::from_bytes(data, Some(PathBuf::from("x.txt")));
        assert_eq!(b.lines.len(), 3);
        assert_eq!(b.lines[0], b"abc");
        assert_eq!(b.lines[1], b"def");
        assert_eq!(b.lines[2], b"\x00\x01\x02");
        assert_eq!(b.to_bytes(), data);
    }

    #[test]
    fn insert_and_backspace() {
        let mut b = EditorBuffer::new_empty();
        b.insert_bytes(b"abc");
        assert_eq!(b.to_bytes(), b"abc");
        b.backspace();
        assert_eq!(b.to_bytes(), b"ab");
        b.insert_newline();
        b.insert_bytes(b"x");
        assert_eq!(b.to_bytes(), b"ab\nx");
    }

    #[test]
    fn overwrite_mode() {
        let mut b = EditorBuffer::new_empty();
        b.insert_bytes(b"abc");
        b.row = 0;
        b.col = 1; // a|bc
        b.toggle_overwrite();
        b.insert_bytes(b"Z");
        assert_eq!(b.to_bytes(), b"aZc");
        // overwrite beyond end appends
        b.col = 3;
        b.insert_bytes(b"YY");
        assert_eq!(b.to_bytes(), b"aZcYY");
    }

    #[test]
    fn undo_works() {
        let mut b = EditorBuffer::new_empty();
        b.insert_bytes(b"abc");
        b.insert_newline();
        b.insert_bytes(b"def");
        assert_eq!(b.to_bytes(), b"abc\ndef");
        assert!(b.undo());
        assert_eq!(b.to_bytes(), b"abc\n");
        assert!(b.undo());
        assert_eq!(b.to_bytes(), b"abc");
    }

    #[test]
    fn search_forward_and_repeat() {
        let mut b = EditorBuffer::from_bytes(b"xx abc\nyy abc\nzz", None);
        assert_eq!(b.search_forward(b"abc"), Some((0, 3)));
        assert_eq!(b.search_next(), Some((1, 3)));
        assert_eq!(b.search_next(), None);
    }
}
