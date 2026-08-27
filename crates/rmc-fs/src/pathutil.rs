use std::path::{Component, Path, PathBuf};

/// Return a canonical remote URL if `path` encodes a remote location.
/// Supports:
/// - URI schemes: ftp://, sftp://, fish://, smb://
/// - Anchor-style: .../#fish:<rest>, .../#smb:<rest>
///   The returned string is a normalized URI (e.g. fish://<rest>).
pub fn extract_remote_canonical_url(path: &Path) -> Option<String> {
    let s = path.as_os_str().to_string_lossy();
    for scheme in ["ftp://", "sftp://", "fish://", "smb://"] {
        if s.starts_with(scheme) {
            return Some(s.to_string());
        }
    }
    // Look for an anchor component ending with '#', and check if it is "#fish:" or "#smb:"
    // Accept both absolute and relative paths; scan raw string for the markers for simplicity.
    if let Some(idx) = s.find("#fish:") {
        // Everything after "#fish:" is the authority+path
        let rest = &s[idx + "#fish:".len()..];
        return Some(format!("fish://{rest}"));
    }
    if let Some(idx) = s.find("#smb:") {
        let rest = &s[idx + "#smb:".len()..];
        return Some(format!("smb://{rest}"));
    }
    None
}

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

/// True when `path` is an archive/extfs `#` VFS path or a remote URL
/// (`ftp://`, `sftp://`, `fish://`, `smb://`).
///
/// CompositeFs does not treat those as `LocalFs`, so `std::fs` on the raw
/// path fails. A `#` marker that is neither a known archive nor a remote URL
/// is still treated as virtual here (extfs uses the same anchor).
pub fn is_virtual_path(path: &Path) -> bool {
    parse_anchor_any(path).is_some() || extract_remote_canonical_url(path).is_some()
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

    #[test]
    fn is_virtual_path_detects_archive_anchor_and_remote_url() {
        assert!(
            !is_virtual_path(Path::new("/tmp/local.txt")),
            "plain local path is not virtual"
        );
        assert!(is_virtual_path(Path::new("/tmp/sample.tar#")));
        assert!(is_virtual_path(Path::new("/tmp/sample.tar#/inner.txt")));
        assert!(is_virtual_path(Path::new("/tmp/sample.zip#/dir/a.txt")));
        assert!(is_virtual_path(Path::new("/tmp/helper.lsar#/a.txt")));
        assert!(is_virtual_path(Path::new("ftp://host/pub/file")));
        assert!(is_virtual_path(Path::new("sftp://user@host/tmp")));
        assert!(is_virtual_path(Path::new("fish://host/tmp")));
        assert!(is_virtual_path(Path::new("smb://host/share")));
    }
}
