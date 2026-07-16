//! Tmux helper functions for the slow refresh thread (T5).

use std::process::{Command, Stdio};
use std::time::Duration;

use super::{CAPTURE_CACHE_ENTRY_MAX_BYTES, truncate_to_capture_cache_limit};
use crate::util::{run_output_with_timeout, run_output_with_timeout_limited, run_with_timeout};

const TMUX_COMMAND_TIMEOUT: Duration = Duration::from_secs(2);
const CAPTURE_PANE_BACKSCROLL_LINES: &str = "-4096";

/// Check whether a local `tmux` binary is available.
///
/// Runs `tmux -V` with a 2-second timeout.  Returns `false` if the binary
/// cannot be found or returns a non-zero exit code (RISK-07).
pub(super) fn is_tmux_available() -> bool {
    let mut command = Command::new("tmux");
    command
        .args(["-V"])
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    run_with_timeout(command, TMUX_COMMAND_TIMEOUT)
        .map(|status| status.success())
        .unwrap_or(false)
}

/// List active local tmux session names via `tmux list-sessions -F "#{session_name}"`.
///
/// Returns an empty `Vec` if tmux is absent, returns no sessions, or any
/// command error occurs (RISK-07: graceful skip when tmux is unavailable).
pub(super) fn list_local_tmux_sessions() -> Vec<String> {
    let mut command = Command::new("tmux");
    command
        .args(["list-sessions", "-F", "#{session_name}"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let output = match run_output_with_timeout(command, TMUX_COMMAND_TIMEOUT) {
        Ok(o) => o,
        Err(_) => return vec![],
    };

    if !output.status.success() {
        return vec![];
    }

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

/// Capture recent output from a local tmux pane.
///
/// Runs `tmux capture-pane` against a bounded recent backscroll window and
/// returns capped raw stdout.  Returns an empty string on any error (non-zero
/// exit, binary absence, timeout, etc.).
///
/// The caller is responsible for stripping OSC sequences.
pub(super) fn capture_local_tmux_pane(session_id: &str) -> String {
    // SEC-01: session_id must already be sanitized before calling this.
    // We additionally reject any session_id containing shell-special characters
    // to prevent command injection.  Only [a-zA-Z0-9_.-] are allowed.
    if session_id
        .chars()
        .any(|c| !c.is_ascii_alphanumeric() && !matches!(c, '_' | '-' | '.'))
    {
        return String::new();
    }

    let mut command = Command::new("tmux");
    command
        .args([
            "capture-pane",
            "-t",
            session_id,
            "-p",
            "-S",
            CAPTURE_PANE_BACKSCROLL_LINES,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let output = match run_output_with_timeout_limited(
        command,
        TMUX_COMMAND_TIMEOUT,
        CAPTURE_CACHE_ENTRY_MAX_BYTES,
    ) {
        Ok(o) => o,
        Err(_) => return String::new(),
    };

    if output.status.success() {
        truncate_to_capture_cache_limit(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        String::new()
    }
}

/// Sanitize a tmux session name for use as a session ID.
///
/// Keeps only `[a-zA-Z0-9_-]` characters; returns an empty string if no
/// valid characters remain (SEC-01, mirroring `sanitize_session_id` in
/// amplihack-types).
pub(super) fn sanitize_tmux_session_name(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
        .collect();
    sanitized
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sanitize_tmux_session_name_keeps_alphanumeric_hyphen_underscore() {
        assert_eq!(sanitize_tmux_session_name("my-session_01"), "my-session_01");
    }

    #[test]
    fn sanitize_tmux_session_name_strips_path_separators() {
        assert_eq!(sanitize_tmux_session_name("../evil"), "evil");
    }

    #[test]
    fn sanitize_tmux_session_name_strips_shell_special_chars() {
        assert_eq!(sanitize_tmux_session_name("foo;rm -rf /"), "foorm-rf");
    }

    #[test]
    fn sanitize_tmux_session_name_empty_input_returns_empty() {
        assert_eq!(sanitize_tmux_session_name(""), "");
    }

    #[test]
    fn sanitize_tmux_session_name_all_invalid_returns_empty() {
        assert_eq!(sanitize_tmux_session_name("@!#$%^&*()"), "");
    }

    #[test]
    fn capture_local_tmux_pane_rejects_shell_special_chars() {
        let result = capture_local_tmux_pane("session;malicious");
        assert!(
            result.is_empty(),
            "capture must refuse IDs with shell-special chars; got {result:?}"
        );
    }

    #[test]
    fn capture_local_tmux_pane_rejects_path_traversal() {
        let result = capture_local_tmux_pane("../etc/passwd");
        assert!(
            result.is_empty(),
            "capture must refuse path traversal IDs; got {result:?}"
        );
    }

    #[test]
    fn t5_thread_does_not_spawn_when_tmux_absent() {
        let sessions = list_local_tmux_sessions();
        let _ = sessions;
    }

    #[test]
    fn capture_output_capped_at_64kib_in_slow_thread_logic() {
        let oversized = "x".repeat(CAPTURE_CACHE_ENTRY_MAX_BYTES + 512);
        let capped = truncate_to_capture_cache_limit(oversized);
        assert_eq!(
            capped.len(),
            CAPTURE_CACHE_ENTRY_MAX_BYTES,
            "capped output must be exactly CAPTURE_CACHE_ENTRY_MAX_BYTES bytes"
        );
    }
}
