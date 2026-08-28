use anyhow::{anyhow, Result};
use std::env;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use users::os::unix::UserExt;

/// GNU mc(1) “The subshell support”: the concurrent subshell (and command-line
/// `-c` execution) is the binary in `$SHELL`; if that is unset or empty, the
/// login shell from `/etc/passwd`; last resort `/bin/sh`.
///
/// GNU documents the invocation override as `SHELL=/bin/myshell mc` — there is
/// no dedicated CLI flag for the shell binary (`-U`/`-u` only enable/disable
/// the concurrent PTY).
pub fn resolve_user_shell() -> PathBuf {
    resolve_user_shell_with(
        env::var_os("SHELL").map(PathBuf::from),
        login_shell_from_passwd(),
    )
}

/// Select a subshell binary with GNU mc(1) precedence. Empty `$SHELL` is treated
/// as unset (an empty path cannot be exec'd).
pub fn resolve_user_shell_with(
    shell_env: Option<PathBuf>,
    passwd_shell: Option<PathBuf>,
) -> PathBuf {
    if let Some(p) = shell_env {
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    if let Some(p) = passwd_shell {
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    PathBuf::from("/bin/sh")
}

fn login_shell_from_passwd() -> Option<PathBuf> {
    let user = users::get_user_by_uid(users::get_current_uid())?;
    let shell = user.shell();
    if shell.as_os_str().is_empty() {
        None
    } else {
        Some(shell.to_path_buf())
    }
}

/// GNU mc(1) concurrent subshell works with bash, ash (BusyBox and Debian),
/// (o/m)ksh, tcsh, zsh and fish. Those (and POSIX `sh`/`dash`) accept `-i`.
/// Custom `SHELL=/bin/myshell` overrides are spawned without extra flags so a
/// wrapper that does not implement `-i` still attaches.
fn wants_interactive_flag(shell: &Path) -> bool {
    let name = shell.file_name().and_then(OsStr::to_str).unwrap_or("");
    matches!(
        name,
        "bash"
            | "rbash"
            | "zsh"
            | "fish"
            | "tcsh"
            | "csh"
            | "ksh"
            | "mksh"
            | "oksh"
            | "pdksh"
            | "ksh93"
            | "dash"
            | "ash"
            | "sh"
            | "busybox"
    )
}

/// State for the shell command line and output buffer.
#[derive(Debug, Clone)]
pub struct Subshell {
    /// Current editable command line.
    pub cmdline: String,
    /// Insertion point in `cmdline` as a Unicode scalar count (0..=char len).
    cursor: usize,
    /// Last killed text (process lifetime; last non-empty kill wins).
    kill_buffer: String,
    /// History of previously executed commands (most recent last).
    history: Vec<String>,
    /// When navigating history, this is Some(index into history). None when not navigating.
    history_index: Option<usize>,
    /// Uncommitted command line saved when M-p starts history walking.
    history_draft: Option<String>,
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
            cursor: 0,
            kill_buffer: String::new(),
            history: Vec::new(),
            history_index: None,
            history_draft: None,
            output_lines: Vec::new(),
            show_output_screen: false,
            output_scroll: 0,
            max_output_lines: 10_000,
        }
    }

    /// Replace the command line and put the cursor at the end.
    pub fn replace_cmdline(&mut self, s: String) {
        self.cmdline = s;
        self.cursor = self.cmdline.chars().count();
    }

    /// Insertion point as a Unicode scalar count (`0..=cmdline.chars().count()`).
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    fn char_len(&self) -> usize {
        self.cmdline.chars().count()
    }

    fn byte_index_of(&self, char_idx: usize) -> usize {
        self.cmdline
            .chars()
            .take(char_idx)
            .map(|c| c.len_utf8())
            .sum()
    }

    fn byte_index_at_cursor(&self) -> usize {
        self.byte_index_of(self.cursor)
    }

    /// Move the insertion point to the start of the line.
    pub fn move_home(&mut self) {
        self.cursor = 0;
    }

    /// Move the insertion point to the end of the line.
    pub fn move_end(&mut self) {
        self.cursor = self.char_len();
    }

    /// Move back one Unicode scalar (no-op at the start).
    pub fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    /// Move forward one Unicode scalar (no-op at the end).
    pub fn move_right(&mut self) {
        let n = self.char_len();
        if self.cursor < n {
            self.cursor += 1;
        }
    }

    /// Move to the start of the previous non-whitespace run.
    pub fn move_word_left(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let chars: Vec<char> = self.cmdline.chars().collect();
        let mut i = self.cursor.min(chars.len());
        while i > 0 && chars[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !chars[i - 1].is_whitespace() {
            i -= 1;
        }
        self.cursor = i;
    }

    /// Move to the end of the next non-whitespace run.
    pub fn move_word_right(&mut self) {
        let chars: Vec<char> = self.cmdline.chars().collect();
        let n = chars.len();
        let mut i = self.cursor.min(n);
        while i < n && chars[i].is_whitespace() {
            i += 1;
        }
        while i < n && !chars[i].is_whitespace() {
            i += 1;
        }
        self.cursor = i;
    }

    /// Delete the Unicode scalar under the cursor (no-op at end of line).
    pub fn delete_char(&mut self) {
        if self.cursor >= self.char_len() {
            return;
        }
        let start = self.byte_index_at_cursor().min(self.cmdline.len());
        let Some(ch) = self.cmdline[start..].chars().next() else {
            return;
        };
        let end = start + ch.len_utf8();
        self.cmdline.replace_range(start..end, "");
    }

    fn kill_char_range(&mut self, from_char: usize, to_char: usize) {
        let n = self.char_len();
        let from = from_char.min(to_char).min(n);
        let to = from_char.max(to_char).min(n);
        if from == to {
            return;
        }
        let a = self.byte_index_of(from);
        let b = self.byte_index_of(to);
        if a >= b
            || b > self.cmdline.len()
            || !self.cmdline.is_char_boundary(a)
            || !self.cmdline.is_char_boundary(b)
        {
            return;
        }
        self.kill_buffer = self.cmdline[a..b].to_string();
        self.cmdline.replace_range(a..b, "");
        self.cursor = from;
    }

    /// Kill from the cursor to the end of the line into the kill buffer.
    pub fn kill_to_end(&mut self) {
        let end = self.char_len();
        self.kill_char_range(self.cursor, end);
    }

    /// Kill the whole command line into the kill buffer.
    pub fn kill_whole_line(&mut self) {
        let end = self.char_len();
        self.kill_char_range(0, end);
    }

    /// Kill the previous word into the kill buffer.
    pub fn kill_prev_word(&mut self) {
        let end = self.cursor;
        self.move_word_left();
        let start = self.cursor;
        self.kill_char_range(start, end);
    }

    /// Kill the next word into the kill buffer.
    pub fn kill_next_word(&mut self) {
        let start = self.cursor;
        self.move_word_right();
        let end = self.cursor;
        self.kill_char_range(start, end);
    }

    /// Insert the kill buffer at the cursor.
    pub fn yank(&mut self) {
        let buf = self.kill_buffer.clone();
        self.insert_str(&buf);
    }

    /// Text to the left of the insertion point (completion looks at this).
    pub fn text_before_cursor(&self) -> &str {
        let i = self.byte_index_at_cursor().min(self.cmdline.len());
        &self.cmdline[..i]
    }

    /// Replace `cmdline[from_byte..cursor]` with `text` and put the cursor after it.
    pub fn replace_range_before_cursor(&mut self, from_byte: usize, text: &str) {
        let end = self.byte_index_at_cursor().min(self.cmdline.len());
        let from = from_byte.min(end);
        self.cmdline.replace_range(from..end, text);
        self.cursor = self.cmdline[..from].chars().count() + text.chars().count();
    }

    /// Insert `s` at the cursor.
    pub fn insert_str(&mut self, s: &str) {
        let i = self.byte_index_at_cursor().min(self.cmdline.len());
        self.cmdline.insert_str(i, s);
        self.cursor += s.chars().count();
    }

    /// Insert one character at the cursor.
    pub fn insert_char(&mut self, c: char) {
        let mut buf = [0u8; 4];
        self.insert_str(c.encode_utf8(&mut buf));
    }

    /// Delete the character before the cursor.
    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.cursor -= 1;
        let start = self.byte_index_at_cursor();
        let ch_len = self.cmdline[start..]
            .chars()
            .next()
            .map(|c| c.len_utf8())
            .unwrap_or(0);
        self.cmdline.replace_range(start..start + ch_len, "");
    }

    /// Insert a filename (or path) at the cursor, adding a separating space if needed
    /// and quoting when the name contains spaces or other specials.
    pub fn append_filename(&mut self, name: &str) {
        if self.cursor > 0 {
            let prev = self.cmdline.chars().nth(self.cursor.saturating_sub(1));
            if prev != Some(' ') {
                self.insert_str(" ");
            }
        }
        self.insert_str(&shell_quote(name));
    }

    /// Move to previous history entry (older). Returns the new cmdline to show.
    pub fn history_prev(&mut self) -> Option<String> {
        if self.history.is_empty() {
            return None;
        }
        let new_index = match self.history_index {
            None => {
                self.history_draft = Some(self.cmdline.clone());
                self.history.len().saturating_sub(1)
            }
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
                // Past newest: restore the uncommitted line (or empty).
                self.history_index = None;
                return Some(self.history_draft.take().unwrap_or_default());
            }
            Some(i) => i + 1,
        };
        self.history_index = Some(new_index);
        self.history.get(new_index).cloned()
    }

    /// Clear history navigation state (called when editing/typing).
    pub fn clear_history_nav(&mut self) {
        self.history_index = None;
        self.history_draft = None;
    }

    /// Previously executed commands, oldest first (most recent last).
    pub fn history(&self) -> &[String] {
        &self.history
    }

    /// Number of recorded commands.
    pub fn history_len(&self) -> usize {
        self.history.len()
    }

    /// Wipe the command history and any Up/Down navigation state.
    pub fn clear_history(&mut self) {
        self.history.clear();
        self.history_index = None;
        self.history_draft = None;
    }

    /// Record `cmd` in history, skipping empty lines and consecutive duplicates.
    fn remember_command(&mut self, cmd: &str) {
        if cmd.is_empty() {
            return;
        }
        if self.history.last().map(String::as_str) != Some(cmd) {
            self.history.push(cmd.to_string());
        }
        self.history_index = None;
        self.history_draft = None;
    }

    /// Execute the current command line using the GNU-selected user shell.
    ///
    /// Uses [`resolve_user_shell`] (`$SHELL`, else passwd login shell, else
    /// `/bin/sh`) with `-c`, `current_dir` set to `cwd`, and combined
    /// stdout+stderr captured into `output_lines`.
    pub fn execute_current(&mut self, cwd: &Path) -> Result<ExecOutcome> {
        self.execute_with_shell(cwd, &resolve_user_shell())
    }

    fn execute_with_shell(&mut self, cwd: &Path, shell: &Path) -> Result<ExecOutcome> {
        let cmd_owned = self.cmdline.trim().to_string();
        if cmd_owned.is_empty() {
            return Ok(ExecOutcome {
                exit_code: 0,
                output_collected: false,
            });
        }
        let child = Command::new(shell)
            .arg("-c")
            .arg(&cmd_owned)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                anyhow!(
                    "failed to spawn shell '{}' for command '{}': {}",
                    shell.display(),
                    cmd_owned,
                    e
                )
            })?;
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
        self.remember_command(&cmd_owned);
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
        self.cursor = 0;
        self.history_index = None;
        self.history_draft = None;
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
                self.remember_command(&cmd_owned);
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

/// POSIX-ish single-quote quoting for command-line filename helpers.
pub fn shell_quote(name: &str) -> String {
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
    cwd_hint: Option<PathBuf>,
    /// Binary exec'd for this session ([`resolve_user_shell`] at spawn).
    shell_path: PathBuf,
}

impl PtySession {
    /// Spawn a persistent GNU-selected user shell under a PTY (C-o when `-U`).
    ///
    /// Uses [`resolve_user_shell`] so `SHELL=/bin/myshell mcr` overrides the
    /// passwd login shell and the `/bin/sh` last resort. Known interactive
    /// shells (bash, zsh, fish, tcsh, …) get `-i`; custom override binaries
    /// do not.
    ///
    /// - `cwd`: working directory to start in
    /// - `rows`, `cols`: initial terminal size
    pub fn spawn(cwd: &Path, rows: u16, cols: u16) -> Result<Self> {
        Self::spawn_with_shell(&resolve_user_shell(), cwd, rows, cols)
    }

    fn spawn_with_shell(shell: &Path, cwd: &Path, rows: u16, cols: u16) -> Result<Self> {
        let pty_system = portable_pty::native_pty_system();
        let pair = pty_system
            .openpty(portable_pty::PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| anyhow!("failed to open PTY: {}", e))?;

        let mut cmd = portable_pty::CommandBuilder::new(shell);
        // Known GNU mc(1) subshell binaries accept `-i`; a custom SHELL override
        // may not. A PTY already makes bash/zsh/fish/tcsh interactive.
        if wants_interactive_flag(shell) {
            cmd.arg("-i");
        }
        cmd.cwd(cwd);
        cmd.env("TERM", "xterm-256color");
        cmd.env("SHELL", shell.as_os_str());

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
            shell_path: shell.to_path_buf(),
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

    /// Shell binary exec'd for this PTY session.
    pub fn shell_path(&self) -> &Path {
        &self.shell_path
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

    fn seed_history(ss: &mut Subshell, cwd: &Path, cmds: &[&str]) {
        for cmd in cmds {
            ss.cmdline = cmd.to_string();
            ss.execute_current(cwd).unwrap();
            ss.clear_cmdline();
        }
    }

    #[test]
    fn history_records_commands_and_clear_empties() {
        let dir = tempdir().unwrap();
        let cwd = dir.path();
        let mut ss = Subshell::new();
        seed_history(&mut ss, cwd, &["echo one", "echo two", "echo three"]);
        assert_eq!(
            ss.history(),
            &[
                "echo one".to_string(),
                "echo two".into(),
                "echo three".into()
            ]
        );
        assert_eq!(ss.history_len(), 3);

        ss.clear_history();
        assert!(ss.history().is_empty());
        assert_eq!(ss.history_len(), 0);
        assert!(ss.history_prev().is_none());
        assert!(ss.history_next().is_none());

        seed_history(&mut ss, cwd, &["echo after-clear"]);
        assert_eq!(ss.history(), &["echo after-clear".to_string()]);
        assert_eq!(ss.history_prev().as_deref(), Some("echo after-clear"));
        // At the only entry; next past newest yields a fresh empty line.
        assert_eq!(ss.history_next().as_deref(), Some(""));
        assert!(ss.history_next().is_none());
    }

    #[test]
    fn history_prev_saves_draft_and_next_restores_it() {
        let dir = tempdir().unwrap();
        let cwd = dir.path();
        let mut ss = Subshell::new();
        seed_history(&mut ss, cwd, &["echo a"]);
        ss.cmdline = "echo b".to_string();
        assert_eq!(ss.history_prev().as_deref(), Some("echo a"));
        assert_eq!(ss.history_next().as_deref(), Some("echo b"));
    }

    #[test]
    fn append_filename_quotes_spaces() {
        let mut ss = Subshell::new();
        ss.append_filename("hello world.txt");
        assert_eq!(ss.cmdline, "'hello world.txt'");
        ss.append_filename("plain");
        assert_eq!(ss.cmdline, "'hello world.txt' plain");
    }

    #[test]
    fn history_skips_consecutive_duplicates() {
        let dir = tempdir().unwrap();
        let cwd = dir.path();
        let mut ss = Subshell::new();
        seed_history(&mut ss, cwd, &["echo same", "echo same", "echo other"]);
        assert_eq!(
            ss.history(),
            &["echo same".to_string(), "echo other".into()]
        );
    }

    #[test]
    fn emacs_moves_and_kills_ascii_and_utf8() {
        let mut ss = Subshell::new();
        for c in "hello world".chars() {
            ss.insert_char(c);
        }
        assert_eq!(ss.cursor(), 11);
        ss.move_home();
        assert_eq!(ss.cursor(), 0);
        ss.move_end();
        assert_eq!(ss.cursor(), 11);
        ss.move_left();
        assert_eq!(ss.cursor(), 10);
        ss.move_right();
        assert_eq!(ss.cursor(), 11);

        ss.move_word_left();
        assert_eq!(ss.cursor(), 6);
        ss.move_word_left();
        assert_eq!(ss.cursor(), 0);
        ss.move_word_right();
        assert_eq!(ss.cursor(), 5);
        ss.move_word_right();
        assert_eq!(ss.cursor(), 11);

        ss.move_home();
        for _ in 0..5 {
            ss.move_right();
        }
        ss.kill_to_end();
        assert_eq!(ss.cmdline, "hello");
        assert_eq!(ss.kill_buffer, " world");
        ss.yank();
        assert_eq!(ss.cmdline, "hello world");

        ss.kill_whole_line();
        assert!(ss.cmdline.is_empty());
        assert_eq!(ss.cursor(), 0);
        assert_eq!(ss.kill_buffer, "hello world");
        ss.yank();
        assert_eq!(ss.cmdline, "hello world");

        ss.move_home();
        ss.move_word_right(); // after "hello"
        ss.kill_next_word();
        assert_eq!(ss.cmdline, "hello");
        assert_eq!(ss.kill_buffer, " world");
        ss.yank();
        ss.kill_prev_word();
        assert_eq!(ss.cmdline, "hello ");
        assert_eq!(ss.kill_buffer, "world");

        ss.replace_cmdline("café".into());
        ss.move_left();
        assert_eq!(ss.cursor(), 3);
        ss.delete_char();
        assert_eq!(ss.cmdline, "caf");
        ss.insert_char('é');
        ss.backspace();
        assert_eq!(ss.cmdline, "caf");
        ss.delete_char();
        assert_eq!(ss.cmdline, "caf");
        assert_eq!(ss.cursor(), 3);
    }

    #[test]
    fn resolve_user_shell_env_wins_over_passwd_and_bin_sh() {
        let override_path = PathBuf::from("/bin/myshell");
        let passwd = PathBuf::from("/usr/bin/zsh");
        assert_eq!(
            resolve_user_shell_with(Some(override_path.clone()), Some(passwd.clone())),
            override_path,
            "GNU invocation override: SHELL=/bin/myshell mcr"
        );
        assert_eq!(
            resolve_user_shell_with(None, Some(passwd.clone())),
            passwd,
            "unset SHELL uses passwd login shell, not /bin/sh"
        );
        assert_eq!(
            resolve_user_shell_with(Some(PathBuf::new()), Some(passwd.clone())),
            passwd,
            "empty SHELL is treated as unset"
        );
        assert_eq!(
            resolve_user_shell_with(None, None),
            PathBuf::from("/bin/sh")
        );
        assert_eq!(
            resolve_user_shell_with(None, Some(PathBuf::new())),
            PathBuf::from("/bin/sh")
        );
    }

    #[test]
    fn resolve_user_shell_live_env_wins_over_hardcoded_bin_sh() {
        match env::var_os("SHELL") {
            Some(s) if !s.is_empty() => {
                let got = resolve_user_shell();
                assert_eq!(got, PathBuf::from(&s));
                if Path::new(&s) != Path::new("/bin/sh") {
                    assert_ne!(
                        got,
                        PathBuf::from("/bin/sh"),
                        "$SHELL must win over the /bin/sh last resort"
                    );
                }
            }
            _ => {
                let got = resolve_user_shell();
                if let Some(passwd) = login_shell_from_passwd() {
                    assert_eq!(got, passwd);
                } else {
                    assert_eq!(got, PathBuf::from("/bin/sh"));
                }
            }
        }
    }

    fn write_recording_shell(dir: &Path, name: &str) -> (PathBuf, PathBuf) {
        let shell = dir.join(name);
        let argv_log = dir.join(format!("{name}.argv"));
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$0\" \"$@\" > '{}'\nexec /bin/sh \"$@\"\n",
            argv_log.display()
        );
        fs::write(&shell, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&shell).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&shell, perms).unwrap();
        }
        (shell, argv_log)
    }

    fn wait_for_argv_log(path: &Path) -> String {
        use std::time::{Duration, Instant};
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Ok(s) = fs::read_to_string(path) {
                if !s.trim().is_empty() {
                    return s;
                }
            }
            thread::sleep(Duration::from_millis(20));
        }
        fs::read_to_string(path).unwrap_or_default()
    }

    #[test]
    fn execute_with_shell_override_wins_over_default() {
        let dir = tempdir().unwrap();
        let cwd = dir.path();
        let (wrapper, argv_log) = write_recording_shell(cwd, "myshell");
        let mut ss = Subshell::new();
        ss.cmdline = "echo override-c-exec".to_string();
        let out = ss.execute_with_shell(cwd, &wrapper).unwrap();
        assert_eq!(out.exit_code, 0);
        let got = ss.output_lines.join("\n");
        assert!(got.contains("override-c-exec"), "output: {got}");
        let logged = fs::read_to_string(&argv_log).unwrap_or_default();
        assert!(
            logged.contains(wrapper.file_name().unwrap().to_str().unwrap()),
            "must exec SHELL override, not /bin/sh; argv log: {logged}"
        );
        assert!(
            logged.lines().any(|l| l == "-c"),
            "command-line execution uses -c; argv log: {logged}"
        );
    }

    #[test]
    fn execute_current_runs_resolved_shell_as_dollar_zero() {
        let dir = tempdir().unwrap();
        let cwd = dir.path();
        let mut ss = Subshell::new();
        ss.cmdline = "printf '%s\\n' \"$0\"".to_string();
        ss.execute_current(cwd).unwrap();
        let got = ss.output_lines.join("\n");
        let shell = resolve_user_shell();
        let shell_s = shell.to_string_lossy();
        let name = shell.file_name().and_then(|n| n.to_str()).unwrap_or("");
        assert!(
            got.contains(shell_s.as_ref()) || (!name.is_empty() && got.contains(name)),
            "execute_current must run resolve_user_shell() as $0, got {got:?} want {shell_s}"
        );
    }

    #[test]
    fn execute_with_dash_override_is_not_bin_sh_when_dash_exists() {
        let dash = Path::new("/bin/dash");
        if !dash.exists() {
            return;
        }
        let dir = tempdir().unwrap();
        let cwd = dir.path();
        let mut ss = Subshell::new();
        ss.cmdline = "readlink -f /proc/$$/exe".to_string();
        ss.execute_with_shell(cwd, dash).unwrap();
        let got = ss.output_lines.join("\n");
        let exe = got.lines().next().unwrap_or("").trim();
        let base = Path::new(exe)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        assert_eq!(
            base, "dash",
            "SHELL=/bin/dash must exec dash, not sh; output: {got}"
        );
    }

    #[test]
    fn pty_spawn_records_override_shell_and_skips_dash_i_for_custom_name() {
        let dir = match tempdir() {
            Ok(d) => d,
            Err(_) => return,
        };
        let cwd = dir.path();
        let (wrapper, argv_log) = write_recording_shell(cwd, "myshell");
        let mut sess = match PtySession::spawn_with_shell(&wrapper, cwd, 24, 80) {
            Ok(s) => s,
            Err(_) => return,
        };
        assert_eq!(sess.shell_path(), wrapper.as_path());
        let logged = wait_for_argv_log(&argv_log);
        let _ = sess.kill();
        assert!(
            logged.contains("myshell"),
            "C-o PTY must exec SHELL override; argv log: {logged}"
        );
        assert!(
            !logged.lines().any(|l| l == "-i"),
            "custom override must not be given -i; argv log: {logged}"
        );
    }

    #[test]
    fn pty_spawn_passes_interactive_flag_for_bash_named_override() {
        let dir = match tempdir() {
            Ok(d) => d,
            Err(_) => return,
        };
        let cwd = dir.path();
        let (wrapper, argv_log) = write_recording_shell(cwd, "bash");
        let mut sess = match PtySession::spawn_with_shell(&wrapper, cwd, 24, 80) {
            Ok(s) => s,
            Err(_) => return,
        };
        assert_eq!(sess.shell_path(), wrapper.as_path());
        let logged = wait_for_argv_log(&argv_log);
        let _ = sess.kill();
        assert!(
            logged.lines().any(|l| l == "-i"),
            "bash/zsh/fish/tcsh PTY subshell is interactive; argv log: {logged}"
        );
    }

    #[test]
    fn pty_session_public_spawn_uses_resolved_user_shell() {
        let dir = match tempdir() {
            Ok(d) => d,
            Err(_) => return,
        };
        let mut sess = match PtySession::spawn(dir.path(), 24, 80) {
            Ok(s) => s,
            Err(_) => return,
        };
        assert_eq!(sess.shell_path(), resolve_user_shell().as_path());
        let _ = sess.kill();
    }

    #[test]
    fn wants_interactive_flag_for_gnu_embedded_shells() {
        for name in [
            "bash", "zsh", "fish", "tcsh", "dash", "ash", "ksh", "mksh", "sh",
        ] {
            assert!(
                wants_interactive_flag(Path::new(&format!("/usr/bin/{name}"))),
                "{name}"
            );
        }
        assert!(!wants_interactive_flag(Path::new("/bin/myshell")));
        assert!(!wants_interactive_flag(Path::new("/tmp/custom-shell")));
    }
}
