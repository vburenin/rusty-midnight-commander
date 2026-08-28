use crate::panel::{FileEntry, SortBy};
use std::cmp::Ordering;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDir {
    Asc,
    Desc,
}

/// Original Apache-2.0 natural/version compare (digit runs as integers).
///
/// Public behavior matches the well-known `strverscmp` contract: `file2` < `file10`,
/// and digit runs with more leading zeros compare as the smaller “fractional” form.
pub fn version_cmp(a: &str, b: &str) -> Ordering {
    let a = a.as_bytes();
    let b = b.as_bytes();
    let mut i = 0usize;
    let mut j = 0usize;
    while i < a.len() && j < b.len() {
        if a[i].is_ascii_digit() && b[j].is_ascii_digit() {
            let i_zeros_start = i;
            let j_zeros_start = j;
            while i < a.len() && a[i] == b'0' {
                i += 1;
            }
            while j < b.len() && b[j] == b'0' {
                j += 1;
            }
            let a_zeros = i - i_zeros_start;
            let b_zeros = j - j_zeros_start;
            let i_digits = i;
            let j_digits = j;
            while i < a.len() && a[i].is_ascii_digit() {
                i += 1;
            }
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            let a_len = i - i_digits;
            let b_len = j - j_digits;
            if a_len != b_len {
                return a_len.cmp(&b_len);
            }
            let digits = a[i_digits..i].cmp(&b[j_digits..j]);
            if digits != Ordering::Equal {
                return digits;
            }
            if a_zeros != b_zeros {
                // More leading zeros → smaller (fractional) when the magnitude matches.
                return b_zeros.cmp(&a_zeros);
            }
        } else {
            match a[i].cmp(&b[j]) {
                Ordering::Equal => {
                    i += 1;
                    j += 1;
                }
                o => return o,
            }
        }
    }
    (a.len() - i).cmp(&(b.len() - j))
}

fn name_key(name: &str) -> String {
    name.to_lowercase()
}

fn cmp_dir<T: Ord>(a: &T, b: &T, dir: SortDir) -> Ordering {
    match dir {
        SortDir::Asc => a.cmp(b),
        SortDir::Desc => b.cmp(a),
    }
}

pub fn sort_by_name(entries: &mut [FileEntry], dir: SortDir) {
    entries.sort_by(|a, b| {
        let ord = version_cmp(&name_key(&a.name), &name_key(&b.name));
        match dir {
            SortDir::Asc => ord,
            SortDir::Desc => ord.reverse(),
        }
    });
}

pub fn sort_by_ext(entries: &mut [FileEntry], dir: SortDir) {
    fn ext_key(name: &str) -> (String, String) {
        // Return (ext, basename) for tie-breaking; directories may have no ext
        let lower = name.to_lowercase();
        match lower.rsplit_once('.') {
            Some((base, ext)) if !base.is_empty() && !ext.is_empty() => {
                (ext.to_string(), base.to_string())
            }
            _ => ("".to_string(), lower),
        }
    }
    entries.sort_by(|a, b| {
        let (ea, ba) = ext_key(&a.name);
        let (eb, bb) = ext_key(&b.name);
        match dir {
            SortDir::Asc => ea.cmp(&eb).then_with(|| ba.cmp(&bb)),
            SortDir::Desc => eb.cmp(&ea).then_with(|| bb.cmp(&ba)),
        }
    });
}

pub fn sort_by_size(entries: &mut [FileEntry], dir: SortDir) {
    entries.sort_by(|a, b| match dir {
        SortDir::Asc => a.size.cmp(&b.size),
        SortDir::Desc => b.size.cmp(&a.size),
    });
}

pub fn sort_by_time(entries: &mut [FileEntry], dir: SortDir) {
    entries.sort_by(|a, b| match dir {
        SortDir::Asc => a.modified.cmp(&b.modified),
        SortDir::Desc => b.modified.cmp(&a.modified),
    });
}

pub fn sort_by_atime(entries: &mut [FileEntry], dir: SortDir) {
    entries.sort_by(|a, b| {
        cmp_dir(&a.accessed, &b.accessed, dir)
            .then_with(|| version_cmp(&name_key(&a.name), &name_key(&b.name)))
    });
}

pub fn sort_by_ctime(entries: &mut [FileEntry], dir: SortDir) {
    entries.sort_by(|a, b| {
        cmp_dir(&a.changed, &b.changed, dir)
            .then_with(|| version_cmp(&name_key(&a.name), &name_key(&b.name)))
    });
}

pub fn sort_by_inode(entries: &mut [FileEntry], dir: SortDir) {
    entries.sort_by(|a, b| {
        cmp_dir(&a.inode, &b.inode, dir)
            .then_with(|| version_cmp(&name_key(&a.name), &name_key(&b.name)))
    });
}

/// Keep `list_dir` order (after `..`); Reverse flips that slice.
pub fn sort_unsorted(entries: &mut [FileEntry], dir: SortDir) {
    if matches!(dir, SortDir::Desc) {
        entries.reverse();
    }
}

pub fn sort_entries(entries: &mut [FileEntry], by: SortBy, dir: SortDir) {
    match by {
        SortBy::Name => sort_by_name(entries, dir),
        SortBy::Ext => sort_by_ext(entries, dir),
        SortBy::Size => sort_by_size(entries, dir),
        SortBy::Time => sort_by_time(entries, dir),
        SortBy::Atime => sort_by_atime(entries, dir),
        SortBy::Ctime => sort_by_ctime(entries, dir),
        SortBy::Inode => sort_by_inode(entries, dir),
        SortBy::Unsorted => sort_unsorted(entries, dir),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_cmp_natural_order() {
        assert_eq!(version_cmp("file2", "file10"), Ordering::Less);
        assert_eq!(version_cmp("file10", "file2"), Ordering::Greater);
        assert_eq!(version_cmp("jan1", "jan2"), Ordering::Less);
        assert_eq!(version_cmp("1.2.3", "1.10.3"), Ordering::Less);
        assert_eq!(version_cmp("abc", "abc"), Ordering::Equal);
        assert_eq!(version_cmp("abc", "abd"), Ordering::Less);
        // Leading zeros: more zeros is smaller when the magnitude matches.
        assert_eq!(version_cmp("007", "07"), Ordering::Less);
        assert_eq!(version_cmp("07", "7"), Ordering::Less);
    }

    #[test]
    fn name_sort_uses_natural_order() {
        use std::path::PathBuf;
        use std::time::SystemTime;
        fn e(name: &str) -> FileEntry {
            FileEntry {
                name: name.to_string(),
                path: PathBuf::from(name),
                is_dir: false,
                is_symlink: false,
                symlink_target: None,
                is_exe: false,
                size: 0,
                modified: SystemTime::UNIX_EPOCH,
                accessed: SystemTime::UNIX_EPOCH,
                changed: SystemTime::UNIX_EPOCH,
                permissions: 0,
                owner: None,
                group: None,
                nlink: 1,
                inode: 0,
                uid: 0,
                gid: 0,
                is_stale_symlink: false,
            }
        }
        let mut v = vec![e("file10"), e("file2"), e("file1")];
        sort_by_name(&mut v, SortDir::Asc);
        let names: Vec<_> = v.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, vec!["file1", "file2", "file10"]);
    }
}
