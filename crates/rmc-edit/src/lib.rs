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
    /// Whether last search (and default find) is case-insensitive (ASCII).
    pub last_search_case_insensitive: bool,
    /// Undo stack of snapshots; simple but reliable. Each entry is (lines, row, col).
    undo: Vec<(Vec<Vec<u8>>, usize, usize)>,
    /// Block selection start and end (row, col), normalized so (start <= end) when both present.
    sel_start: Option<(usize, usize)>,
    sel_end: Option<(usize, usize)>,
    /// Internal clipboard for block operations: selected content as lines (without trailing '\n').
    clipboard: Option<Vec<Vec<u8>>>,
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
            last_search_case_insensitive: false,
            undo: Vec::new(),
            sel_start: None,
            sel_end: None,
            clipboard: None,
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
            last_search_case_insensitive: false,
            undo: Vec::new(),
            sel_start: None,
            sel_end: None,
            clipboard: None,
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

    /// Set case-insensitive flag for subsequent searches (ASCII only).
    pub fn set_search_case_insensitive(&mut self, on: bool) {
        self.last_search_case_insensitive = on;
    }

    /// Find next occurrence of the given needle (raw bytes) starting from cursor (inclusive).
    /// Returns (row, col) if found and moves cursor there. Records last_search and ci flag.
    pub fn search_forward(&mut self, needle: &[u8]) -> Option<(usize, usize)> {
        self.search_forward_opts(needle, self.last_search_case_insensitive, false)
    }

    /// Search forward with options: case-insensitive (ASCII) and wrap to start if not found.
    pub fn search_forward_opts(
        &mut self,
        needle: &[u8],
        case_insensitive: bool,
        wrap: bool,
    ) -> Option<(usize, usize)> {
        if needle.is_empty() {
            return None;
        }
        self.last_search = needle.to_vec();
        self.last_search_case_insensitive = case_insensitive;
        // First pass: from current cursor to end
        if let Some((r, c)) = self.search_in_range(needle, case_insensitive, self.row, self.col) {
            self.row = r;
            self.col = c;
            self.ensure_cursor_visible();
            return Some((r, c));
        }
        if wrap {
            // Second pass: from start to current position (inclusive)
            if let Some((r, c)) = self.search_in_range(needle, case_insensitive, 0, 0) {
                // Only accept results strictly before original position to avoid infinite loop
                if r < self.row || (r == self.row && c <= self.col) {
                    self.row = r;
                    self.col = c;
                    self.ensure_cursor_visible();
                    return Some((r, c));
                }
            }
        }
        None
    }

    fn search_in_range(
        &self,
        needle: &[u8],
        case_insensitive: bool,
        start_row: usize,
        start_col: usize,
    ) -> Option<(usize, usize)> {
        if needle.is_empty() {
            return None;
        }
        let mut r = start_row;
        let mut c = start_col;
        loop {
            if r >= self.lines.len() {
                return None;
            }
            let hay = &self.lines[r];
            let pos = if case_insensitive {
                find_bytes_ascii_ci(hay, needle, c)
            } else {
                find_bytes(hay, needle, c)
            };
            if let Some(p) = pos {
                return Some((r, p));
            }
            r += 1;
            c = 0;
        }
    }

    /// Repeat last search, if any.
    pub fn search_next(&mut self) -> Option<(usize, usize)> {
        self.search_next_opts(false)
    }

    /// Repeat last search with optional wrap.
    pub fn search_next_opts(&mut self, wrap: bool) -> Option<(usize, usize)> {
        let needle = self.last_search.clone();
        if needle.is_empty() {
            return None;
        }
        // Start after current cursor position to avoid finding same spot
        let r = self.row;
        let c = self.col.saturating_add(1);
        // First pass on current and following lines
        if let Some((rr, cc)) =
            self.search_in_range(&needle, self.last_search_case_insensitive, r, c)
        {
            self.row = rr;
            self.col = cc;
            self.ensure_cursor_visible();
            return Some((rr, cc));
        }
        if wrap {
            // From start until (row,col)
            if let Some((rr, cc)) =
                self.search_in_range(&needle, self.last_search_case_insensitive, 0, 0)
            {
                if rr < self.row || (rr == self.row && cc <= self.col) {
                    self.row = rr;
                    self.col = cc;
                    self.ensure_cursor_visible();
                    return Some((rr, cc));
                }
            }
        }
        None
    }

    /// Replace next occurrence of `from` with `to`. Returns replaced (row,col) start.
    pub fn replace_next(
        &mut self,
        from: &[u8],
        to: &[u8],
        case_insensitive: bool,
        wrap: bool,
    ) -> Option<(usize, usize)> {
        let found = self.search_forward_opts(from, case_insensitive, wrap)?;
        self.replace_at(found.0, found.1, from.len(), to);
        Some(found)
    }

    /// Replace all occurrences of `from` with `to`. Returns number of replacements.
    pub fn replace_all(&mut self, from: &[u8], to: &[u8], case_insensitive: bool) -> usize {
        if from.is_empty() {
            return 0;
        }
        let mut count = 0usize;
        let mut r = 0usize;
        while r < self.lines.len() {
            let mut c = 0usize;
            loop {
                let hay = &self.lines[r];
                let pos = if case_insensitive {
                    find_bytes_ascii_ci(hay, from, c)
                } else {
                    find_bytes(hay, from, c)
                };
                if let Some(p) = pos {
                    self.replace_at(r, p, from.len(), to);
                    count += 1;
                    // Continue after the inserted text
                    c = p.saturating_add(to.len());
                } else {
                    break;
                }
            }
            r += 1;
        }
        count
    }

    fn replace_at(&mut self, row: usize, col: usize, from_len: usize, to: &[u8]) {
        self.push_undo();
        if let Some(line) = self.lines.get_mut(row) {
            // bounds-safe removal and insertion
            let end = col.saturating_add(from_len).min(line.len());
            line.splice(col..end, to.iter().copied());
            // Move cursor to start of replaced text
            self.row = row;
            self.col = col;
            self.dirty = true;
            self.ensure_cursor_visible();
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

    /// Begin block selection at current cursor (mark start).
    pub fn mark_start(&mut self) {
        self.sel_start = Some((self.row, self.col));
        self.sel_end = None;
    }

    /// Set block selection end at current cursor; normalizes start/end order.
    pub fn mark_end(&mut self) {
        if let Some((sr, sc)) = self.sel_start {
            let (er, ec) = (self.row, self.col);
            let ((a_r, a_c), (b_r, b_c)) = normalize_points((sr, sc), (er, ec));
            self.sel_start = Some((a_r, a_c));
            self.sel_end = Some((b_r, b_c));
        }
    }

    /// Clear any existing block selection.
    pub fn clear_selection(&mut self) {
        self.sel_start = None;
        self.sel_end = None;
    }

    /// Returns the current normalized selection ((start_row, start_col), (end_row, end_col)), if any.
    pub fn selection_bounds(&self) -> Option<((usize, usize), (usize, usize))> {
        if let (Some(s), Some(e)) = (self.sel_start, self.sel_end) {
            let (a, b) = normalize_points(s, e);
            Some((a, b))
        } else {
            None
        }
    }

    /// Copy selected block at current cursor position. Keeps selection.
    /// Returns true if a selection existed and was copied.
    pub fn copy_block_here(&mut self) -> bool {
        let sel = match self.selection_bounds() {
            Some(s) => s,
            None => return false,
        };
        let content = self.extract_selection_lines(sel);
        self.clipboard = Some(content.clone());
        self.insert_block(&content);
        true
    }

    /// Move selected block to current cursor position (cut+paste). Clears selection.
    /// Returns true if a selection existed and was moved.
    pub fn move_block_here(&mut self) -> bool {
        let sel = match self.selection_bounds() {
            Some(s) => s,
            None => return false,
        };
        let orig_target = (self.row, self.col);
        let content = self.extract_selection_lines(sel);
        // If cursor lies inside the selection, do nothing (MC keeps it simple).
        if point_in_range((self.row, self.col), sel) {
            return false;
        }
        // Delete selection first; adjust cursor if deletion precedes it
        self.delete_selection(); // moves cursor to start
        // Compute mapped insertion point after deletion
        let ((sr, sc), (er, ec)) = sel;
        let mut target = orig_target;
        // If target was after the removed region, rows may have shifted up
        if orig_target.0 > er {
            let rows_removed = er.saturating_sub(sr);
            target.0 = orig_target.0.saturating_sub(rows_removed);
        } else if orig_target.0 == er && orig_target.1 >= ec {
            // Target was on the end line after the removed slice: now on sr with shifted column
            target.0 = sr;
            let delta = orig_target.1.saturating_sub(ec);
            target.1 = sc.saturating_add(delta);
        } else if (orig_target.0 > sr && orig_target.0 < er)
            || (orig_target.0 == sr && orig_target.1 >= sc)
            || (orig_target.0 == er && orig_target.1 < ec)
        {
            // Inside selection (should have been caught by earlier check), bail
            return false;
        }
        // Set cursor to mapped target
        self.row = target.0.min(self.lines.len().saturating_sub(1));
        self.col = target.1.min(self.lines[self.row].len());
        // After deletion, insert at desired target
        self.insert_block(&content);
        self.clear_selection();
        true
    }

    /// Delete selected block. Clears selection. Returns true if deleted.
    pub fn delete_selection(&mut self) -> bool {
        let Some(((sr, sc), (er, ec))) = self.selection_bounds() else {
            return false;
        };
        self.push_undo();
        if sr == er {
            // Single line
            let line = &mut self.lines[sr];
            let end = ec.min(line.len());
            let start = sc.min(end);
            line.drain(start..end);
        } else {
            // Multi-line
            // Truncate first line at sc
            {
                let first = &mut self.lines[sr];
                let keep = first[..sc.min(first.len())].to_vec();
                *first = keep;
            }
            // Remove middle full lines
            if er > sr + 1 {
                self.lines.drain(sr + 1..er);
            }
            // Now sr+1 is the original end line; splice tail from ec
            {
                let tail = self.lines.get(sr + 1).cloned().unwrap_or_default();
                let mut keep_tail = Vec::new();
                keep_tail.extend_from_slice(&tail[ec.min(tail.len())..]);
                // Merge tail into first line
                let first = &mut self.lines[sr];
                first.extend_from_slice(&keep_tail);
            }
            // Drop the extra merged line
            if self.lines.len() > sr + 1 {
                self.lines.remove(sr + 1);
            }
        }
        // Move cursor to start of selection
        self.row = sr;
        self.col = sc.min(self.lines[sr].len());
        self.clear_selection();
        self.dirty = true;
        self.ensure_cursor_visible();
        true
    }

    /// Extract selected content as lines (without trailing newlines).
    fn extract_selection_lines(
        &self,
        ((sr, sc), (er, ec)): ((usize, usize), (usize, usize)),
    ) -> Vec<Vec<u8>> {
        if sr == er {
            let line = &self.lines[sr];
            let a = sc.min(line.len());
            let b = ec.min(line.len()).max(a);
            vec![line[a..b].to_vec()]
        } else {
            let mut out = Vec::new();
            // First line: sc..end
            let first = &self.lines[sr];
            let a = sc.min(first.len());
            out.push(first[a..].to_vec());
            // Middle whole lines
            for r in (sr + 1)..er {
                out.push(self.lines[r].clone());
            }
            // Last line: 0..ec
            let last = &self.lines[er];
            let b = ec.min(last.len());
            out.push(last[..b].to_vec());
            out
        }
    }

    /// Insert given block (sequence of lines) at current cursor.
    fn insert_block(&mut self, block: &[Vec<u8>]) {
        if block.is_empty() {
            return;
        }
        self.push_undo();
        let r = self.row;
        let c = self.col.min(self.lines[r].len());
        let suffix = self.lines[r][c..].to_vec();
        // First line gets appended to prefix
        self.lines[r].truncate(c);
        self.lines[r].extend_from_slice(&block[0]);
        if block.len() == 1 {
            // Same line insertion
            self.lines[r].extend_from_slice(&suffix);
            self.col = c + block[0].len();
        } else {
            // Insert middle lines
            let mut insert_at = r + 1;
            for mid in &block[1..block.len() - 1] {
                self.lines.insert(insert_at, mid.clone());
                insert_at += 1;
            }
            // Last line merges with original suffix
            let mut last_line = block.last().cloned().unwrap_or_default();
            last_line.extend_from_slice(&suffix);
            self.lines.insert(insert_at, last_line);
            // Cursor to end of inserted last line prefix (before original suffix)
            self.row = insert_at;
            self.col = block.last().map(|v| v.len()).unwrap_or(0);
        }
        self.dirty = true;
        self.ensure_cursor_visible();
    }

    /// Returns per-line selection spans for the current viewport (start..end exclusive in cols).
    pub fn selection_spans_for_view(
        &self,
        view_row: usize,
        view_col: usize,
        height: usize,
        width: usize,
    ) -> Vec<Option<(usize, usize)>> {
        let mut spans = vec![None; height];
        let Some(((sr, sc), (er, ec))) = self.selection_bounds() else {
            return spans;
        };
        for (i, span) in spans.iter_mut().enumerate() {
            let li = view_row + i;
            if li < sr || li > er {
                continue;
            }
            let (start_c, end_c) = if sr == er {
                (sc, ec)
            } else if li == sr {
                (sc, self.lines[li].len())
            } else if li == er {
                (0, ec)
            } else {
                (0, self.lines[li].len())
            };
            // Map to viewport columns
            let mut a = start_c.saturating_sub(view_col);
            let mut b = end_c.saturating_sub(view_col);
            if end_c <= view_col || start_c >= view_col + width {
                continue;
            }
            // Clamp to [0,width]
            a = a.min(width);
            b = b.min(width);
            *span = Some((a, b));
        }
        spans
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

fn find_bytes_ascii_ci(hay: &[u8], needle: &[u8], start: usize) -> Option<usize> {
    if needle.is_empty() || start >= hay.len() {
        return None;
    }
    // Lowercased needle (ASCII)
    let mut nlc = Vec::with_capacity(needle.len());
    for &b in needle {
        nlc.push(ascii_lower(b));
    }
    let nlen = nlc.len();
    let mut i = start;
    while i + nlen <= hay.len() {
        let mut ok = true;
        for j in 0..nlen {
            if ascii_lower(hay[i + j]) != nlc[j] {
                ok = false;
                break;
            }
        }
        if ok {
            return Some(i);
        }
        i += 1;
    }
    None
}

#[inline]
fn ascii_lower(b: u8) -> u8 {
    if b.is_ascii_uppercase() {
        b + 32
    } else {
        b
    }
}

fn normalize_points(a: (usize, usize), b: (usize, usize)) -> ((usize, usize), (usize, usize)) {
    if a.0 < b.0 || (a.0 == b.0 && a.1 <= b.1) {
        (a, b)
    } else {
        (b, a)
    }
}

fn point_in_range(p: (usize, usize), r: ((usize, usize), (usize, usize))) -> bool {
    let ((sr, sc), (er, ec)) = r;
    if p.0 < sr || p.0 > er {
        return false;
    }
    if sr == er {
        return p.1 >= sc && p.1 < ec;
    }
    if p.0 == sr {
        return p.1 >= sc;
    }
    if p.0 == er {
        return p.1 < ec;
    }
    true
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

    #[test]
    fn search_wrap_and_case_insensitive() {
        let mut b = EditorBuffer::from_bytes(b"One\ntwo\nThree\n", None);
        // Case-sensitive forward no wrap
        assert_eq!(b.search_forward_opts(b"three", false, false), None);
        // Case-insensitive forward with wrap should find at row 2, col 0
        assert_eq!(b.search_forward_opts(b"three", true, true), Some((2, 0)));
        // Next with wrap cycles to "One" when searching "one"
        b.set_search_case_insensitive(true);
        b.last_search = b"one".to_vec();
        b.row = 2;
        b.col = 1;
        assert_eq!(b.search_next_opts(true), Some((0, 0)));
    }

    #[test]
    fn replace_next_and_all() {
        let mut b = EditorBuffer::from_bytes(b"abc ABC abc\nzz", None);
        // Replace next, case-insensitive
        b.row = 0;
        b.col = 0;
        assert_eq!(b.replace_next(b"ABC", b"X", true, false), Some((0, 0)));
        assert_eq!(b.to_bytes(), b"X ABC abc\nzz");
        // Replace all remaining, case-insensitive
        let n = b.replace_all(b"abc", b"Y", true);
        assert_eq!(n, 2);
        assert_eq!(b.to_bytes(), b"X Y Y\nzz");
    }

    #[test]
    fn block_copy_move_delete() {
        let mut b = EditorBuffer::from_bytes(b"hello\nworld\n!", None);
        // Select "ello\nwor"
        b.row = 0;
        b.col = 1;
        b.mark_start();
        b.row = 1;
        b.col = 3;
        b.mark_end();
        // Copy at end of buffer
        b.row = 2;
        b.col = 0;
        assert!(b.copy_block_here());
        // Expect: "hello\nworld\nello\nwor!"
        assert_eq!(b.to_bytes(), b"hello\nworld\nello\nwor!");
        // Rebuild selection again on first occurrence and move to top
        b.row = 0;
        b.col = 1;
        b.mark_start();
        b.row = 1;
        b.col = 3;
        b.mark_end();
        b.row = 0;
        b.col = 0;
        assert!(b.move_block_here());
        // After move: "ello\nworhld\nello\nwor!"
        assert_eq!(b.to_bytes(), b"ello\nworhld\nello\nwor!");
        // Select trailing "wor" on last line and delete
        b.row = 3;
        b.col = 0;
        b.mark_start();
        b.row = 3;
        b.col = 3;
        b.mark_end();
        assert!(b.delete_selection());
        assert_eq!(b.to_bytes(), b"ello\nworhld\nello\n!");
    }
}
