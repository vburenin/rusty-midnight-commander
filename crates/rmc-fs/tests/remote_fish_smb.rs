use rmc_fs::remote::{
    copy_out_with_client, list_dir_with_client, parse_remote_url, parse_remote_url_str,
    RemoteClient, RemoteEntry, RemoteScheme, RemoteUrl,
};
use std::path::Path;
use tempfile::tempdir;

#[test]
fn parse_fish_and_smb_urls_and_anchors() {
    // Scheme URLs
    let u = parse_remote_url_str("fish://user@host:2222/dir").unwrap();
    assert!(matches!(u.scheme, RemoteScheme::Fish));
    assert_eq!(u.user.as_deref(), Some("user"));
    assert_eq!(u.host, "host");
    assert_eq!(u.port, Some(2222));
    assert_eq!(u.path, "/dir");

    let u2 = parse_remote_url_str("smb://server/share/path").unwrap();
    assert!(matches!(u2.scheme, RemoteScheme::Smb));
    assert_eq!(u2.host, "server");
    assert_eq!(u2.path, "/share/path");
    assert!(u2.user.is_none());

    // Anchor URLs
    let ua = parse_remote_url(Path::new("/#fish:user@h:2022/a/b")).unwrap();
    assert!(matches!(ua.scheme, RemoteScheme::Fish));
    assert_eq!(ua.user.as_deref(), Some("user"));
    assert_eq!(ua.host, "h");
    assert_eq!(ua.port, Some(2022));
    assert_eq!(ua.path, "/a/b");

    let ub = parse_remote_url(Path::new("/some/local/#smb:filesrv/share/docs")).unwrap();
    assert!(matches!(ub.scheme, RemoteScheme::Smb));
    assert_eq!(ub.host, "filesrv");
    assert_eq!(ub.path, "/share/docs");
}

struct MockClient {
    entries: Vec<RemoteEntry>,
    blob: Vec<u8>,
}
impl RemoteClient for MockClient {
    fn list(&mut self, _path: &str) -> rmc_fs::FsResult<Vec<RemoteEntry>> {
        Ok(self.entries.clone())
    }
    fn download(
        &mut self,
        _remote_path: &str,
        local_path: &std::path::Path,
    ) -> rmc_fs::FsResult<()> {
        std::fs::write(local_path, &self.blob)?;
        Ok(())
    }
    fn upload(
        &mut self,
        _local_path: &std::path::Path,
        _remote_path: &str,
    ) -> rmc_fs::FsResult<()> {
        Ok(())
    }
    fn remove_file(&mut self, _remote_path: &str) -> rmc_fs::FsResult<()> {
        Ok(())
    }
    fn remove_dir(&mut self, _remote_path: &str) -> rmc_fs::FsResult<()> {
        Ok(())
    }
    fn mkdir(&mut self, _remote_path: &str) -> rmc_fs::FsResult<()> {
        Ok(())
    }
}

#[test]
fn list_dir_with_mock_client_builds_fish_paths() {
    let url = RemoteUrl {
        scheme: RemoteScheme::Fish,
        user: Some("u".to_string()),
        pass: None,
        host: "host".to_string(),
        port: Some(2222),
        path: "/".to_string(),
        compression: false,
        use_rsh: false,
    };
    let mut client = MockClient {
        entries: vec![
            RemoteEntry {
                name: "dir".to_string(),
                is_dir: true,
                size: 0,
            },
            RemoteEntry {
                name: "file.txt".to_string(),
                is_dir: false,
                size: 10,
            },
        ],
        blob: Vec::new(),
    };
    let list = list_dir_with_client(&url, &mut client, false).unwrap();
    assert!(list
        .iter()
        .any(|e| e.path.to_string_lossy() == "fish://u@host:2222/dir"));
}

#[test]
fn copy_out_with_mock_client_smb() {
    let url = RemoteUrl {
        scheme: RemoteScheme::Smb,
        user: Some("u".to_string()),
        pass: Some("p".to_string()),
        host: "filesrv".to_string(),
        port: Some(445),
        path: "/share/readme.txt".to_string(),
        compression: false,
        use_rsh: false,
    };
    let mut client = MockClient {
        entries: vec![],
        blob: b"data".to_vec(),
    };
    let dir = tempdir().unwrap();
    let dst = dir.path().join("out.txt");
    copy_out_with_client(&url, &mut client, &dst).unwrap();
    let data = std::fs::read(&dst).unwrap();
    assert_eq!(data, b"data");
}
