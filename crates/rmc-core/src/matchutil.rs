//! Simple case-insensitive glob-like matching for panel quick search.
//!
//! Supported wildcards:
//!   - `*` matches any sequence of characters (including empty)
//!   - `?` matches exactly one character
//!
//! If the pattern contains no wildcards, we perform a case-insensitive
//!   substring check to mimic MC quick search typing.

/// Returns true if `name` matches `pattern` using simple glob rules.
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
}
