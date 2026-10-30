//! Unit tests for `report.rs`: the recovery and ALERT lines (R7.1, R7.2,
//! step 5b section 8.1), the closed set of result texts (including B5),
//! sanitising, the D13 repeat rule, the B8 mapping of a line to
//! `last_recovery`, and the S20 log append. Status output is in
//! `tests_report_status.rs`.
//!
//! API under test:
//! - `report::sanitize(s: &str, max: usize) -> String`: control characters
//!   become `?`; at most `max` characters
//! - `enum Outcome { Resumed, AlreadyRunning, LivenessUnknown(String),
//!   SessionExists, TmuxUnknown(String), NoConversation, NoLaunchDir,
//!   NoTranscript, Tmux(String), RestartLimit }`, `Display` = result text
//! - `report::recovery_line(now: i64, name: &str, conv: Option<&str>,
//!   lost: Option<&str>, resumed: Option<&str>, result: &str) -> String`
//! - `report::alert_text(n: usize) -> String`, `report::alert_line(now: i64,
//!   name: &str, n: usize) -> String`
//! - `report::not_recorded_line(now: i64, name: &str, reason: &str) -> String`
//! - `report::last_recovery(now: i64, conv: Option<&str>, lost: Option<&str>,
//!   resumed: Option<&str>, result: &str) -> LastRecovery`
//! - `report::suppressed(last: Option<&LastRecovery>, conv: Option<&str>, result: &str) -> bool`
//! - `report::clears_when_healthy(result: &str) -> bool`
//! - `report::append(log: &Path, line: &str) -> std::io::Result<()>`

use super::registry::{LastRecovery, fmt_ts};
use super::report::*;
use serde_json::json;
use std::fs;
use std::os::unix::fs::PermissionsExt;

const ID: &str = "0f8e5c1a-3b2d-4c6e-9a7f-1d2e3f4a5b6c";
/// 2026-10-29T23:28:10Z.
const NOW: i64 = 1_793_316_490;

#[test]
fn the_recovery_line_format() {
    assert_eq!(
        recovery_line(
            NOW,
            "coordinator",
            Some(ID),
            Some("2026-10-29T23:26:58Z"),
            Some("2026-10-29T23:28:10Z"),
            "resumed"
        ),
        format!(
            "2026-10-29T23:28:10Z agent=coordinator conversation={ID} \
             lost=2026-10-29T23:26:58Z resumed=2026-10-29T23:28:10Z result=resumed"
        )
    );
    assert_eq!(
        recovery_line(
            NOW,
            "area-docs",
            None,
            None,
            None,
            "failed: no conversation id"
        ),
        "2026-10-29T23:28:10Z agent=area-docs conversation=- lost=- resumed=- \
         result=failed: no conversation id"
    );
}

#[test]
fn lines_are_sanitised() {
    let l = recovery_line(
        NOW,
        "a",
        Some(ID),
        None,
        None,
        "failed: tmux: x\ny\u{1b}[31m",
    );
    assert!(l.ends_with("result=failed: tmux: x?y?[31m"), "{l}");
    assert!(!l.chars().any(char::is_control));
}

#[test]
fn the_alert_line_format() {
    assert_eq!(
        alert_text(3),
        "stopped: restart limit (3 restarts in the last hour)"
    );
    assert_eq!(
        alert_line(NOW, "area-ci", 3),
        "2026-10-29T23:28:10Z ALERT: agent area-ci stopped: restart limit (3 restarts in the last hour)"
    );
    assert_eq!(
        alert_line(NOW, "x", 1),
        format!("{} ALERT: agent x {}", fmt_ts(NOW), alert_text(1))
    );
}

/// Review issue 10: a healthy pass clears a skip or a failure, after which
/// the same result is no longer a repeat; a resume is kept.
#[test]
fn a_healthy_pass_clears_skips_and_failures() {
    for r in ["skipped: tmux session exists", "failed: no transcript"] {
        assert!(clears_when_healthy(r), "{r}");
        let last = last_recovery(NOW, Some(ID), None, None, r);
        assert!(suppressed(Some(&last), Some(ID), r));
        assert!(!suppressed(None, Some(ID), r), "after clearing");
    }
    for r in ["resumed", "stopped: restart limit"] {
        assert!(!clears_when_healthy(r), "{r}");
    }
}

#[test]
fn the_warning_line_format() {
    assert_eq!(
        not_recorded_line(NOW, "coord", "no claude process found"),
        "2026-10-29T23:28:10Z WARNING: agent coord conversation not recorded: no claude process found"
    );
    let line = not_recorded_line(NOW, "coord", "bad\nreason\u{1b}[31m");
    assert!(line.ends_with(": bad?reason?[31m"), "{line}");
}

#[test]
fn every_result_text() {
    let cases = [
        (Outcome::Resumed, "resumed"),
        (Outcome::AlreadyRunning, "skipped: already running"),
        (
            Outcome::LivenessUnknown("Claude config directory is not private".into()),
            "skipped: already running (liveness unknown: Claude config directory is not private)",
        ),
        (Outcome::SessionExists, "skipped: tmux session exists"),
        (
            Outcome::TmuxUnknown("socket directory is not private".into()),
            "skipped: tmux session exists (tmux state unknown: socket directory is not private)",
        ),
        (Outcome::NoConversation, "failed: no conversation id"),
        (Outcome::NoLaunchDir, "failed: launch directory missing"),
        (Outcome::NoTranscript, "failed: no transcript"),
        (
            Outcome::Tmux("duplicate session: coord".into()),
            "failed: tmux: duplicate session: coord",
        ),
        (Outcome::RestartLimit, "stopped: restart limit"),
    ];
    for (o, text) in cases {
        assert_eq!(o.to_string(), text);
    }
}

#[test]
fn sanitize_replaces_control_characters_and_cuts() {
    assert_eq!(sanitize("a\nb\u{1b}c\rd\te\u{0}f", 200), "a?b?c?d?e?f");
    assert_eq!(sanitize(&"x".repeat(300), 200).chars().count(), 200);
    assert_eq!(
        sanitize(&"é".repeat(250), 200),
        "é".repeat(200),
        "cut on a char boundary"
    );
    assert_eq!(sanitize("/home/u/src/mistt", 200), "/home/u/src/mistt");
    assert_eq!(sanitize("", 10), "");
}

fn last(conv: Option<&str>, result: &str) -> LastRecovery {
    serde_json::from_value(json!({
        "logged_at": "2026-10-29T23:00:00Z", "conversation_id": conv, "result": result
    }))
    .unwrap()
}

#[test]
fn repeats_are_suppressed_except_resumed_d13() {
    let r = "skipped: tmux session exists";
    assert!(!suppressed(None, Some(ID), r), "a first line is written");
    assert!(suppressed(Some(&last(Some(ID), r)), Some(ID), r));
    assert!(suppressed(Some(&last(None, r)), None, r));
    assert!(!suppressed(
        Some(&last(Some(ID), r)),
        Some(ID),
        "skipped: already running"
    ));
    assert!(
        !suppressed(Some(&last(Some(ID), r)), None, r),
        "the conversation changed"
    );
    let other = "6f3b9d2e-4a7c-4e1b-8c5d-2e9a7f1b4d36";
    assert!(!suppressed(Some(&last(Some(ID), r)), Some(other), r));
    assert!(!suppressed(
        Some(&last(Some(ID), "resumed")),
        Some(ID),
        "resumed"
    ));
}

/// The fields of a recovery line, `-` read as `None` (B8).
fn fields(
    line: &str,
) -> (
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
) {
    let (head, result) = line.split_once(" result=").unwrap();
    let parts: Vec<&str> = head.split(' ').collect();
    let val = |i: usize, key: &str| {
        let v = parts[i].strip_prefix(key).unwrap();
        (v != "-").then(|| v.to_string())
    };
    let conv = val(2, "conversation=");
    (
        parts[0].to_string(),
        conv,
        val(3, "lost="),
        val(4, "resumed="),
        result.into(),
    )
}

#[test]
fn last_recovery_holds_the_fields_of_its_line_b8() {
    let lost = Some("2026-10-29T23:26:58Z");
    for (conv, lost, resumed, result) in [
        (Some(ID), lost, Some("2026-10-29T23:28:10Z"), "resumed"),
        (None, lost, None, "failed: no conversation id"),
        (Some(ID), None, None, "skipped: already running"),
    ] {
        let line = recovery_line(NOW, "a", conv, lost, resumed, result);
        let lr = last_recovery(NOW, conv, lost, resumed, result);
        let (ts, c, l, r, res) = fields(&line);
        assert_eq!(lr.logged_at, ts);
        assert_eq!(lr.conversation_id, c);
        assert_eq!(lr.lost, l);
        assert_eq!(lr.resumed, r);
        assert_eq!(lr.result, res);
    }
}

#[test]
fn append_writes_one_line_to_a_private_file() {
    let tmp = tempfile::tempdir().unwrap();
    let log = tmp.path().join("supervisor.log");
    append(&log, "first").unwrap();
    append(&log, "second").unwrap();
    assert_eq!(fs::read_to_string(&log).unwrap(), "first\nsecond\n");
    let mode = fs::metadata(&log).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

#[test]
fn append_refuses_a_symlink_or_a_hard_link_s20() {
    let tmp = tempfile::tempdir().unwrap();
    let secret = tmp.path().join("id_ed25519");
    fs::write(&secret, "KEY\n").unwrap();
    fs::set_permissions(&secret, fs::Permissions::from_mode(0o600)).unwrap();
    let log = tmp.path().join("supervisor.log");
    std::os::unix::fs::symlink(&secret, &log).unwrap();
    assert!(append(&log, "line").is_err());
    fs::remove_file(&log).unwrap();
    fs::hard_link(&secret, &log).unwrap();
    assert!(append(&log, "line").is_err());
    assert_eq!(
        fs::read_to_string(&secret).unwrap(),
        "KEY\n",
        "the target is unchanged"
    );
    let open = tmp.path().join("open.log");
    fs::write(&open, "").unwrap();
    fs::set_permissions(&open, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(append(&open, "line").is_err(), "group or other bits");
}
