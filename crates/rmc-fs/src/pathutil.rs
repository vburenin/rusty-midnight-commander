use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveKind {
    Tar,
    TarGz,
    Zip,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchivePath {
    pub archive: PathBuf,
    pub inner: PathBuf, // normalized, may be empty for root
    pub kind: ArchiveKind,
}

pub fn parse_archive_path(path: &Path) -> Option<ArchivePath> {
    // Find a component that ends with '#' which marks the archive root dir.
    let mut comps = Vec::<Component<'_>>::new();
    for c in path.components() {
        comps.push(c);
    }
    let mut anchor_index: Option<usize> = None;
    for (i, c) in comps.iter().enumerate() {
        let s = c.as_os_str().to_string_lossy();
        if s.ends_with('#') {
            anchor_index = Some(i);
            break;
        }
    }
    let Some(idx) = anchor_index else {
        return None;
    };
    // Build the archive filesystem path (strip trailing '#')
    let mut archive = PathBuf::new();
    for c in &comps[..=idx] {
        let mut s = c.as_os_str().to_string_lossy().to_string();
        if s.ends_with('#') {
            s.pop(); // remove '#'
        }
        archive.push(s);
    }
    // Build inner path from remaining components
    let mut inner = PathBuf::new();
    for c in &comps[idx + 1..] {
        inner.push(c.as_os_str());
    }
    // Determine archive kind by extension
    let kind = detect_archive_kind(&archive)?;
    Some(ArchivePath { archive, inner, kind })
}

pub fn detect_archive_kind(path: &Path) -> Option<ArchiveKind> {
    let name = path.file_name()?.to_string_lossy().to_lowercase();
    if name.ends_with(".tar") {
        Some(ArchiveKind::Tar)
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        Some(ArchiveKind::TarGz)
    } else if name.ends_with(".zip") {
        Some(ArchiveKind::Zip)
    } else {
        None
    }
}

pub fn append_anchor(path: &Path) -> PathBuf {
    // Create a new PathBuf with a trailing '#' on the last component.
    let mut s = path.as_os_str().to_string_lossy().to_string();
    s.push('#');
    PathBuf::from(s)
}

