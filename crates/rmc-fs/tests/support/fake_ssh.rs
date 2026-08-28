//! Local fake SSH/SFTP/FISH server (no internet) for sftpfs and fish tests.
use rand_core::OsRng;
use russh::server::{Auth, Msg, Server as _, Session};
use russh::{Channel, ChannelId, CryptoVec};
use russh_sftp::protocol::{
    Attrs, Data, File, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode,
};
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

pub type SharedTree = Arc<Mutex<BTreeMap<String, Node>>>;

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

fn extract_quoted_paths(cmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = cmd;
    while let Some(start) = rest.find('\'') {
        rest = &rest[start + 1..];
        if let Some(end) = rest.find('\'') {
            out.push(normalize(&rest[..end]));
            rest = &rest[end + 1..];
        } else {
            break;
        }
    }
    out
}

fn rename_tree(tree: &mut BTreeMap<String, Node>, old: &str, new: &str) {
    let mut updates = Vec::new();
    for (k, v) in tree.iter() {
        if k == old || k.starts_with(&format!("{old}/")) {
            let suffix = &k[old.len()..];
            updates.push((k.clone(), format!("{new}{suffix}"), v.clone()));
        }
    }
    for (oldk, newk, v) in updates {
        tree.remove(&oldk);
        tree.insert(newk, v);
    }
}

fn rm_tree(tree: &mut BTreeMap<String, Node>, path: &str, recursive: bool) {
    if recursive {
        let keys: Vec<String> = tree
            .keys()
            .filter(|k| *k == path || k.starts_with(&format!("{path}/")))
            .cloned()
            .collect();
        for k in keys {
            tree.remove(&k);
        }
    } else {
        tree.remove(path);
    }
}

enum FishAction {
    Output(Vec<u8>, u32),
    PendingCatWrite(String),
}

fn fish_exec(tree: &SharedTree, cmd: &str) -> FishAction {
    if cmd.contains("cat >") {
        let path = extract_quoted_paths(cmd)
            .into_iter()
            .next()
            .unwrap_or_else(|| "/".into());
        return FishAction::PendingCatWrite(path);
    }
    let mut g = tree.lock().unwrap();
    if cmd.contains("mkdir") {
        let path = extract_quoted_paths(cmd)
            .into_iter()
            .next()
            .unwrap_or_else(|| "/".into());
        g.insert(path, Node::Dir);
        return FishAction::Output(Vec::new(), 0);
    }
    if cmd.contains("mv ") {
        let paths = extract_quoted_paths(cmd);
        if paths.len() >= 2 {
            let src = paths[0].clone();
            let dst = paths[1].clone();
            rename_tree(&mut g, &src, &dst);
            return FishAction::Output(Vec::new(), 0);
        }
        return FishAction::Output(Vec::new(), 1);
    }
    if cmd.contains("rm ") {
        let path = extract_quoted_paths(cmd)
            .into_iter()
            .next()
            .unwrap_or_else(|| "/".into());
        let recursive = cmd.contains("rm -r") || cmd.contains("rm -rf");
        rm_tree(&mut g, &path, recursive);
        return FishAction::Output(Vec::new(), 0);
    }
    if cmd.contains("ls -ld") {
        let path = extract_quoted_paths(cmd)
            .into_iter()
            .next()
            .unwrap_or_else(|| "/".into());
        return match g.get(&path) {
            Some(node) => {
                let name = if path == "/" {
                    "/".to_string()
                } else {
                    path.rsplit('/').next().unwrap_or(&path).to_string()
                };
                FishAction::Output(format!("{}\n", unix_list_line(&name, node)).into_bytes(), 0)
            }
            None => FishAction::Output(Vec::new(), 1),
        };
    }
    if cmd.contains("ls -l") {
        let path = extract_quoted_paths(cmd)
            .into_iter()
            .next()
            .unwrap_or_else(|| "/".into());
        match g.get(&path) {
            Some(Node::Dir) => {
                let listing = children_of(&g, &path)
                    .into_iter()
                    .map(|(n, node)| unix_list_line(&n, &node))
                    .collect::<Vec<_>>()
                    .join("\n");
                let mut b = listing.into_bytes();
                if !b.is_empty() {
                    b.push(b'\n');
                }
                FishAction::Output(b, 0)
            }
            Some(node) => {
                let name = path.rsplit('/').next().unwrap_or(&path).to_string();
                FishAction::Output(format!("{}\n", unix_list_line(&name, node)).into_bytes(), 0)
            }
            None => FishAction::Output(Vec::new(), 1),
        }
    } else if cmd.contains("cat ") {
        let path = extract_quoted_paths(cmd)
            .into_iter()
            .next()
            .unwrap_or_else(|| "/".into());
        match g.get(&path) {
            Some(Node::File(bytes)) => FishAction::Output(bytes.clone(), 0),
            _ => FishAction::Output(Vec::new(), 1),
        }
    } else {
        FishAction::Output(Vec::new(), 1)
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

fn ok_status(id: u32) -> Status {
    Status {
        id,
        status_code: StatusCode::Ok,
        error_message: "Ok".into(),
        language_tag: "en-US".into(),
    }
}

struct OpenFile {
    path: String,
    bytes: Vec<u8>,
    writable: bool,
}

struct SftpFs {
    tree: SharedTree,
    dirs: HashMap<String, (Vec<File>, bool)>,
    files: HashMap<String, OpenFile>,
    next: u32,
}

impl SftpFs {
    fn new(tree: SharedTree) -> Self {
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
        if let Some(f) = self.files.remove(&handle) {
            if f.writable {
                self.tree
                    .lock()
                    .unwrap()
                    .insert(f.path, Node::File(f.bytes));
            }
        }
        Ok(ok_status(id))
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        let path = normalize(&path);
        let g = self.tree.lock().unwrap();
        match g.get(&path) {
            Some(Node::Dir) => {
                let files = children_of(&g, &path)
                    .into_iter()
                    .map(|(n, node)| File::new(n, attrs_for(&node)))
                    .collect();
                drop(g);
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
        match self.tree.lock().unwrap().get(&path) {
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
        pflags: OpenFlags,
        _attrs: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        let path = normalize(&filename);
        let write = pflags.contains(OpenFlags::WRITE)
            || pflags.contains(OpenFlags::CREATE)
            || pflags.contains(OpenFlags::TRUNCATE);
        let mut g = self.tree.lock().unwrap();
        if write {
            if !g.contains_key(&path) {
                if pflags.contains(OpenFlags::CREATE) || pflags.contains(OpenFlags::WRITE) {
                    g.insert(path.clone(), Node::File(Vec::new()));
                } else {
                    return Err(StatusCode::NoSuchFile);
                }
            }
            let bytes = match g.get(&path) {
                Some(Node::File(b)) => {
                    if pflags.contains(OpenFlags::TRUNCATE) || pflags.contains(OpenFlags::CREATE) {
                        Vec::new()
                    } else {
                        b.clone()
                    }
                }
                Some(Node::Dir) => return Err(StatusCode::Failure),
                None => Vec::new(),
            };
            drop(g);
            let h = format!("f{}", self.next);
            self.next += 1;
            self.files.insert(
                h.clone(),
                OpenFile {
                    path,
                    bytes,
                    writable: true,
                },
            );
            Ok(Handle { id, handle: h })
        } else {
            match g.get(&path) {
                Some(Node::File(bytes)) => {
                    let bytes = bytes.clone();
                    drop(g);
                    let h = format!("f{}", self.next);
                    self.next += 1;
                    self.files.insert(
                        h.clone(),
                        OpenFile {
                            path,
                            bytes,
                            writable: false,
                        },
                    );
                    Ok(Handle { id, handle: h })
                }
                _ => Err(StatusCode::NoSuchFile),
            }
        }
    }

    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        len: u32,
    ) -> Result<Data, Self::Error> {
        let Some(f) = self.files.get(&handle) else {
            return Err(StatusCode::Failure);
        };
        let start = offset as usize;
        if start >= f.bytes.len() {
            return Err(StatusCode::Eof);
        }
        let end = (start + len as usize).min(f.bytes.len());
        Ok(Data {
            id,
            data: f.bytes[start..end].to_vec(),
        })
    }

    async fn write(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<Status, Self::Error> {
        let Some(f) = self.files.get_mut(&handle) else {
            return Err(StatusCode::Failure);
        };
        let start = offset as usize;
        let end = start + data.len();
        if f.bytes.len() < end {
            f.bytes.resize(end, 0);
        }
        f.bytes[start..end].copy_from_slice(&data);
        Ok(ok_status(id))
    }

    async fn mkdir(
        &mut self,
        id: u32,
        path: String,
        _attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        let path = normalize(&path);
        self.tree.lock().unwrap().insert(path, Node::Dir);
        Ok(ok_status(id))
    }

    async fn remove(&mut self, id: u32, filename: String) -> Result<Status, Self::Error> {
        let path = normalize(&filename);
        match self.tree.lock().unwrap().remove(&path) {
            Some(Node::File(_)) => Ok(ok_status(id)),
            _ => Err(StatusCode::NoSuchFile),
        }
    }

    async fn rmdir(&mut self, id: u32, path: String) -> Result<Status, Self::Error> {
        let path = normalize(&path);
        match self.tree.lock().unwrap().remove(&path) {
            Some(Node::Dir) => Ok(ok_status(id)),
            _ => Err(StatusCode::NoSuchFile),
        }
    }

    async fn rename(
        &mut self,
        id: u32,
        oldpath: String,
        newpath: String,
    ) -> Result<Status, Self::Error> {
        let old = normalize(&oldpath);
        let new = normalize(&newpath);
        rename_tree(&mut self.tree.lock().unwrap(), &old, &new);
        Ok(ok_status(id))
    }
}

struct PendingWrite {
    path: String,
    buf: Vec<u8>,
}

struct SshSession {
    tree: SharedTree,
    channels: Arc<tokio::sync::Mutex<HashMap<ChannelId, Channel<Msg>>>>,
    pending: HashMap<ChannelId, PendingWrite>,
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

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        if let Some(p) = self.pending.get_mut(&channel) {
            p.buf.extend_from_slice(data);
        }
        Ok(())
    }

    async fn channel_eof(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if let Some(p) = self.pending.remove(&channel) {
            self.tree.lock().unwrap().insert(p.path, Node::File(p.buf));
            session.exit_status_request(channel, 0)?;
            session.eof(channel)?;
            session.close(channel)?;
        }
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let cmd = String::from_utf8_lossy(data).into_owned();
        match fish_exec(&self.tree, &cmd) {
            FishAction::PendingCatWrite(path) => {
                session.channel_success(channel)?;
                self.pending.insert(
                    channel,
                    PendingWrite {
                        path,
                        buf: Vec::new(),
                    },
                );
            }
            FishAction::Output(out, status) => {
                session.channel_success(channel)?;
                if !out.is_empty() {
                    session.data(channel, CryptoVec::from(out))?;
                }
                session.exit_status_request(channel, status)?;
                session.eof(channel)?;
                session.close(channel)?;
            }
        }
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
            let sftp = SftpFs::new(Arc::clone(&self.tree));
            russh_sftp::server::run(channel.into_stream(), sftp).await;
        } else {
            session.channel_failure(channel_id)?;
        }
        Ok(())
    }
}

struct FakeServer {
    tree: SharedTree,
}

impl russh::server::Server for FakeServer {
    type Handler = SshSession;

    fn new_client(&mut self, _: Option<SocketAddr>) -> Self::Handler {
        SshSession {
            tree: Arc::clone(&self.tree),
            channels: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            pending: HashMap::new(),
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
        let shared: SharedTree = Arc::new(Mutex::new(tree));
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
                let mut server = FakeServer { tree: shared };
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
