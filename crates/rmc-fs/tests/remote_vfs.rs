use rmc_fs::remote::{
    copy_out_with_client, ftp_connect_target, list_dir, list_dir_with_client, parse_remote_url_str,
    set_ftp_proxy, RemoteClient, RemoteEntry, RemoteScheme, RemoteUrl,
};
use std::sync::Mutex;
use tempfile::tempdir;

/// Live `set_ftp_proxy` + TCP mock tests share process-wide proxy state.
static FTP_PROXY_LIVE_LOCK: Mutex<()> = Mutex::new(());

fn lock_ftp_proxy_tests() -> std::sync::MutexGuard<'static, ()> {
    FTP_PROXY_LIVE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

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

#[test]
fn ftp_connect_sends_user_at_host_to_proxy() {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    let _serial = lock_ftp_proxy_tests();

    struct ClearProxy;
    impl Drop for ClearProxy {
        fn drop(&mut self) {
            set_ftp_proxy(None);
        }
    }
    let _clear = ClearProxy;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel::<String>();
    thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut stream = stream;
        stream.write_all(b"220 mock ftp\r\n").unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let _ = tx.send(line.clone());
        stream.write_all(b"331 need password\r\n").unwrap();
        line.clear();
        let _ = reader.read_line(&mut line);
        stream.write_all(b"230 logged in\r\n").unwrap();
        line.clear();
        let _ = reader.read_line(&mut line); // TYPE I
        let _ = stream.write_all(b"200 ok\r\n");
    });

    set_ftp_proxy(Some(&format!("127.0.0.1:{port}")));
    let url = ftp_url("ftp.example.com", None, Some("alice"));
    let _ = list_dir(&url, std::path::Path::new("."), false);

    let user_line = rx
        .recv_timeout(Duration::from_secs(3))
        .expect("proxy should receive USER");
    assert_eq!(user_line, "USER alice@ftp.example.com\r\n");
}

#[test]
fn ftp_connect_direct_sends_plain_user_to_real_host() {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    let _serial = lock_ftp_proxy_tests();

    struct ClearProxy;
    impl Drop for ClearProxy {
        fn drop(&mut self) {
            set_ftp_proxy(None);
        }
    }
    let _clear = ClearProxy;
    set_ftp_proxy(None);

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel::<String>();
    thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut stream = stream;
        stream.write_all(b"220 mock ftp\r\n").unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let _ = tx.send(line.clone());
        stream.write_all(b"331 need password\r\n").unwrap();
        line.clear();
        let _ = reader.read_line(&mut line);
        stream.write_all(b"230 logged in\r\n").unwrap();
        line.clear();
        let _ = reader.read_line(&mut line);
        let _ = stream.write_all(b"200 ok\r\n");
    });

    let url = ftp_url("127.0.0.1", Some(port), Some("alice"));
    let _ = list_dir(&url, std::path::Path::new("."), false);

    let user_line = rx
        .recv_timeout(Duration::from_secs(3))
        .expect("real host should receive USER");
    assert_eq!(user_line, "USER alice\r\n");
}
