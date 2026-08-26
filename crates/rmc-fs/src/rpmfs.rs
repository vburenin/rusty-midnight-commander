use crate::{DirEntry, FsError, FsResult, Metadata};
use flate2::read::GzDecoder;
use std::fs::File;
use std::io::{Cursor, Read};
use std::path::Path;
use std::time::UNIX_EPOCH;
use xz2::read::XzDecoder;

enum PayloadKind {
    Plain,
    Gzip,
    Xz,
    Zstd,
}

fn find_payload(buf: &[u8]) -> Option<(usize, PayloadKind)> {
    let mut best: Option<(usize, PayloadKind)> = None;
    // cpio newc magic
    let needle_newc = b"070701";
    for (i, w) in buf.windows(needle_newc.len()).enumerate() {
        if w == needle_newc {
            best = Some((i, PayloadKind::Plain));
            break;
        }
    }
    // gzip
    for (i, w) in buf.windows(2).enumerate() {
        if w == [0x1f, 0x8b] {
            match best {
                Some((bix, _)) if bix <= i => {}
                _ => best = Some((i, PayloadKind::Gzip)),
            }
            break;
        }
    }
    // xz
    let xz_magic = [0xfd, b'7', b'z', b'X', b'Z', 0x00];
    for (i, w) in buf.windows(xz_magic.len()).enumerate() {
        if w == xz_magic {
            match best {
                Some((bix, _)) if bix <= i => {}
                _ => best = Some((i, PayloadKind::Xz)),
            }
            break;
        }
    }
    // zstd
    let zstd_magic = [0x28, 0xb5, 0x2f, 0xfd];
    for (i, w) in buf.windows(zstd_magic.len()).enumerate() {
        if w == zstd_magic {
            match best {
                Some((bix, _)) if bix <= i => {}
                _ => best = Some((i, PayloadKind::Zstd)),
            }
            break;
        }
    }
    best
}

fn read_payload_bytes(archive_path: &Path) -> FsResult<Vec<u8>> {
    let mut data = Vec::new();
    File::open(archive_path)
        .and_then(|mut f| std::io::copy(&mut f, &mut data).map(|_| ()))
        .map_err(|e| FsError::Message(format!("rpm read: {e}")))?;
    let (off, kind) = find_payload(&data)
        .ok_or_else(|| FsError::Message("rpm: failed to locate cpio payload".into()))?;
    let slice = data.split_off(off);
    let mut out = Vec::new();
    match kind {
        PayloadKind::Plain => {
            out = slice;
        }
        PayloadKind::Gzip => {
            let mut dec = GzDecoder::new(Cursor::new(slice));
            dec.read_to_end(&mut out)
                .map_err(|e| FsError::Message(format!("rpm gzip: {e}")))?;
        }
        PayloadKind::Xz => {
            let mut dec = XzDecoder::new(Cursor::new(slice));
            dec.read_to_end(&mut out)
                .map_err(|e| FsError::Message(format!("rpm xz: {e}")))?;
        }
        PayloadKind::Zstd => {
            let mut dec = zstd::stream::read::Decoder::new(Cursor::new(slice))
                .map_err(|e| FsError::Message(format!("rpm zstd init: {e}")))?;
            dec.read_to_end(&mut out)
                .map_err(|e| FsError::Message(format!("rpm zstd: {e}")))?;
        }
    }
    Ok(out)
}

pub fn list_dir(
    archive_path: &Path,
    vfs_root: &Path,
    inner: &Path,
    show_hidden: bool,
) -> FsResult<Vec<DirEntry>> {
    let data = read_payload_bytes(archive_path)?;
    crate::cpiofs::list_dir_from_bytes(&data, inner, vfs_root, show_hidden)
}

pub fn read_file(archive_path: &Path, inner_full: &Path) -> FsResult<Box<dyn Read + Send>> {
    let data = read_payload_bytes(archive_path)?;
    crate::cpiofs::read_file_from_bytes(&data, inner_full)
}

pub fn stat(archive_path: &Path, inner_full: &Path) -> FsResult<Metadata> {
    if inner_full.as_os_str().is_empty() {
        return Ok(Metadata {
            is_dir: true,
            is_symlink: false,
            is_executable: false,
            size: 0,
            modified: UNIX_EPOCH,
            permissions: 0o755,
            owner: None,
            group: None,
        });
    }
    let data = read_payload_bytes(archive_path)?;
    crate::cpiofs::stat_from_bytes(&data, inner_full)
}

pub fn copy_out(archive_path: &Path, src_inner: &Path, dst: &Path) -> FsResult<()> {
    let data = read_payload_bytes(archive_path)?;
    crate::cpiofs::copy_out_from_bytes(&data, src_inner, dst)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn find_payload_finds_gzip() {
        let mut v = Vec::new();
        v.extend_from_slice(b"rpmheader...");
        v.extend_from_slice(&[0x1f, 0x8b, 0x08, 0x00, 0x00]); // gzip magic
        assert!(matches!(
            find_payload(&v),
            Some((i, PayloadKind::Gzip)) if i == 12
        ));
    }
    #[test]
    fn find_payload_finds_xz() {
        let mut v = vec![0u8; 100];
        v.extend_from_slice(&[0xfd, b'7', b'z', b'X', b'Z', 0x00]);
        let res = find_payload(&v);
        assert!(matches!(res, Some((_i, PayloadKind::Xz))));
    }
}
