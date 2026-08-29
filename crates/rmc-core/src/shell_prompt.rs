//! GNU Midnight Commander 4.8 command-line prompt chrome (string only).
//!
//! Learned from live `mc` 4.8.30 on 80×24 and from the public mc.hlp line:
//! the basic prompt is `user@host:current_path$ ` (`#` when euid is 0).
//! Home is shown as `~`. A path that does not fit is left-truncated (GNU does
//! not mid-tilde the prompt row). The last three columns are the history
//! widget `[^]`; five columns before that stay reserved for input.

/// Live GNU prompt-row history widget (`[^]` at the right edge).
pub const GNU_PROMPT_HISTORY: &str = "[^]";
/// Columns occupied by [`GNU_PROMPT_HISTORY`].
pub const GNU_PROMPT_HISTORY_WIDTH: u16 = 3;
/// Minimum input columns GNU leaves before `[^]` when the prompt is long.
pub const GNU_PROMPT_INPUT_RESERVE: u16 = 5;

/// Maximum character width of the prompt text (including the trailing space
/// after `$`/`#`) on a `cols`-wide screen.
///
/// Live GNU 4.8.30 on 80 columns: 72 (`80 − 3 − 5`).
pub fn gnu_prompt_text_max(cols: u16) -> usize {
    cols.saturating_sub(GNU_PROMPT_HISTORY_WIDTH + GNU_PROMPT_INPUT_RESERVE) as usize
}

/// First column of the `[^]` widget (0-based).
pub fn gnu_prompt_history_col(cols: u16) -> u16 {
    cols.saturating_sub(GNU_PROMPT_HISTORY_WIDTH)
}

/// `$` for a user, `#` when the effective uid is 0 (GNU / POSIX shell).
pub fn gnu_prompt_sigil(euid: u32) -> char {
    if euid == 0 {
        '#'
    } else {
        '$'
    }
}

/// Replace `home` with `~` the way GNU / bash `\w` does on the prompt.
pub fn gnu_prompt_cwd(cwd: &str, home: &str) -> String {
    if home.is_empty() {
        return cwd.to_string();
    }
    if cwd == home {
        return "~".to_string();
    }
    if home != "/" {
        let prefix = format!("{home}/");
        if let Some(rest) = cwd.strip_prefix(&prefix) {
            return format!("~/{rest}");
        }
    }
    cwd.to_string()
}

/// GNU basic command prompt, left-truncated to `prompt_max` characters.
///
/// Includes the trailing space after `$`/`#` when it fits, so the input field
/// starts at `returned.chars().count()`.
pub fn gnu_shell_prompt(
    user: &str,
    host: &str,
    cwd: &str,
    home: &str,
    euid: u32,
    prompt_max: usize,
) -> String {
    let path = gnu_prompt_cwd(cwd, home);
    let sigil = gnu_prompt_sigil(euid);
    let full = format!("{user}@{host}:{path}{sigil} ");
    if full.chars().count() <= prompt_max {
        full
    } else {
        full.chars().take(prompt_max).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_cwd_matches_live_gnu_4_8_30() {
        // Live mc 4.8.30 + debian `\u@\h:\w\$ ` on 80×24 at /tmp/mcr-fixture.
        let p = gnu_shell_prompt(
            "ubuntu",
            "cursor",
            "/tmp/mcr-fixture",
            "/home/ubuntu",
            1000,
            gnu_prompt_text_max(80),
        );
        assert_eq!(p, "ubuntu@cursor:/tmp/mcr-fixture$ ");
        assert_eq!(p.chars().count(), 32);
    }

    #[test]
    fn home_is_tilde_and_subdir_keeps_slash() {
        assert_eq!(
            gnu_shell_prompt("ubuntu", "cursor", "/home/ubuntu", "/home/ubuntu", 1000, 72),
            "ubuntu@cursor:~$ "
        );
        assert_eq!(
            gnu_shell_prompt(
                "ubuntu",
                "cursor",
                "/tmp/mcr-home/subdir",
                "/tmp/mcr-home",
                1000,
                72
            ),
            "ubuntu@cursor:~/subdir$ "
        );
        assert_eq!(
            gnu_prompt_cwd("/tmp/mcr-home2", "/tmp/mcr-home"),
            "/tmp/mcr-home2",
            "home must be a path prefix, not a string prefix"
        );
    }

    #[test]
    fn root_uses_hash_sigil() {
        assert_eq!(
            gnu_shell_prompt("root", "cursor", "/tmp/mcr-fixture", "/root", 0, 72),
            "root@cursor:/tmp/mcr-fixture# "
        );
    }

    #[test]
    fn long_path_is_left_truncated_not_mid_tilde() {
        let home = "/tmp/mcr-home";
        let d60 = format!("{}/{}", home, "d".repeat(60));
        let p = gnu_shell_prompt("ubuntu", "cursor", &d60, home, 1000, gnu_prompt_text_max(80));
        assert_eq!(p.chars().count(), 72);
        assert!(
            !p.contains('~') || p.starts_with("ubuntu@cursor:~/"),
            "GNU prompt does not mid-tilde; got {p:?}"
        );
        assert!(
            !p.contains("~/d~"),
            "must not insert a mid-path tilde: {p:?}"
        );
        // Live 80×24: left 72 of user@host:~/ddd…$  loses the sigil.
        assert_eq!(
            p,
            format!("ubuntu@cursor:~/{}", "d".repeat(56)),
            "live GNU left-keeps 72 columns and drops `$` when the path is long"
        );
    }

    #[test]
    fn n55_under_home_still_has_dollar() {
        let home = "/tmp/mcr-home";
        let d55 = format!("{}/{}", home, "d".repeat(55));
        let p = gnu_shell_prompt("ubuntu", "cursor", &d55, home, 1000, 72);
        assert_eq!(p.chars().count(), 72);
        assert!(p.ends_with('$'), "n55 live GNU still shows `$`: {p:?}");
        assert!(!p.ends_with(' '), "trailing input space is clipped at max");
    }

    #[test]
    fn history_widget_sits_at_right_edge() {
        assert_eq!(gnu_prompt_history_col(80), 77);
        assert_eq!(gnu_prompt_history_col(60), 57);
        assert_eq!(gnu_prompt_history_col(100), 97);
        assert_eq!(gnu_prompt_text_max(80), 72);
        assert_eq!(GNU_PROMPT_HISTORY, "[^]");
    }
}
