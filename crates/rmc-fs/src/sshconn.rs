//! SSH connection helper for sftpfs and FISH.
//!
//! Host-key checks use `~/.ssh/known_hosts` (or a test override). When the
//! host/key pair is missing or mismatched, a prompt is stashed so the UI can
//! show the GNU mc(1) SFTP filesystem Yes / Ignore / No buttons.

use crate::remote::RemoteUrl;
use crate::{FsError, FsResult};
use russh::client::{self, AuthResult, Handle};
use russh::keys::{HashAlg, PublicKey};
use russh::ChannelMsg;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// GNU mc(1) SFTP filesystem host-key buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostKeyAction {
    /// Add the host/key pair to known_hosts and continue.
    Yes,
    /// Continue without adding the host/key pair.
    Ignore,
    /// Abort the connection.
    No,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostKeyKind {
    /// Host/key pair is not in known_hosts.
    Unknown,
    /// Host is in known_hosts but the key does not match.
    Mismatch,
}

#[derive(Debug, Clone)]
pub struct HostKeyPrompt {
    pub kind: HostKeyKind,
    pub host: String,
    pub port: u16,
    pub key_type: String,
    pub fingerprint: String,
    /// OpenSSH known_hosts line to append on Yes.
    pub known_hosts_line: String,
}

impl HostKeyPrompt {
    /// Original wording from public mc(1) “SFTP filesystem” (not GPL C strings).
    pub fn dialog_title() -> &'static str {
        "SFTP filesystem"
    }

    pub fn dialog_message(&self) -> String {
        match self.kind {
            HostKeyKind::Unknown => format!(
                "The host/key pair for {} is not in ~/.ssh/known_hosts.\n\
                 {} fingerprint is {}.\n\
                 Yes adds the host/key pair to ~/.ssh/known_hosts and continues.\n\
                 Ignore continues without adding it.\n\
                 No aborts the connection.",
                self.host, self.key_type, self.fingerprint
            ),
            HostKeyKind::Mismatch => format!(
                "The host {} is in ~/.ssh/known_hosts, but the key does not match.\n\
                 {} fingerprint is {}.\n\
                 Yes adds the host/key pair to ~/.ssh/known_hosts and continues.\n\
                 Ignore continues without adding it.\n\
                 No aborts the connection.",
                self.host, self.key_type, self.fingerprint
            ),
        }
    }

    pub fn to_fs_error(&self) -> FsError {
        FsError::Message(self.dialog_message())
    }
}

struct HostKeyState {
    known_hosts: Option<PathBuf>,
    pending: Option<HostKeyAction>,
    last_prompt: Option<HostKeyPrompt>,
    ignored: Vec<String>,
}

fn host_key_state() -> &'static Mutex<HostKeyState> {
    static STATE: OnceLock<Mutex<HostKeyState>> = OnceLock::new();
    STATE.get_or_init(|| {
        Mutex::new(HostKeyState {
            known_hosts: None,
            pending: None,
            last_prompt: None,
            ignored: Vec::new(),
        })
    })
}

fn lock_state() -> std::sync::MutexGuard<'static, HostKeyState> {
    host_key_state().lock().unwrap_or_else(|e| e.into_inner())
}

/// Override the known_hosts file (tests). `None` restores `~/.ssh/known_hosts`.
pub fn set_known_hosts_path(path: Option<PathBuf>) {
    lock_state().known_hosts = path;
}

/// Answer the next host-key prompt (Yes add / Ignore / No abort).
pub fn set_host_key_action(action: HostKeyAction) {
    lock_state().pending = Some(action);
}

pub fn take_host_key_prompt() -> Option<HostKeyPrompt> {
    lock_state().last_prompt.take()
}

fn known_hosts_path() -> PathBuf {
    if let Some(p) = lock_state().known_hosts.clone() {
        return p;
    }
    let home = std::env::var_os("HOME").unwrap_or_else(|| "/tmp".into());
    PathBuf::from(home).join(".ssh").join("known_hosts")
}

fn ssh_runtime() -> FsResult<&'static tokio::runtime::Runtime> {
    static RT: OnceLock<std::result::Result<tokio::runtime::Runtime, String>> = OnceLock::new();
    match RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())
    }) {
        Ok(rt) => Ok(rt),
        Err(e) => Err(FsError::Message(format!("tokio runtime: {e}"))),
    }
}

pub fn block_on<T>(fut: impl std::future::Future<Output = T>) -> T {
    ssh_runtime().expect("tokio runtime").block_on(fut)
}

pub(crate) struct ClientHandler {
    host: String,
    port: u16,
}

impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKey,
    ) -> Result<bool, Self::Error> {
        match verify_server_key(&self.host, self.port, server_public_key) {
            Ok(()) => Ok(true),
            Err(_) => Ok(false),
        }
    }
}

fn key_type_name(key: &PublicKey) -> String {
    key.algorithm().to_string()
}

fn fingerprint_sha256(key: &PublicKey) -> String {
    match key.fingerprint(HashAlg::Sha256) {
        fp => format!("{fp}"),
    }
}

fn known_hosts_line(host: &str, port: u16, key: &PublicKey) -> String {
    let hostpart = if port == 22 {
        host.to_string()
    } else {
        format!("[{host}]:{port}")
    };
    let body = key
        .to_openssh()
        .unwrap_or_else(|_| format!("{} {}", key.algorithm(), fallback_b64(key)));
    format!("{hostpart} {body}")
}

fn fallback_b64(key: &PublicKey) -> String {
    use base64::Engine;
    match key.to_bytes() {
        Ok(blob) => base64::engine::general_purpose::STANDARD.encode(blob.as_slice()),
        Err(_) => String::new(),
    }
}

fn verify_server_key(host: &str, port: u16, key: &PublicKey) -> FsResult<()> {
    let fp = fingerprint_sha256(key);
    let line = known_hosts_line(host, port, key);
    let key_type = key_type_name(key);
    {
        let st = lock_state();
        if st.ignored.iter().any(|s| s == &fp) {
            return Ok(());
        }
    }
    let path = known_hosts_path();
    let status = match_known_hosts(&path, host, port, key);
    match status {
        KnownStatus::Match => Ok(()),
        KnownStatus::Unknown | KnownStatus::Mismatch => {
            let kind = if matches!(status, KnownStatus::Mismatch) {
                HostKeyKind::Mismatch
            } else {
                HostKeyKind::Unknown
            };
            let prompt = HostKeyPrompt {
                kind,
                host: host.to_string(),
                port,
                key_type,
                fingerprint: fp.clone(),
                known_hosts_line: line.clone(),
            };
            let action = lock_state().pending.take();
            match action {
                Some(HostKeyAction::Yes) => {
                    append_known_hosts(&path, &line)?;
                    Ok(())
                }
                Some(HostKeyAction::Ignore) => {
                    lock_state().ignored.push(fp);
                    Ok(())
                }
                Some(HostKeyAction::No) | None => {
                    lock_state().last_prompt = Some(prompt.clone());
                    Err(prompt.to_fs_error())
                }
            }
        }
    }
}

enum KnownStatus {
    Match,
    Unknown,
    Mismatch,
}

fn match_known_hosts(path: &Path, host: &str, port: u16, key: &PublicKey) -> KnownStatus {
    let Ok(text) = std::fs::read_to_string(path) else {
        return KnownStatus::Unknown;
    };
    let want_host = if port == 22 {
        host.to_string()
    } else {
        format!("[{host}]:{port}")
    };
    let want_plain = host.to_string();
    let key_body = key.to_openssh().unwrap_or_default();
    let mut saw_host = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('@') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(names) = parts.next() else {
            continue;
        };
        let host_hit = names
            .split(',')
            .any(|n| n == want_host || (port == 22 && n == want_plain));
        if !host_hit {
            continue;
        }
        saw_host = true;
        let rest: Vec<&str> = parts.collect();
        if rest.len() < 2 {
            continue;
        }
        let listed = format!("{} {}", rest[0], rest[1]);
        if listed == key_body || line.contains(key_body.split_whitespace().nth(1).unwrap_or("")) {
            return KnownStatus::Match;
        }
    }
    if saw_host {
        KnownStatus::Mismatch
    } else {
        KnownStatus::Unknown
    }
}

fn append_known_hosts(path: &Path, line: &str) -> FsResult<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(f, "{line}")?;
    Ok(())
}

async fn authenticate(session: &mut Handle<ClientHandler>, url: &RemoteUrl) -> FsResult<()> {
    let user = url.user.clone().unwrap_or_else(whoami::username);
    if let Some(pass) = &url.pass {
        match session.authenticate_password(&user, pass).await {
            Ok(AuthResult::Success) => return Ok(()),
            Ok(_) => {}
            Err(e) => return Err(FsError::Message(format!("SSH password auth: {e}"))),
        }
    }
    if try_publickey(session, &user).await? {
        return Ok(());
    }
    if url.pass.is_none() {
        match session.authenticate_password(&user, "").await {
            Ok(AuthResult::Success) => return Ok(()),
            Ok(_) => {}
            Err(_) => {}
        }
    }
    Err(FsError::Message("SSH authentication failed".into()))
}

async fn try_publickey(session: &mut Handle<ClientHandler>, user: &str) -> FsResult<bool> {
    let home = std::env::var_os("HOME").unwrap_or_else(|| "/tmp".into());
    let ssh_dir = PathBuf::from(home).join(".ssh");
    for name in ["id_ed25519", "id_rsa", "id_ecdsa"] {
        let path = ssh_dir.join(name);
        if !path.is_file() {
            continue;
        }
        let key = match russh::keys::load_secret_key(&path, None) {
            Ok(k) => k,
            Err(_) => continue,
        };
        match session
            .authenticate_publickey(
                user,
                russh::keys::PrivateKeyWithHashAlg::new(std::sync::Arc::new(key), None),
            )
            .await
        {
            Ok(AuthResult::Success) => return Ok(true),
            _ => continue,
        }
    }
    Ok(false)
}

/// Open an SSH session (password and/or default public keys).
pub(crate) async fn connect_handle(url: &RemoteUrl) -> FsResult<Handle<ClientHandler>> {
    let port = url.port.unwrap_or(22);
    let mut config = russh::client::Config::default();
    config.inactivity_timeout = Some(CONNECT_TIMEOUT);
    if url.compression {
        // Best-effort; russh default preferred list may already include zlib.
    }
    lock_state().last_prompt = None;
    let handler = ClientHandler {
        host: url.host.clone(),
        port,
    };
    let mut session = match tokio::time::timeout(
        CONNECT_TIMEOUT,
        client::connect(
            std::sync::Arc::new(config),
            (url.host.as_str(), port),
            handler,
        ),
    )
    .await
    {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            if let Some(p) = take_host_key_prompt() {
                lock_state().last_prompt = Some(p.clone());
                return Err(p.to_fs_error());
            }
            return Err(FsError::Message(format!("SSH connect: {e}")));
        }
        Err(_) => return Err(FsError::Message("SSH connect timed out".into())),
    };
    authenticate(&mut session, url).await?;
    Ok(session)
}

pub async fn exec_bytes(url: &RemoteUrl, command: &str) -> FsResult<Vec<u8>> {
    if url.use_rsh {
        return exec_rsh(url, command);
    }
    let session = connect_handle(url).await?;
    let mut channel = session
        .channel_open_session()
        .await
        .map_err(|e| FsError::Message(format!("SSH channel: {e}")))?;
    channel
        .exec(true, command)
        .await
        .map_err(|e| FsError::Message(format!("SSH exec: {e}")))?;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut status: Option<u32> = None;
    loop {
        match channel.wait().await {
            Some(ChannelMsg::Data { ref data }) => stdout.extend_from_slice(data),
            Some(ChannelMsg::ExtendedData { ref data, .. }) => stderr.extend_from_slice(data),
            Some(ChannelMsg::ExitStatus { exit_status }) => status = Some(exit_status),
            Some(ChannelMsg::Eof) | None => break,
            _ => {}
        }
    }
    if status.unwrap_or(0) != 0 && stdout.is_empty() {
        let err = String::from_utf8_lossy(&stderr);
        return Err(FsError::Message(format!("SSH exec failed: {err}")));
    }
    Ok(stdout)
}

fn exec_rsh(url: &RemoteUrl, command: &str) -> FsResult<Vec<u8>> {
    use std::process::Command;
    let mut cmd = Command::new("rsh");
    if let Some(u) = &url.user {
        cmd.arg("-l").arg(u);
    }
    cmd.arg(&url.host).arg(command);
    let out = cmd
        .output()
        .map_err(|e| FsError::Message(format!("rsh: {e}")))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(FsError::Message(format!("rsh failed: {err}")));
    }
    Ok(out.stdout)
}
