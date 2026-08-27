use rmc_fs::remote::{
    copy_out_with_client, ftp_connect_target, list_dir_with_client, parse_remote_url_str,
    RemoteClient, RemoteEntry, RemoteScheme, RemoteUrl,
};
use tempfile::tempdir;

#[test]
fn parse_ftp_and_sftp_urls() {
    let u = parse_remote_url_str("ftp://example.com/").unwrap();
    assert!(matches!(u.scheme, RemoteScheme::Ftp));
    assert_eq!(u.host, "example.com");
    assert_eq!(u.path, "/");
    assert!(u.user.is_none());
    let u2 = parse_remote_url_str("sftp://user@host:2222/path/to").unwrap();
    assert!(matches!(u2.scheme, RemoteScheme::Sftp));
    assert_eq!(u2.user.as_deref(), Some("user"));
    assert_eq!(u2.host, "host");
    assert_eq!(u2.port, Some(2222));
    assert_eq!(u2.path, "/path/to");
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
fn list_dir_with_mock_client_filters_and_paths() {
    let url = RemoteUrl {
        scheme: RemoteScheme::Ftp,
        user: None,
        pass: None,
        host: "example.com".to_string(),
        port: None,
        path: "/".to_string(),
    };
    let mut client = MockClient {
        entries: vec![
            RemoteEntry {
                name: "dir".to_string(),
                is_dir: true,
                size: 0,
            },
            RemoteEntry {
                name: ".hidden".to_string(),
                is_dir: false,
                size: 1,
            },
            RemoteEntry {
                name: "file.txt".to_string(),
                is_dir: false,
                size: 123,
            },
        ],
        blob: Vec::new(),
    };
    let list = list_dir_with_client(&url, &mut client, false).unwrap();
    // Expect parent marker + dir + file.txt (no .hidden)
    assert!(list.iter().any(|e| e.name == ".."));
    assert!(list.iter().any(|e| e.name == "dir" && e.meta.is_dir));
    assert!(list.iter().any(|e| e.name == "file.txt" && !e.meta.is_dir));
    assert!(!list.iter().any(|e| e.name == ".hidden"));
    // Paths should be ftp://example.com/<name>
    assert!(list
        .iter()
        .any(|e| e.path.to_string_lossy().as_ref() == "ftp://example.com/dir"));
}

#[test]
fn copy_out_with_mock_client_writes_file() {
    let url = RemoteUrl {
        scheme: RemoteScheme::Sftp,
        user: Some("u".to_string()),
        pass: None,
        host: "h".to_string(),
        port: Some(22),
        path: "/file.txt".to_string(),
    };
    let mut client = MockClient {
        entries: vec![],
        blob: b"hello".to_vec(),
    };
    let dir = tempdir().unwrap();
    let dst = dir.path().join("out.txt");
    copy_out_with_client(&url, &mut client, &dst).unwrap();
    let data = std::fs::read(&dst).unwrap();
    assert_eq!(data, b"hello");
}

fn ftp_url(host: &str, port: Option<u16>, user: Option<&str>) -> RemoteUrl {
    RemoteUrl {
        scheme: RemoteScheme::Ftp,
        user: user.map(str::to_string),
        pass: None,
        host: host.to_string(),
        port,
        path: "/".to_string(),
    }
}

#[test]
fn ftp_connect_target_direct_when_proxy_empty() {
    let url = ftp_url("ftp.example.com", None, Some("alice"));
    assert_eq!(
        ftp_connect_target(&url, None),
        ("ftp.example.com:21".into(), "alice".into())
    );
    assert_eq!(
        ftp_connect_target(&url, Some("")),
        ("ftp.example.com:21".into(), "alice".into())
    );
    assert_eq!(
        ftp_connect_target(&url, Some("   ")),
        ("ftp.example.com:21".into(), "alice".into())
    );
    let url_port = ftp_url("ftp.example.com", Some(2121), Some("alice"));
    assert_eq!(
        ftp_connect_target(&url_port, None),
        ("ftp.example.com:2121".into(), "alice".into())
    );
}

#[test]
fn ftp_connect_target_user_at_host_gateway() {
    let url = ftp_url("ftp.example.com", None, Some("alice"));
    assert_eq!(
        ftp_connect_target(&url, Some("proxy.example.net")),
        (
            "proxy.example.net:21".into(),
            "alice@ftp.example.com".into()
        )
    );
    assert_eq!(
        ftp_connect_target(&url, Some("proxy.example.net:3128")),
        (
            "proxy.example.net:3128".into(),
            "alice@ftp.example.com".into()
        )
    );
    let url_port = ftp_url("ftp.example.com", Some(2121), Some("alice"));
    assert_eq!(
        ftp_connect_target(&url_port, Some("proxy.example.net")),
        (
            "proxy.example.net:21".into(),
            "alice@ftp.example.com:2121".into()
        )
    );
    let anon = ftp_url("ftp.gnu.org", None, None);
    assert_eq!(
        ftp_connect_target(&anon, Some("gw.local")),
        ("gw.local:21".into(), "anonymous@ftp.gnu.org".into())
    );
}
