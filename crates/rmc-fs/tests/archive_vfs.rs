use rmc_fs::composite::CompositeFs;
use rmc_fs::Vfs;
use std::fs::{self, File};
use std::io::{Write};
use std::path::{Path, PathBuf};
use tempfile::tempdir;

fn make_sample_tree(root: &Path) {
    fs::create_dir_all(root.join("dir1/sub")).
        expect("create dirs");
    fs::write(root.join("root.txt"), b"root").unwrap();
    fs::write(root.join("dir1/file1.txt"), b"hello").unwrap();
    fs::write(root.join("dir1/sub/inner.txt"), b"inner").unwrap();
}

fn build_tar(path: &Path, src_root: &Path) {
    let f = File::create(path).unwrap();
    let mut builder = tar::Builder::new(f);
    builder.append_dir_all(".", src_root).unwrap();
    builder.finish().unwrap();
}

fn build_targz(path: &Path, src_root: &Path) {
    let f = File::create(path).unwrap();
    let enc = flate2::write::GzEncoder::new(f, flate2::Compression::default());
    let mut builder = tar::Builder::new(enc);
    builder.append_dir_all(".", src_root).unwrap();
    builder.finish().unwrap();
}

fn build_zip(path: &Path, src_root: &Path) {
    let f = File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(f);
    let options = zip::write::FileOptions::default();
    // root.txt
    zip.start_file("root.txt", options).unwrap();
    zip.write_all(b"root").unwrap();
    // dir1/file1.txt
    zip.add_directory("dir1/", options).unwrap();
    zip.start_file("dir1/file1.txt", options).unwrap();
    zip.write_all(b"hello").unwrap();
    // dir1/sub/inner.txt
    zip.add_directory("dir1/sub/", options).unwrap();
    zip.start_file("dir1/sub/inner.txt", options).unwrap();
    zip.write_all(b"inner").unwrap();
    zip.finish().unwrap();
}

fn anchor_path(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_string_lossy().to_string();
    s.push('#');
    PathBuf::from(s)
}

#[test]
fn tar_browse_and_copy_out() {
    let tmp = tempdir().unwrap();
    let src_root = tmp.path().join("src");
    fs::create_dir_all(&src_root).unwrap();
    make_sample_tree(&src_root);
    let tar_path = tmp.path().join("sample.tar");
    build_tar(&tar_path, &src_root);

    let vfs = CompositeFs::new();
    // Enter path should be suggested for tar file
    let enter = vfs.enter_path(&tar_path).expect("enterable");
    assert_eq!(enter, anchor_path(&tar_path));

    // List archive root
    let root = anchor_path(&tar_path);
    let list = vfs.list_dir(&root, true).unwrap();
    let names: Vec<_> = list.iter().map(|e| e.name.clone()).collect();
    assert!(names.contains(&"dir1".to_string()));
    assert!(names.contains(&"root.txt".to_string()));

    // Read file from inside
    let inner_file = root.join("root.txt");
    let mut r = vfs.read_file(&inner_file).unwrap();
    let mut buf = String::new();
    use std::io::Read;
    r.read_to_string(&mut buf).unwrap();
    assert_eq!(buf, "root");

    // Copy-out file
    let out = tmp.path().join("out.txt");
    vfs.copy(&inner_file, &out).unwrap();
    assert_eq!(fs::read_to_string(&out).unwrap(), "root");
}

#[test]
fn targz_browse_and_copy_out() {
    let tmp = tempdir().unwrap();
    let src_root = tmp.path().join("src");
    fs::create_dir_all(&src_root).unwrap();
    make_sample_tree(&src_root);
    let tar_path = tmp.path().join("sample.tar.gz");
    build_targz(&tar_path, &src_root);

    let vfs = CompositeFs::new();
    let root = anchor_path(&tar_path);
    let list = vfs.list_dir(&root, true).unwrap();
    let names: Vec<_> = list.iter().map(|e| e.name.clone()).collect();
    assert!(names.contains(&"dir1".to_string()));
    assert!(names.contains(&"root.txt".to_string()));

    let inner_file = root.join("dir1").join("file1.txt");
    let mut r = vfs.read_file(&inner_file).unwrap();
    let mut buf = String::new();
    use std::io::Read;
    r.read_to_string(&mut buf).unwrap();
    assert_eq!(buf, "hello");
}

#[test]
fn zip_browse_and_copy_out() {
    let tmp = tempdir().unwrap();
    let src_root = tmp.path().join("src");
    fs::create_dir_all(&src_root).unwrap();
    make_sample_tree(&src_root);
    let zip_path = tmp.path().join("sample.zip");
    build_zip(&zip_path, &src_root);

    let vfs = CompositeFs::new();
    // Enter path should be suggested for zip file
    let enter = vfs.enter_path(&zip_path).expect("enterable");
    assert_eq!(enter, anchor_path(&zip_path));

    let root = anchor_path(&zip_path);
    let list = vfs.list_dir(&root, false).unwrap();
    let names: Vec<_> = list.iter().map(|e| e.name.clone()).collect();
    assert!(names.contains(&"dir1".to_string()));
    assert!(names.contains(&"root.txt".to_string()));

    // Copy-out nested file
    let inner_file = root.join("dir1/sub/inner.txt");
    let out = tmp.path().join("inner-out.txt");
    vfs.copy(&inner_file, &out).unwrap();
    assert_eq!(fs::read_to_string(&out).unwrap(), "inner");
}

