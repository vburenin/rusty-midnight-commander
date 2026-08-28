//! Local fake SSH/SFTP/FISH server (no internet) for sftpfs and fish tests.
use rand_core::OsRng;
use russh::server::{Auth, Msg, Server as _, Session};
use russh::{Channel, ChannelId, CryptoVec};
use russh_sftp::protocol::{Attrs, Data, File, FileAttributes, Handle, Name, Status, StatusCode};
use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

#[derive(Clone)]
pub enum Node {
    File(Vec<u8>),
    Dir,
}

pub fn fixture_tree() -> BTreeMap<String, Node> {
    let mut m = BTreeMap::new();
    m.insert("/".into(), Node::Dir);
    m.insert("/hello.txt".into(), Node::File(b"hello-ssh".to_vec()));
    m.insert("/pub".into(), Node::Dir);
    m.insert(
        "/pub/inner.txt".into(),
        Node::File(b"inner-payload".to_vec()),
    );
    m.insert("/.hidden".into(), Node::File(b"hid".to_vec()));
    m
}

pub fn normalize(path: &str) -> String {
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
        Node::Dir => format!("drwxr-xr-x 2 owner group 4096 Jan 01 00:00 {name}"),
        Node::File(b) => format!("-rw-r--r-- 1 owner group {} Jan 01 00:00 {name}", b.len()),
    }
}

fn extract_quoted_path(cmd: &str) -> Option<String> {
    let start = cmd.find('\'')?;
    let rest = &cmd[start + 1..];
    let end = rest.find('\'')?;
    Some(normalize(&rest[..end]))
}

fn fish_exec(tree: &BTreeMap<String, Node>, cmd: &str) -> (Vec<u8>, u32) {
    if cmd.contains("ls -ld") {
        let path = extract_quoted_path(cmd).unwrap_or_else(|| "/".into());
        return match tree.get(&path) {
            Some(node) => {
                let name = if path == "/" {
                    "/".to_string()
                } else {
                    path.rsplit('/').next().unwrap_or(&path).to_string()
                };
                (format!("{}\n", unix_list_line(&name, node)).into_bytes(), 0)
            }
            None => (Vec::new(), 1),
        };
    }
    if cmd.contains("ls -l") {
        let path = extract_quoted_path(cmd).unwrap_or_else(|| "/".into());
        match tree.get(&path) {
            Some(Node::Dir) => {
                let listing = children_of(tree, &path)
                    .into_iter()
                    .map(|(n, node)| unix_list_line(&n, &node))
                    .collect::<Vec<_>>()
                    .join("\n");
                let mut b = listing.into_bytes();
                if !b.is_empty() {
                    b.push(b'\n');
                }
                (b, 0)
            }
            Some(node) => {
                let name = path.rsplit('/').next().unwrap_or(&path).to_string();
                (format!("{}\n", unix_list_line(&name, node)).into_bytes(), 0)
            }
            None => (Vec::new(), 1),
        }
    } else if cmd.contains("cat ") {
        let path = extract_quoted_path(cmd).unwrap_or_else(|| "/".into());
        match tree.get(&path) {
            Some(Node::File(bytes)) => (bytes.clone(), 0),
            _ => (Vec::new(), 1),
        }
    } else {
        (Vec::new(), 1)
    }
}

fn attrs_for(node: &Node) -> FileAttributes {
    let mut a = FileAttributes::default();
    match node {
        Node::Dir => {
            a.size = Some(4096);
            a.set_dir(true);
            a.permissions = Some(a.permissions.unwrap_or(0) | 0o755);
        }
        Node::File(b) => {
            a.size = Some(b.len() as u64);
            a.set_regular(true);
            a.permissions = Some(a.permissions.unwrap_or(0) | 0o644);
        }
    }
    a
}

struct SftpFs {
    tree: BTreeMap<String, Node>,
    dirs: HashMap<String, (Vec<File>, bool)>,
    files: HashMap<String, Vec<u8>>,
    next: u32,
}

impl SftpFs {
    fn new(tree: BTreeMap<String, Node>) -> Self {
        Self {
            tree,
            dirs: HashMap::new(),
            files: HashMap::new(),
            next: 1,
        }
    }
}

impl russh_sftp::server::Handler for SftpFs {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        self.dirs.remove(&handle);
        self.files.remove(&handle);
        Ok(Status {
            id,
            status_code: StatusCode::Ok,
            error_message: "Ok".into(),
            language_tag: "en-US".into(),
        })
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        let path = normalize(&path);
        match self.tree.get(&path) {
            Some(Node::Dir) => {
                let files = children_of(&self.tree, &path)
                    .into_iter()
                    .map(|(n, node)| File::new(n, attrs_for(&node)))
                    .collect();
                let h = format!("d{}", self.next);
                self.next += 1;
                self.dirs.insert(h.clone(), (files, false));
                Ok(Handle { id, handle: h })
            }
            _ => Err(StatusCode::NoSuchFile),
        }
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
        let Some((files, sent)) = self.dirs.get_mut(&handle) else {
            return Err(StatusCode::Failure);
        };
        if *sent {
            return Err(StatusCode::Eof);
        }
        *sent = true;
        Ok(Name {
            id,
            files: files.clone(),
        })
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        Ok(Name {
            id,
            files: vec![File::dummy(normalize(&path))],
        })
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        self.stat(id, path).await
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let path = normalize(&path);
        match self.tree.get(&path) {
            Some(node) => Ok(Attrs {
                id,
                attrs: attrs_for(node),
            }),
            None => Err(StatusCode::NoSuchFile),
        }
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        _pflags: russh_sftp::protocol::OpenFlags,
        _attrs: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        let path = normalize(&filename);
        match self.tree.get(&path) {
            Some(Node::File(bytes)) => {
                let h = format!("f{}", self.next);
                self.next += 1;
                self.files.insert(h.clone(), bytes.clone());
                Ok(Handle { id, handle: h })
            }
            _ => Err(StatusCode::NoSuchFile),
        }
    }

    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        len: u32,
    ) -> Result<Data, Self::Error> {
        let Some(bytes) = self.files.get(&handle) else {
            return Err(StatusCode::Failure);
        };
        let start = offset as usize;
        if start >= bytes.len() {
            return Err(StatusCode::Eof);
        }
        let end = (start + len as usize).min(bytes.len());
        Ok(Data {
            id,
            data: bytes[start..end].to_vec(),
        })
    }
}

struct SshSession {
    tree: BTreeMap<String, Node>,
    channels: Arc<tokio::sync::Mutex<HashMap<ChannelId, Channel<Msg>>>>,
}

impl SshSession {
    async fn take_channel(&mut self, id: ChannelId) -> Channel<Msg> {
        self.channels.lock().await.remove(&id).expect("channel")
    }
}

impl russh::server::Handler for SshSession {
    type Error = russh::Error;

    async fn auth_password(&mut self, _user: &str, _password: &str) -> Result<Auth, Self::Error> {
        Ok(Auth::Accept)
    }

    async fn auth_publickey(
        &mut self,
        _user: &str,
        _key: &russh::keys::PublicKey,
    ) -> Result<Auth, Self::Error> {
        Ok(Auth::Accept)
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        _session: &mut Session,
    ) -> Result<bool, Self::Error> {
        self.channels.lock().await.insert(channel.id(), channel);
        Ok(true)
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let cmd = String::from_utf8_lossy(data).into_owned();
        let (out, status) = fish_exec(&self.tree, &cmd);
        session.channel_success(channel)?;
        if !out.is_empty() {
            session.data(channel, CryptoVec::from(out))?;
        }
        session.exit_status_request(channel, status)?;
        session.eof(channel)?;
        session.close(channel)?;
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        channel_id: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if name == "sftp" {
            let channel = self.take_channel(channel_id).await;
            session.channel_success(channel_id)?;
            let sftp = SftpFs::new(self.tree.clone());
            russh_sftp::server::run(channel.into_stream(), sftp).await;
        } else {
            session.channel_failure(channel_id)?;
        }
        Ok(())
    }
}

struct FakeServer {
    tree: BTreeMap<String, Node>,
}

impl russh::server::Server for FakeServer {
    type Handler = SshSession;

    fn new_client(&mut self, _: Option<SocketAddr>) -> Self::Handler {
        SshSession {
            tree: self.tree.clone(),
            channels: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        }
    }
}

pub struct FakeSsh {
    pub port: u16,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

#[allow(dead_code)] // included from both sftpfs and fish integration tests
impl FakeSsh {
    pub fn spawn(tree: BTreeMap<String, Node>) -> Self {
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_t = Arc::clone(&stop);
        let join = thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime");
            rt.block_on(async move {
                let key =
                    russh::keys::PrivateKey::random(&mut OsRng, russh::keys::Algorithm::Ed25519)
                        .expect("host key");
                let config = russh::server::Config {
                    inactivity_timeout: Some(Duration::from_secs(30)),
                    auth_rejection_time: Duration::from_secs(0),
                    auth_rejection_time_initial: Some(Duration::from_secs(0)),
                    keys: vec![key],
                    ..Default::default()
                };
                let socket = tokio::net::TcpListener::bind("127.0.0.1:0")
                    .await
                    .expect("bind");
                let port = socket.local_addr().expect("addr").port();
                let _ = tx.send(port);
                let mut server = FakeServer { tree };
                let run = server.run_on_socket(Arc::new(config), &socket);
                tokio::select! {
                    _ = run => {}
                    _ = async {
                        while !stop_t.load(Ordering::SeqCst) {
                            tokio::time::sleep(Duration::from_millis(20)).await;
                        }
                    } => {}
                }
            });
        });
        let port = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("server port");
        Self {
            port,
            stop,
            join: Some(join),
        }
    }

    pub fn sftp_url(&self, remote: &str) -> std::path::PathBuf {
        url("sftp", self.port, remote)
    }

    pub fn sh_url(&self, remote: &str) -> std::path::PathBuf {
        url("sh", self.port, remote)
    }
}

fn url(scheme: &str, port: u16, remote: &str) -> std::path::PathBuf {
    let remote = remote.trim_start_matches('/');
    if remote.is_empty() {
        std::path::PathBuf::from(format!("{scheme}://user:pass@127.0.0.1:{port}"))
    } else {
        std::path::PathBuf::from(format!("{scheme}://user:pass@127.0.0.1:{port}/{remote}"))
    }
}

impl Drop for FakeSsh {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = std::net::TcpStream::connect(("127.0.0.1", self.port));
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

/// Serialize tests that mutate process-wide known_hosts policy.
pub fn lock_host_keys() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}
