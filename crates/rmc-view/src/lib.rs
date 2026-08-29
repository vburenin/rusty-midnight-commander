//! Internal viewer logic for Rusty Midnight Commander.
//! This crate contains windowed, Unicode-safe rendering helpers for the viewer (F3),
//! including text/hex modes, wrapping, navigation, and searching.
//! UI crates are expected to provide chrome (frame, titles, status) and feed sizes/keys.

use anyhow::{bail, Result};
use std::cmp::min;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use unicode_width::UnicodeWidthChar;

/// Baked Apache-2.0 `data/mc.ext.ini` `[view]` helpers (not GPL GNU mc.ext.ini).
const SHIPPED_EXT_INI: &str = include_str!("../../../data/mc.ext.ini");

/// Maximum bytes to read per window. Large enough for wrapping but bounded.
const WINDOW_BYTES: usize = 256 * 1024; // 256 KiB
const LOOKBACK_BYTES: usize = 8 * 1024; // include some context before offset to find boundaries

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ViewOptions {
    pub hex: bool,
    pub wrap: bool,
    /// When true, visualize carriage return at EOL as ^M (CRLF files)
    pub show_cr: bool,
    /// GNU mcview format/unformat: interpret nroff overstrike (`c\\x08c`, `_\\x08c`).
    pub format: bool,
}

#[derive(Debug, Clone)]
pub struct RenderResult {
    pub lines: Vec<String>,
    /// File byte range `[start, end)` for each visual line (selection highlight).
    pub line_byte_ranges: Vec<(u64, u64)>,
    /// Normalized offset used for this render (may be shifted to a sane boundary)
    pub offset: u64,
    pub eof: bool,
    /// Byte offset of the first visible byte of the last rendered line's next line
    /// (i.e., where to continue when paging down by screenful).
    pub next_screen_offset: u64,
}

/// How to feed the source file into an external filter command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterInput {
    /// Provide file contents on filter's stdin
    Stdin,
    /// Append the file path as the last argument
    ArgPath,
}

/// External filter specification: program, args, and how to pass input.
#[derive(Debug, Clone)]
pub struct ExternalFilter {
    pub program: String,
    pub args: Vec<String>,
    pub input: FilterInput,
}

impl ExternalFilter {
    pub fn new<P: Into<String>>(program: P) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            input: FilterInput::Stdin,
        }
    }
    pub fn with_args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }
    pub fn with_input(mut self, input: FilterInput) -> Self {
        self.input = input;
        self
    }
}

/// A view target that may reference the original path or a filtered temporary file.
/// Keep this struct alive while rendering/navigating filtered content.
#[derive(Debug)]
pub struct ViewData {
    path: PathBuf,
    /// When Some, holds the temporary file so it is removed on drop.
    _tmp: Option<tempfile::NamedTempFile>,
}

impl ViewData {
    /// Use a real file path directly (no filtering).
    pub fn from_path(path: PathBuf) -> Self {
        Self { path, _tmp: None }
    }
    /// Open a view for `path`, applying a decompress filter from `mc.ext.ini`
    /// `[view]` when the extension matches (e.g. `.gz` → `gzip -dc`).
    ///
    /// Archive paths (`.tar.gz`, `.tgz`, …) are not filtered. If a helper is
    /// missing or the filter exits non-zero, returns `Err` — callers must not
    /// treat the compressed bytes as text.
    pub fn open_view(path: &Path) -> Result<Self> {
        if let Some(filter) = guess_filter_for_path(path) {
            return ViewData::from_filter(path, &filter);
        }
        Ok(ViewData::from_path(path.to_path_buf()))
    }
    /// Apply an external filter once and keep its stdout in a temporary file.
    /// The returned handle owns the temp file while in scope.
    pub fn from_filter(src: &Path, filter: &ExternalFilter) -> Result<Self> {
        let tmp = apply_external_filter_to_temp(src, filter)?;
        let path = tmp.path().to_path_buf();
        Ok(Self {
            path,
            _tmp: Some(tmp),
        })
    }
    /// Path to render/search against.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Run the external filter command against `src` and capture its stdout into a temporary file.
/// Returns an owned temporary file that will be cleaned up on drop.
/// Errors if the helper is missing or the command fails (never returns compressed garbage).
pub fn apply_external_filter_to_temp(
    src: &Path,
    filter: &ExternalFilter,
) -> Result<tempfile::NamedTempFile> {
    if !helper_on_path(&filter.program) {
        bail!("Cannot view: {} not found", filter.program);
    }
    // Prepare the temporary destination first
    let tmp = tempfile::Builder::new()
        .prefix("rmc-view-filter-")
        .suffix(".txt")
        .tempfile()?;
    // Build command
    let mut cmd = Command::new(&filter.program);
    for a in &filter.args {
        cmd.arg(a);
    }
    // Choose how to feed input
    match filter.input {
        FilterInput::Stdin => {
            let infile = File::open(src)?;
            cmd.stdin(Stdio::from(infile));
        }
        FilterInput::ArgPath => {
            cmd.arg(src);
            cmd.stdin(Stdio::null());
        }
    }
    // Direct stdout to the temp file
    let out_file = tmp.reopen()?; // separate handle for the child
    cmd.stdout(Stdio::from(out_file));
    // Silence stderr to avoid polluting test output/terminal
    cmd.stderr(Stdio::null());
    let status = match cmd.status() {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            bail!("Cannot view: {} not found", filter.program);
        }
        Err(e) => return Err(e.into()),
    };
    if !status.success() {
        bail!("Cannot view: {} failed", filter.program);
    }
    Ok(tmp)
}

/// True when `program` is an executable file on `PATH`.
pub fn helper_on_path(program: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| {
        let p = dir.join(program);
        p.is_file()
    })
}

fn normalize_view_ext(key: &str) -> String {
    let k = key.trim().to_ascii_lowercase();
    if k.starts_with('.') {
        k
    } else {
        format!(".{k}")
    }
}

fn parse_view_filters(text: &str) -> HashMap<String, ExternalFilter> {
    let mut section = String::new();
    let mut by_ext = HashMap::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].trim().to_ascii_lowercase();
            continue;
        }
        if section != "view" {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let key = normalize_view_ext(k);
        let cmd = v.trim();
        if key.is_empty() || cmd.is_empty() {
            continue;
        }
        let mut parts = cmd.split_whitespace();
        let Some(program) = parts.next() else {
            continue;
        };
        let args: Vec<String> = parts.map(str::to_string).collect();
        by_ext.insert(
            key,
            ExternalFilter::new(program)
                .with_args(args)
                .with_input(FilterInput::ArgPath),
        );
    }
    by_ext
}

fn view_filter_map() -> &'static HashMap<String, ExternalFilter> {
    static MAP: OnceLock<HashMap<String, ExternalFilter>> = OnceLock::new();
    MAP.get_or_init(|| parse_view_filters(SHIPPED_EXT_INI))
}

/// Filter from `mc.ext.ini` `[view]` for this filename extension.
/// Conservative: archives (`.tar.gz`, `.tgz`, `.cpio.gz`, …) are not filtered
/// so F3/Enter can still VFS-enter them. Maps GNU-documented helpers:
/// - .gz   -> gzip -dc
/// - .bz2  -> bzip2 -dc
/// - .xz / .lzma -> xz -dc
/// - .zst  -> zstd -dc
/// - .lz4  -> lz4 -dc
/// - .lz   -> lzip -dc
pub fn guess_filter_for_path(path: &Path) -> Option<ExternalFilter> {
    if rmc_fs::pathutil::detect_archive_kind(path).is_some() {
        return None;
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())?;
    view_filter_map().get(&format!(".{ext}")).cloned()
}

pub fn file_len(path: &Path) -> Result<u64> {
    Ok(std::fs::metadata(path)?.len())
}

/// GNU mcview 4.8.x row-0 status: path left, `bytes/total` or `0xOFFSET` anchored
/// so `/` (text) or the last hex digit sit at `cols-23`, percent flush right.
pub fn gnu_status_line(
    cols: usize,
    path: &Path,
    hex: bool,
    view_offset: u64,
    end_bytes: u64,
    total: u64,
) -> String {
    if cols == 0 {
        return String::new();
    }
    let pct = if total == 0 {
        100
    } else if hex {
        view_offset.saturating_mul(100) / total
    } else {
        end_bytes.saturating_mul(100) / total
    };
    let pct_s = format!("{pct}%");
    let mut line = vec![' '; cols];
    let pct_ch: Vec<char> = pct_s.chars().collect();
    if pct_ch.len() <= cols {
        let start = cols - pct_ch.len();
        for (i, ch) in pct_ch.iter().enumerate() {
            line[start + i] = *ch;
        }
    }
    // Live 80-col GNU: `/` of `1738/3160` and last digit of `0x00000000` sit at col 57.
    let anchor = cols.saturating_sub(23);
    if hex {
        let mid: Vec<char> = format!("0x{view_offset:08X}").chars().collect();
        let start = (anchor + 1).saturating_sub(mid.len());
        for (i, ch) in mid.iter().enumerate() {
            if start + i < cols {
                line[start + i] = *ch;
            }
        }
    } else {
        let left: Vec<char> = end_bytes.to_string().chars().collect();
        let right: Vec<char> = total.to_string().chars().collect();
        if anchor < cols {
            line[anchor] = '/';
        }
        let ls = anchor.saturating_sub(left.len());
        for (i, ch) in left.iter().enumerate() {
            if ls + i < cols {
                line[ls + i] = *ch;
            }
        }
        for (i, ch) in right.iter().enumerate() {
            if anchor + 1 + i < cols {
                line[anchor + 1 + i] = *ch;
            }
        }
    }
    let mut path_max = 0usize;
    while path_max < cols && line[path_max] == ' ' {
        path_max += 1;
    }
    path_max = path_max.saturating_sub(1); // keep one space before the mid field
    if path_max > 0 {
        let shown = gnu_mid_tilde_trunc(&path.display().to_string(), path_max);
        for (i, ch) in shown.chars().enumerate() {
            if i < cols {
                line[i] = ch;
            }
        }
    }
    line.into_iter().collect()
}

fn gnu_mid_tilde_trunc(s: &str, width: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= width {
        return chars.into_iter().collect();
    }
    if width == 0 {
        return String::new();
    }
    if width == 1 {
        return "~".to_string();
    }
    let keep_left = (width - 1) / 2;
    let keep_right = width - 1 - keep_left;
    let mut out: String = chars.iter().take(keep_left).collect();
    out.push('~');
    out.extend(chars.iter().skip(chars.len() - keep_right));
    out
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
            line_byte_ranges: vec![],
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
        render_text(
            &mut f,
            offset,
            cols,
            rows,
            opts.wrap,
            opts.show_cr,
            opts.format,
        )
    }
}

/// Live GNU mcview 4.8.30 hex dump on 80 cols: 8-digit address, four 4-byte
/// groups separated by ` │ `, ASCII (0x20..=0x7E else `.`) starting at column 63.
const GNU_HEX_ASCII_COL: usize = 63;
const GNU_HEX_GROUP_SEP: char = '│';

fn gnu_hex_ascii_char(b: u8) -> char {
    if (0x20..=0x7e).contains(&b) {
        b as char
    } else {
        '.'
    }
}

/// One GNU hex-dump row (address + grouped bytes + ASCII). No trailing pad;
/// the viewer fill supplies the rest of the line.
fn format_gnu_hex_line(offset: u64, chunk: &[u8]) -> String {
    let mut hex = String::new();
    for (i, &b) in chunk.iter().enumerate() {
        if i > 0 {
            hex.push(' ');
        }
        hex.push_str(&format!("{b:02X}"));
        if i % 4 == 3 && i != 15 {
            hex.push(' ');
            hex.push(GNU_HEX_GROUP_SEP);
        }
    }
    let ascii: String = chunk.iter().copied().map(gnu_hex_ascii_char).collect();
    let mut line = format!("{offset:08X} {hex}");
    while line.chars().count() < GNU_HEX_ASCII_COL {
        line.push(' ');
    }
    line.push_str(&ascii);
    line
}

fn render_hex(f: &mut File, offset: u64, _cols: u16, rows: u16) -> Result<RenderResult> {
    // GNU mcview: 16 bytes per row, address + `│` 4-byte groups + ASCII.
    let lines_capacity = rows as usize;
    let mut buf = vec![0u8; 16 * lines_capacity];
    f.seek(SeekFrom::Start(offset))?;
    let read = f.read(&mut buf)?;
    buf.truncate(read);
    let mut lines = Vec::with_capacity(lines_capacity);
    for (i, chunk) in buf.chunks(16).enumerate() {
        let row_off = offset.saturating_add((i * 16) as u64);
        lines.push(format_gnu_hex_line(row_off, chunk));
        if lines.len() >= lines_capacity {
            break;
        }
    }
    let eof = offset.saturating_add(read as u64) >= f.metadata()?.len();
    let next_screen_offset = offset.saturating_add((16 * lines.len()) as u64);
    let mut line_byte_ranges = Vec::with_capacity(lines.len());
    for (i, chunk) in buf.chunks(16).enumerate() {
        let start = offset.saturating_add((i * 16) as u64);
        line_byte_ranges.push((start, start.saturating_add(chunk.len() as u64)));
    }
    Ok(RenderResult {
        lines,
        line_byte_ranges,
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
    format: bool,
) -> Result<RenderResult> {
    let len = f.metadata()?.len();
    if len == 0 {
        return Ok(RenderResult {
            lines: vec![],
            line_byte_ranges: vec![],
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
    let mut line_byte_ranges = Vec::new();
    let mut i = line_start;
    while i < buf.len() && (visual_lines.len() as u16) < rows {
        // Extract one physical line
        let line_end = find_line_end(&buf, i);
        let slice = &buf[i..line_end];
        let phys_start = start + i as u64;
        let phys_end = start + line_end as u64;
        // Lossy decode to avoid panics on binary
        let decoded = if format {
            String::from_utf8_lossy(&apply_nroff_overstrike(slice)).to_string()
        } else {
            String::from_utf8_lossy(slice).to_string()
        };
        let mut line = decoded;
        // If requested, show CR as ^M at EOL when present.
        // Detect robustly around CRLF or lone CR:
        // - If current newline marker is '\r' (line_end points at CR) -> show ^M
        // - Or if current newline is '\n' but the last byte before it is '\r' -> show ^M
        let cr_at_newline = (line_end < buf.len() && buf[line_end] == b'\r')
            || (line_end > 0 && line_end <= buf.len() && buf[line_end.saturating_sub(1)] == b'\r');
        if show_cr && cr_at_newline {
            line.push('^');
            line.push('M');
        }
        // Render with wrapping if enabled
        if wrap {
            let before = visual_lines.len();
            wrap_line(&line, cols as usize, &mut visual_lines);
            for _ in before..visual_lines.len() {
                line_byte_ranges.push((phys_start, phys_end));
            }
        } else {
            visual_lines.push(truncate_line(&line, cols as usize));
            line_byte_ranges.push((phys_start, phys_end));
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
        line_byte_ranges,
        offset: normalized_offset,
        eof,
        next_screen_offset,
    })
}

/// GNU mcview format mode: nroff overstrike. `c\\x08c` is bold, `_\\x08c` / `c\\x08_`
/// is underline. Public mc(1): format interprets those sequences with colors.
fn apply_nroff_overstrike(buf: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(buf.len());
    let mut i = 0;
    while i < buf.len() {
        if i + 2 < buf.len() && buf[i + 1] == 0x08 {
            let a = buf[i];
            let b = buf[i + 2];
            let glyph = if a == b'_' { b } else { a };
            out.push(glyph);
            i += 3;
            continue;
        }
        if buf[i] == 0x08 {
            i += 1;
            continue;
        }
        out.push(buf[i]);
        i += 1;
    }
    out
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
            format: false,
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
            format: false,
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
                format: false,
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
                format: false,
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

/// GNU mcview 4.8.33 Search-dialog type radios.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SearchKind {
    #[default]
    Normal,
    RegularExpression,
    Hexadecimal,
    WildcardSearch,
}

/// GNU mcview Search-dialog flags (type radios + four checkboxes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SearchOptions {
    pub case_sensitive: bool,
    pub backwards: bool,
    pub whole_words: bool,
    pub kind: SearchKind,
    pub all_charsets: bool,
}

/// Search with GNU mcview Search-dialog options. Empty needle is a no-op.
/// Invalid regular expressions do not panic; they yield no match.
/// Invalid hexadecimal patterns return an error whose message uses the GNU
/// `Hex pattern error at position %d:` wording.
pub fn search_with_options(
    path: &Path,
    start_offset: u64,
    needle: &str,
    opts: SearchOptions,
    wrap: bool,
) -> Result<Option<u64>> {
    search_impl(path, start_offset, needle, opts, wrap, false)
}

/// Repeat a search from `start_offset`, skipping a match that begins there.
pub fn search_next_with_options(
    path: &Path,
    start_offset: u64,
    needle: &str,
    opts: SearchOptions,
    wrap: bool,
) -> Result<Option<u64>> {
    search_impl(path, start_offset, needle, opts, wrap, true)
}

fn search_impl(
    path: &Path,
    start_offset: u64,
    needle: &str,
    opts: SearchOptions,
    wrap: bool,
    skip_current: bool,
) -> Result<Option<u64>> {
    if needle.is_empty() {
        return Ok(None);
    }
    let hay = std::fs::read(path)?;
    let prepared = prepare_search_needles(needle, opts)?;
    if prepared.is_empty() {
        return Ok(None);
    }
    let len = hay.len() as u64;

    let orig = start_offset.min(len);
    let from = if skip_current {
        if opts.backwards {
            orig
        } else {
            orig.saturating_add(1)
        }
    } else {
        orig
    };

    if let Some(pos) = find_prepared(&hay, &prepared, from, opts, skip_current) {
        return Ok(Some(pos));
    }
    if wrap {
        if opts.backwards {
            if let Some(pos) = find_prepared(&hay, &prepared, len, opts, false) {
                if pos >= orig {
                    return Ok(Some(pos));
                }
            }
        } else if let Some(pos) = find_prepared(&hay, &prepared, 0, opts, false) {
            if pos <= orig {
                return Ok(Some(pos));
            }
        }
    }
    Ok(None)
}

enum PreparedNeedle {
    Bytes(Vec<u8>),
    Regex(regex::bytes::Regex),
}

fn prepare_search_needles(needle: &str, opts: SearchOptions) -> Result<Vec<PreparedNeedle>> {
    let encodings = charset_encodings(needle, opts.all_charsets);
    let mut out = Vec::new();
    let mut hex_err = None;
    for enc in encodings {
        match opts.kind {
            SearchKind::Hexadecimal => match parse_hex_pattern(&enc) {
                Ok(bytes) if !bytes.is_empty() => out.push(PreparedNeedle::Bytes(bytes)),
                Ok(_) => {}
                Err(e) => {
                    if hex_err.is_none() {
                        hex_err = Some(e);
                    }
                }
            },
            SearchKind::RegularExpression => {
                if let Some(re) = compile_search_regex(&enc, !opts.case_sensitive, opts.whole_words)
                {
                    out.push(PreparedNeedle::Regex(re));
                }
            }
            SearchKind::WildcardSearch => {
                let Some(pat) = std::str::from_utf8(&enc).ok() else {
                    continue;
                };
                let translated = glob_to_regex(pat);
                if let Some(re) = compile_search_regex(
                    translated.as_bytes(),
                    !opts.case_sensitive,
                    opts.whole_words,
                ) {
                    out.push(PreparedNeedle::Regex(re));
                }
            }
            SearchKind::Normal => {
                if !enc.is_empty() {
                    out.push(PreparedNeedle::Bytes(enc));
                }
            }
        }
    }
    if opts.kind == SearchKind::Hexadecimal && out.is_empty() {
        if let Some(e) = hex_err {
            bail!("{e}");
        }
    }
    Ok(out)
}

fn find_prepared(
    hay: &[u8],
    prepared: &[PreparedNeedle],
    from: u64,
    opts: SearchOptions,
    backwards_exclusive: bool,
) -> Option<u64> {
    let mut best = None;
    for item in prepared {
        let hit = match item {
            PreparedNeedle::Bytes(needle) => {
                let mut byte_opts = opts;
                if opts.kind == SearchKind::Hexadecimal {
                    byte_opts.whole_words = false;
                }
                find_in_hay(hay, needle, from, byte_opts, None, backwards_exclusive)
            }
            PreparedNeedle::Regex(re) => {
                find_in_hay(hay, b"", from, opts, Some(re), backwards_exclusive)
            }
        };
        best = pick_hit(best, hit, opts.backwards);
    }
    best
}

fn pick_hit(best: Option<u64>, hit: Option<u64>, backwards: bool) -> Option<u64> {
    match (best, hit) {
        (None, h) | (h, None) => h,
        (Some(a), Some(b)) if backwards => Some(a.max(b)),
        (Some(a), Some(b)) => Some(a.min(b)),
    }
}

/// GNU 4.8.33 hex-pattern wording (`Hex pattern error at position %d:\n%s.`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct HexPatternError {
    position: usize,
    detail: &'static str,
}

impl std::fmt::Display for HexPatternError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Hex pattern error at position {}:\n{}.",
            self.position, self.detail
        )
    }
}

/// Parse a GNU mcview Hexadecimal needle: whitespace-separated byte numbers
/// (optional `0x` prefix) mixed with double-quoted literal spans.
fn parse_hex_pattern(input: &[u8]) -> Result<Vec<u8>, HexPatternError> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < input.len() {
        while i < input.len() && input[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= input.len() {
            break;
        }
        if input[i] == b'"' {
            let quote_pos = i + 1;
            i += 1;
            let start = i;
            while i < input.len() && input[i] != b'"' {
                i += 1;
            }
            if i >= input.len() {
                return Err(HexPatternError {
                    position: quote_pos,
                    detail: "Unmatched quotes character",
                });
            }
            out.extend_from_slice(&input[start..i]);
            i += 1;
            continue;
        }
        let tok_pos = i + 1;
        let tok_start = i;
        while i < input.len() && !input[i].is_ascii_whitespace() && input[i] != b'"' {
            i += 1;
        }
        match parse_hex_byte(&input[tok_start..i]) {
            Ok(b) => out.push(b),
            Err(detail) => {
                return Err(HexPatternError {
                    position: tok_pos,
                    detail,
                });
            }
        }
    }
    Ok(out)
}

fn parse_hex_byte(tok: &[u8]) -> Result<u8, &'static str> {
    if tok.is_empty() {
        return Err("Invalid character");
    }
    let digits = if let Some(rest) = tok.strip_prefix(b"0x").or_else(|| tok.strip_prefix(b"0X")) {
        rest
    } else {
        tok
    };
    if digits.is_empty() || !digits.iter().all(|b| b.is_ascii_hexdigit()) {
        return Err("Invalid character");
    }
    let s = std::str::from_utf8(digits).map_err(|_| "Invalid character")?;
    match u32::from_str_radix(s, 16) {
        Ok(n) if n <= 0xFF => Ok(n as u8),
        Ok(_) => {
            Err("Number out of range (should be in byte range, 0 <= n <= 0xFF, expressed in hex)")
        }
        Err(_) => Err("Invalid character"),
    }
}

/// Translate a GNU Wildcard-search glob into a regex (`*` → `.*`, `?` → `.`).
fn glob_to_regex(pat: &str) -> String {
    let chars: Vec<char> = pat.chars().collect();
    let mut out = String::new();
    let mut i = 0usize;
    while i < chars.len() {
        match chars[i] {
            '*' => out.push_str(".*"),
            '?' => out.push('.'),
            '[' => {
                out.push('[');
                i += 1;
                if i < chars.len() && (chars[i] == '!' || chars[i] == '^') {
                    out.push('^');
                    i += 1;
                }
                while i < chars.len() && chars[i] != ']' {
                    out.push(chars[i]);
                    i += 1;
                }
                if i < chars.len() && chars[i] == ']' {
                    out.push(']');
                }
            }
            c @ ('\\' | '.' | '+' | '(' | ')' | '|' | '^' | '$' | '{' | '}') => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
        i += 1;
    }
    out
}

/// UTF-8 plus 8-bit recodings used when GNU **All charsets** is on.
fn charset_encodings(needle: &str, all_charsets: bool) -> Vec<Vec<u8>> {
    let mut out = vec![needle.as_bytes().to_vec()];
    if !all_charsets {
        return out;
    }
    if let Some(latin1) = encode_iso8859_1(needle) {
        push_unique_bytes(&mut out, latin1);
    }
    if let Some(cp1252) = encode_windows1252(needle) {
        push_unique_bytes(&mut out, cp1252);
    }
    out
}

fn push_unique_bytes(out: &mut Vec<Vec<u8>>, bytes: Vec<u8>) {
    if !out.iter().any(|e| e == &bytes) {
        out.push(bytes);
    }
}

fn encode_iso8859_1(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len());
    for c in s.chars() {
        let v = u32::from(c);
        if v > 0xFF {
            return None;
        }
        out.push(v as u8);
    }
    Some(out)
}

/// Windows-1252: Latin-1 plus the usual 0x80–0x9F extra letters/symbols.
fn encode_windows1252(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len());
    for c in s.chars() {
        let b = match u32::from(c) {
            v if v <= 0x7F => v as u8,
            v if (0xA0..=0xFF).contains(&v) => v as u8,
            0x20AC => 0x80,
            0x201A => 0x82,
            0x0192 => 0x83,
            0x201E => 0x84,
            0x2026 => 0x85,
            0x2020 => 0x86,
            0x2021 => 0x87,
            0x02C6 => 0x88,
            0x2030 => 0x89,
            0x0160 => 0x8A,
            0x2039 => 0x8B,
            0x0152 => 0x8C,
            0x017D => 0x8E,
            0x2018 => 0x91,
            0x2019 => 0x92,
            0x201C => 0x93,
            0x201D => 0x94,
            0x2022 => 0x95,
            0x2013 => 0x96,
            0x2014 => 0x97,
            0x02DC => 0x98,
            0x2122 => 0x99,
            0x0161 => 0x9A,
            0x203A => 0x9B,
            0x0153 => 0x9C,
            0x017E => 0x9E,
            0x0178 => 0x9F,
            _ => return None,
        };
        out.push(b);
    }
    Some(out)
}

fn find_in_hay(
    hay: &[u8],
    needle: &[u8],
    from: u64,
    opts: SearchOptions,
    re: Option<&regex::bytes::Regex>,
    backwards_exclusive: bool,
) -> Option<u64> {
    if opts.backwards {
        // Next-match from offset 0 has nothing earlier in the file.
        if backwards_exclusive && from == 0 {
            return None;
        }
        let limit = from as usize;
        let max_start = if backwards_exclusive {
            limit.saturating_sub(1)
        } else {
            limit.min(hay.len())
        };
        find_last_match(hay, needle, max_start, opts, re)
    } else {
        let start = (from as usize).min(hay.len());
        find_first_match(hay, needle, start, opts, re)
    }
}

fn find_first_match(
    hay: &[u8],
    needle: &[u8],
    start: usize,
    opts: SearchOptions,
    re: Option<&regex::bytes::Regex>,
) -> Option<u64> {
    if let Some(re) = re {
        return re.find_at(hay, start).map(|m| m.start() as u64);
    }
    let mut pos = start;
    loop {
        let found = if opts.case_sensitive {
            find_bytes(hay, needle, pos)
        } else {
            find_bytes_ascii_ci(hay, needle, pos)
        }?;
        if !opts.whole_words || is_whole_word_at(hay, found, needle.len()) {
            return Some(found as u64);
        }
        pos = found.saturating_add(1);
    }
}

fn find_last_match(
    hay: &[u8],
    needle: &[u8],
    max_start: usize,
    opts: SearchOptions,
    re: Option<&regex::bytes::Regex>,
) -> Option<u64> {
    if let Some(re) = re {
        let mut last = None;
        let mut pos = 0usize;
        while pos <= hay.len() {
            match re.find_at(hay, pos) {
                Some(m) if m.start() <= max_start => {
                    last = Some(m.start() as u64);
                    let next = m.start().saturating_add(1);
                    if next == pos {
                        break;
                    }
                    pos = next;
                }
                Some(_) | None => break,
            }
        }
        return last;
    }
    let mut cap = max_start;
    loop {
        let found = if opts.case_sensitive {
            rfind_bytes(hay, needle, cap)
        } else {
            rfind_bytes_ascii_ci(hay, needle, cap)
        }?;
        if !opts.whole_words || is_whole_word_at(hay, found, needle.len()) {
            return Some(found as u64);
        }
        if found == 0 {
            return None;
        }
        cap = found - 1;
    }
}

fn is_ascii_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn is_whole_word_at(hay: &[u8], start: usize, len: usize) -> bool {
    let before_ok = start == 0 || !is_ascii_word_byte(hay[start - 1]);
    let end = start.saturating_add(len);
    let after_ok = end >= hay.len() || !is_ascii_word_byte(hay[end]);
    before_ok && after_ok
}

fn compile_search_regex(
    needle: &[u8],
    case_insensitive: bool,
    whole_words: bool,
) -> Option<regex::bytes::Regex> {
    let pat = std::str::from_utf8(needle).ok()?;
    let wrapped = if whole_words {
        format!(r"\b(?:{pat})\b")
    } else {
        pat.to_string()
    };
    regex::bytes::RegexBuilder::new(&wrapped)
        .case_insensitive(case_insensitive)
        .unicode(false)
        .build()
        .ok()
}

fn ascii_lower(b: u8) -> u8 {
    b.to_ascii_lowercase()
}

fn find_bytes(hay: &[u8], needle: &[u8], start: usize) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    let start = start.min(hay.len());
    hay[start..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|i| start + i)
}

fn find_bytes_ascii_ci(hay: &[u8], needle: &[u8], start: usize) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    let start = start.min(hay.len());
    hay[start..]
        .windows(needle.len())
        .position(|w| {
            w.iter()
                .zip(needle)
                .all(|(a, b)| ascii_lower(*a) == ascii_lower(*b))
        })
        .map(|i| start + i)
}

fn rfind_bytes(hay: &[u8], needle: &[u8], max_start: usize) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    let last = hay.len() - needle.len();
    let mut i = max_start.min(last);
    loop {
        if &hay[i..i + needle.len()] == needle {
            return Some(i);
        }
        if i == 0 {
            return None;
        }
        i -= 1;
    }
}

fn rfind_bytes_ascii_ci(hay: &[u8], needle: &[u8], max_start: usize) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    let last = hay.len() - needle.len();
    let mut i = max_start.min(last);
    loop {
        let mut ok = true;
        for j in 0..needle.len() {
            if ascii_lower(hay[i + j]) != ascii_lower(needle[j]) {
                ok = false;
                break;
            }
        }
        if ok {
            return Some(i);
        }
        if i == 0 {
            return None;
        }
        i -= 1;
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
    use std::process::{Command, Stdio};
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
                format: false,
            },
            0,
            80,
            10,
        )
        .unwrap();
        assert_eq!(r.lines.len(), 4);
        assert_eq!(
            r.lines[0],
            "00000000 00 01 02 03 │ 04 05 06 07 │ 08 09 0A 0B │ 0C 0D 0E 0F ................"
        );
        assert!(r.lines[0].contains("00 01 02 03 │"));
        assert!(r.lines[0].ends_with("................"));
    }

    #[test]
    fn gnu_hex_line_matches_live_mcview_notes_and_partial() {
        assert_eq!(
            format_gnu_hex_line(0, b"hello from notes"),
            "00000000 68 65 6C 6C │ 6F 20 66 72 │ 6F 6D 20 6E │ 6F 74 65 73 hello from notes"
        );
        assert_eq!(
            format_gnu_hex_line(0x10, &[0x0A]),
            "00000010 0A                                                    ."
        );
        assert_eq!(
            format_gnu_hex_line(0x10, &[0x10, 0x11, 0x12, 0x13]),
            "00000010 10 11 12 13 │                                         ...."
        );
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
                format: false,
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
                format: false,
            },
            0,
            10,
            10,
        )
        .unwrap();
        assert!(r_wrap.lines.len() >= r_nowrap.lines.len());
    }

    #[test]
    fn format_mode_strips_nroff_overstrike() {
        let mut f = NamedTempFile::new().unwrap();
        // nroff bold "Hi": H\x08H i\x08i
        f.write_all(b"H\x08Hi\x08i\n").unwrap();
        let path = f.path().to_path_buf();
        let raw = render_window(
            &path,
            ViewOptions {
                hex: false,
                wrap: false,
                show_cr: false,
                format: false,
            },
            0,
            80,
            5,
        )
        .unwrap();
        assert!(raw.lines[0].contains('\u{8}') || raw.lines[0].len() > 2);
        let fmt = render_window(
            &path,
            ViewOptions {
                hex: false,
                wrap: false,
                show_cr: false,
                format: true,
            },
            0,
            80,
            5,
        )
        .unwrap();
        assert_eq!(fmt.lines[0], "Hi");
        assert_eq!(fmt.line_byte_ranges[0], (0, 6));
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

    fn so(case_sensitive: bool, backwards: bool, whole_words: bool, regexp: bool) -> SearchOptions {
        SearchOptions {
            case_sensitive,
            backwards,
            whole_words,
            kind: if regexp {
                SearchKind::RegularExpression
            } else {
                SearchKind::Normal
            },
            all_charsets: false,
        }
    }

    fn so_kind(kind: SearchKind) -> SearchOptions {
        SearchOptions {
            case_sensitive: true,
            backwards: false,
            whole_words: false,
            kind,
            all_charsets: false,
        }
    }

    #[test]
    fn search_options_case_sensitive() {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(b"Abc").unwrap();
        let path = f.path().to_path_buf();
        assert_eq!(
            search_with_options(&path, 0, "abc", so(false, false, false, false), false).unwrap(),
            Some(0)
        );
        assert_eq!(
            search_with_options(&path, 0, "abc", so(true, false, false, false), false).unwrap(),
            None
        );
    }

    #[test]
    fn search_options_backwards() {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(b"cat x cat").unwrap();
        let path = f.path().to_path_buf();
        // Inclusive first search still sees the match under the start offset.
        assert_eq!(
            search_with_options(&path, 6, "cat", so(true, true, false, false), false).unwrap(),
            Some(6)
        );
        assert_eq!(
            search_next_with_options(&path, 6, "cat", so(true, true, false, false), false).unwrap(),
            Some(0)
        );
    }

    #[test]
    fn search_options_whole_words() {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(b"category").unwrap();
        let path = f.path().to_path_buf();
        assert_eq!(
            search_with_options(&path, 0, "cat", so(true, false, true, false), false).unwrap(),
            None
        );
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(b"category cat").unwrap();
        let path = f.path().to_path_buf();
        assert_eq!(
            search_with_options(&path, 0, "cat", so(true, false, true, false), false).unwrap(),
            Some(9)
        );
    }

    #[test]
    fn search_options_regex_and_invalid() {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(b"aaab").unwrap();
        let path = f.path().to_path_buf();
        assert_eq!(
            search_with_options(&path, 0, "a+b", so(true, false, false, true), false).unwrap(),
            Some(0)
        );
        assert_eq!(
            search_with_options(&path, 0, "(", so(true, false, false, true), false).unwrap(),
            None
        );
    }

    #[test]
    fn search_next_honors_stored_options_and_wrap() {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(b"cat category cat").unwrap();
        let path = f.path().to_path_buf();
        let opts = so(true, false, true, false);
        assert_eq!(
            search_with_options(&path, 0, "cat", opts, true).unwrap(),
            Some(0)
        );
        assert_eq!(
            search_next_with_options(&path, 0, "cat", opts, true).unwrap(),
            Some(13)
        );
        assert_eq!(
            search_next_with_options(&path, 13, "cat", opts, true).unwrap(),
            Some(0)
        );
    }

    #[test]
    fn search_empty_is_noop() {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(b"abc").unwrap();
        let path = f.path().to_path_buf();
        assert_eq!(
            search_with_options(&path, 0, "", so(false, false, false, false), true).unwrap(),
            None
        );
    }

    #[test]
    fn search_hex_file_bytes() {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(&[0x00, 0x41, 0x42, 0x00]).unwrap();
        let path = f.path().to_path_buf();
        assert_eq!(
            search_with_options(&path, 0, "AB", so(true, false, false, false), false).unwrap(),
            Some(1)
        );
    }

    #[test]
    fn search_hexadecimal_numbers_and_quoted() {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(b"xxAByy").unwrap();
        let path = f.path().to_path_buf();
        let opts = so_kind(SearchKind::Hexadecimal);
        assert_eq!(
            search_with_options(&path, 0, "41 42", opts, false).unwrap(),
            Some(2)
        );
        assert_eq!(
            search_with_options(&path, 0, "0x41 \"B\"", opts, false).unwrap(),
            Some(2)
        );
        assert_eq!(
            search_with_options(&path, 0, "41 00", opts, false).unwrap(),
            None
        );
    }

    #[test]
    fn search_hexadecimal_invalid_reports_gnu_wording() {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(b"AB").unwrap();
        let path = f.path().to_path_buf();
        let err = search_with_options(&path, 0, "gg", so_kind(SearchKind::Hexadecimal), false)
            .unwrap_err()
            .to_string();
        assert!(err.starts_with("Hex pattern error at position "), "{err}");
        assert!(err.contains("Invalid character"), "{err}");
        let err = search_with_options(&path, 0, "\"AB", so_kind(SearchKind::Hexadecimal), false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("Unmatched quotes character"), "{err}");
        let err = search_with_options(&path, 0, "100", so_kind(SearchKind::Hexadecimal), false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("Number out of range"), "{err}");
    }

    #[test]
    fn search_wildcard_star_and_qmark() {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(b"cat x cot").unwrap();
        let path = f.path().to_path_buf();
        let opts = so_kind(SearchKind::WildcardSearch);
        assert_eq!(
            search_with_options(&path, 0, "c?t", opts, false).unwrap(),
            Some(0)
        );
        assert_eq!(
            search_next_with_options(&path, 0, "c?t", opts, false).unwrap(),
            Some(6)
        );
        assert_eq!(
            search_with_options(&path, 0, "cat*", opts, false).unwrap(),
            Some(0)
        );
        assert_eq!(
            search_with_options(&path, 0, "dog*", opts, false).unwrap(),
            None
        );
    }

    #[test]
    fn search_all_charsets_finds_latin1() {
        let mut f = NamedTempFile::new().unwrap();
        // Latin-1 café (63 61 66 E9), not UTF-8 C3 A9.
        f.write_all(&[b'c', b'a', b'f', 0xE9]).unwrap();
        let path = f.path().to_path_buf();
        let mut off = so(true, false, false, false);
        assert_eq!(
            search_with_options(&path, 0, "café", off, false).unwrap(),
            None
        );
        off.all_charsets = true;
        assert_eq!(
            search_with_options(&path, 0, "café", off, false).unwrap(),
            Some(0)
        );
    }

    #[test]
    fn external_filter_cat_argpath() {
        // Prepare a simple file
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "hello").unwrap();
        writeln!(f, "world").unwrap();
        let src_path = f.path().to_path_buf();
        // Build filter: cat <path>
        let filter = ExternalFilter::new("cat").with_input(FilterInput::ArgPath);
        let view = ViewData::from_filter(&src_path, &filter).unwrap();
        // Render first page of the filtered output (should equal source)
        let r = render_window(
            view.path(),
            ViewOptions {
                hex: false,
                wrap: false,
                show_cr: false,
                format: false,
            },
            0,
            80,
            5,
        )
        .unwrap();
        assert!(r.lines.iter().any(|l| l.contains("hello")));
        assert!(r.lines.iter().any(|l| l.contains("world")));
    }

    #[test]
    fn external_filter_tr_stdin() {
        // Prepare a simple file with lowercase letters
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "abc def").unwrap();
        let src_path = f.path().to_path_buf();
        // tr a-z A-Z < src
        let filter = ExternalFilter::new("tr")
            .with_args(["a-z", "A-Z"])
            .with_input(FilterInput::Stdin);
        let view = ViewData::from_filter(&src_path, &filter).unwrap();
        let r = render_window(
            view.path(),
            ViewOptions {
                hex: false,
                wrap: false,
                show_cr: false,
                format: false,
            },
            0,
            80,
            5,
        )
        .unwrap();
        let joined = r.lines.join("\n");
        assert!(joined.contains("ABC DEF"));
    }

    #[test]
    fn auto_filter_gzip_by_extension() {
        if !helper_on_path("gzip") {
            return;
        }

        // Create original text file
        let mut src = NamedTempFile::new().unwrap();
        writeln!(src, "hello").unwrap();
        writeln!(src, "world").unwrap();
        let src_path = src.path().to_path_buf();

        // Compress to a .gz file using `gzip -c`
        let gz = tempfile::Builder::new()
            .prefix("rmc-view-test-")
            .suffix(".gz")
            .tempfile()
            .unwrap();
        let gz_path = gz.path().to_path_buf();
        let out = gz.reopen().unwrap();
        let status = Command::new("gzip")
            .arg("-c")
            .arg(&src_path)
            .stdout(Stdio::from(out))
            .stderr(Stdio::null())
            .status()
            .unwrap();
        if !status.success() {
            return;
        }

        // Open view — should auto-apply gzip -dc based on .gz extension
        let view = ViewData::open_view(&gz_path).unwrap();
        let r = render_window(
            view.path(),
            ViewOptions {
                hex: false,
                wrap: false,
                show_cr: false,
                format: false,
            },
            0,
            80,
            10,
        )
        .unwrap();
        let joined = r.lines.join("\n");
        assert!(joined.contains("hello"));
        assert!(joined.contains("world"));

        // Hex mode is the decoded payload, not the gzip header.
        let hx = render_window(
            view.path(),
            ViewOptions {
                hex: true,
                wrap: false,
                show_cr: false,
                format: false,
            },
            0,
            80,
            4,
        )
        .unwrap();
        let hex_joined = hx.lines.join("\n");
        assert!(
            !hex_joined.contains("1F 8B"),
            "hex of .gz must be decompressed payload, not gzip magic"
        );
        assert!(hex_joined.contains("hello") || hex_joined.to_ascii_uppercase().contains("68 65"));
    }

    #[test]
    fn guess_filter_skips_archives_and_plain_text() {
        assert!(guess_filter_for_path(Path::new("notes.txt")).is_none());
        assert!(guess_filter_for_path(Path::new("archive.tar.gz")).is_none());
        assert!(guess_filter_for_path(Path::new("archive.tgz")).is_none());
        assert!(guess_filter_for_path(Path::new("notes.txt.gz")).is_some());
        assert!(guess_filter_for_path(Path::new("log.BZ2")).is_some());
        assert!(guess_filter_for_path(Path::new("file.xz")).is_some());
        let gz = guess_filter_for_path(Path::new("a.gz")).expect("gz filter");
        assert_eq!(gz.program, "gzip");
        assert_eq!(gz.args, vec!["-dc"]);
    }

    #[test]
    fn missing_helper_does_not_return_raw_bytes() {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(&[0x1f, 0x8b, 0x08, 0x00]).unwrap();
        let src = f.path().to_path_buf();
        let filter = ExternalFilter::new("rmc-missing-decompressor-xyz")
            .with_args(["-dc"])
            .with_input(FilterInput::ArgPath);
        let err = ViewData::from_filter(&src, &filter).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("not found") || msg.contains("Cannot view"),
            "missing helper must error, got {msg}"
        );
    }

    #[test]
    fn shipped_view_section_maps_gnu_helpers() {
        let map = parse_view_filters(SHIPPED_EXT_INI);
        assert_eq!(map.get(".gz").map(|f| f.program.as_str()), Some("gzip"));
        assert_eq!(map.get(".bz2").map(|f| f.program.as_str()), Some("bzip2"));
        assert_eq!(map.get(".xz").map(|f| f.program.as_str()), Some("xz"));
    }

    #[test]
    fn gnu_status_line_matches_live_mc_4_8_30() {
        use std::path::PathBuf;
        let notes = PathBuf::from("/tmp/mcr-fixture/notes.txt");
        assert_eq!(
            gnu_status_line(80, &notes, false, 0, 17, 17),
            "/tmp/mcr-fixture/notes.txt                             17/17                100%"
        );
        assert_eq!(
            gnu_status_line(80, &notes, true, 0, 17, 17),
            "/tmp/mcr-fixture/notes.txt                      0x00000000                    0%"
        );
        let long = PathBuf::from("/tmp/mcr-fixture/long.txt");
        assert_eq!(
            gnu_status_line(80, &long, false, 0, 1738, 3160),
            "/tmp/mcr-fixture/long.txt                            1738/3160               55%"
        );
        let z = PathBuf::from("/tmp/mcr-fixture/z");
        assert_eq!(
            gnu_status_line(80, &z, false, 0, 1760, 5000),
            "/tmp/mcr-fixture/z                                   1760/5000               35%"
        );
        assert_eq!(
            gnu_status_line(80, &z, false, 0, 5000, 5000),
            "/tmp/mcr-fixture/z                                   5000/5000              100%"
        );
        let tiny = PathBuf::from("/tmp/mcr-fixture/tiny");
        assert_eq!(
            gnu_status_line(80, &tiny, false, 0, 0, 0),
            "/tmp/mcr-fixture/tiny                                   0/0                 100%"
        );
    }

    #[test]
    fn open_view_plain_file_is_unfiltered() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "plain-notes").unwrap();
        let path = f.path().to_path_buf();
        let view = ViewData::open_view(&path).unwrap();
        assert_eq!(view.path(), path.as_path());
        let r = render_window(
            view.path(),
            ViewOptions {
                hex: false,
                wrap: false,
                show_cr: false,
                format: false,
            },
            0,
            80,
            5,
        )
        .unwrap();
        assert!(r.lines.iter().any(|l| l.contains("plain-notes")));
    }
}
