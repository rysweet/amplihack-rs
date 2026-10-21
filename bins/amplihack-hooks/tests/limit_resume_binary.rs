//! Issue #1658 (D): `amplihack-hooks limit-resume` at the process boundary.
//!
//! Protocol: exit 2 with exactly `RESUME_MSG\n` on stderr to wake the
//! session, otherwise exit 0 with nothing on stdout or stderr. Stdout is
//! always empty. The subcommand is dispatched outside `run_hook`, which
//! always prints JSON.

#![cfg(unix)]

use amplihack_hooks::limit_resume::RESUME_MSG;
use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

/// Generous, but far below the hook's 20-minute wait: a test that reaches it
/// means the hook polled when it should have decided at once.
const CHILD_LIMIT: Duration = Duration::from_secs(60);

struct Env {
    _home: tempfile::TempDir,
    home: PathBuf,
    root: PathBuf,
}

fn private_dir(p: &Path) {
    fs::create_dir_all(p).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(0o700)).unwrap();
}

fn private_file(p: &Path, bytes: &[u8]) {
    fs::write(p, bytes).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(0o600)).unwrap();
}

fn env() -> Env {
    let home = tempfile::tempdir().unwrap();
    let h = home.path().to_path_buf();
    fs::set_permissions(&h, fs::Permissions::from_mode(0o700)).unwrap();
    let root = h.join("profiles-root");
    private_dir(&root);
    Env {
        _home: home,
        home: h,
        root,
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// A rotate ledger whose last cycle was recorded `offset` seconds from now.
fn write_ledger(root: &Path, outcome: &str, offset: i64, alert: bool) {
    let dir = root.join(".auto-switch.d/rotate");
    private_dir(&root.join(".auto-switch.d"));
    private_dir(&dir);
    let mut l = json!({
        "version": 1,
        "live": "spare",
        "last_cycle": {"at": now() + offset, "outcome": outcome}
    });
    if alert {
        l["alert"] = json!("all-over:-");
    }
    private_file(&dir.join("ledger.json"), l.to_string().as_bytes());
}

fn limit_payload(session: &str) -> String {
    json!({
        "hook_event_name": "StopFailure",
        "session_id": session,
        "transcript_path": "/tmp/t.jsonl",
        "error": "rate_limit",
        "error_details": "429",
        "last_assistant_message": "You've hit your weekly limit · resets 9pm"
    })
    .to_string()
}

fn run(e: &Env, toggle: Option<&str>, stdin: &[u8]) -> (Output, Duration) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_amplihack-hooks"));
    cmd.arg("limit-resume")
        .current_dir(&e.home)
        .env("HOME", &e.home)
        .env("AMPLIHACK_HOME", e.home.join(".amplihack"))
        .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", &e.root)
        .env_remove("AMPLIHACK_CLAUDE_AUTO_SWITCH")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(t) = toggle {
        cmd.env("AMPLIHACK_CLAUDE_AUTO_SWITCH", t);
    }
    let start = Instant::now();
    let mut child = cmd.spawn().unwrap();
    {
        let mut sin = child.stdin.take().unwrap();
        // The hook may stop reading at its 1 MiB cap; a broken pipe is fine.
        let _ = sin.write_all(stdin);
    }
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if start.elapsed() > CHILD_LIMIT {
            let _ = child.kill();
            panic!("limit-resume did not exit within {CHILD_LIMIT:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    (child.wait_with_output().unwrap(), start.elapsed())
}

fn assert_silent_zero(out: &Output) {
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.stdout.is_empty(),
        "stdout: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        out.stderr.is_empty(),
        "stderr: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn resumes_when_a_fresh_cycle_has_capacity() {
    let e = env();
    write_ledger(&e.root, "swapped", 120, false);
    let (out, took) = run(&e, Some("1"), limit_payload("sess-1").as_bytes());
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    assert_eq!(out.stderr, format!("{RESUME_MSG}\n").into_bytes());
    assert!(took < Duration::from_secs(30), "took {took:?}");

    let resume = e.root.join(".auto-switch.d/resume");
    assert_eq!(fs::read_dir(resume.join("events")).unwrap().count(), 1);
    assert!(resume.join("resume.log").is_file());
}

#[test]
fn second_run_on_the_same_event_is_a_silent_duplicate() {
    let e = env();
    write_ledger(&e.root, "swapped", 120, false);
    let (first, _) = run(&e, Some("1"), limit_payload("sess-1").as_bytes());
    assert_eq!(first.status.code(), Some(2));
    let (second, took) = run(&e, Some("1"), limit_payload("sess-1").as_bytes());
    assert_silent_zero(&second);
    assert!(
        took < Duration::from_secs(10),
        "duplicate must not wait: {took:?}"
    );
    let log = fs::read_to_string(e.root.join(".auto-switch.d/resume/resume.log")).unwrap();
    assert!(log.contains("outcome=duplicate"), "{log}");
}

#[test]
fn toggle_file_turns_it_on_too() {
    let e = env();
    private_file(&e.root.join(".auto-switch"), b"enabled=on\n");
    write_ledger(&e.root, "stayed", 120, false);
    let (out, _) = run(&e, None, limit_payload("sess-1").as_bytes());
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn auto_switch_off_is_silent_and_creates_nothing() {
    for toggle in [Some("0"), None] {
        let e = env();
        write_ledger(&e.root, "swapped", 120, false);
        if toggle.is_none() {
            private_file(&e.root.join(".auto-switch"), b"enabled=off\n");
        }
        let (out, took) = run(&e, toggle, limit_payload("sess-1").as_bytes());
        assert_silent_zero(&out);
        assert!(took < Duration::from_secs(10));
        assert!(
            !e.root.join(".auto-switch.d/resume").exists(),
            "FS-5: nothing created when off"
        );
    }
}

#[test]
fn unresolvable_root_is_silent() {
    let e = env();
    let mut cmd_env = Env {
        _home: tempfile::tempdir().unwrap(),
        home: e.home.clone(),
        root: PathBuf::from("relative/root"),
    };
    let (out, _) = run(&cmd_env, Some("1"), limit_payload("s").as_bytes());
    assert_silent_zero(&out);
    cmd_env.root = PathBuf::from("/a/../b");
    let (out, _) = run(&cmd_env, Some("1"), limit_payload("s").as_bytes());
    assert_silent_zero(&out);
}

#[test]
fn garbage_and_malformed_stdin_are_silent() {
    let e = env();
    write_ledger(&e.root, "swapped", 120, false);
    for stdin in [
        b"".to_vec(),
        b"not json".to_vec(),
        b"\xff\xfe\x00".to_vec(),
        json!({"hook_event_name": "StopFailure", "session_id": "s", "error": 429})
            .to_string()
            .into_bytes(),
    ] {
        let (out, _) = run(&e, Some("1"), &stdin);
        assert_silent_zero(&out);
    }
    assert!(!e.root.join(".auto-switch.d/resume/events").exists());
}

#[test]
fn more_than_one_mib_of_stdin_is_silent() {
    let e = env();
    write_ledger(&e.root, "swapped", 120, false);
    let pad = "x".repeat(1024 * 1024 + 1);
    let payload = json!({
        "hook_event_name": "StopFailure",
        "session_id": "s",
        "error": "rate_limit",
        "error_details": pad
    })
    .to_string();
    let (out, _) = run(&e, Some("1"), payload.as_bytes());
    assert_silent_zero(&out);
}

#[test]
fn non_limit_errors_are_silent() {
    let e = env();
    write_ledger(&e.root, "swapped", 120, false);
    for error in ["overloaded", "authentication_failed"] {
        let payload: Value = json!({
            "hook_event_name": "StopFailure",
            "session_id": "s",
            "error": error,
            "last_assistant_message": "You've hit your weekly limit"
        });
        let (out, _) = run(&e, Some("1"), payload.to_string().as_bytes());
        assert_silent_zero(&out);
    }
}

#[test]
fn stop_hook_active_is_silent() {
    let e = env();
    write_ledger(&e.root, "swapped", 120, false);
    let payload = json!({
        "hook_event_name": "StopFailure",
        "session_id": "s",
        "error": "rate_limit",
        "stop_hook_active": true
    });
    let (out, took) = run(&e, Some("1"), payload.to_string().as_bytes());
    assert_silent_zero(&out);
    assert!(took < Duration::from_secs(10));
}

#[test]
fn unsafe_resume_dir_is_silent_and_untouched() {
    let e = env();
    write_ledger(&e.root, "swapped", 120, false);
    let resume = e.root.join(".auto-switch.d/resume");
    fs::create_dir(&resume).unwrap();
    fs::set_permissions(&resume, fs::Permissions::from_mode(0o777)).unwrap();
    let (out, _) = run(&e, Some("1"), limit_payload("s").as_bytes());
    assert_silent_zero(&out);
    assert_eq!(
        fs::metadata(&resume).unwrap().permissions().mode() & 0o777,
        0o777
    );
    assert_eq!(fs::read_dir(&resume).unwrap().count(), 0);
}

#[test]
fn usage_text_lists_limit_resume() {
    let out = Command::new(env!("CARGO_BIN_EXE_amplihack-hooks"))
        .arg("no-such-hook")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("limit-resume"));
}
