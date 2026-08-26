#[cfg(test)]
mod tests {
    use crate::composite::CompositeFs;
    use crate::Vfs;
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::fs::File;
    use std::io::{Seek, SeekFrom, Write};
    use std::path::PathBuf;
    use tar::Builder as TarBuilder;
    use tempfile::tempdir;

    fn write_sysv_ar(path: &std::path::Path, files: Vec<(&str, Vec<u8>)>) {
        let mut f = File::create(path).unwrap();
        f.write_all(b"!<arch>\n").unwrap();
        for (name, data) in files {
            assert!(name.len() < 16, "name too long for minimal ar writer");
            let mut header = [b' '; 60];
            // name (16)
            let name_bytes = name.as_bytes();
            header[..name_bytes.len()].copy_from_slice(name_bytes);
            // timestamp (12)
            let ts = b"0";
            header[16..16 + ts.len()].copy_from_slice(ts);
            // owner (6)
            header[28..29].copy_from_slice(b"0");
            // group (6)
            header[34..35].copy_from_slice(b"0");
            // mode (8) - octal string
            let mode = b"100644";
            header[40..40 + mode.len()].copy_from_slice(mode);
            // size (10)
            let size_str = data.len().to_string();
            let size_bytes = size_str.as_bytes();
            let start = 48;
            header[start..start + size_bytes.len()].copy_from_slice(size_bytes);
            // fmag (2) - "`\n"
            header[58] = b'`';
            header[59] = b'\n';
            f.write_all(&header).unwrap();
            f.write_all(&data).unwrap();
            if data.len() % 2 == 1 {
                f.write_all(b"\n").unwrap();
            }
        }
        f.flush().unwrap();
        f.seek(SeekFrom::Start(0)).unwrap();
    }

    #[test]
    fn ar_list_and_read() {
        let dir = tempdir().unwrap();
        let ar_path = dir.path().join("sample.ar");
        write_sysv_ar(
            &ar_path,
            vec![
                ("root.txt", b"hi root".to_vec()),
                ("dir/file.txt", b"hello".to_vec()),
            ],
        );
        let fs = CompositeFs::new();
        // Enter anchor
        let anchor = fs.enter_path(&ar_path).expect("enter ar");
        // Root listing should include root.txt and dir
        let list = fs.list_dir(&anchor, true).unwrap();
        assert!(list.iter().any(|e| e.name == "root.txt" && !e.meta.is_dir));
        assert!(list.iter().any(|e| e.name == "dir" && e.meta.is_dir));
        // Read file inside dir
        let mut r = fs
            .read_file(&anchor.join("dir").join("file.txt"))
            .expect("read file");
        let mut buf = String::new();
        use std::io::Read;
        r.read_to_string(&mut buf).unwrap();
        assert_eq!(buf, "hello");
    }

    #[test]
    fn deb_list_and_read_inside_data() {
        let dir = tempdir().unwrap();
        // Build control.tar.gz
        let mut control_tar = Vec::new();
        {
            let gz = GzEncoder::new(&mut control_tar, Compression::default());
            let mut tb = TarBuilder::new(gz);
            let mut header = tar::Header::new_gnu();
            header.set_path("./control").unwrap();
            header.set_size(7);
            header.set_mode(0o644);
            header.set_mtime(1);
            header.set_cksum();
            tb.append(&header, &b"Package"[..]).unwrap();
            tb.into_inner().unwrap();
        }
        // Build data.tar.gz with hello.txt
        let mut data_tar = Vec::new();
        {
            let gz = GzEncoder::new(&mut data_tar, Compression::default());
            let mut tb = TarBuilder::new(gz);
            let mut header = tar::Header::new_gnu();
            header.set_path("./hello.txt").unwrap();
            header.set_size(5);
            header.set_mode(0o644);
            header.set_mtime(1);
            header.set_cksum();
            tb.append(&header, &b"world"[..]).unwrap();
            tb.into_inner().unwrap();
        }
        // Build .deb (ar) with debian-binary, control.tar.gz, data.tar.gz
        let deb_path = dir.path().join("sample.deb");
        write_sysv_ar(
            &deb_path,
            vec![
                ("debian-binary", b"2.0\n".to_vec()),
                ("control.tar.gz", control_tar),
                ("data.tar.gz", data_tar),
            ],
        );
        let fs = CompositeFs::new();
        let anchor = fs.enter_path(&deb_path).expect("enter deb");
        let list = fs.list_dir(&anchor, true).unwrap();
        assert!(list.iter().any(|e| e.name == "debian-binary"));
        assert!(list.iter().any(|e| e.name == "data.tar.gz"));
        // Read inner file from data.tar.gz
        let mut r = fs
            .read_file(&anchor.join("data.tar.gz").join("hello.txt"))
            .expect("read inner");
        let mut s = String::new();
        use std::io::Read;
        r.read_to_string(&mut s).unwrap();
        assert_eq!(s, "world");
    }
}
