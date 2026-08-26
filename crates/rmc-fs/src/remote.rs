//! Remote VFS backends (FTP/SFTP/FISH/SMB) and helpers.
//!
//! Clean-room implementation to support MC-like `ftp://` and `sftp://` URLs.
//! Networking is performed synchronously. Unit tests avoid live networking by
//! exercising parsers and calling the `*_with_client` helpers with mock clients.
use crate::{pathutil, DirEntry, FsError, FsResult, Metadata};
use std::ffi::OsStr;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteScheme {
    Ftp,
    Sftp,
    Fish,
    Smb,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteUrl {
    pub scheme: RemoteScheme,
    pub user: Option<String>,
    pub pass: Option<String>, // only used by ftp
    pub host: String,
    pub port: Option<u16>,
    pub path: String, // absolute remote path starting with '/'
}

pub fn is_remote_url(path: &Path) -> bool {
    pathutil::extract_remote_canonical_url(path).is_some()
}

pub fn parse_remote_url_str(s: &str) -> Result<RemoteUrl, FsError> {
    let (scheme, rest) = if let Some(t) = s.strip_prefix("ftp://") {
        (RemoteScheme::Ftp, t)
    } else if let Some(t) = s.strip_prefix("sftp://") {
        (RemoteScheme::Sftp, t)
    } else if let Some(t) = s.strip_prefix("fish://") {
        (RemoteScheme::Fish, t)
    } else if let Some(t) = s.strip_prefix("smb://") {
        (RemoteScheme::Smb, t)
    } else {
        return Err(FsError::Message("unsupported remote scheme".into()));
    };
    // Split authority and path
    let (auth, path_part) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, "/"),
    };
    // Extract user[:pass]@host[:port]
    let (userpass_opt, hostport) = match auth.rsplit_once('@') {
        Some((u, h)) => (Some(u), h),
        None => (None, auth),
    };
    let (user_opt, pass_opt) = if let Some(up) = userpass_opt {
        match up.split_once(':') {
            Some((u, p)) => (Some(u.to_string()), Some(p.to_string())),
            None => (Some(up.to_string()), None),
        }
    } else {
        (None, None)
    };
    let (host_str, port_opt) = match hostport.rsplit_once(':') {
        Some((h, pstr)) if !pstr.is_empty() && pstr.chars().all(|c| c.is_ascii_digit()) => {
            let port = pstr
                .parse::<u16>()
                .map_err(|e| FsError::Message(format!("invalid port: {e}")))?;
            (h.to_string(), Some(port))
        }
        _ => (hostport.to_string(), None),
    };
    let path = if path_part.is_empty() {
        "/".to_string()
    } else {
        path_part.to_string()
    };
    Ok(RemoteUrl {
        scheme,
        user: user_opt,
        pass: pass_opt,
        host: host_str,
        port: port_opt,
        path,
    })
}

pub fn parse_remote_url(path: &Path) -> Result<RemoteUrl, FsError> {
    if let Some(canon) = pathutil::extract_remote_canonical_url(path) {
        parse_remote_url_str(&canon)
    } else {
        let s = path.as_os_str().to_string_lossy();
        parse_remote_url_str(&s)
    }
}

fn parent_marker(vfs_root: PathBuf) -> DirEntry {
    DirEntry {
        name: "..".to_string(),
        path: vfs_root,
        meta: Metadata {
            is_dir: true,
            is_symlink: false,
            is_executable: false,
            size: 0,
            modified: SystemTime::UNIX_EPOCH,
            permissions: 0,
            owner: None,
            group: None,
        },
    }
}

fn remote_parent_path(url: &RemoteUrl) -> PathBuf {
    let mut parent = PathBuf::from(match url.scheme {
        RemoteScheme::Ftp => format!("ftp://{}", url.authority()),
        RemoteScheme::Sftp => format!("sftp://{}", url.authority()),
        RemoteScheme::Fish => format!("fish://{}", url.authority()),
        RemoteScheme::Smb => format!("smb://{}", url.authority()),
    });
    // normalize parent of current path
    let cur = Path::new(&url.path);
    if let Some(p) = cur.parent() {
        let p_own = if p.as_os_str().is_empty() {
            "/".to_string()
        } else {
            p.to_string_lossy().to_string()
        };
        let joined = format!(
            "{}/{}",
            parent.to_string_lossy(),
            trim_leading_slash(&p_own)
        );
        parent = PathBuf::from(joined);
    }
    parent
}

fn trim_leading_slash(s: &str) -> &str {
    s.strip_prefix('/').unwrap_or(s)
}

impl RemoteUrl {
    pub fn authority(&self) -> String {
        let mut auth = String::new();
        if let Some(u) = &self.user {
            auth.push_str(u);
            if let Some(p) = &self.pass {
                auth.push(':');
                auth.push_str(p);
            }
            auth.push('@');
        }
        auth.push_str(&self.host);
        if let Some(p) = self.port {
            auth.push(':');
            auth.push_str(&p.to_string());
        }
        auth
    }
    pub fn to_root_string(&self) -> String {
        match self.scheme {
            RemoteScheme::Ftp => format!("ftp://{}", self.authority()),
            RemoteScheme::Sftp => format!("sftp://{}", self.authority()),
            RemoteScheme::Fish => format!("fish://{}", self.authority()),
            RemoteScheme::Smb => format!("smb://{}", self.authority()),
        }
    }
    pub fn to_string_path(&self) -> String {
        format!(
            "{}/{}",
            self.to_root_string(),
            trim_leading_slash(&self.path)
        )
    }
}

// Abstraction for testable remote operations
pub trait RemoteClient {
    fn list(&mut self, path: &str) -> FsResult<Vec<RemoteEntry>>;
    fn download(&mut self, remote_path: &str, local_path: &Path) -> FsResult<()>;
    fn upload(&mut self, local_path: &Path, remote_path: &str) -> FsResult<()>;
    fn remove_file(&mut self, remote_path: &str) -> FsResult<()>;
    fn remove_dir(&mut self, remote_path: &str) -> FsResult<()>;
    fn mkdir(&mut self, remote_path: &str) -> FsResult<()>;
}

#[derive(Debug, Clone)]
pub struct RemoteEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
}

// -------- FTP implementation (via ftp crate) --------
struct FtpClient {
    inner: ftp::FtpStream,
}

impl FtpClient {
    fn connect(url: &RemoteUrl) -> FsResult<Self> {
        let addr = format!("{}:{}", url.host, url.port.unwrap_or(21));
        let mut ftp = ftp::FtpStream::connect(addr)
            .map_err(|e| FsError::Message(format!("FTP connect: {e}")))?;
        // Login (anonymous if not provided)
        match &url.user {
            Some(u) => {
                let pass = url.pass.as_deref().unwrap_or("anonymous@");
                ftp.login(u, pass)
                    .map_err(|e| FsError::Message(format!("FTP login: {e}")))?;
            }
            None => {
                ftp.login("anonymous", "anonymous@")
                    .map_err(|e| FsError::Message(format!("FTP anonymous login: {e}")))?;
            }
        }
        // Use binary for transfers
        let _ = ftp.transfer_type(ftp::types::FileType::Binary);
        Ok(Self { inner: ftp })
    }
}

impl RemoteClient for FtpClient {
    fn list(&mut self, path: &str) -> FsResult<Vec<RemoteEntry>> {
        // Try to change directory; when fails and path is a file we still attempt LIST on parent
        let _ = self.inner.cwd(path);
        let lines = self
            .inner
            .list(Some(path))
            .map_err(|e| FsError::Message(format!("FTP LIST: {e}")))?;
        let mut out = Vec::with_capacity(lines.len());
        for l in lines {
            if let Some(ent) = parse_unix_list_line(&l) {
                out.push(ent);
            }
        }
        Ok(out)
    }
    fn download(&mut self, remote_path: &str, local_path: &Path) -> FsResult<()> {
        let data = self
            .inner
            .simple_retr(remote_path)
            .map_err(|e| FsError::Message(format!("FTP RETR: {e}")))?;
        std::fs::write(local_path, data.into_inner())?;
        Ok(())
    }
    fn upload(&mut self, local_path: &Path, remote_path: &str) -> FsResult<()> {
        let mut f = File::open(local_path)?;
        self.inner
            .put(remote_path, &mut f)
            .map_err(|e| FsError::Message(format!("FTP STOR: {e}")))?;
        Ok(())
    }
    fn remove_file(&mut self, remote_path: &str) -> FsResult<()> {
        self.inner
            .rm(remote_path)
            .map_err(|e| FsError::Message(format!("FTP DELE: {e}")))?;
        Ok(())
    }
    fn remove_dir(&mut self, remote_path: &str) -> FsResult<()> {
        #[allow(deprecated)]
        self.inner
            .rmdir(remote_path)
            .map_err(|e| FsError::Message(format!("FTP RMDIR: {e}")))?;
        Ok(())
    }
    fn mkdir(&mut self, remote_path: &str) -> FsResult<()> {
        self.inner
            .mkdir(remote_path)
            .map_err(|e| FsError::Message(format!("FTP MKD: {e}")))?;
        Ok(())
    }
}

// Parse a typical Unix-style LIST line
fn parse_unix_list_line(line: &str) -> Option<RemoteEntry> {
    // Example: "drwxr-xr-x   2 user group      4096 Jan 01 00:00 dirname"
    let tokens: Vec<&str> = line.split_whitespace().collect();
    if tokens.is_empty() {
        return None;
    }
    let kind = tokens[0].chars().next()?;
    // Heuristic: typical unix format has name as the last token
    let name = tokens.last()?.to_string();
    let is_dir = kind == 'd';
    let size: u64 = tokens
        .get(4)
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    if name == "." || name == ".." {
        return None;
    }
    Some(RemoteEntry { name, is_dir, size })
}

// -------- SFTP implementation (via system `sftp` binary) --------
struct SftpClient {
    // Connection identity; commands are executed per-op in batch mode
    user_at_host: String,
    port: Option<u16>,
}

impl SftpClient {
    fn connect(url: &RemoteUrl) -> FsResult<Self> {
        let user = url.user.clone().unwrap_or_else(whoami::username);
        let id = format!("{user}@{}", url.host);
        Ok(Self {
            user_at_host: id,
            port: url.port,
        })
    }
    fn sftp_cmd(&self) -> Command {
        let mut cmd = Command::new("sftp");
        cmd.arg("-q");
        if let Some(p) = self.port {
            cmd.arg("-o");
            cmd.arg(format!("Port={p}"));
        }
        cmd.arg("-b").arg("-").arg(&self.user_at_host);
        cmd
    }
}

impl RemoteClient for SftpClient {
    fn list(&mut self, path: &str) -> FsResult<Vec<RemoteEntry>> {
        let mut cmd = self.sftp_cmd();
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .map_err(|e| FsError::Message(format!("sftp spawn: {e}")))?;
        {
            let stdin = child
                .stdin
                .as_mut()
                .ok_or_else(|| FsError::Message("sftp stdin".into()))?;
            let _ = writeln!(stdin, "ls -l {}", shell_escape(path));
            let _ = writeln!(stdin, "quit");
        }
        let out = child
            .wait_with_output()
            .map_err(|e| FsError::Message(format!("sftp wait: {e}")))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(FsError::Message(format!("sftp ls: {err}")));
        }
        let stdout = String::from_utf8_lossy(&out.stdout);
        let mut entries = Vec::new();
        for line in stdout.lines() {
            if let Some(ent) = parse_unix_list_line(line) {
                entries.push(ent);
            }
        }
        Ok(entries)
    }
    fn download(&mut self, remote_path: &str, local_path: &Path) -> FsResult<()> {
        let mut cmd = self.sftp_cmd();
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .map_err(|e| FsError::Message(format!("sftp spawn: {e}")))?;
        {
            let stdin = child
                .stdin
                .as_mut()
                .ok_or_else(|| FsError::Message("sftp stdin".into()))?;
            let _ = writeln!(
                stdin,
                "get {} {}",
                shell_escape(remote_path),
                shell_escape_os(local_path.as_os_str())
            );
            let _ = writeln!(stdin, "quit");
        }
        let out = child
            .wait_with_output()
            .map_err(|e| FsError::Message(format!("sftp wait: {e}")))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(FsError::Message(format!("sftp get: {err}")));
        }
        Ok(())
    }
    fn upload(&mut self, local_path: &Path, remote_path: &str) -> FsResult<()> {
        let mut cmd = self.sftp_cmd();
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .map_err(|e| FsError::Message(format!("sftp spawn: {e}")))?;
        {
            let stdin = child
                .stdin
                .as_mut()
                .ok_or_else(|| FsError::Message("sftp stdin".into()))?;
            let _ = writeln!(
                stdin,
                "put {} {}",
                shell_escape_os(local_path.as_os_str()),
                shell_escape(remote_path)
            );
            let _ = writeln!(stdin, "quit");
        }
        let out = child
            .wait_with_output()
            .map_err(|e| FsError::Message(format!("sftp wait: {e}")))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(FsError::Message(format!("sftp put: {err}")));
        }
        Ok(())
    }
    fn remove_file(&mut self, remote_path: &str) -> FsResult<()> {
        let mut cmd = self.sftp_cmd();
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .map_err(|e| FsError::Message(format!("sftp spawn: {e}")))?;
        {
            let stdin = child
                .stdin
                .as_mut()
                .ok_or_else(|| FsError::Message("sftp stdin".into()))?;
            let _ = writeln!(stdin, "rm {}", shell_escape(remote_path));
            let _ = writeln!(stdin, "quit");
        }
        let out = child
            .wait_with_output()
            .map_err(|e| FsError::Message(format!("sftp wait: {e}")))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(FsError::Message(format!("sftp rm: {err}")));
        }
        Ok(())
    }
    fn remove_dir(&mut self, remote_path: &str) -> FsResult<()> {
        let mut cmd = self.sftp_cmd();
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .map_err(|e| FsError::Message(format!("sftp spawn: {e}")))?;
        {
            let stdin = child
                .stdin
                .as_mut()
                .ok_or_else(|| FsError::Message("sftp stdin".into()))?;
            let _ = writeln!(stdin, "rmdir {}", shell_escape(remote_path));
            let _ = writeln!(stdin, "quit");
        }
        let out = child
            .wait_with_output()
            .map_err(|e| FsError::Message(format!("sftp wait: {e}")))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(FsError::Message(format!("sftp rmdir: {err}")));
        }
        Ok(())
    }
    fn mkdir(&mut self, remote_path: &str) -> FsResult<()> {
        let mut cmd = self.sftp_cmd();
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .map_err(|e| FsError::Message(format!("sftp spawn: {e}")))?;
        {
            let stdin = child
                .stdin
                .as_mut()
                .ok_or_else(|| FsError::Message("sftp stdin".into()))?;
            let _ = writeln!(stdin, "mkdir {}", shell_escape(remote_path));
            let _ = writeln!(stdin, "quit");
        }
        let out = child
            .wait_with_output()
            .map_err(|e| FsError::Message(format!("sftp wait: {e}")))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(FsError::Message(format!("sftp mkdir: {err}")));
        }
        Ok(())
    }
}

// -------- FISH implementation (via system `ssh` binary executing shell commands) --------
struct FishClient {
    user_at_host: String,
    port: Option<u16>,
}

impl FishClient {
    fn connect(url: &RemoteUrl) -> FsResult<Self> {
        let user = url.user.clone().unwrap_or_else(whoami::username);
        let id = format!("{user}@{}", url.host);
        Ok(Self {
            user_at_host: id,
            port: url.port,
        })
    }
    fn ssh_cmd(&self) -> Command {
        let mut cmd = Command::new("ssh");
        cmd.arg("-T"); // disable tty allocation, no interactive
        if let Some(p) = self.port {
            cmd.arg("-p").arg(p.to_string());
        }
        cmd.arg(&self.user_at_host);
        cmd
    }
    fn run_simple(&self, remote_sh: &str) -> FsResult<std::process::Output> {
        // Run a single remote shell command via `sh -lc "<remote_sh>"`
        let mut cmd = self.ssh_cmd();
        cmd.arg("sh").arg("-lc").arg(remote_sh);
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        let out = cmd
            .output()
            .map_err(|e| FsError::Message(format!("ssh exec: {e}")))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(FsError::Message(format!("ssh error: {err}")));
        }
        Ok(out)
    }
}

impl RemoteClient for FishClient {
    fn list(&mut self, path: &str) -> FsResult<Vec<RemoteEntry>> {
        // Use POSIX ls -la format; filter '.' and '..'
        let escaped = shell_escape(path);
        let sh =
            format!("LC_ALL=C ls -l {escaped} 2>/dev/null || /bin/ls -l {escaped} 2>/dev/null");
        let out = self.run_simple(&sh)?;
        let stdout = String::from_utf8_lossy(&out.stdout);
        let mut entries = Vec::new();
        for line in stdout.lines() {
            if let Some(ent) = parse_unix_list_line(line) {
                entries.push(ent);
            }
        }
        Ok(entries)
    }
    fn download(&mut self, remote_path: &str, local_path: &Path) -> FsResult<()> {
        // ssh ... "cat <remote_path>" > local
        let mut cmd = self.ssh_cmd();
        let sh = format!("exec cat {}", shell_escape(remote_path));
        cmd.arg("sh").arg("-lc").arg(sh);
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        let out = cmd
            .output()
            .map_err(|e| FsError::Message(format!("ssh download: {e}")))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(FsError::Message(format!("ssh cat failed: {err}")));
        }
        std::fs::write(local_path, &out.stdout)?;
        Ok(())
    }
    fn upload(&mut self, local_path: &Path, remote_path: &str) -> FsResult<()> {
        // Pipe file to remote `cat > path`
        let data = std::fs::read(local_path)?;
        let mut cmd = self.ssh_cmd();
        let sh = format!("exec sh -c 'cat > {}'", shell_escape(remote_path));
        cmd.arg("sh").arg("-lc").arg(sh);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .map_err(|e| FsError::Message(format!("ssh upload spawn: {e}")))?;
        {
            let stdin = child
                .stdin
                .as_mut()
                .ok_or_else(|| FsError::Message("ssh stdin".into()))?;
            stdin
                .write_all(&data)
                .map_err(|e| FsError::Message(format!("ssh upload write: {e}")))?;
        }
        let out = child
            .wait_with_output()
            .map_err(|e| FsError::Message(format!("ssh upload wait: {e}")))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(FsError::Message(format!("ssh upload failed: {err}")));
        }
        Ok(())
    }
    fn remove_file(&mut self, remote_path: &str) -> FsResult<()> {
        let sh = format!("exec rm -f {}", shell_escape(remote_path));
        let _ = self.run_simple(&sh)?;
        Ok(())
    }
    fn remove_dir(&mut self, remote_path: &str) -> FsResult<()> {
        let sh = format!("exec rmdir {}", shell_escape(remote_path));
        let _ = self.run_simple(&sh)?;
        Ok(())
    }
    fn mkdir(&mut self, remote_path: &str) -> FsResult<()> {
        let sh = format!("exec mkdir -p {}", shell_escape(remote_path));
        let _ = self.run_simple(&sh)?;
        Ok(())
    }
}

// -------- SMB implementation (via smb2 crate; async runtime per-call) --------
struct SmbClient {
    url: RemoteUrl,
}

impl SmbClient {
    fn new(url: &RemoteUrl) -> Self {
        Self { url: url.clone() }
    }
    fn split_share_and_inner(&self, full_path: &str) -> FsResult<(String, String)> {
        let mut segments = full_path
            .trim_start_matches('/')
            .split('/')
            .filter(|s| !s.is_empty());
        let share = segments
            .next()
            .ok_or_else(|| FsError::Message("smb path must include a share".into()))?;
        let rem: String = segments.collect::<Vec<_>>().join("/");
        let inner = if rem.is_empty() {
            "/".to_string()
        } else {
            format!("/{}", rem)
        };
        Ok((share.to_string(), inner))
    }
    fn build_config(&self) -> smb2::client::ClientConfig {
        smb2::client::ClientConfig {
            addr: format!("{}:{}", self.url.host, self.url.port.unwrap_or(445)),
            timeout: std::time::Duration::from_secs(10),
            username: self.url.user.clone().unwrap_or_default(),
            password: self.url.pass.clone().unwrap_or_default(),
            domain: String::new(),
            auto_reconnect: false,
            compression: false,
            dfs_enabled: true,
            dfs_target_overrides: std::collections::HashMap::new(),
        }
    }
}

impl RemoteClient for SmbClient {
    fn list(&mut self, path: &str) -> FsResult<Vec<RemoteEntry>> {
        let (share, inner) = self.split_share_and_inner(path)?;
        let config = self.build_config();
        // Create a fresh runtime to run the async API
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| FsError::Message(format!("tokio runtime: {e}")))?;
        rt.block_on(async move {
            let mut client = smb2::client::SmbClient::connect(config)
                .await
                .map_err(|e| FsError::Message(format!("smb connect: {e}")))?;
            let mut tree = client
                .connect_share(&share)
                .await
                .map_err(|e| FsError::Message(format!("smb connect_share({share}): {e}")))?;
            let list = client
                .list_directory(&mut tree, &inner)
                .await
                .map_err(|e| FsError::Message(format!("smb list_directory: {e}")))?;
            let mut out = Vec::with_capacity(list.len());
            for e in list {
                out.push(RemoteEntry {
                    name: e.name.clone(),
                    is_dir: e.is_directory,
                    size: e.size,
                });
            }
            Ok::<Vec<RemoteEntry>, FsError>(out)
        })
    }
    fn download(&mut self, remote_path: &str, local_path: &Path) -> FsResult<()> {
        let (share, inner) = self.split_share_and_inner(remote_path)?;
        let config = self.build_config();
        let parent = local_path.parent().map(PathBuf::from);
        let dst = local_path.to_path_buf();
        // Create a fresh runtime
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| FsError::Message(format!("tokio runtime: {e}")))?;
        rt.block_on(async move {
            let mut client = smb2::client::SmbClient::connect(config)
                .await
                .map_err(|e| FsError::Message(format!("smb connect: {e}")))?;
            let mut tree = client
                .connect_share(&share)
                .await
                .map_err(|e| FsError::Message(format!("smb connect_share({share}): {e}")))?;
            let data = client
                .read_file(&mut tree, &inner)
                .await
                .map_err(|e| FsError::Message(format!("smb read_file: {e}")))?;
            if let Some(p) = parent {
                let _ = std::fs::create_dir_all(p);
            }
            std::fs::write(&dst, &data)?;
            Ok::<(), FsError>(())
        })
    }
    fn upload(&mut self, local_path: &Path, remote_path: &str) -> FsResult<()> {
        let (share, inner) = self.split_share_and_inner(remote_path)?;
        let config = self.build_config();
        let data = std::fs::read(local_path)?;
        // Create a fresh runtime
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| FsError::Message(format!("tokio runtime: {e}")))?;
        rt.block_on(async move {
            let mut client = smb2::client::SmbClient::connect(config)
                .await
                .map_err(|e| FsError::Message(format!("smb connect: {e}")))?;
            let mut tree = client
                .connect_share(&share)
                .await
                .map_err(|e| FsError::Message(format!("smb connect_share({share}): {e}")))?;
            // Use pipelined write for performance and auto-flush semantics
            let _written = client
                .write_file_pipelined(&mut tree, &inner, &data)
                .await
                .map_err(|e| FsError::Message(format!("smb write_file_pipelined: {e}")))?;
            Ok::<(), FsError>(())
        })
    }
    fn remove_file(&mut self, remote_path: &str) -> FsResult<()> {
        let (share, inner) = self.split_share_and_inner(remote_path)?;
        let config = self.build_config();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| FsError::Message(format!("tokio runtime: {e}")))?;
        rt.block_on(async move {
            let mut client = smb2::client::SmbClient::connect(config)
                .await
                .map_err(|e| FsError::Message(format!("smb connect: {e}")))?;
            let mut tree = client
                .connect_share(&share)
                .await
                .map_err(|e| FsError::Message(format!("smb connect_share({share}): {e}")))?;
            client
                .delete_file(&mut tree, &inner)
                .await
                .map_err(|e| FsError::Message(format!("smb delete_file: {e}")))?;
            Ok::<(), FsError>(())
        })
    }
    fn remove_dir(&mut self, remote_path: &str) -> FsResult<()> {
        let (share, inner) = self.split_share_and_inner(remote_path)?;
        let config = self.build_config();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| FsError::Message(format!("tokio runtime: {e}")))?;
        rt.block_on(async move {
            let mut client = smb2::client::SmbClient::connect(config)
                .await
                .map_err(|e| FsError::Message(format!("smb connect: {e}")))?;
            let mut tree = client
                .connect_share(&share)
                .await
                .map_err(|e| FsError::Message(format!("smb connect_share({share}): {e}")))?;
            client
                .delete_directory(&mut tree, &inner)
                .await
                .map_err(|e| FsError::Message(format!("smb delete_directory: {e}")))?;
            Ok::<(), FsError>(())
        })
    }
    fn mkdir(&mut self, remote_path: &str) -> FsResult<()> {
        let (share, inner) = self.split_share_and_inner(remote_path)?;
        let config = self.build_config();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| FsError::Message(format!("tokio runtime: {e}")))?;
        rt.block_on(async move {
            let mut client = smb2::client::SmbClient::connect(config)
                .await
                .map_err(|e| FsError::Message(format!("smb connect: {e}")))?;
            let mut tree = client
                .connect_share(&share)
                .await
                .map_err(|e| FsError::Message(format!("smb connect_share({share}): {e}")))?;
            client
                .create_directory(&mut tree, &inner)
                .await
                .map_err(|e| FsError::Message(format!("smb create_directory: {e}")))?;
            Ok::<(), FsError>(())
        })
    }
}

fn shell_escape(s: &str) -> String {
    // Wrap in single quotes and escape existing single quotes by closing/opening
    if s.is_empty() {
        return "''".to_string();
    }
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}
fn shell_escape_os(s: &OsStr) -> String {
    shell_escape(&s.to_string_lossy())
}

// -------- High-level helpers used by CompositeFs --------
fn with_client<T>(
    url: &RemoteUrl,
    f: impl FnOnce(&mut dyn RemoteClient) -> FsResult<T>,
) -> FsResult<T> {
    match url.scheme {
        RemoteScheme::Ftp => {
            let mut c = FtpClient::connect(url)?;
            f(&mut c)
        }
        RemoteScheme::Sftp => {
            let mut c = SftpClient::connect(url)?;
            f(&mut c)
        }
        RemoteScheme::Fish => {
            let mut c = FishClient::connect(url)?;
            f(&mut c)
        }
        RemoteScheme::Smb => {
            let mut c = SmbClient::new(url);
            f(&mut c)
        }
    }
}

pub fn list_dir(url: &RemoteUrl, _vfs_root: &Path, show_hidden: bool) -> FsResult<Vec<DirEntry>> {
    let mut entries = with_client(url, |c| c.list(&url.path))?;
    let mut out = Vec::with_capacity(entries.len() + 1);
    // Parent marker: navigate to one directory above
    out.push(parent_marker(remote_parent_path(url)));
    for e in entries.drain(..) {
        if !show_hidden && e.name.starts_with('.') {
            continue;
        }
        let mut child = url.clone();
        let p = if url.path.ends_with('/') {
            format!("{}{}", url.path, e.name)
        } else if url.path == "/" {
            format!("/{}", e.name)
        } else {
            format!("{}/{}", url.path, e.name)
        };
        child.path = p;
        out.push(DirEntry {
            name: e.name,
            path: PathBuf::from(child.to_string_path()),
            meta: Metadata {
                is_dir: e.is_dir,
                is_symlink: false,
                is_executable: false,
                size: e.size,
                modified: SystemTime::UNIX_EPOCH,
                permissions: 0,
                owner: None,
                group: None,
            },
        });
    }
    Ok(out)
}

pub fn copy_out(url: &RemoteUrl, dst: &Path) -> FsResult<()> {
    with_client(url, |c| {
        // Ensure parent directory exists
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        c.download(&url.path, dst)
    })
}

pub fn copy_in(src: &Path, url: &RemoteUrl) -> FsResult<()> {
    with_client(url, |c| c.upload(src, &url.path))
}

pub fn remove(url: &RemoteUrl, recursive: bool) -> FsResult<()> {
    with_client(url, |c| {
        if recursive {
            // Best-effort recursive remove: list children and delete depth-first
            // If listing fails, try file remove as a fallback
            if let Ok(children) = c.list(&url.path) {
                for ce in children {
                    let mut child = url.clone();
                    child.path = if url.path.ends_with('/') {
                        format!("{}{}", url.path, ce.name)
                    } else if url.path == "/" {
                        format!("/{}", ce.name)
                    } else {
                        format!("{}/{}", url.path, ce.name)
                    };
                    if ce.is_dir {
                        let _ = remove(&child, true);
                    } else {
                        let _ = c.remove_file(&child.path);
                    }
                }
                // Remove the directory itself
                return c.remove_dir(&url.path);
            }
        }
        // Non-recursive or listing failed: try file, then dir
        match c.remove_file(&url.path) {
            Ok(()) => Ok(()),
            Err(_) => c.remove_dir(&url.path),
        }
    })
}

pub fn mkdir(url: &RemoteUrl) -> FsResult<()> {
    with_client(url, |c| c.mkdir(&url.path))
}

pub fn read_file_to_temp(url: &RemoteUrl) -> FsResult<File> {
    let tmp =
        tempfile::NamedTempFile::new().map_err(|e| FsError::Message(format!("tempfile: {e}")))?;
    let p = tmp.path().to_path_buf();
    // Download to the named temp path
    with_client(url, |c| c.download(&url.path, &p))?;
    // Reopen a handle to return; dropping `tmp` unlinks the path but the handle remains valid
    let f = tmp
        .reopen()
        .map_err(|e| FsError::Message(format!("temp reopen: {e}")))?;
    drop(tmp);
    Ok(f)
}

pub struct RemoteWrite {
    tmp_path: PathBuf,
    url: RemoteUrl,
    file: File,
}

impl RemoteWrite {
    pub fn new(url: RemoteUrl) -> FsResult<Self> {
        let tmp = tempfile::NamedTempFile::new()
            .map_err(|e| FsError::Message(format!("tempfile: {e}")))?;
        let tmp_path = tmp.path().to_path_buf();
        let file = tmp
            .reopen()
            .map_err(|e| FsError::Message(format!("temp reopen: {e}")))?;
        Ok(Self {
            tmp_path,
            url,
            file,
        })
    }
}

impl Write for RemoteWrite {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.file.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

impl Drop for RemoteWrite {
    fn drop(&mut self) {
        // Best-effort upload; errors cannot be surfaced here
        let _ = with_client(&self.url, |c| c.upload(&self.tmp_path, &self.url.path));
        let _ = std::fs::remove_file(&self.tmp_path);
    }
}

// -------- Helpers usable for tests (public for integration tests) --------
pub fn list_dir_with_client(
    url: &RemoteUrl,
    client: &mut dyn RemoteClient,
    show_hidden: bool,
) -> FsResult<Vec<DirEntry>> {
    // Mirror list_dir but using injected client
    let mut entries = client.list(&url.path)?;
    let mut out = Vec::with_capacity(entries.len() + 1);
    out.push(parent_marker(remote_parent_path(url)));
    for e in entries.drain(..) {
        if !show_hidden && e.name.starts_with('.') {
            continue;
        }
        let mut child = url.clone();
        child.path = if url.path == "/" {
            format!("/{}", e.name)
        } else {
            format!("{}/{}", url.path, e.name)
        };
        out.push(DirEntry {
            name: e.name,
            path: PathBuf::from(child.to_string_path()),
            meta: Metadata {
                is_dir: e.is_dir,
                is_symlink: false,
                is_executable: false,
                size: e.size,
                modified: SystemTime::UNIX_EPOCH,
                permissions: 0,
                owner: None,
                group: None,
            },
        });
    }
    Ok(out)
}

pub fn copy_out_with_client(
    url: &RemoteUrl,
    client: &mut dyn RemoteClient,
    dst: &Path,
) -> FsResult<()> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    client.download(&url.path, dst)
}
