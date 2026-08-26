use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveKind {
    Tar,
    TarGz,
    Zip,
    Cpio,
    CpioGz,
    SevenZ,
    Iso,
    Rar,
    Ar,
    Deb,
    Rpm,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchivePath {
    pub archive: PathBuf,
    pub inner: PathBuf, // normalized, may be empty for root
    pub kind: ArchiveKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchorPath {
    pub base: PathBuf,
    pub inner: PathBuf, // normalized, may be empty for root
}

/// Parse a generic VFS anchor path (component ending with '#'), regardless of archive kind.
/// This is useful for UI display (caption) to hide the internal '#' marker for extfs and others.
pub fn parse_anchor_any(path: &Path) -> Option<AnchorPath> {
    // Find a component that ends with '#'
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
    let idx = anchor_index?;
    // Build base path (strip trailing '#')
    let mut base = PathBuf::new();
    for c in &comps[..=idx] {
        let mut s = c.as_os_str().to_string_lossy().to_string();
        if s.ends_with('#') {
            s.pop();
        }
        base.push(s);
    }
    // Build inner path from remaining components
    let mut inner = PathBuf::new();
    for c in &comps[idx + 1..] {
        inner.push(c.as_os_str());
    }
    Some(AnchorPath { base, inner })
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
    let idx = anchor_index?;
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
    Some(ArchivePath {
        archive,
        inner,
        kind,
    })
}

pub fn detect_archive_kind(path: &Path) -> Option<ArchiveKind> {
    let name = path.file_name()?.to_string_lossy().to_lowercase();
    if name.ends_with(".tar") {
        Some(ArchiveKind::Tar)
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        Some(ArchiveKind::TarGz)
    } else if name.ends_with(".zip") {
        Some(ArchiveKind::Zip)
    } else if name.ends_with(".cpio") {
        Some(ArchiveKind::Cpio)
    } else if name.ends_with(".cpio.gz") {
        Some(ArchiveKind::CpioGz)
    } else if name.ends_with(".7z") {
        Some(ArchiveKind::SevenZ)
    } else if name.ends_with(".iso") {
        Some(ArchiveKind::Iso)
    } else if name.ends_with(".rar") {
        Some(ArchiveKind::Rar)
    } else if name.ends_with(".ar") {
        Some(ArchiveKind::Ar)
    } else if name.ends_with(".deb") {
        Some(ArchiveKind::Deb)
    } else if name.ends_with(".rpm") {
        Some(ArchiveKind::Rpm)
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parse_anchor_any_splits_base_and_inner() {
        let p = PathBuf::from("/tmp/sample.lsar#");
        let a = parse_anchor_any(&p).expect("anchor");
        assert_eq!(a.base, PathBuf::from("/tmp/sample.lsar"));
        assert!(a.inner.as_os_str().is_empty());
        let p2 = PathBuf::from("/tmp/sample.lsar#/a.txt");
        let a2 = parse_anchor_any(&p2).expect("anchor");
        assert_eq!(a2.base, PathBuf::from("/tmp/sample.lsar"));
        assert_eq!(a2.inner, PathBuf::from("a.txt"));
    }
}
