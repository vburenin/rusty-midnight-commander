use anyhow::{anyhow, Result};
use std::env;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

/// State for the shell command line and output buffer.
#[derive(Debug, Clone)]
pub struct Subshell {
    /// Current editable command line.
    pub cmdline: String,
    /// History of previously executed commands (most recent last).
    history: Vec<String>,
    /// When navigating history, this is Some(index into history). None when not navigating.
    history_index: Option<usize>,
    /// Combined stdout/stderr output of the last executed command(s), capped in size.
    pub output_lines: Vec<String>,
    /// Whether the subshell/output full-screen view is currently shown (C-o toggle).
    pub show_output_screen: bool,
    /// Scroll offset for output screen: 0 = show newest (bottom). Increases to scroll up.
    pub output_scroll: usize,
    /// Maximum number of lines to retain in `output_lines`.
    max_output_lines: usize,
}

impl Default for Subshell {
    fn default() -> Self {
        Self::new()
    }
}

impl Subshell {
    pub fn new() -> Self {
        Self {
            cmdline: String::new(),
            history: Vec::new(),
            history_index: None,
            output_lines: Vec::new(),
            show_output_screen: false,
            output_scroll: 0,
            max_output_lines: 10_000,
        }
    }

    /// Append a filename into the cmdline, adding a separating space if needed and quoting if necessary.
    pub fn append_filename(&mut self, name: &str) {
        if !self.cmdline.is_empty() && !self.cmdline.ends_with(' ') {
            self.cmdline.push(' ');
        }
        self.cmdline.push_str(&shell_quote(name));
    }

    /// Move to previous history entry (older). Returns the new cmdline to show.
    pub fn history_prev(&mut self) -> Option<String> {
        if self.history.is_empty() {
            return None;
        }
        let new_index = match self.history_index {
            None => self.history.len().saturating_sub(1),
            Some(0) => 0,
            Some(i) => i.saturating_sub(1),
        };
        self.history_index = Some(new_index);
        self.history.get(new_index).cloned()
    }

    /// Move to next history entry (newer). Returns the new cmdline to show.
    pub fn history_next(&mut self) -> Option<String> {
        if self.history.is_empty() {
            return None;
        }
        let new_index = match self.history_index {
            None => return None,
            Some(i) if i + 1 >= self.history.len() => {
                // Past newest: clear nav and return empty to allow editing a fresh command
                self.history_index = None;
                return Some(String::new());
            }
            Some(i) => i + 1,
        };
        self.history_index = Some(new_index);
        self.history.get(new_index).cloned()
    }

    /// Clear history navigation state (called when editing/typing).
    pub fn clear_history_nav(&mut self) {
        self.history_index = None;
    }

    /// Execute the current command line using the user's shell.
    /// - Uses $SHELL or falls back to /bin/sh
    /// - Runs with `current_dir` set to `cwd`
    /// - Captures combined stdout+stderr into output_lines
    pub fn execute_current(&mut self, cwd: &Path) -> Result<ExecOutcome> {
        let cmd_owned = self.cmdline.trim().to_string();
        if cmd_owned.is_empty() {
            return Ok(ExecOutcome {
                exit_code: 0,
                output_collected: false,
            });
        }
        // Determine shell to use
        let shell_path = env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        let child = Command::new(shell_path)
            .arg("-c")
            .arg(&cmd_owned)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| anyhow!("failed to spawn shell for command '{}': {}", cmd_owned, e))?;
        let output = child
            .wait_with_output()
            .map_err(|e| anyhow!("failed to wait for command '{}': {}", cmd_owned, e))?;
        let status = output.status;
        let mut combined = Vec::new();
        combined.extend_from_slice(&output.stdout);
        if !output.stderr.is_empty() {
            // Add newline if stdout didn't end with it and both streams have content
            if !combined.is_empty() && !combined.ends_with(b"\n") {
                combined.push(b'\n');
            }
            combined.extend_from_slice(&output.stderr);
        }
        self.append_output_bytes(&combined);
        // Record history (avoid duplicate consecutive entries)
        if self.history.last() != Some(&cmd_owned) {
            self.history.push(cmd_owned);
        }
        self.history_index = None;
        Ok(ExecOutcome {
            exit_code: status.code().unwrap_or_default(),
            output_collected: true,
        })
    }

    fn append_output_bytes(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let text = String::from_utf8_lossy(bytes);
        for line in text.split_inclusive('\n') {
            let l = line.to_string();
            // Ensure lines end without trailing CR; keep '\n' handling simple.
            if let Some(stripped) = l.strip_suffix('\n') {
                self.output_lines.push(stripped.replace('\r', ""));
            } else {
                self.output_lines.push(l.replace('\r', ""));
            }
            if self.output_lines.len() > self.max_output_lines {
                let overflow = self.output_lines.len() - self.max_output_lines;
                self.output_lines.drain(0..overflow);
            }
        }
    }

    /// Clear the current command line (after execution or cancel).
    pub fn clear_cmdline(&mut self) {
        self.cmdline.clear();
        self.history_index = None;
    }

    /// Toggle the output full-screen flag.
    pub fn toggle_output_screen(&mut self) {
        self.show_output_screen = !self.show_output_screen;
        if self.show_output_screen {
            // Reset scroll to bottom when opening.
            self.output_scroll = 0;
        }
    }

    /// Set whether the output full-screen is shown.
    pub fn set_output_screen(&mut self, val: bool) {
        self.show_output_screen = val;
    }

    /// Returns an iterator over the tail of output lines limited to `max_rows`.
    pub fn tail(&self, max_rows: usize) -> impl Iterator<Item = &String> {
        let len = self.output_lines.len();
        let start = len.saturating_sub(max_rows);
        self.output_lines[start..].iter()
    }

    /// Returns a window of lines honoring `output_scroll`, sized to `max_rows`.
    pub fn window(&self, max_rows: usize) -> &[String] {
        let len = self.output_lines.len();
        if len == 0 || max_rows == 0 {
            return &[];
        }
        // Compute bottom-aligned window, then apply scroll up by output_scroll lines.
        let base_start = len.saturating_sub(max_rows);
        // Maximum scroll we can apply before window goes past top
        let max_scroll = base_start;
        let applied = self.output_scroll.min(max_scroll);
        let start = base_start.saturating_sub(applied);
        let end = len;
        let slice = &self.output_lines[start..end];
        // If slice longer than max_rows (can happen when start is 0 and len > max_rows), take last max_rows.
        let take_start = slice.len().saturating_sub(max_rows);
        &slice[take_start..]
    }

    pub fn scroll_page_up(&mut self, rows: usize) {
        self.output_scroll = self.output_scroll.saturating_add(rows);
    }

    pub fn scroll_page_down(&mut self, rows: usize) {
        self.output_scroll = self.output_scroll.saturating_sub(rows);
    }

    /// Optionally execute the current command line inside a live PTY session.
    /// - If `pty` is Some and alive, writes the command + newline into the PTY and records the
    ///   command in history (without waiting for completion).
    /// - If `pty` is None or not alive, falls back to `execute_current`.
    ///
    /// This is a CORE-only helper; UI wiring to display live PTY output is done elsewhere.
    pub fn execute_in_pty(
        &mut self,
        cwd: &Path,
        pty: Option<&mut PtySession>,
    ) -> Result<ExecOutcome> {
        let cmd_owned = self.cmdline.trim().to_string();
        if cmd_owned.is_empty() {
            return Ok(ExecOutcome {
                exit_code: 0,
                output_collected: false,
            });
        }
        if let Some(session) = pty {
            if session.is_alive() {
                // Best effort: ensure session is at the requested cwd by emitting `cd` first
                // if current dir differs. This is kept simple; the UI may track cwd explicitly.
                if let Some(cur) = session.current_dir_hint() {
                    if cur != cwd {
                        let cd_line = format!("cd {}\n", shell_quote(&cwd.to_string_lossy()));
                        let _ = session.write(cd_line.as_bytes());
                        // Give the shell a tiny moment to process `cd`
                        session.drain_output();
                    }
                }
                let line = format!("{}\n", cmd_owned);
                session.write(line.as_bytes())?;
                // Record history (avoid duplicate consecutive entries)
                if self.history.last() != Some(&cmd_owned) {
                    self.history.push(cmd_owned);
                }
                self.history_index = None;
                // We do not synchronously collect output here; it's available via the PTY session.
                return Ok(ExecOutcome {
                    exit_code: 0,
                    output_collected: false,
                });
            }
        }
        self.execute_current(cwd)
    }
}

/// Result of executing a command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecOutcome {
    pub exit_code: i32,
    /// true when some output was collected (stdout or stderr)
    pub output_collected: bool,
}

fn shell_quote(name: &str) -> String {
    // Simple POSIX-ish single-quote quoting.
    // Replace each single quote with '\'' sequence.
    if name.is_empty() {
        "''".to_string()
    } else if name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '/' || c == '-')
    {
        name.to_string()
    } else {
        let mut s = String::from("'");
        for ch in name.chars() {
            if ch == '\'' {
                s.push_str("'\\''");
            } else {
                s.push(ch);
            }
        }
        s.push('\'');
        s
    }
}

// ================================
// PTY session (persistent subshell)
// ================================
//
// This is a minimal, standalone PTY-backed shell session intended for integration by rmc-ui later.
// It keeps a child $SHELL running under a PTY, supports writing input, draining collected output,
// resizing, and termination checks.
//
// Notes:
// - We purposefully avoid changing the Subshell struct API used by the UI today.
// - Output is collected on a dedicated reader thread and buffered for non-blocking drains.

pub struct PtySession {
    master: Box<dyn portable_pty::MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send>,
    writer: Box<dyn std::io::Write + Send>,
    output: Arc<Mutex<Vec<u8>>>,
    // Best-effort current dir tracking; updated only via hints in spawn().
    cwd_hint: Option<std::path::PathBuf>,
}

impl PtySession {
    /// Spawn a persistent $SHELL (or /bin/sh) under a PTY.
    /// - `cwd`: working directory to start in
    /// - `rows`, `cols`: initial terminal size
    pub fn spawn(cwd: &Path, rows: u16, cols: u16) -> Result<Self> {
        let pty_system = portable_pty::native_pty_system();
        let pair = pty_system
            .openpty(portable_pty::PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| anyhow!("failed to open PTY: {}", e))?;

        let shell_path = env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        let mut cmd = portable_pty::CommandBuilder::new(shell_path);
        // Request an interactive shell; many shells auto-detect TTY and become interactive,
        // but explicit -i improves compatibility without affecting `echo` in tests.
        cmd.arg("-i");
        cmd.cwd(cwd);
        cmd.env("TERM", "xterm-256color");

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| anyhow!("failed to spawn shell in PTY: {}", e))?;
        drop(pair.slave);

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| anyhow!("failed to clone PTY reader: {}", e))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| anyhow!("failed to take PTY writer: {}", e))?;

        let output = Arc::new(Mutex::new(Vec::<u8>::new()));
        let output_clone = Arc::clone(&output);
        // Spawn a blocking reader thread that appends to the shared buffer.
        thread::spawn(move || {
            let mut reader = reader;
            let mut buf = [0u8; 8192];
            loop {
                match std::io::Read::read(&mut reader, &mut buf) {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        if let Ok(mut guard) = output_clone.lock() {
                            guard.extend_from_slice(&buf[..n]);
                        }
                    }
                    Err(_) => {
                        // On read error, exit the thread; consumer side can decide how to proceed.
                        break;
                    }
                }
            }
        });

        Ok(Self {
            master: pair.master,
            child,
            writer,
            output,
            cwd_hint: Some(cwd.to_path_buf()),
        })
    }

    /// Non-blocking: drain any bytes collected from the PTY since the last call.
    pub fn drain_output(&self) -> Vec<u8> {
        let mut guard = self.output.lock().expect("output lock poisoned");
        if guard.is_empty() {
            return Vec::new();
        }
        guard.split_off(0)
    }

    /// Write raw bytes to the PTY (e.g., keystrokes or "cmd\n").
    pub fn write(&mut self, bytes: &[u8]) -> Result<()> {
        use std::io::Write;
        self.writer
            .write_all(bytes)
            .map_err(|e| anyhow!("pty write failed: {}", e))?;
        self.writer
            .flush()
            .map_err(|e| anyhow!("pty flush failed: {}", e))?;
        Ok(())
    }

    /// Resize the PTY.
    pub fn resize(&mut self, rows: u16, cols: u16) -> Result<()> {
        self.master
            .resize(portable_pty::PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| anyhow!("pty resize failed: {}", e))
    }

    /// Return true if the child shell appears to be alive.
    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Attempt to terminate the shell.
    pub fn kill(&mut self) -> Result<()> {
        self.child
            .kill()
            .map_err(|e| anyhow!("failed to kill pty child: {}", e))
    }

    /// Best-effort hint of the cwd initially requested at spawn time.
    pub fn current_dir_hint(&self) -> Option<&Path> {
        self.cwd_hint.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn executes_true_successfully() {
        let dir = tempdir().unwrap();
        let cwd = dir.path();
        let mut ss = Subshell::new();
        ss.cmdline = "true".to_string();
        let out = ss.execute_current(cwd).unwrap();
        assert_eq!(out.exit_code, 0);
    }

    #[test]
    fn executes_echo_and_captures_output() {
        let dir = tempdir().unwrap();
        let cwd = dir.path();
        let mut ss = Subshell::new();
        ss.cmdline = "echo hello-world".to_string();
        let _ = ss.execute_current(cwd).unwrap();
        let got = ss.output_lines.join("\n");
        assert!(got.contains("hello-world"), "output: {}", got);
    }

    #[test]
    fn executes_in_specified_cwd() {
        let dir = tempdir().unwrap();
        let cwd = dir.path();
        // create a file and list it
        let p = cwd.join("xfile.txt");
        fs::write(&p, "x").unwrap();
        let mut ss = Subshell::new();
        // Use 'pwd' to verify cwd, and 'ls' to ensure relative resolution.
        ss.cmdline = "pwd; ls -1".to_string();
        let _ = ss.execute_current(cwd).unwrap();
        let got = ss.output_lines.join("\n");
        assert!(got.contains(cwd.to_string_lossy().as_ref()), "pwd: {}", got);
        assert!(got.contains("xfile.txt"), "ls output missing file: {}", got);
    }

    #[test]
    fn pty_session_echoes_text() {
        // If PTY allocation is not possible in this environment, just skip.
        use std::time::{Duration, Instant};
        let dir = match tempdir() {
            Ok(d) => d,
            Err(_) => return,
        };
        let cwd = dir.path();
        // Try to spawn; skip if not available (e.g. restricted CI containers)
        let mut sess = match PtySession::spawn(cwd, 24, 80) {
            Ok(s) => s,
            Err(_) => return,
        };
        // Send echo command
        let _ = sess.write(b"echo pty-hello\n");
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut buf = String::new();
        while Instant::now() < deadline {
            let bytes = sess.drain_output();
            if !bytes.is_empty() {
                buf.push_str(&String::from_utf8_lossy(&bytes));
                if buf.contains("pty-hello") {
                    break;
                }
            }
            thread::sleep(Duration::from_millis(50));
        }
        // Be best-effort on cleanup.
        let _ = sess.kill();
        // Either we saw the string, or the environment prevented it; if the latter,
        // treat as skipped by early-returning above.
        assert!(
            buf.contains("pty-hello"),
            "PTY did not echo expected output; got: {}",
            buf
        );
    }
}
