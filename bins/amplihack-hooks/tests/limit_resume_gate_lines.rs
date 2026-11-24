//! Issue #3172: the real `amplihack-hooks limit-resume` binary leaves a line
//! for every call, and sends keys for a recorded real payload shape.
//!
//! - Test 1: a StopFailure payload shaped as Claude Code 2.1.329 sends it
//!   (`tests/fixtures/stopfailure-rate-limit-claude-code-2.1.329.json` in the
//!   hooks crate) leads to the four keys at a stand-in `tmux`, after the guard
//!   claim, and one `resumed-keys` line.
//! - Test 2: each early exit (`disabled`, `not-limit`, `bad-session-id`,
//!   `bad-payload`) writes its line, with an `error=` value in `[a-z_]`.
//!   A line that cannot be written is recorded where the operator sees it.
//! - Test 3: seven concurrent calls for one swap event write seven lines.
//!
//! The hook runs in a sandbox with a temporary profile root and a cleared
//! environment. `tmux` is a stand-in found through `PATH`; no real tmux
//! server, pane or terminal is reached, and no other session's pane text is
//! read. No test sleeps, polls or sets a timeout. They assert behaviour,
//! never wording.

#![cfg(target_os = "linux")]

#[path = "support/limit_resume_harness.rs"]
mod harness;
#[path = "../../../tests/support/isolated_hook.rs"]
mod isolated_hook;

use amplihack_hooks::limit_resume::RESUME_MSG;
use harness::{
    LIMIT_MENU, PLAIN_SCREEN, Pty, Rig, StandInRotator, assert_error_fields_reduced, field,
    with_transcript,
};
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

/// A StopFailure payload as Claude Code 2.1.329 sends it, with placeholder
/// text only.
const RECORDED: &str = include_str!(
    "../../../crates/amplihack-hooks/tests/fixtures/stopfailure-rate-limit-claude-code-2.1.329.json"
);

const KEYS: [&str; 4] = [
    "[send-keys][-t][%7][Escape]",
    "[send-keys][-t][%7][C-u]",
    "[send-keys][-t][%7][-l][continue]",
    "[send-keys][-t][%7][Enter]",
];

fn recorded() -> Value {
    serde_json::from_str(RECORDED).unwrap()
}

fn payload(error: Value) -> Vec<u8> {
    json!({
        "hook_event_name": "StopFailure",
        "session_id": "00000000-0000-4000-8000-000000000002",
        "error": error,
        "error_details": "placeholder",
        "last_assistant_message": "placeholder"
    })
    .to_string()
    .into_bytes()
}

fn assert_zero_with_no_output(out: &std::process::Output) {
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.stdout.is_empty());
    assert!(
        out.stderr.is_empty(),
        "stderr: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn assert_zero_with_one_stderr_line(out: &std::process::Output) {
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty());
    let text = String::from_utf8(out.stderr.clone()).expect("stderr is text");
    assert_eq!(text.lines().count(), 1, "{text:?}");
    assert!(text.ends_with('\n'), "{text:?}");
}

fn lines(log: &str) -> Vec<&str> {
    log.lines().collect()
}

// ── Test 1: a recorded payload shape sends keys ─────────────────────────────

#[test]
fn recorded_payload_sends_keys_through_the_stand_in_tmux() {
    let rig = Rig::new(LIMIT_MENU, false);
    let _rotator = StandInRotator::start(&rig.root);
    rig.private_resume_dir();
    let transcript = rig.swap_already_happened();
    assert_eq!(recorded()["hook_event_name"], "StopFailure");
    assert_eq!(recorded()["error"], "rate_limit");

    let out = rig.run_on_pty("1", &with_transcript(&recorded(), &transcript));
    assert_zero_with_no_output(&out);

    assert_eq!(rig.send_keys_calls(), KEYS, "{:?}", rig.tmux_calls());
    assert_eq!(
        rig.claim_seen(),
        ["yes", "yes", "yes", "yes"],
        "the guard claim must exist before each key"
    );
    let claims = fs::read_dir(rig.resume_dir().join("events"))
        .unwrap()
        .count();
    assert_eq!(claims, 1, "one guard claim");

    let log = rig.resume_log();
    let resumed: Vec<_> = lines(&log)
        .into_iter()
        .filter(|l| field(l, "outcome") == Some("resumed-keys"))
        .collect();
    assert_eq!(resumed.len(), 1, "{log}");
    assert_eq!(field(resumed[0], "error"), Some("rate_limit"));
    assert_error_fields_reduced(&log);
}

#[test]
fn an_unseen_error_value_with_a_screen_the_keys_do_not_answer_types_nothing() {
    let rig = Rig::new(PLAIN_SCREEN, false);
    let _rotator = StandInRotator::start(&rig.root);
    rig.private_resume_dir();
    let transcript = rig.swap_already_happened();
    let mut v = recorded();
    v["error"] = json!("usage_limit_reached");

    let out = rig.run_on_pty("1", &with_transcript(&v, &transcript));
    assert_eq!(out.status.code(), Some(2), "a wake, not keys");
    assert!(String::from_utf8_lossy(&out.stderr).contains(RESUME_MSG));
    assert!(rig.send_keys_calls().is_empty(), "{:?}", rig.tmux_calls());
    let log = rig.resume_log();
    assert_eq!(lines(&log).len(), 1, "{log}");
    assert_eq!(field(&log, "outcome"), Some("screen-unrecognised"));
    assert_eq!(field(&log, "error"), Some("usage_limit_reached"));
}

#[test]
fn billing_error_and_a_numeric_error_reach_the_keys_when_the_screen_is_a_limit_menu() {
    for error in [json!("billing_error"), json!(42)] {
        let rig = Rig::new(LIMIT_MENU, false);
        let _rotator = StandInRotator::start(&rig.root);
        rig.private_resume_dir();
        let transcript = rig.swap_already_happened();
        let mut v = recorded();
        v["error"] = error.clone();
        let out = rig.run_on_pty("1", &with_transcript(&v, &transcript));
        assert_zero_with_no_output(&out);
        assert_eq!(rig.send_keys_calls(), KEYS, "{error}");
        assert_error_fields_reduced(&rig.resume_log());
    }
}

// ── Test 2: each early exit writes its line ─────────────────────────────────

#[test]
fn disabled_with_an_existing_resume_folder_writes_one_line() {
    let rig = Rig::new(LIMIT_MENU, false);
    let _rotator = StandInRotator::start(&rig.root);
    write_off(&rig);
    let dir = rig.private_resume_dir();
    let out = rig.run_plain(Some("0"), &payload(json!("rate_limit")));
    assert_zero_with_no_output(&out);
    let log = rig.resume_log();
    assert_eq!(lines(&log).len(), 1, "{log}");
    assert_eq!(field(&log, "outcome"), Some("disabled"));
    assert_eq!(field(&log, "error"), Some("rate_limit"));
    assert!(rig.tmux_calls().is_empty());
    assert!(!dir.join("log-write-failed").exists());
}

#[test]
fn disabled_without_a_resume_folder_creates_nothing() {
    let rig = Rig::new(LIMIT_MENU, false);
    write_off(&rig);
    let out = rig.run_plain(Some("0"), &payload(json!("rate_limit")));
    assert_zero_with_no_output(&out);
    assert!(
        !rig.resume_dir().exists(),
        "auto-switch off must create nothing (FS-5)"
    );
}

fn write_off(rig: &Rig) {
    harness::private_dir(&rig.root.join(".auto-switch.d"));
    harness::private_file(&rig.root.join(".auto-switch"), b"enabled=off\n");
}

#[test]
fn not_limit_writes_one_line_per_call_and_calls_no_tmux() {
    let rig = Rig::new(LIMIT_MENU, false);
    let _rotator = StandInRotator::start(&rig.root);
    rig.private_resume_dir();
    harness::write_ledger(&rig.root, "swapped", -30);
    let known = [
        "authentication_failed",
        "oauth_org_not_allowed",
        "account_on_hold",
        "verification_required",
        "overloaded",
        "invalid_request",
        "model_not_found",
        "server_error",
        "max_output_tokens",
        "cloud_credential_error",
    ];
    for (n, error) in known.iter().enumerate() {
        let out = rig.run_plain(Some("1"), &payload(json!(error)));
        assert_zero_with_no_output(&out);
        let log = rig.resume_log();
        assert_eq!(lines(&log).len(), n + 1, "one line per call: {log}");
        let last = lines(&log).pop().unwrap();
        assert_eq!(field(last, "outcome"), Some("not-limit"), "{error}");
        assert_eq!(field(last, "error"), Some(*error));
    }
    assert!(rig.tmux_calls().is_empty(), "no stand-in tmux call");
    assert_error_fields_reduced(&rig.resume_log());
}

#[test]
fn a_hostile_error_value_is_reduced_on_the_line() {
    let rig = Rig::new(LIMIT_MENU, false);
    let _rotator = StandInRotator::start(&rig.root);
    rig.private_resume_dir();
    harness::write_ledger(&rig.root, "swapped", -30);
    // `Stop` events are not limits, so this call ends at the gate whatever
    // the error text is.
    let v = json!({
        "hook_event_name": "Stop",
        "session_id": "s",
        "error": "Rate Limit!\nmessage text=1 outcome=resumed-keys"
    });
    let out = rig.run_plain(Some("1"), v.to_string().as_bytes());
    assert_zero_with_no_output(&out);
    let log = rig.resume_log();
    assert_eq!(lines(&log).len(), 1, "{log}");
    assert_eq!(field(&log, "outcome"), Some("not-limit"));
    assert_error_fields_reduced(&log);
    assert!(!log.contains("message"), "no message text on a line: {log}");
}

#[test]
fn a_bad_session_id_writes_one_line_with_dashes() {
    let rig = Rig::new(LIMIT_MENU, false);
    let _rotator = StandInRotator::start(&rig.root);
    rig.private_resume_dir();
    harness::write_ledger(&rig.root, "swapped", -30);
    for (n, session) in [json!(null), json!(""), json!("x".repeat(257))]
        .into_iter()
        .enumerate()
    {
        let v = json!({
            "hook_event_name": "StopFailure",
            "session_id": session,
            "error": "rate_limit"
        });
        let out = rig.run_plain(Some("1"), v.to_string().as_bytes());
        assert_zero_with_no_output(&out);
        let log = rig.resume_log();
        assert_eq!(lines(&log).len(), n + 1, "{log}");
        let last = lines(&log).pop().unwrap();
        assert_eq!(field(last, "outcome"), Some("bad-session-id"));
        assert_eq!(field(last, "session"), Some("-"));
        assert_eq!(field(last, "event"), Some("-"));
        assert_eq!(field(last, "error"), Some("rate_limit"));
    }
    assert!(rig.tmux_calls().is_empty());
}

#[test]
fn a_bad_payload_writes_one_line() {
    let rig = Rig::new(LIMIT_MENU, false);
    let _rotator = StandInRotator::start(&rig.root);
    rig.private_resume_dir();
    harness::write_ledger(&rig.root, "swapped", -30);
    let oversize = format!(
        "{{\"hook_event_name\":\"StopFailure\",\"session_id\":\"s\",\"error_details\":\"{}\"}}",
        "x".repeat(1024 * 1024 + 1)
    );
    let inputs: [&[u8]; 4] = [b"", b"not json", b"\xff\xfe\x00", oversize.as_bytes()];
    for (n, stdin) in inputs.into_iter().enumerate() {
        let out = rig.run_plain(Some("1"), stdin);
        assert_zero_with_no_output(&out);
        let log = rig.resume_log();
        assert_eq!(lines(&log).len(), n + 1, "{log}");
        let last = lines(&log).pop().unwrap();
        assert_eq!(field(last, "outcome"), Some("bad-payload"));
        assert_eq!(field(last, "session"), Some("-"));
        assert_eq!(field(last, "event"), Some("-"));
        assert_eq!(field(last, "error"), Some(""));
    }
    assert!(rig.tmux_calls().is_empty());
}

// ── A line that cannot be written is recorded ───────────────────────────────

#[test]
fn an_unprivate_resume_folder_is_left_alone_and_one_line_goes_to_stderr() {
    let rig = Rig::new(LIMIT_MENU, false);
    let _rotator = StandInRotator::start(&rig.root);
    harness::write_ledger(&rig.root, "swapped", -30);
    let dir = rig.resume_dir();
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o777)).unwrap();
    let out = rig.run_plain(Some("1"), &payload(json!("server_error")));
    assert_zero_with_one_stderr_line(&out);
    assert_eq!(
        fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
        0o777
    );
    assert_eq!(
        fs::read_dir(&dir).unwrap().count(),
        0,
        "nothing is written into an unprivate folder, not even a marker"
    );
}

#[test]
fn an_unopenable_log_in_a_private_folder_leaves_the_marker_and_one_stderr_line() {
    let rig = Rig::new(LIMIT_MENU, false);
    let _rotator = StandInRotator::start(&rig.root);
    harness::write_ledger(&rig.root, "swapped", -30);
    let dir = rig.private_resume_dir();
    let target = rig.sandbox.root().join("elsewhere");
    fs::write(&target, b"keep").unwrap();
    symlink(&target, dir.join("resume.log")).unwrap();

    let out = rig.run_plain(Some("1"), &payload(json!("server_error")));
    assert_zero_with_one_stderr_line(&out);
    assert_eq!(fs::read(&target).unwrap(), b"keep", "never written through");

    let marker = dir.join("log-write-failed");
    let text = fs::read_to_string(&marker).expect("the marker records the failure");
    assert_eq!(text.lines().count(), 1, "{text:?}");
    assert!(text.contains("not-limit"), "{text:?}");
    assert!(text.contains("open_failed"), "{text:?}");
    assert_eq!(fs::metadata(&marker).unwrap().mode() & 0o7777, 0o600);

    // A second failure overwrites it; it does not grow.
    let out = rig.run_plain(Some("1"), &payload(json!("overloaded")));
    assert_zero_with_one_stderr_line(&out);
    let again = fs::read_to_string(&marker).unwrap();
    assert_eq!(again.lines().count(), 1, "{again:?}");
}

#[test]
fn a_failed_write_never_stops_the_keys() {
    let rig = Rig::new(LIMIT_MENU, false);
    let _rotator = StandInRotator::start(&rig.root);
    let dir = rig.private_resume_dir();
    let transcript = rig.swap_already_happened();
    let target = rig.sandbox.root().join("elsewhere");
    fs::write(&target, b"keep").unwrap();
    symlink(&target, dir.join("resume.log")).unwrap();

    let out = rig.run_on_pty("1", &with_transcript(&recorded(), &transcript));
    assert_zero_with_one_stderr_line(&out);
    assert_eq!(
        rig.send_keys_calls(),
        KEYS,
        "the keys go out although the line cannot be written"
    );
    assert!(dir.join("log-write-failed").exists());
}

#[test]
fn an_unresolvable_profile_root_writes_one_stderr_line() {
    let rig = Rig::new(LIMIT_MENU, false);
    for root in ["relative/root", "/a/../b"] {
        let mut cmd = rig.sandbox.command(env!("CARGO_BIN_EXE_amplihack-hooks"));
        cmd.arg("limit-resume")
            .env("PATH", &rig.standin.bin)
            .env("HOME", &rig.home)
            .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", root)
            .env("AMPLIHACK_CLAUDE_AUTO_SWITCH", "1")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = cmd.spawn().unwrap();
        std::io::Write::write_all(
            &mut child.stdin.take().unwrap(),
            &payload(json!("rate_limit")),
        )
        .unwrap();
        let out = child.wait_with_output().unwrap();
        assert_zero_with_one_stderr_line(&out);
    }
}

// ── Test 3: seven concurrent calls for one swap event ───────────────────────

#[test]
fn seven_concurrent_calls_for_one_swap_write_seven_lines() {
    const CALLS: usize = 7;
    let rig = Rig::new(LIMIT_MENU, true);
    let _rotator = StandInRotator::start(&rig.root);
    rig.private_resume_dir();
    let transcript = rig.swap_already_happened();
    let body = with_transcript(&recorded(), &transcript);

    // Each call has a pty of its own and they share a session, a pane id and
    // a swap event. The stand-in holds every call at its first
    // `display-message`, which each reaches after it has read the guard and
    // before any of them can claim the event. Once all seven are there they
    // are released together, so none can start after another's claim.
    let ptys: Vec<Pty> = (0..CALLS).map(|_| Pty::open()).collect();
    let children: Vec<_> = ptys
        .iter()
        .map(|p| rig.spawn_on_pty("1", &body, p))
        .collect();
    rig.barrier(CALLS);
    let outputs: Vec<_> = children
        .into_iter()
        .map(|c| c.wait_with_output().unwrap())
        .collect();
    drop(ptys);

    for out in &outputs {
        assert_eq!(out.status.code(), Some(0));
        assert!(out.stdout.is_empty());
        assert!(
            out.stderr.is_empty(),
            "stderr: {:?}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let log = rig.resume_log();
    assert_eq!(lines(&log).len(), CALLS, "one line per call: {log}");
    let count = |outcome: &str| {
        lines(&log)
            .into_iter()
            .filter(|l| field(l, "outcome") == Some(outcome))
            .count()
    };
    assert_eq!(count("resumed-keys"), 1, "{log}");
    assert_eq!(count("duplicate"), CALLS - 1, "{log}");
    assert_error_fields_reduced(&log);

    // One delivery: the four keys once, in order.
    assert_eq!(rig.send_keys_calls(), KEYS, "{:?}", rig.tmux_calls());
    assert_eq!(rig.claim_seen(), ["yes", "yes", "yes", "yes"]);
}
