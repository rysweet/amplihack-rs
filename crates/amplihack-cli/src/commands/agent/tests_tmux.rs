//! Unit tests for the parts of `tmux.rs` that run no process: the A8 reading
//! of `has-session` from tmux's exit status and stderr (rows 4 to 6), the
//! S13 parsing of `display-message` output, the first stderr line of a
//! failed call, the client environment removals, and the socket paths. tmux
//! is never run here. The S12 checks and A8 rows 1 to 3 are in
//! `tests_socket_check.rs`.
//!
//! API under test:
//! - `tmux::read_has_session(exit: Option<i32>, stderr: &str) -> SessionState`:
//!   A8 rows 4 to 6; `None` is a spawn error, timeout or signal
//! - `tmux::parse_names(out: &str) -> Result<(String, String), String>` and
//!   `tmux::parse_pane_pid(out: &str) -> Result<u32, String>` (S13; names are
//!   then checked with `validate::session_name` and `window_name`)
//! - `tmux::failure_line(stderr: &[u8]) -> String`: first line, sanitised,
//!   at most 200 characters
//! - `tmux::CLIENT_REMOVE: &[&str]`, `tmux::client_drops(name: &str) -> bool`
//! - `tmux::default_socket_for(tmux_tmpdir: Option<&OsStr>, uid: u32) -> PathBuf`
//! - `tmux::socket_from_env(tmux_value: &str) -> Option<PathBuf>`

use super::tmux::*;
use std::ffi::OsStr;
use std::path::Path;

#[test]
fn has_session_rows_4_to_6() {
    assert_eq!(read_has_session(Some(0), ""), SessionState::Present);
    for text in [
        "can't find session: coord\n",
        "no server running on /tmp/tmux-1000/default\n",
        "session not found: coord\n",
    ] {
        assert_eq!(
            read_has_session(Some(1), text),
            SessionState::Absent,
            "{text}"
        );
    }
    for (exit, text) in [
        (
            Some(1),
            "error connecting to /tmp/tmux-1000/default (Permission denied)\n",
        ),
        (Some(2), "can't find session: coord\n"),
        (Some(1), ""),
        (None, "timed out after 10 s"),
    ] {
        let got = read_has_session(exit, text);
        assert!(
            matches!(&got, SessionState::Unknown(r) if !r.is_empty()),
            "{exit:?} {text}: {got:?}"
        );
    }
    let got = read_has_session(Some(2), "bad\u{1b}[31m\nsecond line");
    let SessionState::Unknown(r) = got else {
        panic!("{got:?}")
    };
    assert!(
        !r.chars().any(char::is_control) && !r.contains("second"),
        "{r:?}"
    );
}

#[test]
fn display_output_must_be_exactly_the_fields() {
    let got = parse_names("coord\tmain\n").unwrap();
    assert_eq!(got, ("coord".to_string(), "main".to_string()));
    let got = parse_names("my session\tw 1\n").unwrap();
    assert_eq!(got, ("my session".to_string(), "w 1".to_string()));
    for bad in [
        "",
        "\n",
        "coord\n",
        "coord\tmain\textra\n",
        "coord\tmain\nmore\n",
        "\tmain\n",
        "coord\t\n",
        "co:rd\tmain\n",
        "coord\tma#in\n",
        "coord\tma\rin\n",
    ] {
        assert!(parse_names(bad).is_err(), "{bad:?}");
    }
    assert_eq!(parse_pane_pid("4242\n"), Ok(4242));
    for bad in [
        "",
        "\n",
        "-1\n",
        "42x\n",
        " 4242\n",
        "4242\n5\n",
        "99999999999\n",
    ] {
        assert!(parse_pane_pid(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn failure_lines_are_the_first_line_sanitised_and_cut() {
    assert_eq!(
        failure_line(b"duplicate session: coord\nmore\n"),
        "duplicate session: coord"
    );
    assert_eq!(failure_line(b"a\x1bb\tc\n"), "a?b?c");
    assert_eq!(
        failure_line("x".repeat(300).as_bytes()).chars().count(),
        200
    );
    assert_eq!(
        failure_line("é".repeat(300).as_bytes()).chars().count(),
        200
    );
}

#[test]
fn the_client_environment_removals() {
    let want = [
        "TMUX",
        "TMUX_PANE",
        "CLAUDECODE",
        "CLAUDE_CODE_SESSION_ID",
        "CLAUDE_CODE_ENTRYPOINT",
        "CLAUDE_PROJECT_DIR",
        "CODEX_THREAD_ID",
        "CODEX_SESSION_ID",
        "CLAUDE_CONFIG_DIR",
        "AMPLIHACK_CLAUDE_PROFILE",
    ];
    let mut got = CLIENT_REMOVE.to_vec();
    got.sort_unstable();
    let mut want_sorted = want.to_vec();
    want_sorted.sort_unstable();
    assert_eq!(got, want_sorted);
    for n in want.iter().chain(&[
        "COPILOT_CLI",
        "COPILOT_AGENT",
        "GITHUB_COPILOT",
        "GITHUB_COPILOT_TOKEN",
    ]) {
        assert!(client_drops(n), "{n}");
    }
    for n in [
        "HOME",
        "PATH",
        "TMUX_TMPDIR",
        "AMPLIHACK_CLAUDE_PROFILE_ROOT",
        "XCOPILOT_CLI",
    ] {
        assert!(!client_drops(n), "{n} is kept");
    }
}

#[test]
fn socket_paths() {
    let d = default_socket_for(Some(OsStr::new("/run/user/1000")), 1000);
    assert_eq!(d, Path::new("/run/user/1000/tmux-1000/default"));
    assert_eq!(
        default_socket_for(None, 1000),
        Path::new("/tmp/tmux-1000/default")
    );
    assert_eq!(
        default_socket_for(Some(OsStr::new("")), 7),
        Path::new("/tmp/tmux-7/default")
    );
    let s = socket_from_env("/tmp/tmux-1000/default,4242,0");
    assert_eq!(s.as_deref(), Some(Path::new("/tmp/tmux-1000/default")));
    assert_eq!(socket_from_env(""), None);
    assert_eq!(socket_from_env(",4242,0"), None);
}
