use crate::matchutil::name_matches;

#[derive(Debug, Clone, Default)]
pub struct QuickSearchState {
    pub pattern: String,
    /// Index to continue "next" search from (exclusive). Uses current cursor if None.
    pub next_from: Option<usize>,
}

impl QuickSearchState {
    pub fn new() -> Self {
        Self {
            pattern: String::new(),
            next_from: None,
        }
    }
}

/// Next listing index matching `pattern`, wrapping once.
///
/// `start_after` is exclusive (`Some(i)` starts at `i + 1`). `None` starts at
/// index 0 after wrap-around of an empty range. Skips the `..` parent marker.
/// Empty pattern matches nothing (no cursor jump).
pub fn find_next_match<'a, I>(
    entries: I,
    pattern: &str,
    start_after: Option<usize>,
) -> Option<usize>
where
    I: IntoIterator<Item = &'a str>,
{
    if pattern.is_empty() {
        return None;
    }
    let names: Vec<&str> = entries.into_iter().collect();
    if names.is_empty() {
        return None;
    }
    let mut start = start_after.map(|i| i.saturating_add(1)).unwrap_or(0);
    if start >= names.len() {
        start = 0;
    }
    let total = names.len();
    for pass in 0..2 {
        let (begin, end) = if pass == 0 {
            (start, total)
        } else {
            (0, start.min(total))
        };
        for (i, name) in names
            .iter()
            .enumerate()
            .skip(begin)
            .take(end.saturating_sub(begin))
        {
            if *name == ".." {
                continue;
            }
            if name_matches(pattern, name) {
                return Some(i);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_pattern_does_not_match() {
        let names = ["..", "abc.txt", "zzz.txt"];
        assert_eq!(find_next_match(names, "", None), None);
        assert_eq!(find_next_match(names, "", Some(0)), None);
    }

    #[test]
    fn skips_parent_and_wraps() {
        let names = ["..", "apple.txt", "banana.txt", "apricot.txt"];
        assert_eq!(find_next_match(names, "a", Some(usize::MAX)), Some(1));
        assert_eq!(find_next_match(names, "a", Some(1)), Some(3));
        assert_eq!(find_next_match(names, "a", Some(3)), Some(1));
        assert_eq!(find_next_match(names, "banana", Some(2)), Some(2));
    }

    #[test]
    fn prefix_not_mid_string() {
        let names = ["..", "abc.txt", "xxabc.txt"];
        assert_eq!(find_next_match(names, "abc", Some(usize::MAX)), Some(1));
        assert_eq!(find_next_match(names, "xx", Some(usize::MAX)), Some(2));
        assert_eq!(find_next_match(names, "nope", Some(usize::MAX)), None);
    }
}
