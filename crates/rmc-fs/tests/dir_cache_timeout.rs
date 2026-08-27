//! Directory cache timeout: remote/archive listings honor
//! `VfsOptions.dir_cache_timeout_secs` without live networking.
use rmc_fs::composite::CompositeFs;
use rmc_fs::Vfs;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use tempfile::tempdir;

fn write_zip(path: &Path, files: &[(&str, &[u8])]) {
    let f = File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(f);
    let options = zip::write::FileOptions::default();
    for (name, data) in files {
        zip.start_file(*name, options).unwrap();
        zip.write_all(data).unwrap();
    }
    zip.finish().unwrap();
}

fn anchor(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_string_lossy().to_string();
    s.push('#');
    PathBuf::from(s)
}

fn listing_names(vfs: &CompositeFs, root: &Path) -> Vec<String> {
    let mut names: Vec<String> = vfs
        .list_dir(root, true)
        .unwrap()
        .into_iter()
        .filter(|e| e.name != "..")
        .map(|e| e.name)
        .collect();
    names.sort();
    names
}

#[test]
fn timeout_zero_never_hits_archive_cache() {
    let tmp = tempdir().unwrap();
    let zip_path = tmp.path().join("sample.zip");
    write_zip(&zip_path, &[("a.txt", b"a")]);
    let vfs = CompositeFs::new();
    vfs.set_dir_cache_timeout_secs(0);
    let root = anchor(&zip_path);
    assert_eq!(listing_names(&vfs, &root), ["a.txt"]);

    write_zip(&zip_path, &[("a.txt", b"a"), ("b.txt", b"b")]);
    assert_eq!(
        listing_names(&vfs, &root),
        ["a.txt", "b.txt"],
        "timeout 0 must re-read the archive"
    );
}

#[test]
fn timeout_n_reuses_archive_listing_until_refresh() {
    let tmp = tempdir().unwrap();
    let zip_path = tmp.path().join("sample.zip");
    write_zip(&zip_path, &[("a.txt", b"a")]);
    let vfs = CompositeFs::new();
    vfs.set_dir_cache_timeout_secs(900);
    let root = anchor(&zip_path);
    assert_eq!(listing_names(&vfs, &root), ["a.txt"]);

    write_zip(&zip_path, &[("a.txt", b"a"), ("b.txt", b"b")]);
    assert_eq!(
        listing_names(&vfs, &root),
        ["a.txt"],
        "TTL still valid: must reuse the cached listing"
    );

    vfs.invalidate_dir_cache(Some(&root));
    assert_eq!(
        listing_names(&vfs, &root),
        ["a.txt", "b.txt"],
        "Refresh must bypass a still-fresh cache"
    );
}

#[test]
fn local_disk_listings_are_not_cached() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    std::fs::write(root.join("a.txt"), b"a").unwrap();
    let vfs = CompositeFs::new();
    vfs.set_dir_cache_timeout_secs(900);
    assert!(listing_names(&vfs, root).contains(&"a.txt".to_string()));
    std::fs::write(root.join("b.txt"), b"b").unwrap();
    let names = listing_names(&vfs, root);
    assert!(
        names.contains(&"b.txt".to_string()),
        "local disk must stay uncached"
    );
}
