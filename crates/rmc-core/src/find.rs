use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::sync::mpsc::Receiver;
use walkdir::WalkDir;

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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindDialogFocus {
    StartDir,
    NamePattern,
    Content,
    CaseSensitive,
    ButtonOk,
    ButtonStop,
    ButtonChdir,
    ButtonPanelize,
    ButtonQuit,
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
        Self { inner: Arc::new(AtomicBool::new(false)) }
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
    pub focus: FindDialogFocus,
    pub running: bool,
    pub results: FindResults,
    pub cancel: Option<CancelHandle>,
    pub results_rx: Option<Receiver<Vec<PathBuf>>>,
}

impl FindDialogState {
    pub fn new(start_dir: PathBuf) -> Self {
        Self {
            params: FindParams {
                start_dir,
                name_pattern: NamePattern::Glob("*".into()),
                content_substring: None,
                case_sensitive: false,
            },
            focus: FindDialogFocus::NamePattern,
            running: false,
            results: FindResults::default(),
            cancel: None,
            results_rx: None,
        }
    }
}

pub fn search_files(params: &FindParams, cancel: &Arc<AtomicBool>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    // Prepare name matcher (basic glob with * and ?)
    let matcher = GlobMatcher::new(match &params.name_pattern {
        NamePattern::Glob(s) => s,
    });
    let content_query = params.content_substring.clone();
    let case_sensitive = params.case_sensitive;

    for entry in WalkDir::new(&params.start_dir).into_iter().filter_map(Result::ok) {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let p = entry.path();
        // Skip directories for name-only filtering, but allow returning directories if they match the name pattern and no content search requested.
        let is_dir = entry.file_type().is_dir();
        let name = entry.file_name().to_string_lossy();
        if !matcher.is_match(&name) {
            continue;
        }
        // If content search requested, only check regular files
        if let Some(q) = &content_query {
            if is_dir {
                continue;
            }
            if file_contains(p, q, case_sensitive) {
                out.push(p.to_path_buf());
            }
        } else {
            out.push(p.to_path_buf());
        }
    }
    out
}

fn file_contains(path: &Path, needle: &str, case_sensitive: bool) -> bool {
    use std::fs::File;
    use std::io::{BufRead, BufReader};
    let file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let mut buf = String::new();
    let mut reader = BufReader::new(file);
    // Simple line-by-line scan
    loop {
        buf.clear();
        match reader.read_line(&mut buf) {
            Ok(0) => break,
            Ok(_) => {
                if case_sensitive {
                    if buf.contains(needle) {
                        return true;
                    }
                } else if buf.to_lowercase().contains(&needle.to_lowercase()) {
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
}

impl GlobMatcher {
    fn new(pat: &str) -> Self {
        Self { pat: pat.to_string() }
    }
    fn is_match(&self, name: &str) -> bool {
        glob_match_simple(&self.pat, name)
    }
}

fn glob_match_simple(pattern: &str, text: &str) -> bool {
    glob_match_bytes(pattern.as_bytes(), text.as_bytes())
}

fn glob_match_bytes(pat: &[u8], text: &[u8]) -> bool {
    // Classic backtracking matcher for '*' and '?' only
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star_idx, mut match_idx) = (None, 0usize);
    while ti < text.len() {
        if pi < pat.len() && (pat[pi] == b'?' || pat[pi] == text[ti]) {
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
    #[test]
    fn test_glob_match() {
        assert!(glob_match_simple("*.rs", "main.rs"));
        assert!(glob_match_simple("m?in.rs", "main.rs"));
        assert!(!glob_match_simple("*.rs", "main.c"));
        assert!(glob_match_simple("*", "anything"));
    }

    #[test]
    fn test_search_name_and_content() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let foo = root.join("foo.txt");
        let bar = root.join("bar.log");
        std::fs::write(&foo, "Hello World").unwrap();
        std::fs::write(&bar, "nothing here").unwrap();
        let params = FindParams {
            start_dir: root.to_path_buf(),
            name_pattern: NamePattern::Glob("*.txt".into()),
            content_substring: Some("world".into()),
            case_sensitive: false,
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let res = search_files(&params, &cancel);
        assert_eq!(res.len(), 1);
        assert_eq!(res[0], foo);
    }
}

