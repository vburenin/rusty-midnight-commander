use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use walkdir::WalkDir;

/// Dialog height lower bound so GNU checkboxes, ignore-dirs field, and the results list fit.
pub const FIND_DIALOG_MIN_H: u16 = 23;
/// Dialog height upper bound (terminal-relative, same slack as before plus new rows).
pub const FIND_DIALOG_MAX_H: u16 = 29;
/// Rows from the top of the dialog to the results list (fields, checkboxes, ignore field, status).
pub const FIND_DIALOG_LIST_TOP: u16 = 17;
/// `list_h = dialog_h - FIND_DIALOG_LIST_CHROME` (list top + button/border chrome).
pub const FIND_DIALOG_LIST_CHROME: u16 = 19;

pub fn find_dialog_height(rows: u16) -> u16 {
    rows.saturating_sub(4)
        .clamp(FIND_DIALOG_MIN_H, FIND_DIALOG_MAX_H)
}

pub fn find_dialog_list_rows(dialog_h: u16) -> usize {
    dialog_h.saturating_sub(FIND_DIALOG_LIST_CHROME) as usize
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum NamePattern {
    Glob(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindParams {
    pub start_dir: PathBuf,
    pub name_pattern: NamePattern,
    pub content_substring: Option<String>,
    pub case_sensitive: bool,
    /// Filename and Content are regexes when set; otherwise glob + substring.
    pub regular_expression: bool,
    /// When false, only immediate children of `start_dir` are searched.
    pub find_recursively: bool,
    pub follow_symlinks: bool,
    /// Skip names starting with `.` (except `..`); hidden dirs are not descended.
    pub skip_hidden: bool,
    /// Content matches must form whole words (like grep -w). Filename pattern is unchanged.
    #[serde(default)]
    pub whole_words: bool,
    /// When set, skip directories listed in `ignore_dirs` during the walk.
    #[serde(default)]
    pub enable_ignore_dirs: bool,
    /// Colon-separated directory names or absolute paths; unused when the checkbox is off.
    #[serde(default)]
    pub ignore_dirs: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindDialogFocus {
    StartDir,
    NamePattern,
    Content,
    WholeWords,
    CaseSensitive,
    RegularExpression,
    FindRecursively,
    FollowSymlinks,
    SkipHidden,
    EnableIgnoreDirs,
    IgnoreDirs,
    ButtonStart,
    ButtonAgain,
    ButtonStop,
    ButtonChdir,
    ButtonPanelize,
    ButtonQuit,
}

impl FindDialogFocus {
    pub fn next(self) -> Self {
        match self {
            Self::StartDir => Self::NamePattern,
            Self::NamePattern => Self::Content,
            Self::Content => Self::WholeWords,
            Self::WholeWords => Self::CaseSensitive,
            Self::CaseSensitive => Self::RegularExpression,
            Self::RegularExpression => Self::FindRecursively,
            Self::FindRecursively => Self::FollowSymlinks,
            Self::FollowSymlinks => Self::SkipHidden,
            Self::SkipHidden => Self::EnableIgnoreDirs,
            Self::EnableIgnoreDirs => Self::IgnoreDirs,
            Self::IgnoreDirs => Self::ButtonStart,
            Self::ButtonStart => Self::ButtonStop,
            Self::ButtonStop => Self::ButtonChdir,
            Self::ButtonChdir => Self::ButtonAgain,
            Self::ButtonAgain => Self::ButtonPanelize,
            Self::ButtonPanelize => Self::ButtonQuit,
            Self::ButtonQuit => Self::StartDir,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::StartDir => Self::ButtonQuit,
            Self::NamePattern => Self::StartDir,
            Self::Content => Self::NamePattern,
            Self::WholeWords => Self::Content,
            Self::CaseSensitive => Self::WholeWords,
            Self::RegularExpression => Self::CaseSensitive,
            Self::FindRecursively => Self::RegularExpression,
            Self::FollowSymlinks => Self::FindRecursively,
            Self::SkipHidden => Self::FollowSymlinks,
            Self::EnableIgnoreDirs => Self::SkipHidden,
            Self::IgnoreDirs => Self::EnableIgnoreDirs,
            Self::ButtonStart => Self::IgnoreDirs,
            Self::ButtonStop => Self::ButtonStart,
            Self::ButtonChdir => Self::ButtonStop,
            Self::ButtonAgain => Self::ButtonChdir,
            Self::ButtonPanelize => Self::ButtonAgain,
            Self::ButtonQuit => Self::ButtonPanelize,
        }
    }

    pub fn is_checkbox(self) -> bool {
        matches!(
            self,
            Self::WholeWords
                | Self::CaseSensitive
                | Self::RegularExpression
                | Self::FindRecursively
                | Self::FollowSymlinks
                | Self::SkipHidden
                | Self::EnableIgnoreDirs
        )
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct FindResults {
    pub paths: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct CancelHandle {
    inner: Arc<AtomicBool>,
}

impl CancelHandle {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(AtomicBool::new(false)),
        }
    }
    pub fn flag(&self) -> Arc<AtomicBool> {
        self.inner.clone()
    }
    pub fn cancel(&self) {
        self.inner.store(true, Ordering::Relaxed);
    }
    pub fn is_canceled(&self) -> bool {
        self.inner.load(Ordering::Relaxed)
    }
}

impl Default for CancelHandle {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub struct FindDialogState {
    pub params: FindParams,
    pub start_dir_edit: String,
    pub focus: FindDialogFocus,
    pub running: bool,
    pub results: FindResults,
    pub cancel: Option<CancelHandle>,
    pub results_rx: Option<Receiver<PathBuf>>,
    pub selected_index: usize,
    pub scroll_top: usize,
}

impl FindDialogState {
    pub fn new(start_dir: PathBuf) -> Self {
        Self {
            params: FindParams {
                start_dir: start_dir.clone(),
                name_pattern: NamePattern::Glob("*".into()),
                content_substring: None,
                case_sensitive: false,
                regular_expression: false,
                find_recursively: true,
                follow_symlinks: false,
                skip_hidden: false,
                whole_words: false,
                enable_ignore_dirs: false,
                ignore_dirs: String::new(),
            },
            start_dir_edit: start_dir.display().to_string(),
            focus: FindDialogFocus::NamePattern,
            running: false,
            results: FindResults::default(),
            cancel: None,
            results_rx: None,
            selected_index: 0,
            scroll_top: 0,
        }
    }

    /// Toggle the focused GNU checkbox. Returns true if a checkbox was focused.
    pub fn toggle_focused_checkbox(&mut self) -> bool {
        match self.focus {
            FindDialogFocus::CaseSensitive => {
                self.params.case_sensitive = !self.params.case_sensitive;
                true
            }
            FindDialogFocus::RegularExpression => {
                self.params.regular_expression = !self.params.regular_expression;
                true
            }
            FindDialogFocus::FindRecursively => {
                self.params.find_recursively = !self.params.find_recursively;
                true
            }
            FindDialogFocus::FollowSymlinks => {
                self.params.follow_symlinks = !self.params.follow_symlinks;
                true
            }
            FindDialogFocus::SkipHidden => {
                self.params.skip_hidden = !self.params.skip_hidden;
                true
            }
            FindDialogFocus::WholeWords => {
                self.params.whole_words = !self.params.whole_words;
                true
            }
            FindDialogFocus::EnableIgnoreDirs => {
                self.params.enable_ignore_dirs = !self.params.enable_ignore_dirs;
                true
            }
            _ => false,
        }
    }
}

pub fn search_files(params: &FindParams, cancel: &Arc<AtomicBool>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    search_files_streaming(params, cancel, |p| out.push(p));
    out
}

pub fn search_files_streaming<F: FnMut(PathBuf)>(
    params: &FindParams,
    cancel: &Arc<AtomicBool>,
    mut on_hit: F,
) {
    let name_pat = match &params.name_pattern {
        NamePattern::Glob(s) => s.as_str(),
    };

    let name_re = if params.regular_expression {
        match RegexBuilder::new(name_pat)
            .case_insensitive(!params.case_sensitive)
            .build()
        {
            Ok(re) => Some(re),
            Err(_) => return,
        }
    } else {
        None
    };
    let glob = if params.regular_expression {
        None
    } else {
        Some(GlobMatcher::new(name_pat, params.case_sensitive))
    };

    let content_filter = compile_content_filter(params);
    let whole_words = params.whole_words;

    let root = params.start_dir.clone();
    let skip_hidden = params.skip_hidden;
    let ignore = IgnoreSpec::from_params(params);
    let mut walker = WalkDir::new(&root).follow_links(params.follow_symlinks);
    if !params.find_recursively {
        walker = walker.max_depth(1);
    }
    for entry in walker
        .into_iter()
        .filter_entry(|e| keep_walk_entry(e, skip_hidden, ignore.as_ref()))
        .filter_map(Result::ok)
    {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let p = entry.path();
        // Do not include the search root directory itself as a hit
        if p == root {
            continue;
        }
        let name = entry.file_name().to_string_lossy();
        let name_ok = if let Some(re) = &name_re {
            re.is_match(&name)
        } else if let Some(g) = &glob {
            g.is_match(&name)
        } else {
            false
        };
        if !name_ok {
            continue;
        }
        match &content_filter {
            ContentFilter::None => on_hit(p.to_path_buf()),
            ContentFilter::InvalidRegex => {}
            ContentFilter::Substring(q) => {
                if entry.file_type().is_file()
                    && file_contains(p, q, params.case_sensitive, whole_words)
                {
                    on_hit(p.to_path_buf());
                }
            }
            ContentFilter::Regex(re) => {
                if entry.file_type().is_file() && file_contains_regex(p, re, whole_words) {
                    on_hit(p.to_path_buf());
                }
            }
        }
    }
}

fn keep_walk_entry(
    entry: &walkdir::DirEntry,
    skip_hidden: bool,
    ignore: Option<&IgnoreSpec>,
) -> bool {
    if let Some(spec) = ignore {
        if spec.skips(entry) {
            return false;
        }
    }
    if !skip_hidden || entry.depth() == 0 {
        return true;
    }
    let name = entry.file_name().to_string_lossy();
    !(name.starts_with('.') && name.as_ref() != "..")
}

/// Directories to skip when "Enable ignore directories" is on (mc(1) Find File).
struct IgnoreSpec {
    /// Absolute paths: skip the path itself and anything under it.
    abs: Vec<PathBuf>,
    /// Relative tokens: skip a directory whose file name equals the token.
    names: Vec<String>,
}

impl IgnoreSpec {
    fn from_params(params: &FindParams) -> Option<Self> {
        if !params.enable_ignore_dirs {
            return None;
        }
        let mut abs = Vec::new();
        let mut names = Vec::new();
        for token in params.ignore_dirs.split(':') {
            if token.is_empty() {
                continue;
            }
            if token == "." {
                // Man page: a lone dot means the current absolute start directory.
                abs.push(params.start_dir.clone());
            } else {
                let p = Path::new(token);
                if p.is_absolute() {
                    abs.push(p.to_path_buf());
                } else {
                    names.push(token.to_string());
                }
            }
        }
        Some(Self { abs, names })
    }

    fn skips(&self, entry: &walkdir::DirEntry) -> bool {
        let path = entry.path();
        if self.abs.iter().any(|p| path.starts_with(p)) {
            return true;
        }
        if entry.file_type().is_dir() {
            let name = entry.file_name().to_string_lossy();
            if self.names.iter().any(|n| n == name.as_ref()) {
                return true;
            }
        }
        false
    }
}

enum ContentFilter<'a> {
    None,
    Substring(&'a str),
    Regex(Regex),
    InvalidRegex,
}

fn compile_content_filter(params: &FindParams) -> ContentFilter<'_> {
    match params.content_substring.as_deref() {
        None => ContentFilter::None,
        Some(q) if params.regular_expression => match RegexBuilder::new(q)
            .case_insensitive(!params.case_sensitive)
            .build()
        {
            Ok(re) => ContentFilter::Regex(re),
            Err(_) => ContentFilter::InvalidRegex,
        },
        Some(q) => ContentFilter::Substring(q),
    }
}

fn file_contains(path: &Path, needle: &str, case_sensitive: bool, whole_words: bool) -> bool {
    for_each_line(path, |buf| {
        line_contains(buf, needle, case_sensitive, whole_words)
    })
}

fn file_contains_regex(path: &Path, re: &Regex, whole_words: bool) -> bool {
    for_each_line(path, |buf| {
        if !whole_words {
            re.is_match(buf)
        } else {
            // Apply grep -w bounds around each match; do not wrap the user pattern
            // (wrapping would break anchors such as ^ and $).
            re.find_iter(buf)
                .any(|m| match_is_whole_word(buf, m.start(), m.end()))
        }
    })
}

fn is_ascii_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// grep -w: the match is a whole word if it is bounded by non-word chars or string edges.
fn match_is_whole_word(hay: &str, start: usize, end: usize) -> bool {
    let before_ok = start == 0
        || hay
            .get(..start)
            .and_then(|s| s.chars().next_back())
            .is_none_or(|c| !is_ascii_word_char(c));
    let after_ok = end >= hay.len()
        || hay
            .get(end..)
            .and_then(|s| s.chars().next())
            .is_none_or(|c| !is_ascii_word_char(c));
    before_ok && after_ok
}

fn line_contains(hay: &str, needle: &str, case_sensitive: bool, whole_words: bool) -> bool {
    if !whole_words {
        return if case_sensitive {
            hay.contains(needle)
        } else {
            hay.to_lowercase().contains(&needle.to_lowercase())
        };
    }
    if needle.is_empty() {
        return true;
    }
    if case_sensitive {
        find_whole_word_substring(hay, needle)
    } else {
        find_whole_word_substring(&hay.to_lowercase(), &needle.to_lowercase())
    }
}

fn find_whole_word_substring(hay: &str, needle: &str) -> bool {
    let mut from = 0;
    while from <= hay.len() {
        let Some(rel) = hay.get(from..).and_then(|rest| rest.find(needle)) else {
            return false;
        };
        let start = from + rel;
        let end = start + needle.len();
        if match_is_whole_word(hay, start, end) {
            return true;
        }
        let Some(ch) = hay[start..].chars().next() else {
            return false;
        };
        from = start + ch.len_utf8();
    }
    false
}

fn for_each_line(path: &Path, mut pred: impl FnMut(&str) -> bool) -> bool {
    use std::fs::File;
    use std::io::{BufRead, BufReader};
    let file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let mut buf = String::new();
    let mut reader = BufReader::new(file);
    loop {
        buf.clear();
        match reader.read_line(&mut buf) {
            Ok(0) => break,
            Ok(_) => {
                if pred(&buf) {
                    return true;
                }
            }
            Err(_) => break,
        }
    }
    false
}

// Very small glob matcher supporting * and ?
struct GlobMatcher {
    pat: String,
    case_sensitive: bool,
}

impl GlobMatcher {
    fn new(pat: &str, case_sensitive: bool) -> Self {
        Self {
            pat: pat.to_string(),
            case_sensitive,
        }
    }
    fn is_match(&self, name: &str) -> bool {
        glob_match_simple(&self.pat, name, self.case_sensitive)
    }
}

fn glob_match_simple(pattern: &str, text: &str, case_sensitive: bool) -> bool {
    glob_match_bytes(pattern.as_bytes(), text.as_bytes(), case_sensitive)
}

fn glob_byte_eq(a: u8, b: u8, case_sensitive: bool) -> bool {
    if case_sensitive {
        a == b
    } else {
        a.eq_ignore_ascii_case(&b)
    }
}

fn glob_match_bytes(pat: &[u8], text: &[u8], case_sensitive: bool) -> bool {
    // Classic backtracking matcher for '*' and '?' only
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star_idx, mut match_idx) = (None, 0usize);
    while ti < text.len() {
        if pi < pat.len() && (pat[pi] == b'?' || glob_byte_eq(pat[pi], text[ti], case_sensitive)) {
            pi += 1;
            ti += 1;
        } else if pi < pat.len() && pat[pi] == b'*' {
            star_idx = Some(pi);
            match_idx = ti;
            pi += 1;
        } else if let Some(si) = star_idx {
            pi = si + 1;
            match_idx += 1;
            ti = match_idx;
        } else {
            return false;
        }
    }
    while pi < pat.len() && pat[pi] == b'*' {
        pi += 1;
    }
    pi == pat.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn params(root: PathBuf) -> FindParams {
        FindParams {
            start_dir: root,
            name_pattern: NamePattern::Glob("*".into()),
            content_substring: None,
            case_sensitive: true,
            regular_expression: false,
            find_recursively: true,
            follow_symlinks: false,
            skip_hidden: false,
            whole_words: false,
            enable_ignore_dirs: false,
            ignore_dirs: String::new(),
        }
    }

    fn hits(p: &FindParams) -> Vec<PathBuf> {
        search_files(p, &Arc::new(AtomicBool::new(false)))
    }

    #[test]
    fn test_glob_match() {
        assert!(glob_match_simple("*.rs", "main.rs", true));
        assert!(glob_match_simple("m?in.rs", "main.rs", true));
        assert!(!glob_match_simple("*.rs", "main.c", true));
        assert!(glob_match_simple("*", "anything", true));
        assert!(glob_match_simple("*.txt", "FOO.TXT", false));
        assert!(!glob_match_simple("*.txt", "FOO.TXT", true));
    }

    #[test]
    fn test_search_name_and_content() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let foo = root.join("foo.txt");
        let bar = root.join("bar.log");
        std::fs::write(&foo, "Hello World").unwrap();
        std::fs::write(&bar, "nothing here").unwrap();
        let mut p = params(root.to_path_buf());
        p.name_pattern = NamePattern::Glob("*.txt".into());
        p.content_substring = Some("world".into());
        p.case_sensitive = false;
        let res = hits(&p);
        assert_eq!(res.len(), 1);
        assert_eq!(res[0], foo);
    }

    #[test]
    fn root_dir_not_included_for_star() {
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::write(root.join("x"), "x").unwrap();
        let p = params(root.clone());
        let found = hits(&p);
        assert!(!found.iter().any(|h| h == &root));
    }

    #[test]
    fn glob_txt_with_regex_off() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let txt = root.join("note.txt");
        let log = root.join("note.log");
        std::fs::write(&txt, "a").unwrap();
        std::fs::write(&log, "b").unwrap();
        let mut p = params(root.to_path_buf());
        p.name_pattern = NamePattern::Glob("*.txt".into());
        p.regular_expression = false;
        let res = hits(&p);
        assert_eq!(res, vec![txt]);
    }

    #[test]
    fn regex_filename_foo_dot_star_txt() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let foo = root.join("foo_bar.txt");
        let other = root.join("x.txt");
        std::fs::write(&foo, "a").unwrap();
        std::fs::write(&other, "b").unwrap();
        let mut p = params(root.to_path_buf());
        p.name_pattern = NamePattern::Glob(r"foo.*\.txt".into());
        p.regular_expression = true;
        let res = hits(&p);
        assert_eq!(res, vec![foo]);
        assert!(!res.contains(&other));
    }

    #[test]
    fn content_substring_vs_regex() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let f = root.join("n.txt");
        std::fs::write(&f, "hello world\n").unwrap();
        let mut p = params(root.to_path_buf());
        p.name_pattern = NamePattern::Glob("*".into());
        p.content_substring = Some("wor.d".into());
        p.regular_expression = false;
        assert!(
            hits(&p).is_empty(),
            "substring must not treat '.' as any char"
        );
        p.name_pattern = NamePattern::Glob(".*".into());
        p.regular_expression = true;
        assert_eq!(hits(&p), vec![f]);
    }

    #[test]
    fn skip_hidden_dot_secret() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let visible = root.join("visible.txt");
        let secret = root.join(".secret");
        std::fs::write(&visible, "a").unwrap();
        std::fs::write(&secret, "b").unwrap();
        let mut p = params(root.to_path_buf());
        p.name_pattern = NamePattern::Glob("*".into());
        p.skip_hidden = true;
        let hidden_on = hits(&p);
        assert!(hidden_on.contains(&visible));
        assert!(!hidden_on.contains(&secret));
        p.skip_hidden = false;
        let hidden_off = hits(&p);
        assert!(hidden_off.contains(&visible));
        assert!(hidden_off.contains(&secret));
    }

    #[test]
    fn skip_hidden_does_not_descend_dot_dir() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let hidden_dir = root.join(".hid");
        std::fs::create_dir(&hidden_dir).unwrap();
        let nested = hidden_dir.join("inside.txt");
        std::fs::write(&nested, "x").unwrap();
        let mut p = params(root.to_path_buf());
        p.name_pattern = NamePattern::Glob("*.txt".into());
        p.skip_hidden = true;
        assert!(!hits(&p).contains(&nested));
        p.skip_hidden = false;
        assert!(hits(&p).contains(&nested));
    }

    #[test]
    fn follow_symlinks_descends_symlink_dir() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let start = root.join("start");
        let target = root.join("target");
        std::fs::create_dir(&start).unwrap();
        std::fs::create_dir(&target).unwrap();
        let child = start.join("child.txt");
        let inside = target.join("inside.txt");
        std::fs::write(&child, "c").unwrap();
        std::fs::write(&inside, "i").unwrap();
        let link = start.join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let mut p = params(start.clone());
        p.name_pattern = NamePattern::Glob("*.txt".into());
        p.follow_symlinks = false;
        let off = hits(&p);
        assert!(off.contains(&child));
        assert!(!off.iter().any(|h| h.ends_with("inside.txt")));

        p.follow_symlinks = true;
        let on = hits(&p);
        assert!(on.contains(&child));
        assert!(
            on.iter().any(|h| h.ends_with("inside.txt")),
            "symlink-to-dir must be descended when Follow symlinks is on: {on:?}"
        );
    }

    #[test]
    fn recursively_off_skips_nested() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let child = root.join("child.txt");
        let sub = root.join("sub");
        std::fs::create_dir(&sub).unwrap();
        let nested = sub.join("nested.txt");
        std::fs::write(&child, "c").unwrap();
        std::fs::write(&nested, "n").unwrap();
        let mut p = params(root.to_path_buf());
        p.name_pattern = NamePattern::Glob("*.txt".into());
        p.find_recursively = false;
        let res = hits(&p);
        assert!(res.contains(&child));
        assert!(!res.contains(&nested));
        p.find_recursively = true;
        let rec = hits(&p);
        assert!(rec.contains(&child));
        assert!(rec.contains(&nested));
    }

    #[test]
    fn invalid_filename_regex_yields_zero_hits() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("foo.txt"), "x").unwrap();
        let mut p = params(root.to_path_buf());
        p.name_pattern = NamePattern::Glob("*".into());
        p.regular_expression = true;
        assert!(hits(&p).is_empty());
    }

    #[test]
    fn whole_words_skips_category() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let category = root.join("category.txt");
        let cat = root.join("cat.txt");
        std::fs::write(&category, "category\n").unwrap();
        std::fs::write(&cat, "a cat here\n").unwrap();
        let mut p = params(root.to_path_buf());
        p.content_substring = Some("cat".into());
        p.whole_words = true;
        let res = hits(&p);
        assert_eq!(res, vec![cat]);
        assert!(!res.contains(&category));
    }

    #[test]
    fn whole_words_off_hits_category() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let category = root.join("category.txt");
        let cat = root.join("cat.txt");
        std::fs::write(&category, "category\n").unwrap();
        std::fs::write(&cat, "a cat here\n").unwrap();
        let mut p = params(root.to_path_buf());
        p.content_substring = Some("cat".into());
        p.whole_words = false;
        let res = hits(&p);
        assert!(
            res.contains(&category),
            "substring cat must hit category: {res:?}"
        );
        assert!(
            res.contains(&cat),
            "substring cat must hit cat.txt: {res:?}"
        );
    }

    #[test]
    fn ignore_dirs_relative_git() {
        let dir = tempdir().unwrap();
        let start = dir.path().join("start");
        std::fs::create_dir(&start).unwrap();
        let keep = start.join("keep.txt");
        std::fs::write(&keep, "k").unwrap();
        let git = start.join(".git");
        std::fs::create_dir(&git).unwrap();
        std::fs::write(git.join("HEAD"), "ref").unwrap();
        std::fs::create_dir(git.join("objects")).unwrap();
        std::fs::write(git.join("objects").join("x"), "blob").unwrap();

        let mut p = params(start);
        p.enable_ignore_dirs = true;
        p.ignore_dirs = ".git".into();
        let res = hits(&p);
        assert_eq!(res, vec![keep]);
        assert!(
            !res.iter()
                .any(|h| h.components().any(|c| c.as_os_str() == ".git")),
            "must not report hits under .git: {res:?}"
        );
    }

    #[test]
    fn ignore_dirs_colon_list() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let git = root.join(".git");
        let cvs = root.join("CVS");
        let visible = root.join("visible");
        std::fs::create_dir(&git).unwrap();
        std::fs::create_dir(&cvs).unwrap();
        std::fs::create_dir(&visible).unwrap();
        std::fs::write(git.join("HEAD"), "ref").unwrap();
        std::fs::write(cvs.join("Entries"), "e").unwrap();
        let kept = visible.join("file.txt");
        std::fs::write(&kept, "v").unwrap();

        let mut p = params(root.to_path_buf());
        p.enable_ignore_dirs = true;
        p.ignore_dirs = ".git:CVS".into();
        let res = hits(&p);
        assert!(
            res.contains(&kept),
            "file under visible/ must be kept: {res:?}"
        );
        assert!(
            !res.iter()
                .any(|h| h.starts_with(&git) || h.starts_with(&cvs)),
            ".git and CVS must both be skipped: {res:?}"
        );
    }

    #[test]
    fn ignore_dirs_checkbox_off_does_not_skip() {
        let dir = tempdir().unwrap();
        let start = dir.path().join("start");
        std::fs::create_dir(&start).unwrap();
        let keep = start.join("keep.txt");
        std::fs::write(&keep, "k").unwrap();
        let git = start.join(".git");
        std::fs::create_dir(&git).unwrap();
        let head = git.join("HEAD");
        std::fs::write(&head, "ref").unwrap();

        let mut p = params(start);
        p.enable_ignore_dirs = false;
        p.ignore_dirs = ".git".into();
        let res = hits(&p);
        assert!(res.contains(&keep));
        assert!(
            res.contains(&head),
            "ignore list must be unused when the checkbox is off: {res:?}"
        );
    }

    #[test]
    fn ignore_dirs_dot_means_start_dir() {
        // Man page: a lone "." in the ignore list is the current absolute start
        // directory. Skipping that path does not descend, so the walk yields no hits.
        let dir = tempdir().unwrap();
        let start = dir.path().join("start");
        std::fs::create_dir(&start).unwrap();
        std::fs::write(start.join("keep.txt"), "k").unwrap();

        let mut p = params(start);
        p.enable_ignore_dirs = true;
        p.ignore_dirs = ".".into();
        assert!(
            hits(&p).is_empty(),
            "ignoring start_dir via '.' must skip the whole walk"
        );
    }

    #[test]
    fn find_params_deserialize_defaults_new_fields() {
        let json = r#"{
            "start_dir": "/tmp",
            "name_pattern": {"Glob": "*"},
            "content_substring": null,
            "case_sensitive": false,
            "regular_expression": false,
            "find_recursively": true,
            "follow_symlinks": false,
            "skip_hidden": false
        }"#;
        let p: FindParams = serde_json::from_str(json).unwrap();
        assert!(!p.whole_words);
        assert!(!p.enable_ignore_dirs);
        assert!(p.ignore_dirs.is_empty());
    }
}
