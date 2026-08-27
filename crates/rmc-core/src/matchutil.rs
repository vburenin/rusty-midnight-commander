//! Filename matching for quick search, Select/Unselect group, and the panel filter.
//!
//! Quick search (`name_matches`) stays a case-insensitive glob/substring helper.
//! Select group, Unselect group, and the panel filter honor GNU mc
//! Options → Configuration → Use shell patterns (`filename_pattern_matches`).

/// Returns true if `name` matches `pattern` using simple glob rules.
///
/// Supported wildcards:
///   - `*` matches any sequence of characters (including empty)
///   - `?` matches exactly one character
///
/// If the pattern contains no wildcards, we perform a case-insensitive
/// substring check to mimic MC quick search typing.
pub fn name_matches(pattern: &str, name: &str) -> bool {
    if pattern.is_empty() {
        return true;
    }
    let has_wildcards = pattern.contains('*') || pattern.contains('?');
    if !has_wildcards {
        return name.to_lowercase().contains(&pattern.to_lowercase());
    }
    glob_match_case_insensitive(pattern, name)
}

/// GNU mc Options → Configuration → Use shell patterns.
///
/// - `shell_patterns == true` (default): `pattern` is a shell glob (`*`, `?`,
///   `[abc]` / `[a-z]` / `[!…]`), matched against the whole file name.
/// - `shell_patterns == false`: `pattern` is a regular expression. Invalid
///   patterns match nothing.
pub fn filename_pattern_matches(pattern: &str, name: &str, shell_patterns: bool) -> bool {
    if shell_patterns {
        shell_glob_matches(pattern, name)
    } else {
        regex_matches(pattern, name)
    }
}

fn glob_match_case_insensitive(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let s: Vec<char> = name.to_lowercase().chars().collect();
    glob_match(&p, &s)
}

fn glob_match(pat: &[char], text: &[char]) -> bool {
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star_idx, mut match_idx) = (None, 0usize);
    while ti < text.len() {
        if pi < pat.len() && (pat[pi] == '?' || pat[pi] == text[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < pat.len() && pat[pi] == '*' {
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
    while pi < pat.len() && pat[pi] == '*' {
        pi += 1;
    }
    pi == pat.len()
}

fn regex_matches(pattern: &str, name: &str) -> bool {
    regex::Regex::new(pattern).is_ok_and(|re| re.is_match(name))
}

#[derive(Clone)]
enum GlobAtom {
    Star,
    Any,
    Lit(char),
    Class(CharClass),
}

#[derive(Clone)]
struct CharClass {
    negated: bool,
    ranges: Vec<(char, char)>,
}

impl CharClass {
    fn contains(&self, c: char) -> bool {
        let inside = self.ranges.iter().any(|&(a, b)| c >= a && c <= b);
        if self.negated {
            !inside
        } else {
            inside
        }
    }
}

fn shell_glob_matches(pattern: &str, name: &str) -> bool {
    let atoms = parse_shell_glob(pattern);
    let text: Vec<char> = name.chars().collect();
    glob_atoms_match(&atoms, &text)
}

fn parse_shell_glob(pattern: &str) -> Vec<GlobAtom> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut atoms = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        match chars[i] {
            '*' => {
                atoms.push(GlobAtom::Star);
                i += 1;
            }
            '?' => {
                atoms.push(GlobAtom::Any);
                i += 1;
            }
            '[' => {
                if let Some((class, next)) = parse_char_class(&chars, i) {
                    atoms.push(GlobAtom::Class(class));
                    i = next;
                } else {
                    atoms.push(GlobAtom::Lit('['));
                    i += 1;
                }
            }
            '\\' if i + 1 < chars.len() => {
                atoms.push(GlobAtom::Lit(chars[i + 1]));
                i += 2;
            }
            c => {
                atoms.push(GlobAtom::Lit(c));
                i += 1;
            }
        }
    }
    atoms
}

/// Parse `[abc]`, `[a-z]`, `[!abc]` / `[^abc]`. Unclosed `[` is not a class.
fn parse_char_class(chars: &[char], open: usize) -> Option<(CharClass, usize)> {
    let mut i = open + 1;
    if i >= chars.len() {
        return None;
    }
    let negated = matches!(chars[i], '!' | '^');
    if negated {
        i += 1;
    }
    if i >= chars.len() {
        return None;
    }
    let mut ranges = Vec::new();
    let mut first = true;
    while i < chars.len() {
        if !first && chars[i] == ']' {
            return Some((CharClass { negated, ranges }, i + 1));
        }
        let start = chars[i];
        i += 1;
        if i + 1 < chars.len() && chars[i] == '-' && chars[i + 1] != ']' {
            let end = chars[i + 1];
            i += 2;
            if start <= end {
                ranges.push((start, end));
            } else {
                ranges.push((end, start));
            }
        } else {
            ranges.push((start, start));
        }
        first = false;
    }
    None
}

fn glob_atoms_match(atoms: &[GlobAtom], text: &[char]) -> bool {
    let (mut ai, mut ti) = (0usize, 0usize);
    let (mut star_ai, mut star_ti) = (None, 0usize);
    while ti < text.len() {
        if ai < atoms.len() && atom_matches_char(&atoms[ai], text[ti]) {
            ai += 1;
            ti += 1;
        } else if ai < atoms.len() && matches!(atoms[ai], GlobAtom::Star) {
            star_ai = Some(ai);
            ai += 1;
            star_ti = ti;
        } else if let Some(sa) = star_ai {
            ai = sa + 1;
            star_ti += 1;
            ti = star_ti;
        } else {
            return false;
        }
    }
    while ai < atoms.len() && matches!(atoms[ai], GlobAtom::Star) {
        ai += 1;
    }
    ai == atoms.len()
}

fn atom_matches_char(atom: &GlobAtom, c: char) -> bool {
    match atom {
        GlobAtom::Star => false,
        GlobAtom::Any => true,
        GlobAtom::Lit(x) => *x == c,
        GlobAtom::Class(cl) => cl.contains(c),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plain_substring() {
        assert!(name_matches("abc", "zzzAbCd"));
        assert!(!name_matches("abc", "acb"));
    }

    #[test]
    fn test_qmark() {
        assert!(name_matches("a?c", "AbC"));
        assert!(!name_matches("a?c", "ac"));
    }

    #[test]
    fn test_star() {
        assert!(name_matches("a*c", "abbbc"));
        assert!(name_matches("*", "anything"));
        assert!(name_matches("*.rs", "lib.RS"));
        assert!(!name_matches("a*c", "abbd"));
    }

    #[test]
    fn shell_glob_star_and_qmark() {
        assert!(filename_pattern_matches("*.txt", "foo.txt", true));
        assert!(!filename_pattern_matches("*.txt", "foo.rs", true));
        assert!(filename_pattern_matches("foo.?xt", "foo.txt", true));
        assert!(!filename_pattern_matches("foo.?xt", "foo.rs", true));
        assert!(filename_pattern_matches("*", "foo.txt", true));
    }

    #[test]
    fn shell_glob_character_class() {
        assert!(filename_pattern_matches("[ab].txt", "a.txt", true));
        assert!(filename_pattern_matches("[ab].txt", "b.txt", true));
        assert!(!filename_pattern_matches("[ab].txt", "c.txt", true));
        assert!(filename_pattern_matches("[a-c].c", "b.c", true));
        assert!(!filename_pattern_matches("[a-c].c", "d.c", true));
        assert!(filename_pattern_matches("[!a].txt", "b.txt", true));
        assert!(!filename_pattern_matches("[!a].txt", "a.txt", true));
    }

    #[test]
    fn regex_mode_matches_anchored_txt() {
        assert!(filename_pattern_matches(r".*\.txt$", "foo.txt", false));
        assert!(!filename_pattern_matches(r".*\.txt$", "foo.rs", false));
        assert!(!filename_pattern_matches("*.txt", "foo.txt", false));
    }
}
