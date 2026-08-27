//! GNU mc(1) completion (public spec only: Completion section).
//!
//! Attempt completion on the text before the cursor, in order:
//! 1. Variable if the token begins with `$`
//! 2. Username if it begins with `~` (but `~/` is a home path, i.e. filename)
//! 3. Hostname if it begins with `@`
//! 4. Command if this is a command-line command position (reserved words + builtins + PATH)
//! 5. Otherwise filename completion

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Kind of completion being attempted for the current token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionKind {
    Variable,
    Username,
    Hostname,
    Command,
    Filename,
}

/// One completion candidate. `replacement` replaces the token (no trailing space).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionItem {
    pub replacement: String,
    pub display: String,
    pub kind: CompletionKind,
    /// GNU adds a trailing space after a completed command or non-directory filename.
    pub trailing_space: bool,
}

impl CompletionItem {
    /// Text substituted into the input line (filename `?` `*` `&` escaped).
    pub fn insert_text(&self) -> String {
        let mut s = if self.kind == CompletionKind::Filename {
            escape_filename_meta(&self.replacement)
        } else {
            self.replacement.clone()
        };
        if self.trailing_space {
            s.push(' ');
        }
        s
    }
}

/// Files / env / PATH sources used to gather candidates. Overrides are for tests (no network).
#[derive(Debug, Clone, Copy)]
pub struct CompletionSources<'a> {
    pub cwd: &'a Path,
    /// Command completion is command-line specific.
    pub allow_command: bool,
    /// `PATH`-style override (`:`-separated). `None` reads `std::env::var("PATH")`.
    pub path: Option<&'a str>,
    pub passwd_path: Option<&'a Path>,
    pub hosts_path: Option<&'a Path>,
    pub home: Option<&'a Path>,
}

/// Shell reserved words and a small builtin set from public mc(1) wording.
const SHELL_WORDS: &[&str] = &[
    "if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac",
    "in", "function", "select", "time", "cd", "echo", "export", "set", "unset", "alias", "bg",
    "fg", "jobs", "kill", "pwd", "umask", "wait", "type", "hash", "true", "false", "test",
    "source", "eval", "exec", "exit", "return", "break", "continue", "shift", "read", "printf",
    "let", "local", "declare", "readonly", "ls", "cat", "mkdir", "rmdir", "rm", "cp", "mv",
    "touch",
];

/// Byte offset of the completion token and the token itself (text before the cursor).
pub fn token_before_cursor(text_before: &str) -> (usize, &str) {
    let mut start = 0;
    for (i, c) in text_before.char_indices() {
        if is_token_delimiter(c) {
            start = i + c.len_utf8();
        }
    }
    (start, &text_before[start..])
}

fn is_token_delimiter(c: char) -> bool {
    c.is_whitespace() || matches!(c, ';' | '|' | '&' | '<' | '>' | '(' | ')' | '`')
}

/// True when the token sits where a shell command name may be typed.
pub fn is_command_position(text_before: &str, token_start: usize) -> bool {
    let before = text_before.get(..token_start).unwrap_or("").trim_end();
    if before.is_empty() {
        return true;
    }
    before.ends_with(';')
        || before.ends_with('|')
        || before.ends_with('&')
        || before.ends_with('(')
        || before.ends_with('`')
}

/// Classify the token per mc(1) Completion (variable / user / host / command / filename).
pub fn classify_token(
    token: &str,
    allow_command: bool,
    text_before: &str,
    token_start: usize,
) -> CompletionKind {
    if token.starts_with('$') {
        CompletionKind::Variable
    } else if token.starts_with('~') && !token.starts_with("~/") {
        CompletionKind::Username
    } else if token.starts_with('@') {
        CompletionKind::Hostname
    } else if allow_command && is_command_position(text_before, token_start) {
        CompletionKind::Command
    } else {
        CompletionKind::Filename
    }
}

/// Gather matches for `kind`. If that kind has no hits, filename completion is attempted
/// (except we do not re-run filename when the kind was already filename).
pub fn collect_matches(
    token: &str,
    kind: CompletionKind,
    src: &CompletionSources<'_>,
) -> Vec<CompletionItem> {
    let mut items = match kind {
        CompletionKind::Variable => variable_matches(token),
        CompletionKind::Username => username_matches(token, src),
        CompletionKind::Hostname => hostname_matches(token, src),
        CompletionKind::Command => command_matches(token, src),
        CompletionKind::Filename => filename_matches(token, src),
    };
    if items.is_empty() && kind != CompletionKind::Filename {
        items = filename_matches(token, src);
    }
    items
}

/// Longest common prefix of unescaped `replacement` strings.
pub fn common_replacement_prefix(items: &[CompletionItem]) -> String {
    let mut iter = items.iter().map(|i| i.replacement.as_str());
    let Some(first) = iter.next() else {
        return String::new();
    };
    let mut prefix = first.to_string();
    for s in iter {
        let n = prefix
            .chars()
            .zip(s.chars())
            .take_while(|(a, b)| a == b)
            .count();
        prefix = prefix.chars().take(n).collect();
        if prefix.is_empty() {
            break;
        }
    }
    prefix
}

/// Escape `?`, `*`, and `&` as `\?`, `\*`, `\&` when substituting a filename.
pub fn escape_filename_meta(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '?' | '*' | '&') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn variable_matches(token: &str) -> Vec<CompletionItem> {
    let (brace, name_prefix) = if let Some(rest) = token.strip_prefix("${") {
        (true, rest.trim_end_matches('}'))
    } else if let Some(rest) = token.strip_prefix('$') {
        (false, rest)
    } else {
        return Vec::new();
    };
    let mut names: BTreeSet<String> = BTreeSet::new();
    for (k, _) in std::env::vars() {
        if k.starts_with(name_prefix) {
            names.insert(k);
        }
    }
    names
        .into_iter()
        .map(|k| {
            let replacement = if brace {
                format!("${{{k}}}")
            } else {
                format!("${k}")
            };
            CompletionItem {
                display: replacement.clone(),
                replacement,
                kind: CompletionKind::Variable,
                trailing_space: false,
            }
        })
        .collect()
}

fn username_matches(token: &str, src: &CompletionSources<'_>) -> Vec<CompletionItem> {
    let prefix = token.strip_prefix('~').unwrap_or(token);
    let path = src
        .passwd_path
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("/etc/passwd"));
    let Ok(text) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    let mut names: BTreeSet<String> = BTreeSet::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some(name) = line.split(':').next() else {
            continue;
        };
        if name.is_empty() || name.starts_with('+') || name.starts_with('-') {
            continue;
        }
        if name.starts_with(prefix) {
            names.insert(name.to_string());
        }
    }
    names
        .into_iter()
        .map(|name| {
            let replacement = format!("~{name}/");
            CompletionItem {
                display: format!("~{name}"),
                replacement,
                kind: CompletionKind::Username,
                trailing_space: false,
            }
        })
        .collect()
}

fn hostname_matches(token: &str, src: &CompletionSources<'_>) -> Vec<CompletionItem> {
    let prefix = token.strip_prefix('@').unwrap_or(token);
    let path = src
        .hosts_path
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("/etc/hosts"));
    let Ok(text) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    let mut names: BTreeSet<String> = BTreeSet::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.is_empty() {
            continue;
        }
        // /etc/hosts: IP then names. known_hosts-style: hostname[,hostname] key-type …
        let host_fields: Vec<&str> = if looks_like_ip(fields[0]) {
            fields[1..].to_vec()
        } else {
            fields[0]
                .split(',')
                .map(|h| h.trim())
                .filter(|h| !h.is_empty())
                .collect()
        };
        for raw in host_fields {
            let host = strip_known_hosts_host(raw);
            if host.starts_with(prefix) && !host.is_empty() && !looks_like_ip(&host) {
                names.insert(host);
            }
        }
    }
    names
        .into_iter()
        .map(|host| {
            let replacement = format!("@{host}");
            CompletionItem {
                display: replacement.clone(),
                replacement,
                kind: CompletionKind::Hostname,
                trailing_space: false,
            }
        })
        .collect()
}

fn looks_like_ip(s: &str) -> bool {
    if s.chars().all(|c| c.is_ascii_digit() || c == '.') && s.contains('.') {
        return true;
    }
    s.contains(':')
        && s.chars()
            .all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.')
}

fn strip_known_hosts_host(raw: &str) -> String {
    let s = raw.trim();
    if let Some(inner) = s.strip_prefix('[') {
        if let Some(end) = inner.find(']') {
            return inner[..end].to_string();
        }
    }
    s.to_string()
}

fn command_matches(token: &str, src: &CompletionSources<'_>) -> Vec<CompletionItem> {
    let mut names: BTreeSet<String> = BTreeSet::new();
    for w in SHELL_WORDS {
        if w.starts_with(token) {
            names.insert((*w).to_string());
        }
    }
    let path = src
        .path
        .map(str::to_string)
        .or_else(|| std::env::var("PATH").ok())
        .unwrap_or_default();
    for dir in path.split(':').filter(|s| !s.is_empty()) {
        let Ok(rd) = fs::read_dir(dir) else {
            continue;
        };
        for ent in rd.flatten() {
            let name = ent.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if !name.starts_with(token) || name == "." || name == ".." {
                continue;
            }
            let path = ent.path();
            if path.is_file() {
                names.insert(name.to_string());
            }
        }
    }
    names
        .into_iter()
        .map(|name| CompletionItem {
            display: name.clone(),
            replacement: name,
            kind: CompletionKind::Command,
            trailing_space: true,
        })
        .collect()
}

fn filename_matches(token: &str, src: &CompletionSources<'_>) -> Vec<CompletionItem> {
    let (dir, file_prefix, token_dir) = split_path_token(token, src);
    let Ok(rd) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut items = Vec::new();
    for ent in rd.flatten() {
        let name = ent.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name == "." || name == ".." {
            continue;
        }
        if !name.starts_with(&file_prefix) {
            continue;
        }
        let path = ent.path();
        let is_dir = path.is_dir();
        let mut replacement = token_dir.clone();
        replacement.push_str(name);
        if is_dir {
            replacement.push('/');
        }
        items.push(CompletionItem {
            display: replacement.clone(),
            replacement,
            kind: CompletionKind::Filename,
            trailing_space: !is_dir,
        });
    }
    items.sort_by(|a, b| a.replacement.cmp(&b.replacement));
    items
}

fn split_path_token(token: &str, src: &CompletionSources<'_>) -> (PathBuf, String, String) {
    if let Some(rest) = token.strip_prefix("~/") {
        let home = default_home(src);
        let (dir, prefix, sub) = split_under(&home, rest);
        let token_dir = format!("~/{sub}");
        return (dir, prefix, token_dir);
    }
    if let Some(pos) = token.rfind('/') {
        let dir_part = &token[..=pos];
        let prefix = token[pos + 1..].to_string();
        let dir = if dir_part.starts_with('/') {
            PathBuf::from(dir_part)
        } else {
            src.cwd.join(dir_part)
        };
        (dir, prefix, dir_part.to_string())
    } else {
        (src.cwd.to_path_buf(), token.to_string(), String::new())
    }
}

fn split_under(base: &Path, rest: &str) -> (PathBuf, String, String) {
    if let Some(pos) = rest.rfind('/') {
        let sub = &rest[..=pos];
        let prefix = rest[pos + 1..].to_string();
        (base.join(sub), prefix, sub.to_string())
    } else {
        (base.to_path_buf(), rest.to_string(), String::new())
    }
}

fn default_home(src: &CompletionSources<'_>) -> PathBuf {
    if let Some(h) = src.home {
        return h.to_path_buf();
    }
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"))
}

/// Filter already-gathered items to those whose replacement still matches `token`.
pub fn filter_items(items: &[CompletionItem], token: &str) -> Vec<CompletionItem> {
    items
        .iter()
        .filter(|it| it.replacement.starts_with(token) || it.display.starts_with(token))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "rmc-complete-{tag}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn src<'a>(
        cwd: &'a Path,
        allow_command: bool,
        path: Option<&'a str>,
        passwd: Option<&'a Path>,
        hosts: Option<&'a Path>,
        home: Option<&'a Path>,
    ) -> CompletionSources<'a> {
        CompletionSources {
            cwd,
            allow_command,
            path,
            passwd_path: passwd,
            hosts_path: hosts,
            home,
        }
    }

    #[test]
    fn unique_filename_completes_rest_of_name() {
        let dir = temp_dir("file");
        fs::write(dir.join("alpha.txt"), b"a").unwrap();
        fs::write(dir.join("beta.txt"), b"b").unwrap();
        let s = src(&dir, false, None, None, None, None);
        let items = collect_matches("al", CompletionKind::Filename, &s);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].replacement, "alpha.txt");
        assert!(items[0].trailing_space);
        assert_eq!(items[0].insert_text(), "alpha.txt ");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ambiguous_filenames_share_common_prefix() {
        let dir = temp_dir("amb");
        fs::write(dir.join("alpha.txt"), b"a").unwrap();
        fs::write(dir.join("alpine.txt"), b"b").unwrap();
        let s = src(&dir, false, None, None, None, None);
        let items = collect_matches("al", CompletionKind::Filename, &s);
        assert_eq!(items.len(), 2);
        assert_eq!(common_replacement_prefix(&items), "alp");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn directory_completion_adds_trailing_slash() {
        let dir = temp_dir("dir");
        fs::create_dir(dir.join("subdir")).unwrap();
        fs::write(dir.join("subfile"), b"x").unwrap();
        let s = src(&dir, false, None, None, None, None);
        let items = collect_matches("subdir", CompletionKind::Filename, &s);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].replacement, "subdir/");
        assert!(!items[0].trailing_space);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn filename_meta_escaped_on_insert() {
        assert_eq!(escape_filename_meta("a?b*c&d"), r"a\?b\*c\&d");
        let dir = temp_dir("esc");
        fs::write(dir.join("why?.txt"), b"x").unwrap();
        let s = src(&dir, false, None, None, None, None);
        let items = collect_matches("why", CompletionKind::Filename, &s);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].insert_text(), r"why\?.txt ");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn variable_completion_against_env() {
        std::env::set_var("RMC_COMPLETE_TEST_VAR_6607", "1");
        let dir = temp_dir("var");
        let s = src(&dir, false, None, None, None, None);
        let items = collect_matches("$RMC_COMPLETE_TEST_VAR_6607", CompletionKind::Variable, &s);
        assert!(
            items
                .iter()
                .any(|i| i.replacement == "$RMC_COMPLETE_TEST_VAR_6607"),
            "{items:?}"
        );
        let brace = collect_matches("${RMC_COMPLETE_TEST_VAR_6607", CompletionKind::Variable, &s);
        assert!(
            brace
                .iter()
                .any(|i| i.replacement == "${RMC_COMPLETE_TEST_VAR_6607}"),
            "{brace:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn username_from_passwd_file() {
        let dir = temp_dir("user");
        let pw = dir.join("passwd");
        fs::write(
            &pw,
            "root:x:0:0:root:/root:/bin/bash\nrmccompuser:x:1000:1000::/tmp:/bin/bash\n",
        )
        .unwrap();
        let s = src(&dir, false, None, Some(&pw), None, None);
        let items = collect_matches("~rmccomp", CompletionKind::Username, &s);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].replacement, "~rmccompuser/");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn hostname_from_hosts_file() {
        let dir = temp_dir("host");
        let hosts = dir.join("hosts");
        fs::write(&hosts, "127.0.0.1 localhost\n10.0.0.2 rmccomphost alias\n").unwrap();
        let s = src(&dir, false, None, None, Some(&hosts), None);
        let items = collect_matches("@rmccomp", CompletionKind::Hostname, &s);
        assert!(
            items.iter().any(|i| i.replacement == "@rmccomphost"),
            "{items:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn command_from_builtins_and_path() {
        let dir = temp_dir("cmd");
        let bin = dir.join("bin");
        fs::create_dir(&bin).unwrap();
        let exe = bin.join("rmcuniqcmd6607");
        fs::write(&exe, b"#!/bin/sh\n").unwrap();
        let mut perms = fs::metadata(&exe).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&exe, perms).unwrap();
        let path = bin.to_string_lossy();
        let s = src(&dir, true, Some(path.as_ref()), None, None, None);
        let items = collect_matches("rmcuniqcmd", CompletionKind::Command, &s);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].insert_text(), "rmcuniqcmd6607 ");
        let echo = collect_matches("ech", CompletionKind::Command, &s);
        assert!(echo.iter().any(|i| i.replacement == "echo"), "{echo:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn command_kind_only_in_command_position() {
        let text = "echo he";
        let (start, tok) = token_before_cursor(text);
        assert_eq!(tok, "he");
        assert!(!is_command_position(text, start));
        assert_eq!(
            classify_token(tok, true, text, start),
            CompletionKind::Filename
        );
        let cmd = "ec";
        let (cstart, ctok) = token_before_cursor(cmd);
        assert!(is_command_position(cmd, cstart));
        assert_eq!(
            classify_token(ctok, true, cmd, cstart),
            CompletionKind::Command
        );
        assert_eq!(
            classify_token(ctok, false, cmd, cstart),
            CompletionKind::Filename
        );
    }

    #[test]
    fn token_split_after_pipe_is_command_position() {
        let text = "ls | gr";
        let (start, tok) = token_before_cursor(text);
        assert_eq!(tok, "gr");
        assert!(is_command_position(text, start));
    }
}
