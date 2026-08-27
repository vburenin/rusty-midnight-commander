//! Copy/Move progress helpers honoring GNU mc Options → Configuration flags:
//! verbose operation, compute totals, and classic progressbar.
//!
//! Layout strings match the GNU mc file-operations dialog as closely as the
//! existing Copy dialog already does. No live TUI is required to test this.

use crate::app::{ConfigOptions, CopyMoveOp};
use crate::panel::format_byte_size;
use anyhow::Result;
use rmc_fs::Vfs;
use std::path::Path;

/// Pre-scan result used as the progress-bar denominator when Compute totals is on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OpTotals {
    pub bytes: u64,
    pub files: u64,
}

/// Live Copy/Move progress, including the Configuration flags that shape the dialog.
#[derive(Clone, Debug)]
pub struct FileOpProgressState {
    pub op: CopyMoveOp,
    /// Basename (or last path component) of the file currently being processed.
    pub source_name: String,
    pub source_path: String,
    pub target_path: String,
    pub file_done: u64,
    pub file_total: u64,
    pub bytes_done: u64,
    /// `None` when Compute totals is off (no pre-scan, indeterminate overall bar).
    pub bytes_total: Option<u64>,
    pub files_done: u64,
    pub files_total: u64,
    pub verbose: bool,
    pub compute_totals: bool,
    pub classic_progressbar: bool,
}

/// Lines the UI draws. Built from [`FileOpProgressState`] plus bar width / SI flag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileOpProgressView {
    pub title: String,
    /// Current file name. `None` when Verbose operation is off.
    pub source_name: Option<String>,
    pub target_path: String,
    /// GNU mc classic one-line `****` gauge. Present when classic_progressbar is on.
    pub classic_bar: Option<String>,
    /// Current-file gauge. Present when classic_progressbar is off.
    pub file_bar: Option<String>,
    /// Overall-bytes gauge. Present when classic_progressbar is off and totals are known.
    pub total_bar: Option<String>,
    pub files_processed: String,
    /// `Total: X of Y` byte counters. Present when Compute totals is on.
    pub total_bytes: Option<String>,
}

impl FileOpProgressState {
    /// Build progress state, optionally pre-scanning `src` when `opts.compute_totals`.
    pub fn prepare(
        vfs: &dyn Vfs,
        op: CopyMoveOp,
        src: &Path,
        dst: &Path,
        opts: &ConfigOptions,
    ) -> Result<Self> {
        let totals = if opts.compute_totals {
            Some(scan_totals(vfs, src)?)
        } else {
            None
        };
        let source_name = src
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| src.display().to_string());
        let file_total = match vfs.stat(src) {
            Ok(m) if !m.is_dir => m.size,
            _ => 0,
        };
        let files_total = totals.map(|t| t.files).unwrap_or(1);
        Ok(Self {
            op,
            source_name,
            source_path: src.display().to_string(),
            target_path: dst.display().to_string(),
            file_done: 0,
            file_total,
            bytes_done: 0,
            bytes_total: totals.map(|t| t.bytes),
            files_done: 0,
            files_total,
            verbose: opts.verbose,
            compute_totals: opts.compute_totals,
            classic_progressbar: opts.classic_progressbar,
        })
    }

    /// Dialog lines for the current flags and counters.
    pub fn view(&self, bar_width: usize, si: bool) -> FileOpProgressView {
        let title = match self.op {
            CopyMoveOp::Copy => "Copy".to_string(),
            CopyMoveOp::Move => "Move".to_string(),
        };
        let source_name = if self.verbose {
            Some(self.source_name.clone())
        } else {
            None
        };
        let files_processed = format!(
            "Files processed: {} of {}",
            self.files_done, self.files_total
        );
        let total_bytes = if self.compute_totals {
            let total = self.bytes_total.unwrap_or(0);
            Some(format!(
                "Total: {} of {}",
                format_byte_size(self.bytes_done, si),
                format_byte_size(total, si)
            ))
        } else {
            None
        };

        let file_den = if self.file_total > 0 {
            Some(self.file_total)
        } else {
            None
        };
        let overall_den = self.bytes_total.filter(|t| *t > 0);

        if self.classic_progressbar {
            // One-line **** bar: overall size when totals were scanned, else this file.
            let (done, den) = match overall_den {
                Some(t) => (self.bytes_done, Some(t)),
                None => (self.file_done, file_den),
            };
            FileOpProgressView {
                title,
                source_name,
                target_path: self.target_path.clone(),
                classic_bar: Some(classic_gauge(done, den, bar_width)),
                file_bar: None,
                total_bar: None,
                files_processed,
                total_bytes,
            }
        } else {
            FileOpProgressView {
                title,
                source_name,
                target_path: self.target_path.clone(),
                classic_bar: None,
                file_bar: Some(classic_gauge(self.file_done, file_den, bar_width)),
                total_bar: if self.compute_totals {
                    Some(classic_gauge(self.bytes_done, overall_den, bar_width))
                } else {
                    None
                },
                files_processed,
                total_bytes,
            }
        }
    }
}

/// Recursively sum file sizes and file counts under `path`.
/// Directories are walked (not counted); `..` is skipped; symlinks are not followed.
pub fn scan_totals(vfs: &dyn Vfs, path: &Path) -> Result<OpTotals> {
    let meta = vfs.stat(path)?;
    if !meta.is_dir {
        return Ok(OpTotals {
            bytes: meta.size,
            files: 1,
        });
    }
    let mut totals = OpTotals::default();
    scan_dir(vfs, path, &mut totals)?;
    Ok(totals)
}

fn scan_dir(vfs: &dyn Vfs, path: &Path, totals: &mut OpTotals) -> Result<()> {
    for entry in vfs.list_dir(path, true)? {
        if entry.name == ".." || entry.name == "." {
            continue;
        }
        if entry.meta.is_dir {
            if entry.meta.is_symlink {
                // Do not dive into symlink directories (GNU mc Follow links is off by default).
                totals.files = totals.files.saturating_add(1);
                continue;
            }
            scan_dir(vfs, &entry.path, totals)?;
        } else {
            totals.files = totals.files.saturating_add(1);
            totals.bytes = totals.bytes.saturating_add(entry.meta.size);
        }
    }
    Ok(())
}

/// GNU mc classic gauge: `[****    ]  42%`. `total == None` is indeterminate (empty fill, no %).
pub fn classic_gauge(done: u64, total: Option<u64>, width: usize) -> String {
    // "[] 100%" is 7 columns; keep at least one fill column.
    let inner = width.saturating_sub(7).max(1);
    match total {
        Some(t) if t > 0 => {
            let filled = ((done.min(t) as u128 * inner as u128) / t as u128) as usize;
            let pct = ((done.min(t) as u128 * 100) / t as u128) as u32;
            let mut s = String::with_capacity(inner + 7);
            s.push('[');
            s.extend(std::iter::repeat_n('*', filled));
            s.extend(std::iter::repeat_n(' ', inner - filled));
            s.push(']');
            s.push_str(&format!(" {pct:3}%"));
            s
        }
        _ => {
            let mut s = String::with_capacity(inner + 7);
            s.push('[');
            s.extend(std::iter::repeat_n(' ', inner));
            s.push_str("]     ");
            s
        }
    }
}

/// Convenience used by tests and by [`FileOpProgressState::prepare`].
pub fn maybe_scan_totals(
    vfs: &dyn Vfs,
    path: &Path,
    compute_totals: bool,
) -> Result<Option<OpTotals>> {
    if compute_totals {
        Ok(Some(scan_totals(vfs, path)?))
    } else {
        Ok(None)
    }
}

/// GNU mc's original text-mode gauge inner width (before the dialog grew with the screen).
pub fn default_bar_width() -> usize {
    47
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::ConfigOptions;
    use rmc_fs::local::LocalFs;
    use std::fs;
    use std::io::Write;

    fn opts(verbose: bool, compute_totals: bool, classic_progressbar: bool) -> ConfigOptions {
        ConfigOptions {
            verbose,
            compute_totals,
            classic_progressbar,
            ..ConfigOptions::default()
        }
    }

    fn write_file(path: &Path, len: usize) {
        let mut f = fs::File::create(path).unwrap();
        f.write_all(&vec![0xABu8; len]).unwrap();
    }

    struct SampleProgress {
        file_done: u64,
        file_total: u64,
        bytes_done: u64,
        bytes_total: Option<u64>,
    }

    fn copy_state(name: &str, flags: ConfigOptions, p: SampleProgress) -> FileOpProgressState {
        FileOpProgressState {
            op: CopyMoveOp::Copy,
            source_name: name.to_string(),
            source_path: format!("/src/{name}"),
            target_path: format!("/dst/{name}"),
            file_done: p.file_done,
            file_total: p.file_total,
            bytes_done: p.bytes_done,
            bytes_total: p.bytes_total,
            files_done: 0,
            files_total: 1,
            verbose: flags.verbose,
            compute_totals: flags.compute_totals,
            classic_progressbar: flags.classic_progressbar,
        }
    }

    #[test]
    fn copy_progress_defaults_match_gnu_mc() {
        let d = ConfigOptions::default();
        assert!(d.verbose, "GNU mc Verbose operation defaults to true");
        assert!(d.compute_totals, "GNU mc Compute totals defaults to true");
        assert!(
            d.classic_progressbar,
            "GNU mc Classic progressbar defaults to true"
        );
    }

    #[test]
    fn verbose_on_shows_name_off_hides_it() {
        let on = copy_state(
            "readme.txt",
            opts(true, true, true),
            SampleProgress {
                file_done: 0,
                file_total: 100,
                bytes_done: 0,
                bytes_total: Some(100),
            },
        );
        let off = copy_state(
            "readme.txt",
            opts(false, true, true),
            SampleProgress {
                file_done: 0,
                file_total: 100,
                bytes_done: 0,
                bytes_total: Some(100),
            },
        );
        let von = on.view(default_bar_width(), false);
        let voff = off.view(default_bar_width(), false);
        assert_eq!(von.source_name.as_deref(), Some("readme.txt"));
        assert_eq!(voff.source_name, None);
        // Totals/bar remain when verbose is off.
        assert!(voff.classic_bar.is_some());
        assert!(voff.total_bytes.is_some());
        assert!(voff.files_processed.contains("Files processed:"));
    }

    #[test]
    fn classic_progressbar_toggles_bar_style() {
        let classic = copy_state(
            "a.bin",
            opts(true, true, true),
            SampleProgress {
                file_done: 50,
                file_total: 100,
                bytes_done: 50,
                bytes_total: Some(100),
            },
        );
        let v = classic.view(default_bar_width(), false);
        let bar = v.classic_bar.expect("classic one-line bar");
        assert!(
            bar.contains('*'),
            "classic bar should use GNU mc **** fill: {bar}"
        );
        assert!(bar.starts_with('['), "{bar}");
        assert!(bar.contains('%'), "{bar}");
        assert!(v.file_bar.is_none(), "classic style is one bar, not File");
        assert!(v.total_bar.is_none(), "classic style is one bar, not Total");

        let two = copy_state(
            "a.bin",
            opts(true, true, false),
            SampleProgress {
                file_done: 50,
                file_total: 100,
                bytes_done: 50,
                bytes_total: Some(100),
            },
        );
        let v2 = two.view(default_bar_width(), false);
        assert!(v2.classic_bar.is_none());
        let file = v2.file_bar.expect("two-bar File gauge");
        let total = v2.total_bar.expect("two-bar Total gauge");
        assert!(file.contains('*'), "{file}");
        assert!(total.contains('*'), "{total}");
        assert!(file.starts_with('[') && total.starts_with('['));
    }

    #[test]
    fn two_bar_without_totals_is_file_only() {
        let s = copy_state(
            "a.bin",
            opts(true, false, false),
            SampleProgress {
                file_done: 10,
                file_total: 40,
                bytes_done: 10,
                bytes_total: None,
            },
        );
        let v = s.view(default_bar_width(), false);
        assert!(v.file_bar.is_some());
        assert!(
            v.total_bar.is_none(),
            "no overall bar when Compute totals is off"
        );
        assert!(v.total_bytes.is_none());
        assert!(v.classic_bar.is_none());
    }

    #[test]
    fn classic_gauge_asterisks_and_percentage() {
        let empty = classic_gauge(0, Some(100), default_bar_width());
        assert!(empty.starts_with('['), "{empty}");
        assert!(empty.ends_with("  0%"), "{empty}");
        assert!(!empty.contains('*'), "0% should have no fill: {empty}");

        let half = classic_gauge(50, Some(100), default_bar_width());
        let stars = half.chars().filter(|&c| c == '*').count();
        assert!(stars > 0, "{half}");
        assert!(half.ends_with(" 50%"), "{half}");

        let full = classic_gauge(100, Some(100), default_bar_width());
        assert!(full.ends_with("100%"), "{full}");
        assert!(!full.contains('='), "classic fill is *, not equals");

        let unknown = classic_gauge(0, None, default_bar_width());
        assert!(
            !unknown.contains('%'),
            "indeterminate has no percent: {unknown}"
        );
        assert!(!unknown.contains('*'), "{unknown}");
    }

    #[test]
    fn compute_totals_on_prescans_size() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("tree");
        fs::create_dir(&root).unwrap();
        write_file(&root.join("a.bin"), 1000);
        let sub = root.join("sub");
        fs::create_dir(&sub).unwrap();
        write_file(&sub.join("b.bin"), 1500);
        write_file(&sub.join("c.bin"), 500);

        let vfs = LocalFs::new();
        let on = maybe_scan_totals(&vfs, &root, true).unwrap();
        let totals = on.expect("pre-scan when compute_totals is on");
        assert_eq!(totals.bytes, 3000);
        assert_eq!(totals.files, 3);

        let off = maybe_scan_totals(&vfs, &root, false).unwrap();
        assert_eq!(off, None, "skip pre-scan when compute_totals is off");
    }

    #[test]
    fn prepare_honors_compute_totals_flag() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("file.dat");
        write_file(&src, 2500);
        let dst = tmp.path().join("out.dat");
        let vfs = LocalFs::new();

        let with = FileOpProgressState::prepare(
            &vfs,
            CopyMoveOp::Copy,
            &src,
            &dst,
            &opts(true, true, true),
        )
        .unwrap();
        assert_eq!(with.bytes_total, Some(2500));
        assert_eq!(with.files_total, 1);
        assert_eq!(with.source_name, "file.dat");
        assert!(with.verbose);
        assert!(with.classic_progressbar);

        let without = FileOpProgressState::prepare(
            &vfs,
            CopyMoveOp::Move,
            &src,
            &dst,
            &opts(false, false, false),
        )
        .unwrap();
        assert_eq!(without.bytes_total, None);
        assert!(!without.verbose);
        assert!(!without.classic_progressbar);
        let v = without.view(default_bar_width(), false);
        assert_eq!(v.source_name, None);
        assert_eq!(v.title, "Move");
        assert!(v.classic_bar.is_none());
        assert!(v.file_bar.is_some());
        assert!(v.total_bar.is_none());
    }

    #[test]
    fn scan_totals_single_file() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("one.bin");
        write_file(&src, 42);
        let vfs = LocalFs::new();
        let t = scan_totals(&vfs, &src).unwrap();
        assert_eq!(
            t,
            OpTotals {
                bytes: 42,
                files: 1
            }
        );
    }
}
