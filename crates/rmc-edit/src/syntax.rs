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

//! Original Apache-2.0 tokenizers for GNU mcedit-style highlighting.
//! Rules are written from public language grammars; GNU mc syntax files are not used.

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Rust,
    C,
    Python,
    Shell,
    Makefile,
    Json,
    Html,
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
    Heading,
    Emphasis,
    Link,
    Code,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub kind: TokenKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SpanUnit {
    start: usize,
    end: usize,
    kind: TokenKind,
}

/// Guess language from path (filename / extension). Shebang is not considered.
pub fn guess_language(path: Option<&Path>) -> Language {
    guess_language_for_buffer(path, None)
}

/// Filename/extension first; shebang only when there is no extension (and the
/// name was not already recognized, e.g. `Makefile`).
pub fn guess_language_for_buffer(path: Option<&Path>, first_line: Option<&[u8]>) -> Language {
    let from_path = language_from_path(path);
    if from_path != Language::PlainText {
        return from_path;
    }
    let has_ext = path
        .and_then(|p| p.extension())
        .and_then(|s| s.to_str())
        .is_some_and(|e| !e.is_empty());
    if has_ext {
        return Language::PlainText;
    }
    language_from_shebang(first_line).unwrap_or(Language::PlainText)
}

fn language_from_path(path: Option<&Path>) -> Language {
    let Some(p) = path else {
        return Language::PlainText;
    };
    if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
        let lower = name.to_ascii_lowercase();
        if lower == "makefile" || lower == "gnumakefile" {
            return Language::Makefile;
        }
    }
    if let Some(ext) = p
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
    {
        return match ext.as_str() {
            "rs" => Language::Rust,
            "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "hh" | "c++" => Language::C,
            "py" => Language::Python,
            "sh" | "bash" | "zsh" | "ksh" => Language::Shell,
            "mk" | "make" => Language::Makefile,
            "json" => Language::Json,
            "html" | "htm" => Language::Html,
            "ini" | "conf" | "cfg" => Language::Ini,
            "md" | "markdown" => Language::Markdown,
            _ => Language::PlainText,
        };
    }
    Language::PlainText
}

fn language_from_shebang(first_line: Option<&[u8]>) -> Option<Language> {
    let line = first_line?;
    let s = std::str::from_utf8(line).ok()?.trim();
    if !s.starts_with("#!") {
        return None;
    }
    let rest = s[2..].trim();
    let mut parts = rest.split_whitespace();
    let interp = parts.next().unwrap_or("");
    let arg = parts.next().unwrap_or("");
    let interp_base = interp
        .rsplit('/')
        .next()
        .unwrap_or(interp)
        .to_ascii_lowercase();
    let name = if interp_base == "env" {
        arg.to_ascii_lowercase()
    } else {
        interp_base
    };
    if name.starts_with("python") {
        return Some(Language::Python);
    }
    if name == "sh"
        || name.starts_with("bash")
        || name.starts_with("zsh")
        || name.starts_with("ksh")
        || name.starts_with("dash")
    {
        return Some(Language::Shell);
    }
    None
}

/// Expand a source line for display: printable ASCII as-is, tabs to the next
/// tab stop (`tab_width` columns, GNU mcedit `editor_tab_spacing`), else `.`.
pub fn visible_line(bytes: &[u8], tab_width: usize) -> String {
    let tab_width = tab_width.max(1);
    let mut s = String::with_capacity(bytes.len());
    let mut col = 0usize;
    for &b in bytes {
        if b == b'\t' {
            let next = ((col / tab_width) + 1) * tab_width;
            for _ in col..next {
                s.push(' ');
            }
            col = next;
        } else if (0x20..=0x7E).contains(&b) {
            s.push(b as char);
            col += 1;
        } else {
            s.push('.');
            col += 1;
        }
    }
    s
}

/// Visual column of a byte offset on a line (same rules as [`visible_line`]).
pub fn byte_col_to_visual(bytes: &[u8], byte_col: usize, tab_width: usize) -> usize {
    let tab_width = tab_width.max(1);
    let mut col = 0usize;
    for &b in bytes.iter().take(byte_col) {
        if b == b'\t' {
            col = ((col / tab_width) + 1) * tab_width;
        } else {
            col += 1;
        }
    }
    col
}

fn tokenize_visible_line(text: &str, lang: Language) -> Vec<SpanUnit> {
    match lang {
        Language::Rust => tokenize_rust(text),
        Language::C => tokenize_c_like(text),
        Language::Python => tokenize_generic(
            text,
            python_keywords(),
            CommentKind::Hash,
            StringStyle::Python,
            false,
            false,
        ),
        Language::Shell => tokenize_generic(
            text,
            shell_keywords(),
            CommentKind::Hash,
            StringStyle::Shell,
            false,
            false,
        ),
        Language::Makefile => tokenize_generic(
            text,
            makefile_keywords(),
            CommentKind::Hash,
            StringStyle::Shell,
            false,
            true,
        ),
        Language::Json => tokenize_generic(
            text,
            json_keywords(),
            CommentKind::None,
            StringStyle::Json,
            true,
            false,
        ),
        Language::Html => tokenize_html(text),
        Language::Ini => tokenize_ini(text),
        Language::Markdown => tokenize_markdown(text),
        Language::PlainText => {
            if text.is_empty() {
                Vec::new()
            } else {
                vec![SpanUnit {
                    start: 0,
                    end: text.chars().count(),
                    kind: TokenKind::Normal,
                }]
            }
        }
    }
}

fn clip_to_window(
    text: &str,
    spans: Vec<SpanUnit>,
    start_col: usize,
    max_cols: usize,
) -> Vec<Span> {
    let chars: Vec<char> = text.chars().collect();
    let window_end = start_col.saturating_add(max_cols).min(chars.len());
    let mut out: Vec<Span> = Vec::new();
    for su in spans {
        let a = su.start.max(start_col);
        let b = su.end.min(window_end);
        if b <= a {
            continue;
        }
        let seg: String = chars[a..b].iter().collect();
        if let Some(last) = out.last_mut() {
            if last.kind == su.kind {
                last.text.push_str(&seg);
                continue;
            }
        }
        out.push(Span {
            text: seg,
            kind: su.kind,
        });
    }
    out
}

/// Tokenize one source line for the visible editor window (`start_col` / `max_cols`).
/// `start_col` is a visual column after tab expansion (`tab_width`).
pub fn tokenize_for_render(
    line_bytes: &[u8],
    lang: Language,
    start_col: usize,
    max_cols: usize,
    tab_width: usize,
) -> Vec<Span> {
    let full = visible_line(line_bytes, tab_width);
    let spans = tokenize_visible_line(&full, lang);
    clip_to_window(&full, spans, start_col, max_cols)
}

#[derive(Clone, Copy)]
enum CommentKind {
    None,
    Hash,
    CFamily,
}

#[derive(Clone, Copy)]
enum StringStyle {
    CLike,
    Python,
    Shell,
    Json,
}

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
        "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
        "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move",
        "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait",
        "true", "type", "union", "unsafe", "use", "where", "while", "yield",
    ]
}

fn c_keywords() -> &'static [&'static str] {
    &[
        "alignas",
        "alignof",
        "and",
        "and_eq",
        "asm",
        "auto",
        "bitand",
        "bitor",
        "bool",
        "break",
        "case",
        "catch",
        "char",
        "char8_t",
        "char16_t",
        "char32_t",
        "class",
        "compl",
        "concept",
        "const",
        "consteval",
        "constexpr",
        "constinit",
        "const_cast",
        "continue",
        "co_await",
        "co_return",
        "co_yield",
        "decltype",
        "default",
        "delete",
        "do",
        "double",
        "dynamic_cast",
        "else",
        "enum",
        "explicit",
        "export",
        "extern",
        "false",
        "float",
        "for",
        "friend",
        "goto",
        "if",
        "inline",
        "int",
        "long",
        "mutable",
        "namespace",
        "new",
        "noexcept",
        "not",
        "not_eq",
        "nullptr",
        "operator",
        "or",
        "or_eq",
        "private",
        "protected",
        "public",
        "register",
        "reinterpret_cast",
        "requires",
        "restrict",
        "return",
        "short",
        "signed",
        "sizeof",
        "static",
        "static_assert",
        "static_cast",
        "struct",
        "switch",
        "template",
        "this",
        "thread_local",
        "throw",
        "true",
        "try",
        "typedef",
        "typeid",
        "typename",
        "union",
        "unsigned",
        "using",
        "virtual",
        "void",
        "volatile",
        "wchar_t",
        "while",
        "xor",
        "xor_eq",
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
        "case", "do", "done", "elif", "else", "esac", "fi", "for", "function", "if", "in",
        "select", "then", "time", "until", "while",
    ]
}

fn makefile_keywords() -> &'static [&'static str] {
    &[
        "define", "else", "endef", "endif", "export", "ifdef", "ifeq", "ifndef", "ifneq",
        "include", "override", "private", "sinclude", "unexport", "vpath",
    ]
}

fn json_keywords() -> &'static [&'static str] {
    &["true", "false", "null"]
}

fn push_span(out: &mut Vec<SpanUnit>, start: usize, end: usize, kind: TokenKind) {
    if end > start {
        out.push(SpanUnit { start, end, kind });
    }
}

fn tokenize_generic(
    text: &str,
    keywords: &[&str],
    comments: CommentKind,
    strings: StringStyle,
    json_numbers: bool,
    makefile_dot_idents: bool,
) -> Vec<SpanUnit> {
    let mut out: Vec<SpanUnit> = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut i = 0usize;
    while i < n {
        let c = chars[i];
        match comments {
            CommentKind::Hash if c == '#' => {
                push_span(&mut out, i, n, TokenKind::Comment);
                break;
            }
            CommentKind::CFamily if c == '/' && i + 1 < n && chars[i + 1] == '/' => {
                push_span(&mut out, i, n, TokenKind::Comment);
                break;
            }
            CommentKind::CFamily if c == '/' && i + 1 < n && chars[i + 1] == '*' => {
                let start = i;
                i += 2;
                while i + 1 < n && !(chars[i] == '*' && chars[i + 1] == '/') {
                    i += 1;
                }
                if i + 1 < n {
                    i += 2;
                } else {
                    i = n;
                }
                push_span(&mut out, start, i, TokenKind::Comment);
                continue;
            }
            _ => {}
        }
        if matches!(strings, StringStyle::Json)
            && c == '-'
            && i + 1 < n
            && chars[i + 1].is_ascii_digit()
        {
            i = scan_number(&chars, i, json_numbers, &mut out);
            continue;
        }
        if c == '"' || (c == '\'' && !matches!(strings, StringStyle::Json)) {
            i = scan_string(&chars, i, strings, &mut out);
            continue;
        }
        if c.is_ascii_digit() {
            i = scan_number(&chars, i, json_numbers, &mut out);
            continue;
        }
        if makefile_dot_idents && c == '.' && i + 1 < n && is_ident_start(chars[i + 1]) {
            let start = i;
            i += 1;
            while i < n && is_ident_char(chars[i]) {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let kind = if word.eq_ignore_ascii_case(".PHONY")
                || word.eq_ignore_ascii_case(".SUFFIXES")
                || word.eq_ignore_ascii_case(".DEFAULT")
                || word.eq_ignore_ascii_case(".PRECIOUS")
                || word.eq_ignore_ascii_case(".IGNORE")
                || word.eq_ignore_ascii_case(".SILENT")
                || word.eq_ignore_ascii_case(".NOTPARALLEL")
            {
                TokenKind::Keyword
            } else {
                TokenKind::Identifier
            };
            push_span(&mut out, start, i, kind);
            continue;
        }
        if is_ident_start(c) {
            let start = i;
            i += 1;
            while i < n && is_ident_char(chars[i]) {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let kind = if keywords.iter().any(|&k| k == word) {
                TokenKind::Keyword
            } else {
                TokenKind::Identifier
            };
            push_span(&mut out, start, i, kind);
            continue;
        }
        if is_whitespace(c) {
            let start = i;
            i += 1;
            while i < n && is_whitespace(chars[i]) {
                i += 1;
            }
            push_span(&mut out, start, i, TokenKind::Whitespace);
            continue;
        }
        push_span(&mut out, i, i + 1, TokenKind::Operator);
        i += 1;
    }
    out
}

fn scan_string(chars: &[char], start: usize, style: StringStyle, out: &mut Vec<SpanUnit>) -> usize {
    let n = chars.len();
    let quote = chars[start];
    let mut i = start + 1;
    if matches!(style, StringStyle::Python)
        && start + 2 < n
        && chars[start + 1] == quote
        && chars[start + 2] == quote
    {
        i = start + 3;
        while i + 2 < n {
            if chars[i] == quote && chars[i + 1] == quote && chars[i + 2] == quote {
                i += 3;
                break;
            }
            if chars[i] == '\\' && i + 1 < n {
                i += 2;
            } else {
                i += 1;
            }
        }
        if i <= start + 3 {
            i = n;
        }
        push_span(out, start, i, TokenKind::String);
        return i;
    }
    while i < n {
        if chars[i] == '\\' && i + 1 < n {
            if matches!(style, StringStyle::Shell) && quote == '\'' {
                break;
            }
            i += 2;
            continue;
        }
        if chars[i] == quote {
            i += 1;
            break;
        }
        i += 1;
    }
    push_span(out, start, i, TokenKind::String);
    i
}

fn scan_number(chars: &[char], start: usize, json_numbers: bool, out: &mut Vec<SpanUnit>) -> usize {
    let n = chars.len();
    let mut i = start;
    if chars[i] == '-' {
        i += 1;
    }
    if i < n
        && chars[i] == '0'
        && i + 1 < n
        && (chars[i + 1] == 'x' || chars[i + 1] == 'X')
        && !json_numbers
    {
        i += 2;
        while i < n && (chars[i].is_ascii_hexdigit() || chars[i] == '_') {
            i += 1;
        }
        push_span(out, start, i, TokenKind::Number);
        return i;
    }
    while i < n && (chars[i].is_ascii_digit() || (!json_numbers && chars[i] == '_')) {
        i += 1;
    }
    if i < n && chars[i] == '.' && i + 1 < n && chars[i + 1].is_ascii_digit() {
        i += 1;
        while i < n && (chars[i].is_ascii_digit() || (!json_numbers && chars[i] == '_')) {
            i += 1;
        }
    }
    if json_numbers && i < n && (chars[i] == 'e' || chars[i] == 'E') {
        i += 1;
        if i < n && (chars[i] == '+' || chars[i] == '-') {
            i += 1;
        }
        while i < n && chars[i].is_ascii_digit() {
            i += 1;
        }
    }
    push_span(out, start, i, TokenKind::Number);
    i
}

fn tokenize_rust(text: &str) -> Vec<SpanUnit> {
    let mut out: Vec<SpanUnit> = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let kws = rust_keywords();
    let mut i = 0usize;
    while i < n {
        let c = chars[i];
        if c == '/' && i + 1 < n && chars[i + 1] == '/' {
            push_span(&mut out, i, n, TokenKind::Comment);
            break;
        }
        if c == '/' && i + 1 < n && chars[i + 1] == '*' {
            let start = i;
            i += 2;
            while i + 1 < n && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            if i + 1 < n {
                i += 2;
            } else {
                i = n;
            }
            push_span(&mut out, start, i, TokenKind::Comment);
            continue;
        }
        if c == '"' {
            i = scan_string(&chars, i, StringStyle::CLike, &mut out);
            continue;
        }
        if c == '\'' {
            i = scan_rust_tick(&chars, i, &mut out);
            continue;
        }
        if c.is_ascii_digit() {
            i = scan_number(&chars, i, false, &mut out);
            continue;
        }
        if is_ident_start(c) {
            let start = i;
            i += 1;
            while i < n && is_ident_char(chars[i]) {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let kind = if kws.iter().any(|&k| k == word) {
                TokenKind::Keyword
            } else {
                TokenKind::Identifier
            };
            push_span(&mut out, start, i, kind);
            continue;
        }
        if is_whitespace(c) {
            let start = i;
            i += 1;
            while i < n && is_whitespace(chars[i]) {
                i += 1;
            }
            push_span(&mut out, start, i, TokenKind::Whitespace);
            continue;
        }
        push_span(&mut out, i, i + 1, TokenKind::Operator);
        i += 1;
    }
    out
}

/// `'x'` char literal vs `'lifetime` (neither is copied from GNU mc).
fn scan_rust_tick(chars: &[char], start: usize, out: &mut Vec<SpanUnit>) -> usize {
    let n = chars.len();
    let mut i = start + 1;
    if i < n && chars[i] == '\\' {
        return scan_string(chars, start, StringStyle::CLike, out);
    }
    if i < n && is_ident_start(chars[i]) {
        i += 1;
        while i < n && is_ident_char(chars[i]) {
            i += 1;
        }
        if i < n && chars[i] == '\'' {
            i += 1;
            push_span(out, start, i, TokenKind::String);
            return i;
        }
        push_span(out, start, i, TokenKind::Identifier);
        return i;
    }
    scan_string(chars, start, StringStyle::CLike, out)
}

fn tokenize_c_like(text: &str) -> Vec<SpanUnit> {
    let trimmed = text.trim_start();
    let leading_ws = text.chars().count() - trimmed.chars().count();
    if trimmed.starts_with('#') {
        let mut out = Vec::new();
        if leading_ws > 0 {
            push_span(&mut out, 0, leading_ws, TokenKind::Whitespace);
        }
        push_span(
            &mut out,
            leading_ws,
            text.chars().count(),
            TokenKind::Preproc,
        );
        return out;
    }
    tokenize_generic(
        text,
        c_keywords(),
        CommentKind::CFamily,
        StringStyle::CLike,
        false,
        false,
    )
}

fn tokenize_html(text: &str) -> Vec<SpanUnit> {
    let mut out: Vec<SpanUnit> = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut i = 0usize;
    while i < n {
        let c = chars[i];
        if c == '<'
            && i + 3 < n
            && chars[i + 1] == '!'
            && chars[i + 2] == '-'
            && chars[i + 3] == '-'
        {
            let start = i;
            i += 4;
            while i + 2 < n && !(chars[i] == '-' && chars[i + 1] == '-' && chars[i + 2] == '>') {
                i += 1;
            }
            if i + 2 < n {
                i += 3;
            } else {
                i = n;
            }
            push_span(&mut out, start, i, TokenKind::Comment);
            continue;
        }
        if c == '<' {
            let start = i;
            i += 1;
            if i < n && chars[i] == '/' {
                i += 1;
            }
            let name_start = i;
            while i < n && (is_ident_char(chars[i]) || chars[i] == '-' || chars[i] == ':') {
                i += 1;
            }
            if i > name_start {
                push_span(&mut out, start, name_start, TokenKind::Operator);
                push_span(&mut out, name_start, i, TokenKind::Keyword);
            } else {
                push_span(&mut out, start, i, TokenKind::Operator);
            }
            continue;
        }
        if c == '"' || c == '\'' {
            i = scan_string(&chars, i, StringStyle::CLike, &mut out);
            continue;
        }
        if c == '>' || c == '/' || c == '=' {
            push_span(&mut out, i, i + 1, TokenKind::Operator);
            i += 1;
            continue;
        }
        if is_whitespace(c) {
            let start = i;
            i += 1;
            while i < n && is_whitespace(chars[i]) {
                i += 1;
            }
            push_span(&mut out, start, i, TokenKind::Whitespace);
            continue;
        }
        if is_ident_start(c) {
            let start = i;
            i += 1;
            while i < n && (is_ident_char(chars[i]) || chars[i] == '-' || chars[i] == ':') {
                i += 1;
            }
            push_span(&mut out, start, i, TokenKind::Identifier);
            continue;
        }
        let start = i;
        i += 1;
        while i < n {
            let d = chars[i];
            if d == '<'
                || d == '>'
                || d == '"'
                || d == '\''
                || d == '/'
                || d == '='
                || is_whitespace(d)
            {
                break;
            }
            i += 1;
        }
        push_span(&mut out, start, i, TokenKind::Normal);
    }
    out
}

fn tokenize_ini(text: &str) -> Vec<SpanUnit> {
    let mut out: Vec<SpanUnit> = Vec::new();
    let trimmed = text.trim();
    if trimmed.starts_with('[') && trimmed.ends_with(']') && trimmed.len() >= 2 {
        let start = text.find('[').unwrap_or(0);
        let end = text
            .rfind(']')
            .map(|i| i + 1)
            .unwrap_or_else(|| text.chars().count());
        push_span(&mut out, start, end, TokenKind::Heading);
        return out;
    }
    if let Some(pos) = text.find([';', '#']) {
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
    tokenize_generic(
        text,
        &[],
        CommentKind::None,
        StringStyle::CLike,
        false,
        false,
    )
}

fn tokenize_markdown(text: &str) -> Vec<SpanUnit> {
    let mut out: Vec<SpanUnit> = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
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
            push_span(&mut out, 0, n, TokenKind::Heading);
            return out;
        }
    }
    if text.trim_start().starts_with("```") {
        push_span(&mut out, 0, n, TokenKind::Code);
        return out;
    }
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
            push_span(&mut out, start, pos, TokenKind::Code);
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
                if pos < n && chars[pos] == '(' {
                    while pos < n && chars[pos] != ')' {
                        pos += 1;
                    }
                    if pos < n && chars[pos] == ')' {
                        pos += 1;
                    }
                }
            }
            push_span(&mut out, start, pos, TokenKind::Link);
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
                push_span(&mut out, start, pos, TokenKind::Emphasis);
                continue;
            }
            push_span(&mut out, start, start + 1, TokenKind::Normal);
            pos = start + 1;
            continue;
        }
        if is_whitespace(c) {
            let start = pos;
            pos += 1;
            while pos < n && is_whitespace(chars[pos]) {
                pos += 1;
            }
            push_span(&mut out, start, pos, TokenKind::Whitespace);
            continue;
        }
        let start = pos;
        pos += 1;
        while pos < n {
            let d = chars[pos];
            if d == '`' || d == '[' || d == ']' || d == '*' || d == '_' || is_whitespace(d) {
                break;
            }
            pos += 1;
        }
        push_span(&mut out, start, pos, TokenKind::Normal);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans_kinds(text: &str, lang: Language) -> Vec<TokenKind> {
        tokenize_visible_line(text, lang)
            .into_iter()
            .map(|s| s.kind)
            .collect()
    }

    fn kinds_for_text(text: &str, lang: Language) -> Vec<(String, TokenKind)> {
        tokenize_visible_line(text, lang)
            .into_iter()
            .map(|s| {
                let chars: Vec<char> = text.chars().collect();
                (chars[s.start..s.end].iter().collect(), s.kind)
            })
            .collect()
    }

    #[test]
    fn rust_line_tokenization() {
        let kinds = spans_kinds(r#"fn main() { let x = 42; // comment"#, Language::Rust);
        assert!(kinds.contains(&TokenKind::Keyword));
        assert!(kinds.contains(&TokenKind::Number));
        assert!(kinds.contains(&TokenKind::Comment));
        let parts = kinds_for_text("fn let name", Language::Rust);
        assert_eq!(
            parts
                .iter()
                .filter(|(t, _)| t == "fn" || t == "let" || t == "name")
                .map(|(t, k)| (t.as_str(), *k))
                .collect::<Vec<_>>(),
            vec![
                ("fn", TokenKind::Keyword),
                ("let", TokenKind::Keyword),
                ("name", TokenKind::Identifier),
            ]
        );
    }

    #[test]
    fn c_line_preproc_and_keywords() {
        let pre = spans_kinds("#include <stdio.h>", Language::C);
        assert!(pre.contains(&TokenKind::Preproc));
        let kinds = spans_kinds("int main(){ return 0; }", Language::C);
        assert!(kinds.contains(&TokenKind::Keyword));
        assert!(kinds.contains(&TokenKind::Number));
        let cpp = spans_kinds("class Foo { public: void bar(); };", Language::C);
        assert!(cpp.contains(&TokenKind::Keyword));
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
    fn makefile_keywords_and_comment() {
        let kinds = spans_kinds("ifeq ($(X),1) include foo.mk # c", Language::Makefile);
        assert!(kinds.contains(&TokenKind::Keyword));
        assert!(kinds.contains(&TokenKind::Comment));
        let phony = kinds_for_text(".PHONY: all", Language::Makefile);
        assert!(phony
            .iter()
            .any(|(t, k)| t == ".PHONY" && *k == TokenKind::Keyword));
    }

    #[test]
    fn json_keywords_strings_numbers() {
        let kinds = spans_kinds(r#"{"ok": true, "n": -2.5e1}"#, Language::Json);
        assert!(kinds.contains(&TokenKind::Keyword));
        assert!(kinds.contains(&TokenKind::String));
        assert!(kinds.contains(&TokenKind::Number));
        assert!(!kinds.contains(&TokenKind::Comment));
    }

    #[test]
    fn html_tags_comments_strings() {
        let kinds = spans_kinds(r#"<div class="x"><!-- c -->text</div>"#, Language::Html);
        assert!(kinds.contains(&TokenKind::Keyword));
        assert!(kinds.contains(&TokenKind::String));
        assert!(kinds.contains(&TokenKind::Comment));
    }

    #[test]
    fn ini_section_and_kv() {
        let sec = spans_kinds("[section-name]", Language::Ini);
        assert!(sec.contains(&TokenKind::Heading));
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

    #[test]
    fn plaintext_has_no_keywords() {
        let kinds = spans_kinds("fn let name", Language::PlainText);
        assert!(!kinds.contains(&TokenKind::Keyword));
        assert!(kinds.iter().all(|k| *k == TokenKind::Normal));
    }

    #[test]
    fn guess_from_extension_and_makefile_name() {
        assert_eq!(
            guess_language(Some(Path::new("src/main.rs"))),
            Language::Rust
        );
        assert_eq!(guess_language(Some(Path::new("a.c"))), Language::C);
        assert_eq!(guess_language(Some(Path::new("a.cpp"))), Language::C);
        assert_eq!(guess_language(Some(Path::new("a.py"))), Language::Python);
        assert_eq!(guess_language(Some(Path::new("run.sh"))), Language::Shell);
        assert_eq!(
            guess_language(Some(Path::new("Makefile"))),
            Language::Makefile
        );
        assert_eq!(
            guess_language(Some(Path::new("foo.mk"))),
            Language::Makefile
        );
        assert_eq!(guess_language(Some(Path::new("a.json"))), Language::Json);
        assert_eq!(guess_language(Some(Path::new("a.html"))), Language::Html);
        assert_eq!(
            guess_language(Some(Path::new("README.md"))),
            Language::Markdown
        );
        assert_eq!(
            guess_language(Some(Path::new("notes.txt"))),
            Language::PlainText
        );
    }

    #[test]
    fn shebang_fallback_when_no_extension() {
        assert_eq!(
            guess_language_for_buffer(Some(Path::new("tool")), Some(b"#!/usr/bin/env python3")),
            Language::Python
        );
        assert_eq!(
            guess_language_for_buffer(Some(Path::new("tool")), Some(b"#!/bin/sh")),
            Language::Shell
        );
        assert_eq!(
            guess_language_for_buffer(Some(Path::new("main.rs")), Some(b"#!/bin/sh")),
            Language::Rust
        );
        assert_eq!(
            guess_language_for_buffer(Some(Path::new("notes.txt")), Some(b"#!/bin/sh")),
            Language::PlainText
        );
    }

    #[test]
    fn tokenize_for_render_clips_visible_window() {
        let spans = tokenize_for_render(b"fn abcdef", Language::Rust, 3, 3, 8);
        let joined: String = spans.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(joined, "abc");
    }

    #[test]
    fn tab_expands_to_tab_width_and_keyword_stays_keyword() {
        let spans = tokenize_for_render(b"\tfn", Language::Rust, 0, 16, 8);
        let joined: String = spans.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(&joined[..8], "        ", "{spans:?}");
        assert!(
            spans
                .iter()
                .any(|s| s.kind == TokenKind::Keyword && s.text == "fn"),
            "{spans:?}"
        );
        let spans4 = tokenize_for_render(b"\tfn", Language::Rust, 0, 16, 4);
        let joined4: String = spans4.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(&joined4[..4], "    ", "{spans4:?}");
        assert!(
            spans4
                .iter()
                .any(|s| s.kind == TokenKind::Keyword && s.text == "fn"),
            "{spans4:?}"
        );
    }
}
