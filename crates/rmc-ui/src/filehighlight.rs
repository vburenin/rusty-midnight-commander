//! Panel listing name colors from skin `[filehighlight]`.
//! Classification is original GNU-ish logic (not copied from GPL sources).

use crate::mc_colors::McPalette;
use crossterm::style::Color;
use rmc_core::panel::FileEntry;

/// Kind used to pick a `[filehighlight]` foreground for a listing name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileHighlightKind {
    Directory,
    Symlink,
    Executable,
    Archive,
    Source,
    Default,
}

const ARCHIVE_EXTS: &[&str] = &[
    "tar", "gz", "tgz", "bz2", "xz", "zst", "zip", "7z", "rar", "cpio", "iso", "deb", "rpm", "ar",
];

const SOURCE_EXTS: &[&str] = &[
    "c", "h", "cc", "cpp", "hpp", "rs", "go", "py", "js", "ts", "java", "rb", "sh",
];

/// GNU-ish filehighlight class for a panel entry.
///
/// Order: symlink (file or dir-link) → directory (incl. `..`) → executable
/// file → archive extension → source extension → default.
pub(crate) fn filehighlight_kind(ent: &FileEntry) -> FileHighlightKind {
    if ent.is_symlink {
        FileHighlightKind::Symlink
    } else if ent.is_dir {
        FileHighlightKind::Directory
    } else if ent.is_exe {
        FileHighlightKind::Executable
    } else if ext_matches(&ent.name, ARCHIVE_EXTS) {
        FileHighlightKind::Archive
    } else if ext_matches(&ent.name, SOURCE_EXTS) {
        FileHighlightKind::Source
    } else {
        FileHighlightKind::Default
    }
}

/// Foreground from the skin palette for `filehighlight_kind(ent)`.
pub(crate) fn filehighlight_color(ent: &FileEntry, pal: &McPalette) -> Color {
    match filehighlight_kind(ent) {
        FileHighlightKind::Directory => pal.dir_color,
        FileHighlightKind::Symlink => pal.symlink_color,
        FileHighlightKind::Executable => pal.exec_color,
        FileHighlightKind::Archive => pal.archive_color,
        FileHighlightKind::Source => pal.source_color,
        FileHighlightKind::Default => pal.core_default_fg,
    }
}

/// Name foreground for a listing row.
///
/// Selected, marked, and mark+select rows keep their core pair; filehighlight
/// is never applied on top of those.
pub(crate) fn listing_name_color(
    ent: &FileEntry,
    pal: &McPalette,
    is_cursor: bool,
    is_active_panel: bool,
    marked: bool,
) -> Color {
    if is_cursor && is_active_panel {
        pal.selected_fg
    } else if marked && is_cursor {
        pal.markselect_fg
    } else if marked {
        pal.marked_fg
    } else {
        filehighlight_color(ent, pal)
    }
}

/// Byte range of `name` inside a preformatted listing line (exact, or a
/// truncated prefix ending in `…`).
pub(crate) fn name_span_in_line(line: &str, name: &str) -> Option<(usize, usize)> {
    if name.is_empty() || line.is_empty() {
        return None;
    }
    if let Some(start) = line.find(name) {
        return Some((start, start + name.len()));
    }
    let chars: Vec<char> = name.chars().collect();
    for len in (1..chars.len()).rev() {
        let prefix: String = chars[..len].iter().collect();
        let with_ellipsis = format!("{prefix}…");
        if let Some(start) = line.find(&with_ellipsis) {
            return Some((start, start + with_ellipsis.len()));
        }
    }
    None
}

fn ext_matches(name: &str, exts: &[&str]) -> bool {
    match last_extension(name) {
        Some(ext) => exts.iter().any(|e| ext.eq_ignore_ascii_case(e)),
        None => false,
    }
}

fn last_extension(name: &str) -> Option<&str> {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let (stem, ext) = base.rsplit_once('.')?;
    if stem.is_empty() || ext.is_empty() {
        None
    } else {
        Some(ext)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::path::PathBuf;
    use std::time::SystemTime;

    fn entry(name: &str, is_dir: bool, is_symlink: bool, is_exe: bool) -> FileEntry {
        FileEntry {
            name: name.to_string(),
            path: PathBuf::from(name),
            is_dir,
            is_symlink,
            symlink_target: None,
            is_exe,
            size: 0,
            modified: SystemTime::UNIX_EPOCH,
            accessed: SystemTime::UNIX_EPOCH,
            changed: SystemTime::UNIX_EPOCH,
            permissions: 0o644,
            owner: None,
            group: None,
            nlink: 1,
            inode: 0,
            is_stale_symlink: false,
        }
    }

    #[test]
    fn kind_directory_includes_parent_marker() {
        assert_eq!(
            filehighlight_kind(&entry("..", true, false, false)),
            FileHighlightKind::Directory
        );
        assert_eq!(
            filehighlight_kind(&entry("src", true, false, false)),
            FileHighlightKind::Directory
        );
    }

    #[test]
    fn kind_symlink_wins_over_directory_and_exe() {
        assert_eq!(
            filehighlight_kind(&entry("link", false, true, false)),
            FileHighlightKind::Symlink
        );
        assert_eq!(
            filehighlight_kind(&entry("dirlink", true, true, false)),
            FileHighlightKind::Symlink
        );
        assert_eq!(
            filehighlight_kind(&entry("exelink", false, true, true)),
            FileHighlightKind::Symlink
        );
    }

    #[test]
    fn kind_executable_wins_over_source_and_archive() {
        assert_eq!(
            filehighlight_kind(&entry("a.out", false, false, true)),
            FileHighlightKind::Executable
        );
        assert_eq!(
            filehighlight_kind(&entry("run.sh", false, false, true)),
            FileHighlightKind::Executable
        );
        assert_eq!(
            filehighlight_kind(&entry("pkg.zip", false, false, true)),
            FileHighlightKind::Executable
        );
    }

    #[test]
    fn kind_archive_by_extension_case_insensitive() {
        for name in [
            "a.tar",
            "a.gz",
            "a.tgz",
            "a.bz2",
            "a.xz",
            "a.zst",
            "a.zip",
            "a.7z",
            "a.rar",
            "a.cpio",
            "a.iso",
            "a.deb",
            "a.rpm",
            "a.ar",
            "Archive.ZIP",
            "src.tar.gz",
        ] {
            assert_eq!(
                filehighlight_kind(&entry(name, false, false, false)),
                FileHighlightKind::Archive,
                "{name}"
            );
        }
    }

    #[test]
    fn kind_source_by_extension() {
        for name in [
            "a.c", "a.h", "a.cc", "a.cpp", "a.hpp", "a.rs", "a.go", "a.py", "a.js", "a.ts",
            "a.java", "a.rb", "a.sh", "Main.RS",
        ] {
            assert_eq!(
                filehighlight_kind(&entry(name, false, false, false)),
                FileHighlightKind::Source,
                "{name}"
            );
        }
    }

    #[test]
    fn kind_directory_named_like_source_stays_directory() {
        assert_eq!(
            filehighlight_kind(&entry("foo.rs", true, false, false)),
            FileHighlightKind::Directory
        );
    }

    #[test]
    fn kind_regular_file_is_default() {
        assert_eq!(
            filehighlight_kind(&entry("readme.txt", false, false, false)),
            FileHighlightKind::Default
        );
        assert_eq!(
            filehighlight_kind(&entry(".rs", false, false, false)),
            FileHighlightKind::Default
        );
    }

    #[test]
    fn listing_name_color_never_overrides_selected_or_marked() {
        let pal = McPalette::default();
        let dir = entry("src", true, false, false);
        assert_eq!(
            listing_name_color(&dir, &pal, true, true, false),
            pal.selected_fg
        );
        assert_eq!(
            listing_name_color(&dir, &pal, true, false, true),
            pal.markselect_fg
        );
        assert_eq!(
            listing_name_color(&dir, &pal, false, true, true),
            pal.marked_fg
        );
        assert_eq!(
            listing_name_color(&dir, &pal, false, false, false),
            pal.dir_color
        );
        let exe = entry("bin", false, false, true);
        assert_eq!(
            listing_name_color(&exe, &pal, false, true, false),
            pal.exec_color
        );
    }

    #[test]
    fn name_span_finds_exact_and_truncated_names() {
        let line = "/readme.txt      |      42";
        assert_eq!(name_span_in_line(line, "readme.txt"), Some((1, 11)));
        let clipped = "very-long-filen…  12";
        assert_eq!(
            name_span_in_line(clipped, "very-long-filename-that-clips"),
            Some((0, "very-long-filen…".len()))
        );
        assert_eq!(name_span_in_line("nope", "missing"), None);
    }
}
