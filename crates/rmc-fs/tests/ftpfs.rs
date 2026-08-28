//! ftpfs against a local fake FTP server (no internet).
use rmc_fs::composite::CompositeFs;
use rmc_fs::Vfs;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use tempfile::tempdir;

#[derive(Clone)]
enum Node {
    File(Vec<u8>),
    Dir,
}

struct FakeFtp {
    port: u16,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl FakeFtp {
    fn spawn(tree: BTreeMap<String, Node>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_t = Arc::clone(&stop);
        let join = thread::spawn(move || {
            while !stop_t.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let tree = tree.clone();
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                        let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
                        let _ = handle_session(stream, &tree);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            port,
            stop,
            join: Some(join),
        }
    }

    fn url(&self, remote: &str) -> PathBuf {
        let remote = remote.trim_start_matches('/');
        if remote.is_empty() {
            PathBuf::from(format!("ftp://127.0.0.1:{}", self.port))
        } else {
            PathBuf::from(format!("ftp://127.0.0.1:{}/{}", self.port, remote))
        }
    }
}

impl Drop for FakeFtp {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

fn fixture_tree() -> BTreeMap<String, Node> {
    let mut m = BTreeMap::new();
    m.insert("/".into(), Node::Dir);
    m.insert("/hello.txt".into(), Node::File(b"hello-ftp".to_vec()));
    m.insert("/pub".into(), Node::Dir);
    m.insert(
        "/pub/inner.txt".into(),
        Node::File(b"inner-payload".to_vec()),
    );
    m.insert("/.hidden".into(), Node::File(b"hid".to_vec()));
    m
}

fn normalize(path: &str) -> String {
    let mut out = Vec::<&str>::new();
    for part in path.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            out.pop();
        } else {
            out.push(part);
        }
    }
    if out.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", out.join("/"))
    }
}

fn resolve(cwd: &str, arg: Option<&str>) -> String {
    match arg {
        None | Some("") => cwd.to_string(),
        Some(p) if p.starts_with('/') => normalize(p),
        Some(p) => {
            if cwd == "/" {
                normalize(&format!("/{p}"))
            } else {
                normalize(&format!("{cwd}/{p}"))
            }
        }
    }
}

fn children_of(tree: &BTreeMap<String, Node>, dir: &str) -> Vec<(String, Node)> {
    let prefix = if dir == "/" {
        "/".to_string()
    } else {
        format!("{dir}/")
    };
    let mut out = Vec::new();
    for (k, v) in tree {
        if *k == "/" {
            continue;
        }
        if let Some(rest) = k.strip_prefix(&prefix) {
            if !rest.is_empty() && !rest.contains('/') {
                out.push((rest.to_string(), v.clone()));
            }
        }
    }
    out
}

fn unix_list_line(name: &str, node: &Node) -> String {
    match node {
        Node::Dir => format!("drwxr-xr-x  2 owner group 4096 Jan 01 00:00 {name}"),
        Node::File(b) => format!("-rw-r--r--  1 owner group {} Jan 01 00:00 {name}", b.len()),
    }
}

fn handle_session(stream: TcpStream, tree: &BTreeMap<String, Node>) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;
    write!(writer, "220 fake ftp\r\n")?;
    writer.flush()?;
    let mut cwd = "/".to_string();
    let mut pasv: Option<TcpListener> = None;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            break;
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            continue;
        }
        let (cmd, arg) = match line.split_once(' ') {
            Some((c, a)) => (c.to_ascii_uppercase(), Some(a.trim())),
            None => (line.to_ascii_uppercase(), None),
        };
        match cmd.as_str() {
            "USER" => write!(writer, "331 need password\r\n")?,
            "PASS" => write!(writer, "230 logged in\r\n")?,
            "TYPE" | "NOOP" | "MODE" | "STRU" => write!(writer, "200 ok\r\n")?,
            "SYST" => write!(writer, "215 UNIX Type: L8\r\n")?,
            "FEAT" => write!(writer, "211-Features\r\n211 End\r\n")?,
            "PWD" => write!(writer, "257 \"{cwd}\"\r\n")?,
            "CWD" => {
                let next = resolve(&cwd, arg);
                match tree.get(&next) {
                    Some(Node::Dir) => {
                        cwd = next;
                        write!(writer, "250 cwd ok\r\n")?;
                    }
                    _ => write!(writer, "550 no such dir\r\n")?,
                }
            }
            "CDUP" => {
                cwd = resolve(&cwd, Some(".."));
                write!(writer, "250 cwd ok\r\n")?;
            }
            "PASV" => {
                let l = TcpListener::bind("127.0.0.1:0")?;
                let addr = l.local_addr()?;
                let p = addr.port();
                let p1 = p >> 8;
                let p2 = p & 0xff;
                write!(
                    writer,
                    "227 Entering Passive Mode (127,0,0,1,{p1},{p2})\r\n"
                )?;
                pasv = Some(l);
            }
            "LIST" | "NLST" => {
                let path = resolve(&cwd, arg);
                let listing = match tree.get(&path) {
                    Some(Node::Dir) => children_of(tree, &path)
                        .into_iter()
                        .map(|(n, node)| unix_list_line(&n, &node))
                        .collect::<Vec<_>>()
                        .join("\r\n"),
                    Some(node) => {
                        let name = Path::new(&path)
                            .file_name()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_else(|| path.clone());
                        unix_list_line(&name, node)
                    }
                    None => {
                        write!(writer, "550 not found\r\n")?;
                        writer.flush()?;
                        continue;
                    }
                };
                write!(writer, "150 opening data\r\n")?;
                writer.flush()?;
                match take_pasv(&mut pasv) {
                    Some(mut data) => {
                        if cmd == "NLST" {
                            let names = children_of(tree, &path)
                                .into_iter()
                                .map(|(n, _)| n)
                                .collect::<Vec<_>>()
                                .join("\r\n");
                            let _ = write!(data, "{names}\r\n");
                        } else if !listing.is_empty() {
                            let _ = write!(data, "{listing}\r\n");
                        }
                    }
                    None => {
                        write!(writer, "425 no pasv\r\n")?;
                        continue;
                    }
                }
                write!(writer, "226 transfer complete\r\n")?;
            }
            "RETR" => {
                let path = resolve(&cwd, arg);
                match tree.get(&path) {
                    Some(Node::File(bytes)) => {
                        write!(writer, "150 opening data\r\n")?;
                        writer.flush()?;
                        match take_pasv(&mut pasv) {
                            Some(mut data) => {
                                let _ = data.write_all(bytes);
                            }
                            None => {
                                write!(writer, "425 no pasv\r\n")?;
                                continue;
                            }
                        }
                        write!(writer, "226 transfer complete\r\n")?;
                    }
                    _ => write!(writer, "550 not a file\r\n")?,
                }
            }
            "SIZE" => {
                let path = resolve(&cwd, arg);
                match tree.get(&path) {
                    Some(Node::File(b)) => write!(writer, "213 {}\r\n", b.len())?,
                    _ => write!(writer, "550 not a file\r\n")?,
                }
            }
            "MDTM" => {
                let path = resolve(&cwd, arg);
                match tree.get(&path) {
                    Some(Node::File(_)) => write!(writer, "213 20200101120000\r\n")?,
                    _ => write!(writer, "550 not a file\r\n")?,
                }
            }
            "QUIT" => {
                write!(writer, "221 bye\r\n")?;
                break;
            }
            _ => write!(writer, "502 not implemented\r\n")?,
        }
        writer.flush()?;
    }
    Ok(())
}

fn take_pasv(pasv: &mut Option<TcpListener>) -> Option<TcpStream> {
    let listener = pasv.take()?;
    listener.set_nonblocking(true).ok()?;
    let start = Instant::now();
    loop {
        match listener.accept() {
            Ok((s, _)) => {
                let _ = s.set_nonblocking(false);
                return Some(s);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if start.elapsed() > Duration::from_secs(3) {
                    return None;
                }
                thread::sleep(Duration::from_millis(5));
            }
            Err(_) => return None,
        }
    }
}

fn names(list: &[rmc_fs::DirEntry]) -> Vec<String> {
    list.iter().map(|e| e.name.clone()).collect()
}

fn find_parent(vfs: &CompositeFs, path: &Path) -> PathBuf {
    let list = vfs.list_dir(path, true).unwrap();
    list.iter()
        .find(|e| e.name == "..")
        .expect("parent marker")
        .path
        .clone()
}

#[test]
fn ftpfs_list_stat_copy_out_and_parent_leave() {
    let ftp = FakeFtp::spawn(fixture_tree());
    let vfs = CompositeFs::new();
    vfs.set_dir_cache_timeout_secs(0);

    let enter = vfs.enter_path(&ftp.url("")).expect("ftp enterable");
    assert_eq!(
        enter,
        PathBuf::from(format!("/#ftp:127.0.0.1:{}", ftp.port))
    );

    let root = enter.clone();
    let list = vfs.list_dir(&root, false).unwrap();
    let n = names(&list);
    assert!(n.contains(&"..".into()), "parent marker: {n:?}");
    assert!(n.contains(&"hello.txt".into()), "{n:?}");
    assert!(n.contains(&"pub".into()), "{n:?}");
    assert!(!n.contains(&".hidden".into()), "hidden filtered: {n:?}");
    let hello = list.iter().find(|e| e.name == "hello.txt").unwrap();
    assert!(!hello.meta.is_dir);
    assert_eq!(hello.meta.size, b"hello-ftp".len() as u64);
    let pub_ent = list.iter().find(|e| e.name == "pub").unwrap();
    assert!(pub_ent.meta.is_dir);
    assert_eq!(
        pub_ent.path,
        PathBuf::from(format!("/#ftp:127.0.0.1:{}/pub", ftp.port))
    );

    let shown = vfs.list_dir(&root, true).unwrap();
    assert!(names(&shown).contains(&".hidden".into()));

    let hello_path = root.join("hello.txt");
    let st = vfs.stat(&hello_path).unwrap();
    assert!(!st.is_dir);
    assert_eq!(st.size, b"hello-ftp".len() as u64);

    let pub_path = root.join("pub");
    let st_dir = vfs.stat(&pub_path).unwrap();
    assert!(st_dir.is_dir);

    let mut r = vfs.read_file(&hello_path).unwrap();
    let mut buf = Vec::new();
    std::io::Read::read_to_end(&mut r, &mut buf).unwrap();
    assert_eq!(buf, b"hello-ftp");

    let tmp = tempdir().unwrap();
    let dst = tmp.path().join("out.txt");
    vfs.copy(&hello_path, &dst).unwrap();
    assert_eq!(std::fs::read(&dst).unwrap(), b"hello-ftp");

    let inner = pub_path.join("inner.txt");
    let inner_list = vfs.list_dir(&pub_path, true).unwrap();
    assert!(names(&inner_list).contains(&"inner.txt".into()));
    let inner_dst = tmp.path().join("inner.txt");
    vfs.copy(&inner, &inner_dst).unwrap();
    assert_eq!(std::fs::read(&inner_dst).unwrap(), b"inner-payload");

    // Nested `..` stays inside ftpfs; root `..` leaves (GNU archive-style).
    let from_pub = find_parent(&vfs, &pub_path);
    assert_eq!(from_pub, root);
    let outside = find_parent(&vfs, &root);
    assert_eq!(outside, PathBuf::from("/"));
    // ParentDir uses Path::parent of the GNU `/#ftp:` cwd.
    assert_eq!(root.parent(), Some(Path::new("/")));
}

#[test]
fn ftpfs_list_via_ftp_url_and_hash_ftp_prefix() {
    let ftp = FakeFtp::spawn(fixture_tree());
    let vfs = CompositeFs::new();
    vfs.set_dir_cache_timeout_secs(0);

    let url = ftp.url("");
    let list = vfs.list_dir(&url, false).unwrap();
    assert!(names(&list).contains(&"hello.txt".into()));
    let parent = list.iter().find(|e| e.name == "..").unwrap();
    assert_eq!(parent.path, PathBuf::from("/"));

    let hash = PathBuf::from(format!("/#ftp:127.0.0.1:{}", ftp.port));
    let list2 = vfs.list_dir(&hash, false).unwrap();
    assert!(names(&list2).contains(&"pub".into()));
}

#[test]
fn ftpfs_connect_error_is_plain_fserror() {
    let vfs = CompositeFs::new();
    vfs.set_dir_cache_timeout_secs(0);
    let err = vfs
        .list_dir(Path::new("ftp://127.0.0.1:1/"), false)
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("FTP"),
        "error should be a plain FTP FsError for the existing dialog, got {msg}"
    );
}

#[test]
fn ftpfs_nested_parent_then_leave() {
    let ftp = FakeFtp::spawn(fixture_tree());
    let vfs = CompositeFs::new();
    vfs.set_dir_cache_timeout_secs(0);
    let root = vfs.enter_path(&ftp.url("")).unwrap();
    let pub_dir = root.join("pub");
    assert_eq!(find_parent(&vfs, &pub_dir), root);
    assert_eq!(find_parent(&vfs, &root), PathBuf::from("/"));
}
