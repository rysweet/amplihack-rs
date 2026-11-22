//! Issue #2912: what a relaunch runs, and the lines it writes in the form
//! documented in docs/howto/keep-sessions-running.md.
//!
//! The decision and its room gate are in `tests_standing.rs`.

use super::lines::{gone_alert, refused_line, relaunch_line, waiting_line};
use super::relaunch::Lost;
use crate::commands::agent::launch::continue_args;
use crate::commands::agent::registry::Entry;
use crate::commands::agent::report::MAX_LINE;
use serde_json::json;

const NOW: i64 = 1_793_316_490;

// ── the relaunch argv ───────────────────────────────────────────────────────

fn entry(extra: serde_json::Value) -> Entry {
    let mut e = json!({
        "state": "active",
        "tmux_session": "coord",
        "tmux_window": "main",
        "launch_dir": "/home/alex/src/project",
        "conversation_id": "0f8e5c1a-3b2d-4c6e-9a7f-1d2e3f4a5b6c",
        "registered_at": "2026-10-30T09:12:04Z",
        "launched_at": "2026-10-30T09:12:04Z",
        "launch_id": "6f1c0d8e2a4b4f0e9c7d3b5a1e2f4c6d"
    });
    for (k, v) in extra.as_object().unwrap() {
        e[k] = v.clone();
    }
    serde_json::from_value(e).expect("a valid entry")
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

#[test]
fn the_continue_args_carry_the_recorded_options_after_continue() {
    let e = entry(json!({
        "model": "opus",
        "effort": "high",
        "add_dirs": ["/home/alex/notes", "/home/alex/more"],
        "permission_mode": "acceptEdits"
    }));
    assert_eq!(
        continue_args(&e),
        s(&[
            "--continue",
            "--model=opus",
            "--effort=high",
            "--add-dir=/home/alex/notes",
            "--add-dir=/home/alex/more",
            "--permission-mode=acceptEdits",
        ])
    );
}

#[test]
fn a_bare_entry_continues_with_nothing_else() {
    assert_eq!(continue_args(&entry(json!({}))), s(&["--continue"]));
}

#[test]
fn the_continue_args_never_resume_by_id_or_carry_a_message() {
    let args = continue_args(&entry(json!({ "model": "opus" })));
    assert!(!args.iter().any(|a| a == "--resume"), "{args:?}");
    assert!(!args.iter().any(|a| a.contains("0f8e5c1a")), "{args:?}");
    assert!(!args.iter().any(|a| a.contains("supervisor")), "{args:?}");
}

/// Same privilege as the first launch: skipping permissions is carried only
/// when it was recorded, and a recorded permission mode replaces it.
#[test]
fn skip_permissions_is_carried_only_when_recorded() {
    assert!(
        !continue_args(&entry(json!({})))
            .iter()
            .any(|a| a == "--dangerously-skip-permissions")
    );
    assert_eq!(
        continue_args(&entry(json!({ "skip_permissions": true }))),
        s(&["--continue", "--dangerously-skip-permissions"])
    );
    let both = continue_args(&entry(
        json!({ "skip_permissions": true, "permission_mode": "plan" }),
    ));
    assert!(!both.iter().any(|a| a == "--dangerously-skip-permissions"));
}

/// A recorded value that starts with a dash must never become a separate
/// argument that Claude would read as another flag.
#[test]
fn a_recorded_value_cannot_become_another_flag() {
    let args = continue_args(&entry(json!({ "model": "-x", "effort": "--print" })));
    assert!(
        !args.iter().any(|a| a == "-x" || a == "--print"),
        "{args:?}"
    );
}

// ── the relaunch lines ──────────────────────────────────────────────────────

#[test]
fn the_relaunch_line_is_the_documented_one() {
    assert_eq!(
        relaunch_line(NOW, "coordinator", "exit-status-1", true),
        "2026-10-29T23:28:10Z standing session relaunch session=coordinator reason=exit-status-1 waited=room"
    );
    assert_eq!(
        relaunch_line(NOW, "coordinator", "SIGKILL", false),
        "2026-10-29T23:28:10Z standing session relaunch session=coordinator reason=SIGKILL waited=none"
    );
}

#[test]
fn the_refusal_line_is_the_documented_one() {
    assert_eq!(
        refused_line(NOW, "coordinator", "superseded"),
        "2026-10-29T23:28:10Z standing session refused session=coordinator reason=superseded"
    );
}

/// Written once the watch is armed, so an operator reading a quiet pane can
/// tell a session waiting for capacity from one that has stopped.
#[test]
fn the_waiting_line_is_the_documented_one() {
    assert_eq!(
        waiting_line(NOW, "coordinator"),
        "2026-10-29T23:28:10Z standing session waiting session=coordinator reason=no-room"
    );
}

/// The line reaches a live terminal and a log: no control character of the
/// name or the reason may survive, so neither can forge a line or an escape.
#[test]
fn control_characters_never_reach_the_pane_or_the_log() {
    for line in [
        relaunch_line(NOW, "a\u{1b}[31mb\nc\rd", "x\u{1b}]0;t\u{7}y\nz", false),
        refused_line(NOW, "a\u{1b}[31mb\nc\rd", "x\ny\u{1b}"),
        waiting_line(NOW, "a\u{1b}[31mb\nc\rd"),
    ] {
        assert!(!line.chars().any(char::is_control), "{line:?}");
        assert_eq!(line.lines().count(), 1, "{line:?}");
    }
}

/// A wait for room that ended without one, and without a signal, stops a
/// session its owner wants running, so the record says which exit it was
/// and why the wait ended, for `amplihack agent status` and the supervisor.
#[test]
fn the_gone_alert_names_the_exit_and_why_the_wait_ended() {
    let cases = [
        (
            Lost::Deadline,
            "the wait for one reached its limit of 7 days",
        ),
        (
            Lost::Watch,
            "the watch on the rotator's swap record was lost",
        ),
        (
            Lost::Unarmed,
            "the rotator's swap record could not be watched",
        ),
        (
            Lost::Unwatchable,
            "the rotator's directory is missing or not private to this user",
        ),
    ];
    for (why, text) in cases {
        assert_eq!(
            gone_alert("exit-status-1", why),
            format!(
                "not relaunched: it exited exit-status-1 while no login had room, \
                 and {text}, so the session was left for the supervisor"
            )
        );
    }
    let hostile = gone_alert(
        &format!("a\u{1b}[31mb\nc{}", "x".repeat(4096)),
        Lost::Deadline,
    );
    assert!(!hostile.chars().any(char::is_control), "{hostile:?}");
    assert!(hostile.chars().count() <= MAX_LINE, "{hostile:?}");
}
