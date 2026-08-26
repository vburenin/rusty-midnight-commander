use crate::panel::FileEntry;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDir {
    Asc,
    Desc,
}

pub fn sort_by_name(entries: &mut [FileEntry], dir: SortDir) {
    entries.sort_by(|a, b| match dir {
        SortDir::Asc => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        SortDir::Desc => b.name.to_lowercase().cmp(&a.name.to_lowercase()),
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
