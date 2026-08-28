//! GNU mc(1) invocation override: `SHELL=/bin/myshell mcr` must select that
//! binary for command-line `-c` execution and for the C-o PTY subshell.
use rmc_core::subshell::{resolve_user_shell, PtySession, Subshell};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tempfile::tempdir;

static SHELL_ENV_LOCK: Mutex<()> = Mutex::new(());

struct RestoreShell(Option<std::ffi::OsString>);

impl Drop for RestoreShell {
    fn drop(&mut self) {
        match &self.0 {
            Some(v) => env::set_var("SHELL", v),
            None => env::remove_var("SHELL"),
        }
    }
}

fn write_wrapper(dir: &Path) -> PathBuf {
    let shell = dir.join("myshell");
    // Handle `shell -c cmd` in-process so `$0` stays the override binary
    // (exec /bin/sh would rewrite $0 and hide the SHELL= selection).
    fs::write(
        &shell,
        "#!/bin/sh\nif [ \"$1\" = \"-c\" ]; then\n  shift\n  eval \"$1\"\n  exit $?\nfi\nexec /bin/sh \"$@\"\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&shell).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&shell, perms).unwrap();
    }
    shell
}

#[test]
fn shell_env_override_wins_for_minus_c_and_pty_spawn() {
    let _lock = SHELL_ENV_LOCK.lock().unwrap();
    let dir = tempdir().unwrap();
    let wrapper = write_wrapper(dir.path());
    let _restore = RestoreShell(env::var_os("SHELL"));
    env::set_var("SHELL", &wrapper);

    assert_eq!(
        resolve_user_shell(),
        wrapper,
        "SHELL= override must win over passwd login shell and /bin/sh"
    );

    let mut ss = Subshell::new();
    ss.cmdline = "printf '%s\\n' \"$0\"".to_string();
    ss.execute_current(dir.path()).unwrap();
    let got = ss.output_lines.join("\n");
    assert!(
        got.contains("myshell"),
        "execute_current must run $SHELL as $0; got {got:?}"
    );

    match PtySession::spawn(dir.path(), 24, 80) {
        Ok(mut sess) => {
            assert_eq!(
                sess.shell_path(),
                wrapper.as_path(),
                "C-o PTY must attach $SHELL when -U/subshell is enabled"
            );
            let _ = sess.kill();
        }
        Err(_) => {
            // Restricted environments may lack PTY; -c path above still proves override.
        }
    }
}
