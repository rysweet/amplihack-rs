//! Reading tmux's answers, without running tmux: `has-session` (A8 rows 4
//! to 6), `list-sessions` for server placement (G1), and the
//! `display-message` replies of S13. Re-exported by `tmux.rs`.

use super::tmux::{SessionState, failure_line};
use super::validate;

pub(super) const UNEXPECTED: &str = "tmux display-message returned an unexpected answer";

/// A8 rows 4 to 6. `exit` is `None` for a spawn error, a timeout or a
/// signal; tmux's messages are not translated.
pub fn read_has_session(exit: Option<i32>, stderr: &str) -> SessionState {
    const GONE: [&str; 3] = [
        "can't find session",
        "no server running",
        "session not found",
    ];
    match exit {
        Some(0) => SessionState::Present,
        Some(1) if GONE.iter().any(|g| stderr.contains(g)) => SessionState::Absent,
        _ => unknown(exit, stderr),
    }
}

/// `list-sessions` for server placement: exit 0 is a server, exit 1 with
/// `no server running` none, and anything else unknown.
pub fn read_list_sessions(exit: Option<i32>, stderr: &str) -> SessionState {
    match exit {
        Some(0) => SessionState::Present,
        Some(1) if stderr.contains("no server running") => SessionState::Absent,
        _ => unknown(exit, stderr),
    }
}

/// Unknown, with tmux's first stderr line or its exit status as reason.
fn unknown(exit: Option<i32>, stderr: &str) -> SessionState {
    let line = failure_line(stderr.as_bytes());
    let reason = match (line.is_empty(), exit) {
        (false, _) => line,
        (true, Some(code)) => format!("tmux exited with status {code}"),
        (true, None) => "tmux could not be run".to_string(),
    };
    SessionState::Unknown(reason)
}

/// `SESSION<TAB>WINDOW` and at most a trailing newline, both names valid
/// (S13). Anything else is an error, never a partial value.
pub fn parse_names(out: &str) -> Result<(String, String), String> {
    let body = out.strip_suffix('\n').unwrap_or(out);
    let mut parts = body.split('\t');
    let (Some(session), Some(window), None) = (parts.next(), parts.next(), parts.next()) else {
        return Err(UNEXPECTED.into());
    };
    if body.contains('\n') {
        return Err(UNEXPECTED.into());
    }
    validate::session_name(session).map_err(|rule| format!("tmux session name {rule}"))?;
    validate::window_name(window).map_err(|rule| format!("tmux window name {rule}"))?;
    Ok((session.to_string(), window.to_string()))
}

/// A decimal pid and at most a trailing newline (S13).
pub fn parse_pane_pid(out: &str) -> Result<u32, String> {
    let body = out.strip_suffix('\n').unwrap_or(out);
    if body.is_empty() || !body.bytes().all(|b| b.is_ascii_digit()) {
        return Err(UNEXPECTED.into());
    }
    body.parse::<u32>()
        .ok()
        .filter(|&pid| pid > 0)
        .ok_or_else(|| UNEXPECTED.to_string())
}
