use anyhow::Result;
use similar::{Algorithm, TextDiff};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HunkKind {
    Equal,
    Insert,
    Delete,
    Replace,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub kind: HunkKind,
    pub left_start: usize,
    pub left_len: usize,
    pub right_start: usize,
    pub right_len: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffResult {
    pub hunks: Vec<Hunk>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeTarget {
    Left,
    Right,
}

/// Compute a line-based diff between two texts and return hunks.
pub fn compute_diff(left: &str, right: &str) -> DiffResult {
    let diff = TextDiff::configure()
        .algorithm(Algorithm::Myers)
        .timeout(std::time::Duration::from_millis(250))
        .diff_lines(left, right);
    let mut hunks = Vec::new();
    for group in diff.grouped_ops(3) {
        for op in group {
            let (l_count, r_count) = (op.old_range().len(), op.new_range().len());
            let kind = match (l_count, r_count) {
                (0, 0) => HunkKind::Equal,
                (0, _) => HunkKind::Insert,
                (_, 0) => HunkKind::Delete,
                _ => HunkKind::Replace,
            };
            hunks.push(Hunk {
                kind,
                left_start: op.old_range().start,
                left_len: l_count,
                right_start: op.new_range().start,
                right_len: r_count,
            });
        }
    }
    DiffResult { hunks }
}

/// Apply hunk `idx` merging content from the opposite side into `target_side`.
/// `left_lines` and `right_lines` are modified in-place and should be full file line vectors (without trailing newlines).
pub fn apply_hunk_merge(
    left_lines: &mut Vec<String>,
    right_lines: &mut Vec<String>,
    hunks: &[Hunk],
    idx: usize,
    target_side: MergeTarget,
) -> Result<()> {
    let h = hunks
        .get(idx)
        .ok_or_else(|| anyhow::anyhow!("invalid hunk index"))?
        .clone();
    match target_side {
        MergeTarget::Right => {
            // Replace the right hunk region with left lines for this hunk
            let src = &left_lines[h.left_start..h.left_start + h.left_len];
            right_lines.splice(
                h.right_start..h.right_start + h.right_len,
                src.iter().cloned(),
            );
        }
        MergeTarget::Left => {
            let src = &right_lines[h.right_start..h.right_start + h.right_len];
            left_lines.splice(h.left_start..h.left_start + h.left_len, src.iter().cloned());
        }
    }
    Ok(())
}

/// Utility to split text into lines without keeping trailing '\n'.
pub fn split_lines(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in text.chars() {
        if ch == '\n' {
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(ch);
        }
    }
    out.push(cur);
    out
}

/// Utility to join lines with '\n'.
pub fn join_lines(lines: &[String]) -> String {
    let mut out = String::new();
    for (i, l) in lines.iter().enumerate() {
        out.push_str(l);
        if i + 1 != lines.len() {
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn diff_simple_changes() {
        let a = "a\nb\nc\n";
        let b = "a\nB\nc\nd\n";
        let res = compute_diff(a, b);
        assert!(!res.hunks.is_empty());
        // Should detect at least one replace and one insert
        assert!(res
            .hunks
            .iter()
            .any(|h| matches!(h.kind, HunkKind::Replace)));
        assert!(res.hunks.iter().any(|h| matches!(h.kind, HunkKind::Insert)));
    }

    #[test]
    fn next_prev_merge_and_swap_like_ops() {
        let a = split_lines("a\nb\nc\n");
        let b = split_lines("a\nB\nc\nd\n");
        let res = compute_diff(&join_lines(&a), &join_lines(&b));
        let mut left = a.clone();
        let mut right = b.clone();
        // Find first non-equal hunk
        let idx = res
            .hunks
            .iter()
            .position(|h| !matches!(h.kind, HunkKind::Equal))
            .unwrap();
        apply_hunk_merge(&mut left, &mut right, &res.hunks, idx, MergeTarget::Right).unwrap();
        // After merge, the affected region on right should now match left there
        let h = &res.hunks[idx];
        let merged_slice = &right[h.right_start..h.right_start + h.left_len];
        assert_eq!(merged_slice, &left[h.left_start..h.left_start + h.left_len]);
        // Swap sides simulation
        let (mut l2, mut r2) = (right.clone(), left.clone());
        // Merge back opposite to restore original
        let res2 = compute_diff(&join_lines(&l2), &join_lines(&r2));
        let idx2 = res2
            .hunks
            .iter()
            .position(|h| !matches!(h.kind, HunkKind::Equal))
            .unwrap_or(0);
        apply_hunk_merge(&mut l2, &mut r2, &res2.hunks, idx2, MergeTarget::Right).unwrap();
    }
}
