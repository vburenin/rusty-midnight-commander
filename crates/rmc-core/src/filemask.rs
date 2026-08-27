//! GNU mc(1) Copy/Move source mask and destination wildcard expansion.
//!
//! Clean-room from the public mc(1) “Mask Copy/Rename” section: shell globs vs
//! regex, tagged-file filtering, and destination `*` / `\0`…`\9` replacement.
//! Not a port of GNU C.

use regex::Regex;
use std::path::{Path, PathBuf};

/// Replace destination wildcards using groups captured from `src_name` against
/// `mask`. Returns `None` when `src_name` does not match `mask`.
///
/// `using_shell_patterns` follows the Copy/Move dialog checkbox:
/// - `true`: `mask` is a shell glob (`*`, `?`, `[seq]`); `*` and `?` are groups
/// - `false`: `mask` is a GNU/ed-style regex (`\(...\)` groups)
pub fn file_mask_replace(
    mask: &str,
    src_name: &str,
    dest: &str,
    using_shell_patterns: bool,
) -> Option<String> {
    let groups = match_mask(mask, src_name, using_shell_patterns)?;
    Some(apply_dest_mask(dest, &groups, src_name))
}

/// Filter `sources` by the source mask and compute each destination path.
///
/// When `dest_is_dir` is true and `dest` has no replacement tokens, each
/// matching file is placed inside `dest` under its original basename (GNU:
/// default `*` into another directory). Otherwise the destination string is
/// expanded with [`file_mask_replace`].
pub fn resolve_copy_move_pairs(
    sources: &[PathBuf],
    mask: &str,
    dest: &str,
    using_shell_patterns: bool,
    dest_is_dir: bool,
) -> Vec<(PathBuf, PathBuf)> {
    sources
        .iter()
        .filter_map(|src| {
            let name = src.file_name()?.to_string_lossy();
            let dst = resolve_one(&name, mask, dest, using_shell_patterns, dest_is_dir)?;
            Some((src.clone(), dst))
        })
        .collect()
}

/// Destination is an existing directory, or the typed path ends with `/`.
pub fn dest_is_directory(dest: &str, exists_as_dir: bool) -> bool {
    exists_as_dir || dest.ends_with('/')
}

fn resolve_one(
    src_name: &str,
    mask: &str,
    dest: &str,
    using_shell_patterns: bool,
    dest_is_dir: bool,
) -> Option<PathBuf> {
    if dest_is_dir && !dest_has_replace_tokens(dest) {
        match_mask(mask, src_name, using_shell_patterns)?;
        Some(Path::new(dest).join(src_name))
    } else {
        file_mask_replace(mask, src_name, dest, using_shell_patterns).map(PathBuf::from)
    }
}

fn dest_has_replace_tokens(dest: &str) -> bool {
    let chs: Vec<char> = dest.chars().collect();
    let mut i = 0usize;
    while i < chs.len() {
        if chs[i] == '*' {
            return true;
        }
        if chs[i] == '\\' && i + 1 < chs.len() {
            return true;
        }
        i += 1;
    }
    false
}

/// Capture groups from matching `src_name` against `mask`. Index 0 is unused
/// so `\1` maps to `groups[1]`. Returns `None` when the name does not match.
fn match_mask(mask: &str, src_name: &str, using_shell_patterns: bool) -> Option<Vec<String>> {
    let re = if using_shell_patterns {
        Regex::new(&shell_mask_to_regex(mask)).ok()?
    } else {
        Regex::new(&gnu_regex_to_rust(mask)).ok()?
    };
    let caps = re.captures(src_name)?;
    if using_shell_patterns && caps.get(0).map(|m| m.as_str()) != Some(src_name) {
        return None;
    }
    let mut groups = vec![String::new()];
    for i in 1..caps.len() {
        groups.push(
            caps.get(i)
                .map(|m| m.as_str().to_string())
                .unwrap_or_default(),
        );
    }
    Some(groups)
}

fn shell_mask_to_regex(mask: &str) -> String {
    let chs: Vec<char> = mask.chars().collect();
    let mut out = String::from("^");
    let mut i = 0usize;
    while i < chs.len() {
        match chs[i] {
            '*' => {
                out.push_str("(.*)");
                i += 1;
            }
            '?' => {
                out.push_str("(.)");
                i += 1;
            }
            '[' => {
                if let Some((class, next)) = take_char_class(&chs, i) {
                    out.push_str(&class);
                    i = next;
                } else {
                    out.push_str("\\[");
                    i += 1;
                }
            }
            '\\' if i + 1 < chs.len() => {
                out.push_str(&regex::escape(&chs[i + 1].to_string()));
                i += 2;
            }
            c => {
                out.push_str(&regex::escape(&c.to_string()));
                i += 1;
            }
        }
    }
    out.push('$');
    out
}

/// Copy `[...]` into a regex character class. Shell `[!abc]` becomes `[^abc]`.
fn take_char_class(chs: &[char], open: usize) -> Option<(String, usize)> {
    let mut i = open + 1;
    if i >= chs.len() {
        return None;
    }
    let mut body = String::from("[");
    if chs[i] == '!' || chs[i] == '^' {
        body.push('^');
        i += 1;
    }
    if i >= chs.len() {
        return None;
    }
    let mut first = true;
    while i < chs.len() {
        if !first && chs[i] == ']' {
            body.push(']');
            return Some((body, i + 1));
        }
        body.push(chs[i]);
        i += 1;
        first = false;
    }
    None
}

/// GNU/ed `\( \)` groups (and `\{ \}` counts) → Rust regex.
fn gnu_regex_to_rust(pattern: &str) -> String {
    let chs: Vec<char> = pattern.chars().collect();
    let mut out = String::new();
    let mut i = 0usize;
    while i < chs.len() {
        if chs[i] == '\\' && i + 1 < chs.len() {
            match chs[i + 1] {
                '(' => {
                    out.push('(');
                    i += 2;
                }
                ')' => {
                    out.push(')');
                    i += 2;
                }
                '{' => {
                    out.push('{');
                    i += 2;
                }
                '}' => {
                    out.push('}');
                    i += 2;
                }
                other => {
                    out.push('\\');
                    out.push(other);
                    i += 2;
                }
            }
        } else if chs[i] == '(' {
            out.push_str("\\(");
            i += 1;
        } else if chs[i] == ')' {
            out.push_str("\\)");
            i += 1;
        } else {
            out.push(chs[i]);
            i += 1;
        }
    }
    out
}

#[derive(Clone, Copy)]
enum CaseMode {
    None,
    NextUpper,
    NextLower,
    UntilUpper,
    UntilLower,
}

fn apply_dest_mask(dest: &str, groups: &[String], whole: &str) -> String {
    let chs: Vec<char> = dest.chars().collect();
    let mut out = String::new();
    let mut i = 0usize;
    let mut star_n = 1usize;
    let mut sticky = CaseMode::None;
    let mut mode = CaseMode::None;
    while i < chs.len() {
        if chs[i] == '*' {
            push_text(&mut out, group_at(groups, star_n), &mut mode, sticky);
            star_n += 1;
            i += 1;
            continue;
        }
        if chs[i] == '\\' && i + 1 < chs.len() {
            let n = chs[i + 1];
            match n {
                '0'..='9' => {
                    let idx = (n as u8 - b'0') as usize;
                    let text = if idx == 0 {
                        whole
                    } else {
                        group_at(groups, idx)
                    };
                    push_text(&mut out, text, &mut mode, sticky);
                    i += 2;
                }
                'u' => {
                    mode = CaseMode::NextUpper;
                    i += 2;
                }
                'l' => {
                    mode = CaseMode::NextLower;
                    i += 2;
                }
                'U' => {
                    sticky = CaseMode::UntilUpper;
                    mode = CaseMode::UntilUpper;
                    i += 2;
                }
                'L' => {
                    sticky = CaseMode::UntilLower;
                    mode = CaseMode::UntilLower;
                    i += 2;
                }
                'E' => {
                    sticky = CaseMode::None;
                    mode = CaseMode::None;
                    i += 2;
                }
                other => {
                    push_char(&mut out, other, &mut mode, sticky);
                    i += 2;
                }
            }
            continue;
        }
        push_char(&mut out, chs[i], &mut mode, sticky);
        i += 1;
    }
    out
}

fn group_at(groups: &[String], n: usize) -> &str {
    groups.get(n).map(String::as_str).unwrap_or("")
}

fn push_text(out: &mut String, text: &str, mode: &mut CaseMode, sticky: CaseMode) {
    for c in text.chars() {
        push_char(out, c, mode, sticky);
    }
}

fn push_char(out: &mut String, c: char, mode: &mut CaseMode, sticky: CaseMode) {
    let mapped = match *mode {
        CaseMode::None => c,
        CaseMode::NextUpper | CaseMode::UntilUpper => to_upper(c),
        CaseMode::NextLower | CaseMode::UntilLower => to_lower(c),
    };
    out.push(mapped);
    *mode = match *mode {
        CaseMode::NextUpper | CaseMode::NextLower => sticky,
        other => other,
    };
}

fn to_upper(c: char) -> char {
    c.to_uppercase().next().unwrap_or(c)
}

fn to_lower(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn glob_star_c_to_bak() {
        assert_eq!(
            file_mask_replace("*.c", "foo.c", "*.bak", true).as_deref(),
            Some("foo.bak")
        );
        assert_eq!(file_mask_replace("*.c", "foo.rs", "*.bak", true), None);
    }

    #[test]
    fn glob_tar_gz_to_tgz_in_directory() {
        assert_eq!(
            file_mask_replace("*.tar.gz", "foo.tar.gz", "/bla/*.tgz", true).as_deref(),
            Some("/bla/foo.tgz")
        );
    }

    #[test]
    fn glob_swap_basename_and_extension() {
        assert_eq!(
            file_mask_replace("*.*", "file.c", r"\2.\1", true).as_deref(),
            Some("c.file")
        );
    }

    #[test]
    fn glob_star_keeps_name_when_dest_has_no_wildcards() {
        assert_eq!(
            file_mask_replace("*", "foo.c", "/right/foo.c", true).as_deref(),
            Some("/right/foo.c")
        );
    }

    #[test]
    fn glob_question_and_char_class_match() {
        assert_eq!(
            file_mask_replace("?.c", "a.c", "*.bak", true).as_deref(),
            Some("a.bak")
        );
        assert_eq!(file_mask_replace("?.c", "ab.c", "*.bak", true), None);
        assert_eq!(
            file_mask_replace("[ab].txt", "a.txt", r"\0", true).as_deref(),
            Some("a.txt")
        );
        assert_eq!(file_mask_replace("[ab].txt", "c.txt", r"\0", true), None);
    }

    #[test]
    fn regex_off_tar_gz_and_swap() {
        assert_eq!(
            file_mask_replace(r"^\(.*\)\.tar\.gz$", "foo.tar.gz", "/bla/*.tgz", false).as_deref(),
            Some("/bla/foo.tgz")
        );
        assert_eq!(
            file_mask_replace(r"^\(.*\)\.\(.*\)$", "file.c", r"\2.\1", false).as_deref(),
            Some("c.file")
        );
    }

    #[test]
    fn glob_star_does_not_match_as_regex() {
        assert_eq!(
            file_mask_replace("*.txt", "foo.txt", "*", false),
            None,
            "shell glob is not a regex"
        );
        assert_eq!(
            file_mask_replace(r".*\.txt$", "foo.txt", r"\0", false).as_deref(),
            Some("foo.txt")
        );
    }

    #[test]
    fn case_conversion_initial_upper() {
        assert_eq!(
            file_mask_replace("*", "fOO.C", r"\L\u*", true).as_deref(),
            Some("Foo.c")
        );
    }

    #[test]
    fn backslash_zero_is_whole_name() {
        assert_eq!(
            file_mask_replace("*.c", "foo.c", r"saved-\0", true).as_deref(),
            Some("saved-foo.c")
        );
    }

    #[test]
    fn directory_dest_keeps_original_basename() {
        let src = PathBuf::from("/left/foo.c");
        let pairs = resolve_copy_move_pairs(std::slice::from_ref(&src), "*", "/right", true, true);
        assert_eq!(pairs, vec![(src, PathBuf::from("/right/foo.c"))]);
    }

    #[test]
    fn tagged_files_filtered_by_mask() {
        let sources = vec![
            PathBuf::from("/left/foo.c"),
            PathBuf::from("/left/bar.rs"),
            PathBuf::from("/left/baz.c"),
        ];
        let pairs = resolve_copy_move_pairs(&sources, "*.c", "/right/*.bak", true, false);
        assert_eq!(
            pairs,
            vec![
                (
                    PathBuf::from("/left/foo.c"),
                    PathBuf::from("/right/foo.bak")
                ),
                (
                    PathBuf::from("/left/baz.c"),
                    PathBuf::from("/right/baz.bak")
                ),
            ]
        );
    }

    #[test]
    fn dest_directory_flag_from_slash_or_stat() {
        assert!(dest_is_directory("/right/", false));
        assert!(dest_is_directory("/right", true));
        assert!(!dest_is_directory("/right/foo.c", false));
    }

    #[test]
    fn quoted_asterisk_is_literal() {
        assert_eq!(
            file_mask_replace("*", "foo", r"pre\*", true).as_deref(),
            Some("pre*")
        );
    }
}
