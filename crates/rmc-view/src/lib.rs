//! Internal viewer logic for Rusty Midnight Commander.
//! This crate contains windowed, Unicode-safe rendering helpers for the viewer (F3),
//! including text/hex modes, wrapping, navigation, and searching.
//! UI crates are expected to provide chrome (frame, titles, status) and feed sizes/keys.

use anyhow::{bail, Result};
use std::cmp::min;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use unicode_width::UnicodeWidthChar;

/// Maximum bytes to read per window. Large enough for wrapping but bounded.
const WINDOW_BYTES: usize = 256 * 1024; // 256 KiB
const LOOKBACK_BYTES: usize = 8 * 1024; // include some context before offset to find boundaries

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ViewOptions {
    pub hex: bool,
    pub wrap: bool,
    /// When true, visualize carriage return at EOL as ^M (CRLF files)
    pub show_cr: bool,
}

#[derive(Debug, Clone)]
pub struct RenderResult {
    pub lines: Vec<String>,
    /// Normalized offset used for this render (may be shifted to a sane boundary)
    pub offset: u64,
    pub eof: bool,
    /// Byte offset of the first visible byte of the last rendered line's next line
    /// (i.e., where to continue when paging down by screenful).
    pub next_screen_offset: u64,
}

pub fn file_len(path: &Path) -> Result<u64> {
    Ok(std::fs::metadata(path)?.len())
}

pub fn clamp_offset(path: &Path, offset: u64) -> Result<u64> {
    let len = file_len(path)?;
    Ok(min(offset, len))
}

/// Render a window of content for the given file starting at (approximately) `offset`.
/// The content area height should exclude chrome (frame/title/status) if those are handled by UI.
pub fn render_window(
    path: &Path,
    opts: ViewOptions,
    mut offset: u64,
    cols: u16,
    rows: u16,
) -> Result<RenderResult> {
    let cols = cols.max(1);
    let rows = rows.max(1);
    let len = file_len(path)?;
    if len == 0 {
        return Ok(RenderResult {
            lines: vec![],
            offset: 0,
            eof: true,
            next_screen_offset: 0,
        });
    }
    offset = min(offset, len.saturating_sub(1));
    let mut f = File::open(path)?;
    if opts.hex {
        render_hex(&mut f, offset, cols, rows)
    } else {
        render_text(&mut f, offset, cols, rows, opts.wrap, opts.show_cr)
    }
}

fn render_hex(f: &mut File, offset: u64, _cols: u16, rows: u16) -> Result<RenderResult> {
    // 16 bytes per row hex with ASCII column
    let lines_capacity = rows as usize;
    let mut buf = vec![0u8; 16 * lines_capacity];
    f.seek(SeekFrom::Start(offset))?;
    let read = f.read(&mut buf)?;
    buf.truncate(read);
    let mut lines = Vec::with_capacity(lines_capacity);
    for chunk in buf.chunks(16) {
        let hexs: Vec<String> = chunk.iter().map(|b| format!("{b:02X}")).collect();
        let text: String = chunk
            .iter()
            .map(|&b| {
                if (32..=126).contains(&b) {
                    b as char
                } else {
                    '.'
                }
            })
            .collect();
        // 16*3 - 1 = 47 chars for hex (including spaces), then two spaces, then ascii 16
        lines.push(format!("{:47}  {}", hexs.join(" "), text));
        if lines.len() >= lines_capacity {
            break;
        }
    }
    let eof = offset.saturating_add(read as u64) >= f.metadata()?.len();
    let next_screen_offset = offset.saturating_add((16 * lines.len()) as u64);
    Ok(RenderResult {
        lines,
        offset,
        eof,
        next_screen_offset,
    })
}

fn render_text(
    f: &mut File,
    offset: u64,
    cols: u16,
    rows: u16,
    wrap: bool,
    show_cr: bool,
) -> Result<RenderResult> {
    let len = f.metadata()?.len();
    if len == 0 {
        return Ok(RenderResult {
            lines: vec![],
            offset: 0,
            eof: true,
            next_screen_offset: 0,
        });
    }
    // Read a window with lookback to find a line boundary before offset.
    let start = offset.saturating_sub(LOOKBACK_BYTES as u64);
    f.seek(SeekFrom::Start(start))?;
    let mut buf = vec![0u8; WINDOW_BYTES + LOOKBACK_BYTES];
    let read = f.read(&mut buf)?;
    buf.truncate(read);
    let rel_off = (offset - start) as usize;
    // Find a reasonable line start at/before rel_off
    let line_start = find_prev_line_start(&buf, rel_off);
    let mut visual_lines = Vec::new();
    let mut i = line_start;
    while i < buf.len() && (visual_lines.len() as u16) < rows {
        // Extract one physical line
        let line_end = find_line_end(&buf, i);
        let slice = &buf[i..line_end];
        // Lossy decode to avoid panics on binary
        let mut line = String::from_utf8_lossy(slice).to_string();
        // If requested, show CR as ^M at EOL when present.
        // Detect robustly around CRLF or lone CR:
        // - If current newline marker is '\r' (line_end points at CR) -> show ^M
        // - Or if current newline is '\n' but the last byte before it is '\r' -> show ^M
        let cr_at_newline = (line_end < buf.len() && buf[line_end] == b'\r')
            || (line_end > 0
                && line_end <= buf.len()
                && buf[line_end.saturating_sub(1)] == b'\r');
        if show_cr && cr_at_newline {
            line.push('^');
            line.push('M');
        }
        // Render with wrapping if enabled
        if wrap {
            wrap_line(&line, cols as usize, &mut visual_lines);
        } else {
            visual_lines.push(truncate_line(&line, cols as usize));
        }
        // Skip newline characters (\r?\n)
        i = next_line_start(&buf, line_end);
    }
    // Compute the byte offset for the start of the next screen
    let mut next_screen_offset = start + i as u64;
    if next_screen_offset > len {
        next_screen_offset = len;
    }
    let eof = next_screen_offset >= len;
    // Adjust normalized offset to the computed line_start
    let normalized_offset = start + line_start as u64;
    Ok(RenderResult {
        lines: visual_lines,
        offset: normalized_offset,
        eof,
        next_screen_offset,
    })
}

/// Return the byte offset at the start of the given 1-based line number.
/// Line 1 corresponds to offset 0. If the line is past EOF, returns EOF.
pub fn goto_line(path: &Path, line_number: u64) -> Result<u64> {
    if line_number <= 1 {
        return Ok(0);
    }
    let mut f = File::open(path)?;
    let len = f.metadata()?.len();
    if len == 0 {
        return Ok(0);
    }
    let mut cur_line: u64 = 1;
    let mut pos: u64 = 0;
    let mut buf = vec![0u8; 256 * 1024];
    while pos < len && cur_line < line_number {
        f.seek(SeekFrom::Start(pos))?;
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        for &b in &buf[..n] {
            if b == b'\n' {
                cur_line += 1;
                if cur_line == line_number {
                    // Position is after this newline
                    pos += 1;
                    return Ok(pos);
                }
            }
            pos += 1;
            if pos >= len {
                break;
            }
        }
    }
    Ok(len)
}

/// Compute 1-based logical line number at byte offset.
/// Returns at least 1. Counts '\n' before offset.
pub fn line_number_at(path: &Path, offset: u64) -> Result<u64> {
    let mut f = File::open(path)?;
    let len = f.metadata()?.len();
    if len == 0 {
        return Ok(1);
    }
    let mut count: u64 = 1;
    let mut pos: u64 = 0;
    let target = offset.min(len);
    let mut buf = vec![0u8; 256 * 1024];
    while pos < target {
        f.seek(SeekFrom::Start(pos))?;
        let want = (target - pos).min(buf.len() as u64) as usize;
        let n = f.read(&mut buf[..want])?;
        if n == 0 {
            break;
        }
        count += buf[..n].iter().filter(|&&b| b == b'\n').count() as u64;
        pos += n as u64;
    }
    Ok(count)
}
fn find_prev_line_start(buf: &[u8], mut idx: usize) -> usize {
    if idx >= buf.len() {
        idx = buf.len().saturating_sub(1);
    }
    // If idx points to '\n', move before it
    if idx > 0 && buf[idx - 1] == b'\n' {
        return idx;
    }
    let mut i = idx;
    while i > 0 {
        if buf[i - 1] == b'\n' {
            break;
        }
        i -= 1;
    }
    i
}

fn find_line_end(buf: &[u8], mut i: usize) -> usize {
    while i < buf.len() && buf[i] != b'\n' && buf[i] != b'\r' {
        i += 1;
    }
    i
}

fn next_line_start(buf: &[u8], mut i: usize) -> usize {
    // Skip optional CR and mandatory NL
    if i < buf.len() && buf[i] == b'\r' {
        i += 1;
    }
    if i < buf.len() && buf[i] == b'\n' {
        i += 1;
    }
    i
}

fn truncate_line(s: &str, max_cols: usize) -> String {
    // Use display width for a closer approximation; append ellipsis if truncated
    let mut width = 0usize;
    let mut out = String::new();
    for ch in s.chars() {
        let w = ch.width().unwrap_or(0);
        if width + w > max_cols.saturating_sub(1) {
            out.push('…');
            return out;
        }
        out.push(ch);
        width += w;
        if width >= max_cols {
            break;
        }
    }
    out
}

fn wrap_line(s: &str, max_cols: usize, out: &mut Vec<String>) {
    if max_cols == 0 {
        out.push(String::new());
        return;
    }
    let mut cur = String::new();
    let mut width = 0usize;
    for ch in s.chars() {
        let w = ch.width().unwrap_or(0);
        if width + w > max_cols {
            out.push(std::mem::take(&mut cur));
            width = 0;
        }
        cur.push(ch);
        width += w;
        if width == max_cols {
            out.push(std::mem::take(&mut cur));
            width = 0;
        }
    }
    out.push(cur);
}

/// Move one logical line down from `offset` and return the new offset (start of next line).
pub fn nav_line_down(path: &Path, offset: u64) -> Result<u64> {
    let mut f = File::open(path)?;
    let len = f.metadata()?.len();
    if offset >= len {
        return Ok(len);
    }
    let start = offset;
    let mut buf = vec![0u8; 64 * 1024];
    let mut pos = start;
    loop {
        f.seek(SeekFrom::Start(pos))?;
        let n = f.read(&mut buf)?;
        if n == 0 {
            return Ok(len);
        }
        if let Some(i) = buf[..n].iter().position(|&b| b == b'\n') {
            let new_off = pos + i as u64 + 1;
            return Ok(new_off);
        }
        pos += n as u64;
        if pos >= len {
            return Ok(len);
        }
    }
}

/// Move one logical line up from `offset` and return the new offset (start of previous line).
pub fn nav_line_up(path: &Path, offset: u64) -> Result<u64> {
    if offset == 0 {
        return Ok(0);
    }
    let mut f = File::open(path)?;
    let mut pos = offset.saturating_sub(1);
    let mut carry = Vec::new();
    loop {
        let chunk_start = pos.saturating_sub(64 * 1024 - 1);
        let read_len = (pos - chunk_start + 1) as usize;
        let mut buf = vec![0u8; read_len];
        f.seek(SeekFrom::Start(chunk_start))?;
        f.read_exact(&mut buf)?;
        // Exclude the trailing newline at pos if present
        let search_end = buf.len().saturating_sub(carry.len());
        if let Some(i) = buf[..search_end].iter().rposition(|&b| b == b'\n') {
            return Ok(chunk_start + i as u64 + 1);
        }
        if chunk_start == 0 {
            return Ok(0);
        }
        pos = chunk_start.saturating_sub(1);
        carry = buf;
    }
}

pub fn nav_page_down(path: &Path, offset: u64, cols: u16, rows: u16, wrap: bool) -> Result<u64> {
    let rr = render_window(
        path,
        ViewOptions {
            hex: false,
            wrap,
            show_cr: false,
        },
        offset,
        cols,
        rows,
    )?;
    Ok(rr.next_screen_offset)
}

pub fn nav_page_up(path: &Path, offset: u64, cols: u16, rows: u16, wrap: bool) -> Result<u64> {
    // Walk up by rendering from an earlier offset until current offset fits as next screen
    let len = file_len(path)?;
    if offset == 0 || len == 0 {
        return Ok(0);
    }
    // Heuristic: move back by 2 windows and render forward one page
    let back = (WINDOW_BYTES as u64).saturating_mul(2);
    let start = offset.saturating_sub(back);
    let rr = render_window(
        path,
        ViewOptions {
            hex: false,
            wrap,
            show_cr: false,
        },
        start,
        cols,
        rows,
    )?;
    // If our current offset is before rr.next_screen_offset, we are already on first page; else keep paging
    if offset <= rr.next_screen_offset {
        return Ok(rr.offset);
    }
    // Otherwise, try to approximate previous page by stepping down until we are just before current offset
    let mut cur = rr.offset;
    loop {
        let r = render_window(
            path,
            ViewOptions {
                hex: false,
                wrap,
                show_cr: false,
            },
            cur,
            cols,
            rows,
        )?;
        if r.next_screen_offset >= offset || r.eof {
            return Ok(cur);
        }
        if r.next_screen_offset <= cur {
            // safeguard
            return Ok(cur);
        }
        cur = r.next_screen_offset;
    }
}

pub fn nav_home() -> u64 {
    0
}

pub fn nav_end(path: &Path, cols: u16, rows: u16, wrap: bool) -> Result<u64> {
    let len = file_len(path)?;
    if len == 0 {
        return Ok(0);
    }
    // Walk backward from end by a window and then advance to show the last page
    let start = len.saturating_sub(WINDOW_BYTES as u64);
    let mut cur = start;
    loop {
        let r = render_window(
            path,
            ViewOptions {
                hex: false,
                wrap,
                show_cr: false,
            },
            cur,
            cols,
            rows,
        )?;
        if r.eof {
            return Ok(r.offset);
        }
        if r.next_screen_offset <= cur {
            return Ok(cur);
        }
        cur = r.next_screen_offset;
        if cur >= len {
            return Ok(r.offset);
        }
    }
}

/// Forward search for UTF-8 `needle` starting at or after `start_offset`.
/// Returns the byte offset of the match when found.
pub fn search_forward(path: &Path, start_offset: u64, needle: &str) -> Result<Option<u64>> {
    if needle.is_empty() {
        bail!("empty search");
    }
    let needle_bytes = needle.as_bytes();
    let mut f = File::open(path)?;
    let len = f.metadata()?.len();
    let mut pos = min(start_offset, len);
    let mut buf = vec![0u8; WINDOW_BYTES + needle_bytes.len()];
    while pos < len {
        f.seek(SeekFrom::Start(pos))?;
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        if let Some(i) = find_subslice(&buf[..n], needle_bytes) {
            return Ok(Some(pos + i as u64));
        }
        if n < buf.len() {
            break;
        }
        // Overlap by needle length to catch cross-boundary matches
        pos = pos.saturating_add((n - needle_bytes.len()).max(1) as u64);
    }
    Ok(None)
}

/// Reverse search for UTF-8 `needle` ending before `start_offset`.
pub fn search_backward(path: &Path, start_offset: u64, needle: &str) -> Result<Option<u64>> {
    if needle.is_empty() {
        bail!("empty search");
    }
    let needle_bytes = needle.as_bytes();
    let mut f = File::open(path)?;
    let len = f.metadata()?.len();
    if start_offset == 0 || len == 0 {
        return Ok(None);
    }
    let mut end = min(start_offset, len);
    loop {
        let chunk_start = end.saturating_sub(WINDOW_BYTES as u64);
        let read_len = (end - chunk_start) as usize;
        let mut buf = vec![0u8; read_len];
        f.seek(SeekFrom::Start(chunk_start))?;
        f.read_exact(&mut buf)?;
        if let Some(i) = rfind_subslice(&buf, needle_bytes) {
            if chunk_start + (i as u64) < start_offset {
                return Ok(Some(chunk_start + i as u64));
            }
        }
        if chunk_start == 0 {
            break;
        }
        end = chunk_start;
    }
    Ok(None)
}

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}
fn rfind_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).rposition(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn hex_renders_chunks() {
        let mut f = NamedTempFile::new().unwrap();
        let data: Vec<u8> = (0..64).collect(); // 4 lines
        f.write_all(&data).unwrap();
        let path = f.path().to_path_buf();
        let r = render_window(
            &path,
            ViewOptions {
                hex: true,
                wrap: false,
                show_cr: false,
            },
            0,
            80,
            10,
        )
        .unwrap();
        assert_eq!(r.lines.len(), 4);
        assert!(r.lines[0].contains("00 01 02 03"));
        assert!(r.lines[0].ends_with("...............")); // ascii part 16 dots or visible
    }

    #[test]
    fn wrap_changes_line_count() {
        let mut f = NamedTempFile::new().unwrap();
        let s = "αβγδεζηθικλμνξοπρστυφχψω\nshort\n";
        f.write_all(s.as_bytes()).unwrap();
        let path = f.path().to_path_buf();
        let r_nowrap = render_window(
            &path,
            ViewOptions {
                hex: false,
                wrap: false,
                show_cr: false,
            },
            0,
            10,
            10,
        )
        .unwrap();
        let r_wrap = render_window(
            &path,
            ViewOptions {
                hex: false,
                wrap: true,
                show_cr: false,
            },
            0,
            10,
            10,
        )
        .unwrap();
        assert!(r_wrap.lines.len() >= r_nowrap.lines.len());
    }

    #[test]
    fn search_forward_backward() {
        let mut f = NamedTempFile::new().unwrap();
        let s = b"abc 123 abc 456 abc";
        f.write_all(s).unwrap();
        let path = f.path().to_path_buf();
        let first = search_forward(&path, 0, "abc").unwrap().unwrap();
        assert_eq!(first, 0);
        let second = search_forward(&path, first + 1, "abc").unwrap().unwrap();
        assert!(second > first);
        let back = search_backward(&path, second, "abc").unwrap().unwrap();
        assert_eq!(back, first);
    }
}
