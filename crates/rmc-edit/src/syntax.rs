// Copyright 2026 rusty-midnight-commander contributors
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Rust,
    C,
    Python,
    Shell,
    Ini,
    Markdown,
    PlainText,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Normal,
    Whitespace,
    Keyword,
    String,
    Comment,
    Number,
    Type,
    Preproc,
    Identifier,
    Operator,
    Heading,  // markdown / ini section
    Emphasis, // markdown emphasis
    Link,     // markdown link text
    Code,     // markdown inline code
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub kind: TokenKind,
}

// Internal span used during tokenization (positions on visible text)
#[derive(Debug, Clone, PartialEq, Eq)]
struct SpanUnit {
    start: usize,
    end: usize, // exclusive
    kind: TokenKind,
}

pub fn guess_language(path: Option<&Path>) -> Language {
    let Some(p) = path else {
        return Language::PlainText;
    };
    if let Some(ext) = p
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
    {
        match ext.as_str() {
            "rs" => Language::Rust,
            "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "hh" => Language::C,
            "py" => Language::Python,
            "sh" | "bash" | "zsh" | "ksh" => Language::Shell,
            "ini" | "conf" | "cfg" => Language::Ini,
            "md" | "markdown" => Language::Markdown,
            _ => Language::PlainText,
        }
    } else {
        Language::PlainText
    }
}

/// Convert raw bytes to visible string (ASCII printable and tabs; others as '.').
fn bytes_to_visible_string(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len());
    for &b in bytes {
        if (0x20..=0x7E).contains(&b) || b == b'\t' {
            s.push(if b == b'\t' { ' ' } else { b as char });
        } else {
            s.push('.');
        }
    }
    s
}

/// Tokenize one visible text line into SpanUnit list.
fn tokenize_visible_line(text: &str, lang: Language) -> Vec<SpanUnit> {
    match lang {
        Language::Rust => tokenize_rust_like(text, true),
        Language::C => tokenize_c_like(text),
        Language::Python => tokenize_python(text),
        Language::Shell => tokenize_shell(text),
        Language::Ini => tokenize_ini(text),
        Language::Markdown => tokenize_markdown(text),
        Language::PlainText => vec![SpanUnit {
            start: 0,
            end: text.chars().count(),
            kind: TokenKind::Normal,
        }],
    }
}

/// Clip SpanUnits to [start_col, start_col+max_cols) and convert to output Spans with actual text.
fn clip_to_window(
    text: &str,
    spans: Vec<SpanUnit>,
    start_col: usize,
    max_cols: usize,
) -> Vec<Span> {
    let text_len = text.chars().count();
    let window_end = start_col.saturating_add(max_cols).min(text_len);
    // Build char indices to byte indices map for slicing
    let mut char_to_byte: Vec<usize> = Vec::with_capacity(text_len + 1);
    char_to_byte.push(0);
    for (i, (bidx, _)) in text.char_indices().enumerate() {
        if i > 0 {
            // previous already pushed
        }
        // Ensure positions align (we keep start positions)
        if char_to_byte.last().copied().unwrap_or(usize::MAX) != bidx {
            char_to_byte.push(bidx);
        }
    }
    // Ensure final byte length
    char_to_byte.push(text.len());

    let mut out: Vec<Span> = Vec::new();
    for su in spans {
        let a = su.start.max(start_col);
        let b = su.end.min(window_end);
        if b <= a {
            continue;
        }
        let ba = char_index_to_byte(&char_to_byte, a);
        let bb = char_index_to_byte(&char_to_byte, b);
        let seg = &text[ba..bb];
        // Merge adjacent spans of same kind
        if let Some(last) = out.last_mut() {
            if last.kind == su.kind {
                last.text.push_str(seg);
                continue;
            }
        }
        out.push(Span {
            text: seg.to_string(),
            kind: su.kind,
        });
    }
    // Pad to width if needed (like render_window)
    let cur_cols: usize = out.iter().map(|s| s.text.chars().count()).sum();
    if cur_cols < (window_end - start_col) {
        let pad = " ".repeat((window_end - start_col) - cur_cols);
        if let Some(last) = out.last_mut() {
            if last.kind == TokenKind::Normal {
                last.text.push_str(&pad);
            } else {
                out.push(Span {
                    text: pad,
                    kind: TokenKind::Normal,
                });
            }
        } else {
            out.push(Span {
                text: pad,
                kind: TokenKind::Normal,
            });
        }
    }
    // Ensure exactly max_cols width by padding or truncating; current window_end may be less when line shorter
    // If line shorter than requested width, caller will draw blank remainder of row; we keep as-is.
    out
}

fn char_index_to_byte(map: &[usize], ci: usize) -> usize {
    // map[0] is 0, map.last() is len
    if ci >= map.len() - 1 {
        *map.last().unwrap()
    } else {
        map[ci]
    }
}

pub fn tokenize_for_render(
    line_bytes: &[u8],
    lang: Language,
    start_col: usize,
    max_cols: usize,
) -> Vec<Span> {
    // Build full visible text then tokenize and clip
    let full = bytes_to_visible_string(line_bytes);
    let spans = tokenize_visible_line(&full, lang);
    clip_to_window(&full, spans, start_col, max_cols)
}

// ---------- Language tokenizers ----------

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}
fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}
fn is_whitespace(c: char) -> bool {
    c.is_ascii_whitespace()
}

fn rust_keywords() -> &'static [&'static str] {
    &[
        "as", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false",
        "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
        "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type",
        "unsafe", "use", "where", "while", "async", "await", "union", "yield",
    ]
}
fn c_keywords() -> &'static [&'static str] {
    &[
        "auto", "break", "case", "char", "const", "continue", "default", "do", "double", "else",
        "enum", "extern", "float", "for", "goto", "if", "inline", "int", "long", "register",
        "restrict", "return", "short", "signed", "sizeof", "static", "struct", "switch", "typedef",
        "union", "unsigned", "void", "volatile", "while",
    ]
}
fn python_keywords() -> &'static [&'static str] {
    &[
        "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class",
        "continue", "def", "del", "elif", "else", "except", "finally", "for", "from", "global",
        "if", "import", "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return",
        "try", "while", "with", "yield",
    ]
}
fn shell_keywords() -> &'static [&'static str] {
    &[
        "if", "then", "else", "elif", "fi", "for", "do", "done", "case", "esac", "function", "in",
        "while", "until", "select",
    ]
}

fn tokenize_rust_like(text: &str, _is_rust: bool) -> Vec<SpanUnit> {
    // Handles Rust; C-like done separately to catch preproc
    let mut out: Vec<SpanUnit> = Vec::new();
    let mut i = 0usize;
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let kws = rust_keywords();
    while i < n {
        let c = chars[i];
        // Line comment //
        if c == '/' && i + 1 < n && chars[i + 1] == '/' {
            out.push(SpanUnit {
                start: i,
                end: n,
                kind: TokenKind::Comment,
            });
            break;
        }
        // Block comment start /* ... (single line only)
        if c == '/' && i + 1 < n && chars[i + 1] == '*' {
            // From i to end as comment (no multiline handling)
            out.push(SpanUnit {
                start: i,
                end: n,
                kind: TokenKind::Comment,
            });
            break;
        }
        // String literal "..." or '...'
        if c == '"' || c == '\'' {
            let start = i;
            i += 1;
            while i < n {
                if chars[i] == '\\' && i + 1 < n {
                    i += 2;
                    continue;
                }
                if chars[i] == c {
                    i += 1;
                    break;
                }
                i += 1;
            }
            out.push(SpanUnit {
                start,
                end: i,
                kind: TokenKind::String,
            });
            continue;
        }
        // Number
        if c.is_ascii_digit() {
            let start = i;
            i += 1;
            // 0x... hex or 0b... bin or 0o... oct or digits/underscores
            if start + 1 < n
                && chars[start] == '0'
                && (chars[start + 1] == 'x' || chars[start + 1] == 'X')
            {
                i = start + 2;
                while i < n && (chars[i].is_ascii_hexdigit() || chars[i] == '_') {
                    i += 1;
                }
            } else {
                while i < n && (chars[i].is_ascii_digit() || chars[i] == '_') {
                    i += 1;
                }
                if i < n && chars[i] == '.' {
                    i += 1;
                    while i < n && (chars[i].is_ascii_digit() || chars[i] == '_') {
                        i += 1;
                    }
                }
            }
            out.push(SpanUnit {
                start,
                end: i,
                kind: TokenKind::Number,
            });
            continue;
        }
        // Identifier / keyword
        if is_ident_start(c) {
            let start = i;
            i += 1;
            while i < n && is_ident_char(chars[i]) {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let kind = if kws.binary_search_by(|k| k.cmp(&word.as_str())).is_ok() {
                TokenKind::Keyword
            } else {
                TokenKind::Identifier
            };
            out.push(SpanUnit {
                start,
                end: i,
                kind,
            });
            continue;
        }
        // Whitespace
        if is_whitespace(c) {
            let start = i;
            i += 1;
            while i < n && is_whitespace(chars[i]) {
                i += 1;
            }
            out.push(SpanUnit {
                start,
                end: i,
                kind: TokenKind::Whitespace,
            });
            continue;
        }
        // Operators/punct
        out.push(SpanUnit {
            start: i,
            end: i + 1,
            kind: TokenKind::Operator,
        });
        i += 1;
    }
    if out.is_empty() {
        out.push(SpanUnit {
            start: 0,
            end: n,
            kind: TokenKind::Normal,
        });
    }
    out
}

fn tokenize_c_like(text: &str) -> Vec<SpanUnit> {
    // C/C++: preprocessor if starts with '#'
    let trimmed = text.trim_start();
    let leading_ws = text.len() - trimmed.len();
    if trimmed.starts_with('#') {
        return vec![SpanUnit {
            start: leading_ws,
            end: text.chars().count(),
            kind: TokenKind::Preproc,
        }];
    }
    // Otherwise use rust-like with C keywords
    let mut out: Vec<SpanUnit> = Vec::new();
    let mut i = 0usize;
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let kws = c_keywords();
    while i < n {
        let c = chars[i];
        // Line comment //
        if c == '/' && i + 1 < n && chars[i + 1] == '/' {
            out.push(SpanUnit {
                start: i,
                end: n,
                kind: TokenKind::Comment,
            });
            break;
        }
        // Block comment start /* ... (single line only)
        if c == '/' && i + 1 < n && chars[i + 1] == '*' {
            out.push(SpanUnit {
                start: i,
                end: n,
                kind: TokenKind::Comment,
            });
            break;
        }
        // String
        if c == '"' || c == '\'' {
            let start = i;
            i += 1;
            while i < n {
                if chars[i] == '\\' && i + 1 < n {
                    i += 2;
                    continue;
                }
                if chars[i] == c {
                    i += 1;
                    break;
                }
                i += 1;
            }
            out.push(SpanUnit {
                start,
                end: i,
                kind: TokenKind::String,
            });
            continue;
        }
        // Number
        if c.is_ascii_digit() {
            let start = i;
            i += 1;
            if start + 1 < n
                && chars[start] == '0'
                && (chars[start + 1] == 'x' || chars[start + 1] == 'X')
            {
                i = start + 2;
                while i < n && (chars[i].is_ascii_hexdigit() || chars[i] == '_') {
                    i += 1;
                }
            } else {
                while i < n && (chars[i].is_ascii_digit() || chars[i] == '_') {
                    i += 1;
                }
                if i < n && chars[i] == '.' {
                    i += 1;
                    while i < n && (chars[i].is_ascii_digit() || chars[i] == '_') {
                        i += 1;
                    }
                }
            }
            out.push(SpanUnit {
                start,
                end: i,
                kind: TokenKind::Number,
            });
            continue;
        }
        // Identifier / keyword
        if is_ident_start(c) {
            let start = i;
            i += 1;
            while i < n && is_ident_char(chars[i]) {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let kind = if kws.binary_search_by(|k| k.cmp(&word.as_str())).is_ok() {
                TokenKind::Keyword
            } else {
                TokenKind::Identifier
            };
            out.push(SpanUnit {
                start,
                end: i,
                kind,
            });
            continue;
        }
        // Whitespace
        if is_whitespace(c) {
            let start = i;
            i += 1;
            while i < n && is_whitespace(chars[i]) {
                i += 1;
            }
            out.push(SpanUnit {
                start,
                end: i,
                kind: TokenKind::Whitespace,
            });
            continue;
        }
        out.push(SpanUnit {
            start: i,
            end: i + 1,
            kind: TokenKind::Operator,
        });
        i += 1;
    }
    if out.is_empty() {
        out.push(SpanUnit {
            start: 0,
            end: n,
            kind: TokenKind::Normal,
        });
    }
    out
}

fn tokenize_python(text: &str) -> Vec<SpanUnit> {
    let kws = python_keywords();
    let mut out: Vec<SpanUnit> = Vec::new();
    let mut i = 0usize;
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    while i < n {
        let c = chars[i];
        // Comment
        if c == '#' {
            out.push(SpanUnit {
                start: i,
                end: n,
                kind: TokenKind::Comment,
            });
            break;
        }
        // Triple-quoted or single quotes
        if c == '"' || c == '\'' {
            let start = i;
            // Check for triple quotes
            let triple = i + 2 < n && chars[i + 1] == c && chars[i + 2] == c;
            if triple {
                i += 3;
                while i + 2 < n {
                    if chars[i] == c && chars[i + 1] == c && chars[i + 2] == c {
                        i += 3;
                        break;
                    }
                    if chars[i] == '\\' && i + 1 < n {
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
            } else {
                i += 1;
                while i < n {
                    if chars[i] == '\\' && i + 1 < n {
                        i += 2;
                        continue;
                    }
                    if chars[i] == c {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            out.push(SpanUnit {
                start,
                end: i,
                kind: TokenKind::String,
            });
            continue;
        }
        // Number
        if c.is_ascii_digit() {
            let start = i;
            i += 1;
            while i < n && (chars[i].is_ascii_digit() || chars[i] == '_') {
                i += 1;
            }
            if i < n && chars[i] == '.' {
                i += 1;
                while i < n && (chars[i].is_ascii_digit() || chars[i] == '_') {
                    i += 1;
                }
            }
            out.push(SpanUnit {
                start,
                end: i,
                kind: TokenKind::Number,
            });
            continue;
        }
        // Identifier / keyword
        if is_ident_start(c) {
            let start = i;
            i += 1;
            while i < n && is_ident_char(chars[i]) {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let kind = if kws.binary_search_by(|k| k.cmp(&word.as_str())).is_ok() {
                TokenKind::Keyword
            } else {
                TokenKind::Identifier
            };
            out.push(SpanUnit {
                start,
                end: i,
                kind,
            });
            continue;
        }
        // Whitespace
        if is_whitespace(c) {
            let start = i;
            i += 1;
            while i < n && is_whitespace(chars[i]) {
                i += 1;
            }
            out.push(SpanUnit {
                start,
                end: i,
                kind: TokenKind::Whitespace,
            });
            continue;
        }
        out.push(SpanUnit {
            start: i,
            end: i + 1,
            kind: TokenKind::Operator,
        });
        i += 1;
    }
    if out.is_empty() {
        out.push(SpanUnit {
            start: 0,
            end: n,
            kind: TokenKind::Normal,
        });
    }
    out
}

fn tokenize_shell(text: &str) -> Vec<SpanUnit> {
    let kws = shell_keywords();
    let mut out: Vec<SpanUnit> = Vec::new();
    let mut i = 0usize;
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    while i < n {
        let c = chars[i];
        // Comment
        if c == '#' {
            out.push(SpanUnit {
                start: i,
                end: n,
                kind: TokenKind::Comment,
            });
            break;
        }
        // Strings '...' or "..."
        if c == '"' || c == '\'' {
            let start = i;
            i += 1;
            while i < n {
                if chars[i] == '\\' && i + 1 < n && c == '"' {
                    // Only escape inside double quotes for bash; good enough
                    i += 2;
                    continue;
                }
                if chars[i] == c {
                    i += 1;
                    break;
                }
                i += 1;
            }
            out.push(SpanUnit {
                start,
                end: i,
                kind: TokenKind::String,
            });
            continue;
        }
        // Number
        if c.is_ascii_digit() {
            let start = i;
            i += 1;
            while i < n && (chars[i].is_ascii_digit() || chars[i] == '_') {
                i += 1;
            }
            out.push(SpanUnit {
                start,
                end: i,
                kind: TokenKind::Number,
            });
            continue;
        }
        // Identifier / keyword
        if is_ident_start(c) {
            let start = i;
            i += 1;
            while i < n && is_ident_char(chars[i]) {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let kind = if kws.binary_search_by(|k| k.cmp(&word.as_str())).is_ok() {
                TokenKind::Keyword
            } else {
                TokenKind::Identifier
            };
            out.push(SpanUnit {
                start,
                end: i,
                kind,
            });
            continue;
        }
        // Whitespace
        if is_whitespace(c) {
            let start = i;
            i += 1;
            while i < n && is_whitespace(chars[i]) {
                i += 1;
            }
            out.push(SpanUnit {
                start,
                end: i,
                kind: TokenKind::Whitespace,
            });
            continue;
        }
        out.push(SpanUnit {
            start: i,
            end: i + 1,
            kind: TokenKind::Operator,
        });
        i += 1;
    }
    if out.is_empty() {
        out.push(SpanUnit {
            start: 0,
            end: n,
            kind: TokenKind::Normal,
        });
    }
    out
}

fn tokenize_ini(text: &str) -> Vec<SpanUnit> {
    let mut out: Vec<SpanUnit> = Vec::new();
    let trimmed = text.trim();
    if trimmed.starts_with('[') && trimmed.ends_with(']') && trimmed.len() >= 2 {
        // Section heading
        let start = text.find('[').unwrap_or(0);
        let end = text
            .rfind(']')
            .map(|i| i + 1)
            .unwrap_or_else(|| text.chars().count());
        out.push(SpanUnit {
            start,
            end,
            kind: TokenKind::Heading,
        });
        return out;
    }
    // Comment: ';' or '#'
    if let Some(pos) = text.find([';', '#']) {
        // Everything after marker is comment
        // Left side may still be tokenized roughly as key=value
        let (left, _) = text.split_at(pos);
        let mut left_spans = tokenize_kv_like(left);
        let comment_len = text.chars().count() - pos;
        left_spans.push(SpanUnit {
            start: pos,
            end: pos + comment_len,
            kind: TokenKind::Comment,
        });
        return left_spans;
    }
    tokenize_kv_like(text)
}

fn tokenize_kv_like(text: &str) -> Vec<SpanUnit> {
    let mut out: Vec<SpanUnit> = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut i = 0usize;
    // key [whitespace] '=' [whitespace] value
    // We'll color key as Identifier, '=' as Operator, value as String if quoted or Number if numeric
    // Otherwise Normal.
    // Scan left-to-right
    while i < n {
        let c = chars[i];
        if is_whitespace(c) {
            let start = i;
            i += 1;
            while i < n && is_whitespace(chars[i]) {
                i += 1;
            }
            out.push(SpanUnit {
                start,
                end: i,
                kind: TokenKind::Whitespace,
            });
            continue;
        }
        if c == '=' {
            out.push(SpanUnit {
                start: i,
                end: i + 1,
                kind: TokenKind::Operator,
            });
            i += 1;
            continue;
        }
        if c == '"' || c == '\'' {
            let start = i;
            i += 1;
            while i < n {
                if chars[i] == '\\' && i + 1 < n {
                    i += 2;
                    continue;
                }
                if chars[i] == c {
                    i += 1;
                    break;
                }
                i += 1;
            }
            out.push(SpanUnit {
                start,
                end: i,
                kind: TokenKind::String,
            });
            continue;
        }
        if c.is_ascii_digit() {
            let start = i;
            i += 1;
            while i < n && (chars[i].is_ascii_digit() || chars[i] == '_') {
                i += 1;
            }
            out.push(SpanUnit {
                start,
                end: i,
                kind: TokenKind::Number,
            });
            continue;
        }
        // Identifier fragment until whitespace or '='
        if is_ident_start(c) {
            let start = i;
            i += 1;
            while i < n && is_ident_char(chars[i]) {
                i += 1;
            }
            out.push(SpanUnit {
                start,
                end: i,
                kind: TokenKind::Identifier,
            });
            continue;
        }
        // Fallback single char
        out.push(SpanUnit {
            start: i,
            end: i + 1,
            kind: TokenKind::Normal,
        });
        i += 1;
    }
    if out.is_empty() {
        out.push(SpanUnit {
            start: 0,
            end: n,
            kind: TokenKind::Normal,
        });
    }
    out
}

fn tokenize_markdown(text: &str) -> Vec<SpanUnit> {
    let mut out: Vec<SpanUnit> = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    // Heading if starts with 1+ '#'+space
    let mut i = 0usize;
    while i < n && chars[i] == ' ' {
        i += 1;
    }
    if i < n && chars[i] == '#' {
        let mut j = i;
        while j < n && chars[j] == '#' {
            j += 1;
        }
        if j < n && chars[j].is_ascii_whitespace() {
            out.push(SpanUnit {
                start: 0,
                end: n,
                kind: TokenKind::Heading,
            });
            return out;
        }
    }
    // Code fence line ```
    if text.trim_start().starts_with("```") {
        out.push(SpanUnit {
            start: 0,
            end: n,
            kind: TokenKind::Code,
        });
        return out;
    }
    // Otherwise, scan for inline code `...`, links [text](url), emphasis *...* or _..._
    let mut pos = 0usize;
    while pos < n {
        let c = chars[pos];
        if c == '`' {
            let start = pos;
            pos += 1;
            while pos < n && chars[pos] != '`' {
                pos += 1;
            }
            if pos < n && chars[pos] == '`' {
                pos += 1;
            }
            out.push(SpanUnit {
                start,
                end: pos,
                kind: TokenKind::Code,
            });
            continue;
        }
        if c == '[' {
            let start = pos;
            pos += 1;
            while pos < n && chars[pos] != ']' {
                pos += 1;
            }
            if pos < n && chars[pos] == ']' {
                pos += 1;
                // Optional url (...)
                if pos < n && chars[pos] == '(' {
                    while pos < n && chars[pos] != ')' {
                        pos += 1;
                    }
                    if pos < n && chars[pos] == ')' {
                        pos += 1;
                    }
                }
            }
            out.push(SpanUnit {
                start,
                end: pos,
                kind: TokenKind::Link,
            });
            continue;
        }
        if c == '*' || c == '_' {
            let start = pos;
            let mark = c;
            pos += 1;
            while pos < n && chars[pos] != mark {
                pos += 1;
            }
            if pos < n && chars[pos] == mark {
                pos += 1;
                out.push(SpanUnit {
                    start,
                    end: pos,
                    kind: TokenKind::Emphasis,
                });
                continue;
            }
            // Not closed: treat as normal char
            out.push(SpanUnit {
                start,
                end: start + 1,
                kind: TokenKind::Normal,
            });
            pos = start + 1;
            continue;
        }
        if is_whitespace(c) {
            let start = pos;
            pos += 1;
            while pos < n && is_whitespace(chars[pos]) {
                pos += 1;
            }
            out.push(SpanUnit {
                start,
                end: pos,
                kind: TokenKind::Whitespace,
            });
            continue;
        }
        // Accumulate run of non-specials as Normal until next special
        let start = pos;
        pos += 1;
        while pos < n {
            let d = chars[pos];
            if d == '`'
                || d == '['
                || d == ']'
                || d == '('
                || d == ')'
                || d == '*'
                || d == '_'
                || is_whitespace(d)
            {
                break;
            }
            pos += 1;
        }
        out.push(SpanUnit {
            start,
            end: pos,
            kind: TokenKind::Normal,
        });
    }
    if out.is_empty() {
        out.push(SpanUnit {
            start: 0,
            end: n,
            kind: TokenKind::Normal,
        });
    }
    out
}

// ---------- Tests ----------

#[cfg(test)]
mod tests {
    use super::*;

    fn spans_kinds(text: &str, lang: Language) -> Vec<TokenKind> {
        let su = tokenize_visible_line(text, lang);
        su.into_iter().map(|s| s.kind).collect()
    }

    #[test]
    fn rust_line_tokenization() {
        let kinds = spans_kinds(r#"fn main() { let x = 42; // comment"#, Language::Rust);
        assert!(kinds.contains(&TokenKind::Keyword));
        assert!(kinds.contains(&TokenKind::Number));
        assert!(kinds.contains(&TokenKind::Comment));
    }

    #[test]
    fn c_line_preproc_and_keywords() {
        let pre = spans_kinds("#include <stdio.h>", Language::C);
        assert_eq!(pre.len(), 1);
        assert_eq!(pre[0], TokenKind::Preproc);
        let kinds = spans_kinds("int main(){ return 0; }", Language::C);
        assert!(kinds.contains(&TokenKind::Keyword));
        assert!(kinds.contains(&TokenKind::Number));
    }

    #[test]
    fn python_strings_and_comment() {
        let kinds = spans_kinds(r#"def f(x): print("hi") # c"#, Language::Python);
        assert!(kinds.contains(&TokenKind::Keyword));
        assert!(kinds.contains(&TokenKind::String));
        assert!(kinds.contains(&TokenKind::Comment));
    }

    #[test]
    fn shell_keywords_and_comment() {
        let kinds = spans_kinds("if test 1 -eq 1; then echo ok; fi # c", Language::Shell);
        assert!(kinds.contains(&TokenKind::Keyword));
        assert!(kinds.contains(&TokenKind::Comment));
    }

    #[test]
    fn ini_section_and_kv() {
        let sec = spans_kinds("[section-name]", Language::Ini);
        assert_eq!(sec.len(), 1);
        assert_eq!(sec[0], TokenKind::Heading);
        let kv = spans_kinds("port = 8080 ; listen port", Language::Ini);
        assert!(kv.contains(&TokenKind::Identifier));
        assert!(kv.contains(&TokenKind::Number));
        assert!(kv.contains(&TokenKind::Comment));
    }

    #[test]
    fn markdown_variants() {
        let h = spans_kinds("# Title", Language::Markdown);
        assert_eq!(h.len(), 1);
        assert_eq!(h[0], TokenKind::Heading);
        let code = spans_kinds("use `code()` here", Language::Markdown);
        assert!(code.contains(&TokenKind::Code));
        let link = spans_kinds("[x](y)", Language::Markdown);
        assert!(link.contains(&TokenKind::Link));
        let emp = spans_kinds("*hi*", Language::Markdown);
        assert!(emp.contains(&TokenKind::Emphasis));
    }
}
