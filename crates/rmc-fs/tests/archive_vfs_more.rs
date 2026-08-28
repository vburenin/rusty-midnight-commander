use rmc_fs::composite::CompositeFs;
use rmc_fs::Vfs;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use tempfile::tempdir;

fn make_sample_tree(root: &Path) {
    fs::create_dir_all(root.join("dir1/sub")).expect("create dirs");
    fs::write(root.join("root.txt"), b"root").unwrap();
    fs::write(root.join("dir1/file1.txt"), b"hello").unwrap();
    fs::write(root.join("dir1/sub/inner.txt"), b"inner").unwrap();
}

fn anchor_path(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_string_lossy().to_string();
    s.push('#');
    PathBuf::from(s)
}

fn build_cpio(path: &Path, src_root: &Path) {
    use librarium::newc::NewcHeader;
    use librarium::{ArchiveWriter, Header};
    use std::io::Cursor;
    let file = File::create(path).unwrap();
    let mut writer = ArchiveWriter::<NewcHeader>::new(Box::new(file));
    // Add a few files
    let files = [
        ("root.txt", src_root.join("root.txt")),
        ("dir1/file1.txt", src_root.join("dir1/file1.txt")),
        ("dir1/sub/inner.txt", src_root.join("dir1/sub/inner.txt")),
    ];
    for (name, src) in files {
        let data = fs::read(&src).unwrap();
        let header = Header {
            name: name.to_string(),
            ..Header::default()
        };
        writer.push_file(Cursor::new(data), header).unwrap();
    }
    writer.write().unwrap();
}

fn build_cpio_gz(path: &Path, src_root: &Path) {
    let tmp = path.with_extension("cpio");
    build_cpio(&tmp, src_root);
    // gzip it
    let f = File::create(path).unwrap();
    let mut enc = flate2::write::GzEncoder::new(f, flate2::Compression::default());
    let bytes = fs::read(&tmp).unwrap();
    enc.write_all(&bytes).unwrap();
    enc.finish().unwrap();
}

fn build_7z(path: &Path, src_root: &Path) {
    let mut writer = sevenz_rust2::ArchiveWriter::create(path).expect("create 7z");
    writer
        .push_source_path(src_root, |_| true)
        .expect("add path");
    writer.finish().expect("finish 7z");
}

fn build_iso(path: &Path, src_root: &Path) {
    use isobemak::iso::boot_info::BootInfo;
    use isobemak::iso::builder::build_iso;
    use isobemak::iso::iso_image::{IsoImage, IsoImageFile};
    use isobemak::iso::layout_profile::IsoLayoutProfile;
    // Minimal ISO without boot entries: just include files
    let files = vec![
        IsoImageFile {
            source: src_root.join("root.txt"),
            destination: "root.txt".to_string(),
        },
        IsoImageFile {
            source: src_root.join("dir1/file1.txt"),
            destination: "dir1/file1.txt".to_string(),
        },
        IsoImageFile {
            source: src_root.join("dir1/sub/inner.txt"),
            destination: "dir1/sub/inner.txt".to_string(),
        },
    ];
    let image = IsoImage {
        volume_id: Some("RMC_TEST".to_string()),
        files,
        // No boot images
        boot_info: BootInfo {
            bios_boot: None,
            uefi_boot: None,
        },
        layout_profile: IsoLayoutProfile::default(),
    };
    let _ = build_iso(path, &image, false).expect("build iso");
}

fn build_rar(path: &Path, src_root: &Path) {
    let mut builder = rars::builder::Builder::new(rars::version::ArchiveVersion::Rar50);
    builder
        .add_path(&src_root.join("root.txt"), b"root.txt")
        .unwrap();
    builder
        .add_path(&src_root.join("dir1/file1.txt"), b"dir1/file1.txt")
        .unwrap();
    builder
        .add_path(&src_root.join("dir1/sub/inner.txt"), b"dir1/sub/inner.txt")
        .unwrap();
    builder.write_to_path(path, None).unwrap();
}

fn crc16_lha(data: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    for &b in data {
        crc ^= u16::from(b);
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xA001;
            } else {
                crc >>= 1;
            }
        }
    }
    crc
}

/// Minimal stored (`-lh0-`) LHA/LZH archive (level 0 headers). Public format.
fn build_lha(path: &Path, files: &[(&str, &[u8])]) {
    let mut out = Vec::new();
    for (name, data) in files {
        let name_b = name.as_bytes();
        assert!(name_b.len() <= 255, "lha level-0 name too long");
        let packed = u32::try_from(data.len()).expect("lha member too large");
        let crc = crc16_lha(data);
        let mut rest = Vec::new();
        rest.extend_from_slice(b"-lh0-");
        rest.extend_from_slice(&packed.to_le_bytes());
        rest.extend_from_slice(&packed.to_le_bytes());
        rest.extend_from_slice(&0u16.to_le_bytes());
        rest.extend_from_slice(&0u16.to_le_bytes());
        rest.push(0x20);
        rest.push(0);
        rest.push(u8::try_from(name_b.len()).expect("name len"));
        rest.extend_from_slice(name_b);
        rest.extend_from_slice(&crc.to_le_bytes());
        let header_size = u8::try_from(rest.len()).expect("lha header too large");
        let checksum = rest.iter().fold(0u8, |a, b| a.wrapping_add(*b));
        out.push(header_size);
        out.push(checksum);
        out.extend_from_slice(&rest);
        out.extend_from_slice(data);
    }
    out.push(0);
    fs::write(path, out).expect("write lha");
}

#[test]
fn cpio_and_gz_browse_and_copy_out() {
    let tmp = tempdir().unwrap();
    let src_root = tmp.path().join("src");
    fs::create_dir_all(&src_root).unwrap();
    make_sample_tree(&src_root);
    let cpio_path = tmp.path().join("sample.cpio");
    build_cpio(&cpio_path, &src_root);
    let cpio_gz_path = tmp.path().join("sample.cpio.gz");
    build_cpio_gz(&cpio_gz_path, &src_root);

    let vfs = CompositeFs::new();
    // Plain cpio
    let root = anchor_path(&cpio_path);
    let names: Vec<_> = vfs
        .list_dir(&root, true)
        .unwrap()
        .iter()
        .map(|e| e.name.clone())
        .collect();
    assert!(names.contains(&"dir1".to_string()));
    assert!(names.contains(&"root.txt".to_string()));
    let mut r = vfs.read_file(&root.join("root.txt")).unwrap();
    let mut s = String::new();
    use std::io::Read;
    r.read_to_string(&mut s).unwrap();
    assert_eq!(s, "root");
    // GZ variant
    let root_gz = anchor_path(&cpio_gz_path);
    let names_gz: Vec<_> = vfs
        .list_dir(&root_gz, false)
        .unwrap()
        .iter()
        .map(|e| e.name.clone())
        .collect();
    assert!(names_gz.contains(&"dir1".to_string()));
}

#[test]
fn sevenz_iso_rar_browse() {
    let tmp = tempdir().unwrap();
    let src_root = tmp.path().join("src");
    fs::create_dir_all(&src_root).unwrap();
    make_sample_tree(&src_root);
    let p7z = tmp.path().join("sample.7z");
    build_7z(&p7z, &src_root);
    let iso = tmp.path().join("sample.iso");
    build_iso(&iso, &src_root);
    let rar = tmp.path().join("sample.rar");
    build_rar(&rar, &src_root);

    let vfs = CompositeFs::new();
    // 7z
    let root7 = anchor_path(&p7z);
    let names7: Vec<_> = vfs
        .list_dir(&root7, true)
        .unwrap()
        .iter()
        .map(|e| e.name.clone())
        .collect();
    assert!(names7.contains(&"dir1".to_string()));
    assert!(names7.contains(&"root.txt".to_string()));
    // iso
    let rooti = anchor_path(&iso);
    // Read files from ISO
    let mut r0 = vfs.read_file(&rooti.join("root.txt")).unwrap();
    let mut s0 = String::new();
    use std::io::Read;
    r0.read_to_string(&mut s0).unwrap();
    assert_eq!(s0, "root");
    // Read a nested file from ISO
    let mut r = vfs.read_file(&rooti.join("dir1/file1.txt")).unwrap();
    let mut s = String::new();
    r.read_to_string(&mut s).unwrap();
    assert_eq!(s, "hello");
    // rar
    let rootr = anchor_path(&rar);
    let namesr: Vec<_> = vfs
        .list_dir(&rootr, true)
        .unwrap()
        .iter()
        .map(|e| e.name.clone())
        .collect();
    assert!(namesr.contains(&"dir1".to_string()));
    assert!(namesr.contains(&"root.txt".to_string()));
}

#[test]
fn lha_and_lzh_browse_and_copy_out() {
    use std::io::Read;
    let tmp = tempdir().unwrap();
    let lha_path = tmp.path().join("sample.lha");
    let lzh_path = tmp.path().join("sample.lzh");
    let files: &[(&str, &[u8])] = &[
        ("root.txt", b"root"),
        ("dir1/file1.txt", b"hello"),
        ("dir1/sub/inner.txt", b"inner"),
    ];
    build_lha(&lha_path, files);
    build_lha(&lzh_path, files);

    let vfs = CompositeFs::new();
    let root = vfs.enter_path(&lha_path).expect("enter lha");
    let names: Vec<_> = vfs
        .list_dir(&root, true)
        .unwrap()
        .iter()
        .map(|e| e.name.clone())
        .collect();
    assert!(names.contains(&"dir1".to_string()));
    assert!(names.contains(&"root.txt".to_string()));

    let mut r = vfs.read_file(&root.join("root.txt")).unwrap();
    let mut s = String::new();
    r.read_to_string(&mut s).unwrap();
    assert_eq!(s, "root");

    let nested = root.join("dir1").join("file1.txt");
    let mut r2 = vfs.read_file(&nested).unwrap();
    let mut s2 = String::new();
    r2.read_to_string(&mut s2).unwrap();
    assert_eq!(s2, "hello");

    let dst = tmp.path().join("out-root.txt");
    vfs.copy(&root.join("root.txt"), &dst).unwrap();
    assert_eq!(fs::read(&dst).unwrap(), b"root");

    let dst_dir = tmp.path().join("out-dir");
    vfs.copy(&root.join("dir1"), &dst_dir).unwrap();
    assert_eq!(fs::read(dst_dir.join("file1.txt")).unwrap(), b"hello");
    assert_eq!(
        fs::read(dst_dir.join("sub").join("inner.txt")).unwrap(),
        b"inner"
    );

    let root_lzh = vfs.enter_path(&lzh_path).expect("enter lzh");
    let names_lzh: Vec<_> = vfs
        .list_dir(&root_lzh, true)
        .unwrap()
        .iter()
        .map(|e| e.name.clone())
        .collect();
    assert!(names_lzh.contains(&"root.txt".to_string()));
    let mut r3 = vfs.read_file(&root_lzh.join("root.txt")).unwrap();
    let mut s3 = String::new();
    r3.read_to_string(&mut s3).unwrap();
    assert_eq!(s3, "root");
}
