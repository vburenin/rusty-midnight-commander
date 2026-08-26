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
