use anyhow::{anyhow, Result};
use std::env;
use std::path::Path;
use std::process::{Command, Stdio};

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
}
