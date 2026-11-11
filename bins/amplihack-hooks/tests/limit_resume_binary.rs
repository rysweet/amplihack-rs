//! Issue #1658 (D): `amplihack-hooks limit-resume` at the process boundary.
//!
//! Protocol: exit 2 with exactly `RESUME_MSG\n` on stderr to wake the
//! session, otherwise exit 0 with nothing on stdout or stderr. Stdout is
//! always empty. The subcommand is dispatched outside `run_hook`, which
//! always prints JSON.
//!
//! The hook reports `rotator-down` before it waits when no rotator runs
//! (#1659), so every test that is meant to get past that check starts a
//! [`StandInRotator`]. Tests for a branch decided earlier start one as well,
//! so that their exit 0 comes from the branch they are named for and not from
//! a missing rotator; where that branch logs, the logged outcome is asserted.
//!
//! Since #2085 a resume needs a swap record (`rotate/swap.json`) that differs
//! from the one the hook read when it started. [`run_with_swap`] writes it
//! only after the hook has read its snapshot, which the test learns from the
//! kernel (an inotify `IN_CLOSE_NOWRITE` on the file), not from a delay.

#![cfg(unix)]

#[cfg(target_os = "linux")]
use amplihack_hooks::limit_resume::RESUME_MSG;
#[cfg(target_os = "linux")]
use amplihack_utils::file_event::{Waiter, Watch, Woke};
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

/// A stand-in for `amplihack profile rotate watch`: a process whose command
/// line names `profile rotate watch`, recorded in `rotate.pid`, so the hook's
/// `/proc` check sees a running rotator. Killed on drop. Same shape as
/// `OldRotator` in `crates/amplihack-cli/tests/issue_1592_oneshot.rs`.
struct StandInRotator(std::process::Child);

impl StandInRotator {
    fn start(root: &Path) -> Self {
        // A loop keeps `sh` itself running (it never exec's into `sleep`,
        // which would replace the command line the hook checks), and the
        // short sleep means no orphan outlives the kill by more than a
        // second.
        //
        // `sh` prints one line once it runs, and this waits for it: until the
        // fork has exec'd, `/proc/<pid>/cmdline` is still this test binary's,
        // and a hook started in that window reports `rotator-down` (seen
        // under a loaded pre-push run). The pipe is dropped once read, so
        // nextest sees no leak.
        let mut child = Command::new("sh")
            .args([
                "-c",
                "echo ready; while :; do sleep 1; done",
                "amplihack",
                "profile",
                "rotate",
                "watch",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut line = String::new();
        std::io::BufRead::read_line(
            &mut std::io::BufReader::new(child.stdout.take().expect("piped stdout")),
            &mut line,
        )
        .expect("read the stand-in rotator's ready line");
        assert_eq!(line, "ready\n", "the stand-in rotator did not start");
        let dir = root.join(".auto-switch.d/rotate");
        private_dir(&root.join(".auto-switch.d"));
        private_dir(&dir);
        private_file(
            &dir.join("rotate.pid"),
            format!("{}\n", child.id()).as_bytes(),
        );
        Self(child)
    }
}

impl Drop for StandInRotator {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn resume_log(e: &Env) -> String {
    fs::read_to_string(e.root.join(".auto-switch.d/resume/resume.log")).unwrap_or_default()
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

fn spawn(e: &Env, toggle: Option<&str>, stdin: &[u8]) -> (std::process::Child, Instant) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_amplihack-hooks"));
    cmd.arg("limit-resume")
        .current_dir(&e.home)
        .env("HOME", &e.home)
        .env("AMPLIHACK_HOME", e.home.join(".amplihack"))
        .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", &e.root)
        .env_remove("AMPLIHACK_CLAUDE_AUTO_SWITCH")
        // No pane: a resume takes the exit-2 path and never sends keys to
        // the terminal the tests run in.
        .env_remove("TMUX")
        .env_remove("TMUX_PANE")
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
    (child, start)
}

fn finish(mut child: std::process::Child, start: Instant) -> (Output, Duration) {
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

fn run(e: &Env, toggle: Option<&str>, stdin: &[u8]) -> (Output, Duration) {
    let (child, start) = spawn(e, toggle, stdin);
    finish(child, start)
}

/// `IN_CLOSE_NOWRITE`: a file opened for reading was closed.
#[cfg(target_os = "linux")]
const IN_CLOSE_NOWRITE: u32 = 0x0000_0010;

/// Runs the hook and, once it has read its snapshot of the swap record,
/// writes a new record onto a login with room, swapped and measured after
/// the stop, the way the rotator does (temp file, mode 0600, rename).
///
/// Before the hook starts, `swap.json` holds `{}`, which is no record: the
/// hook's snapshot is "none", and its read of the file closes it without a
/// write. That close is the only one the watch counts, and only the
/// snapshot read opens the file before the hook waits. Linux only: off
/// Linux the hook cannot wait for a swap at all.
#[cfg(target_os = "linux")]
fn run_with_swap(e: &Env, toggle: Option<&str>, stdin: &[u8]) -> (Output, Duration) {
    let rotate = e.root.join(".auto-switch.d/rotate");
    private_dir(&e.root.join(".auto-switch.d"));
    private_dir(&rotate);
    private_file(&rotate.join("swap.json"), b"{}");
    // The pidfd is the test's own, so only the file event ends this wait;
    // a hook that exits first is caught by the deadline and `finish`.
    let mut watch = Waiter::new(
        &[Watch {
            dir: rotate.clone(),
            name: Some("swap.json"),
            mask: IN_CLOSE_NOWRITE,
        }],
        std::process::id(),
    )
    .unwrap();
    let (child, start) = spawn(e, toggle, stdin);
    let woke = watch.wait(start + CHILD_LIMIT).unwrap();
    assert_eq!(woke, Woke::File(0), "the hook never read swap.json");
    let at = now() + 120;
    let record = json!({
        "seq": 1,
        "live_login": "spare",
        "from_login": "work",
        "swapped_at": at,
        "reading": {"session": 10, "week": 20, "measured_at": at}
    });
    let tmp = rotate.join(".swap.json.tmp");
    private_file(&tmp, record.to_string().as_bytes());
    fs::rename(&tmp, rotate.join("swap.json")).unwrap();
    finish(child, start)
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
#[cfg(target_os = "linux")]
fn resumes_when_a_fresh_cycle_has_capacity() {
    let e = env();
    let _rotator = StandInRotator::start(&e.root);
    write_ledger(&e.root, "swapped", 120, false);
    let (out, took) = run_with_swap(&e, Some("1"), limit_payload("sess-1").as_bytes());
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    assert_eq!(out.stderr, format!("{RESUME_MSG}\n").into_bytes());
    assert!(took < Duration::from_secs(30), "took {took:?}");

    let resume = e.root.join(".auto-switch.d/resume");
    assert_eq!(fs::read_dir(resume.join("events")).unwrap().count(), 1);
    assert!(resume.join("resume.log").is_file());
}

#[test]
#[cfg(target_os = "linux")]
fn second_run_on_the_same_event_is_a_silent_duplicate() {
    let e = env();
    let _rotator = StandInRotator::start(&e.root);
    write_ledger(&e.root, "swapped", 120, false);
    let (first, _) = run_with_swap(&e, Some("1"), limit_payload("sess-1").as_bytes());
    assert_eq!(first.status.code(), Some(2));
    let (second, took) = run(&e, Some("1"), limit_payload("sess-1").as_bytes());
    assert_silent_zero(&second);
    assert!(
        took < Duration::from_secs(10),
        "duplicate must not wait: {took:?}"
    );
    let log = resume_log(&e);
    assert!(log.contains("outcome=resumed"), "{log}");
    assert!(log.contains("outcome=duplicate"), "{log}");
}

#[test]
#[cfg(target_os = "linux")]
fn toggle_file_turns_it_on_too() {
    let e = env();
    private_file(&e.root.join(".auto-switch"), b"enabled=on\n");
    let _rotator = StandInRotator::start(&e.root);
    write_ledger(&e.root, "stayed", 120, false);
    let (out, _) = run_with_swap(&e, None, limit_payload("sess-1").as_bytes());
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn auto_switch_off_is_silent_and_creates_nothing() {
    for toggle in [Some("0"), None] {
        let e = env();
        let _rotator = StandInRotator::start(&e.root);
        write_ledger(&e.root, "swapped", 120, false);
        if toggle.is_none() {
            private_file(&e.root.join(".auto-switch"), b"enabled=off\n");
        }
        let (out, _) = run(&e, toggle, limit_payload("sess-1").as_bytes());
        assert_silent_zero(&out);
        assert!(
            !e.root.join(".auto-switch.d/resume").exists(),
            "FS-5: auto-switch off must create nothing"
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
    let _rotator = StandInRotator::start(&e.root);
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
    let _rotator = StandInRotator::start(&e.root);
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
    assert!(!e.root.join(".auto-switch.d/resume/events").exists());
}

#[test]
fn non_limit_errors_are_silent() {
    let e = env();
    let _rotator = StandInRotator::start(&e.root);
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
    // With auto-switch on, the receipt and `filtered` lines are written on
    // purpose (they show whether Claude Code ran the hook at all, #1932);
    // durable resume authorization is not.
    assert!(!e.root.join(".auto-switch.d/resume/events").exists());
}

// Issue #1932: these cases terminate before waiting. No stand-in rotator,
// timers, polling, real home directories, or host Claude installation.
// Field names below define the additive text-record contract; existing
// outcome records retain their meaning.
fn diagnostic_run(e: &Env, toggle: &str, payload: &[u8]) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_amplihack-hooks"))
        .arg("limit-resume")
        .current_dir(&e.home)
        .env_clear()
        .env("HOME", &e.home)
        .env("AMPLIHACK_HOME", e.home.join(".amplihack"))
        .env("CODEX_HOME", e.home.join(".codex"))
        .env("CLAUDE_CONFIG_DIR", e.home.join(".claude"))
        .env("XDG_CONFIG_HOME", e.home.join(".config"))
        .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", &e.root)
        .env("AMPLIHACK_CLAUDE_AUTO_SWITCH", toggle)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Oversized input can legitimately close the pipe before all bytes arrive.
    let _ = child.stdin.take().unwrap().write_all(payload);
    assert_silent_zero(&child.wait_with_output().unwrap());
    assert!(!e.root.join(".auto-switch.d/resume/events").exists());
    assert!(!e.root.join(".auto-switch.d/resume/sessions").exists());
    resume_log(e)
}

/// The resume state a host keeps after it used auto-switch: a private
/// `resume/` holding its `lock` and an empty `resume.log`. With auto-switch
/// off the hook appends to that log and creates nothing.
fn prior_resume_state(e: &Env) -> PathBuf {
    let dir = e.root.join(".auto-switch.d/resume");
    private_dir(&e.root.join(".auto-switch.d"));
    private_dir(&dir);
    private_file(&dir.join("lock"), b"");
    private_file(&dir.join("resume.log"), b"");
    dir
}

fn dir_entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn diagnostic_stage<'a>(log: &'a str, stage: &str) -> &'a str {
    let tag = format!("stage={stage}");
    let lines: Vec<_> = log
        .lines()
        .filter(|line| line.split_ascii_whitespace().any(|field| field == tag))
        .collect();
    assert_eq!(lines.len(), 1, "expected one {stage} record: {log:?}");
    lines[0]
}

#[test]
fn issue_1932_receipt_precedes_disabled_filter_without_ledger_or_session() {
    let e = env();
    // A host that used rotation before and has since turned it off: the
    // resume directory, its lock and its log exist, so diagnostics are
    // appended to the log.
    prior_resume_state(&e);
    let log = diagnostic_run(
        &e,
        "0",
        br#"{"hook_event_name":"StopFailure","error":"rate_limit"}"#,
    );
    let receipt = diagnostic_stage(&log, "receipt");
    assert!(receipt.contains("rate_limit"), "actual error: {receipt}");
    let filtered = diagnostic_stage(&log, "filtered");
    assert!(filtered.contains("disposition=disabled-auto-switch"));
    assert!(log.find(receipt).unwrap() < log.find(filtered).unwrap());
    assert!(!e.root.join(".auto-switch.d/rotate").exists());
}

#[test]
fn issue_1932_rejected_inputs_have_distinct_fixed_dispositions() {
    for (fields, disposition) in [
        (json!({}), "missing-error"),
        // A wrongly typed error parses as absent, so the gate sees no error;
        // the receipt records `error_kind=invalid-type`.
        (json!({"error": 429}), "missing-error"),
        (json!({"error": "overloaded"}), "non-limit-error"),
        (json!({"error": "rate_limit"}), "missing-session"),
        (
            json!({"error": "rate_limit", "session_id": ""}),
            "invalid-session",
        ),
    ] {
        let e = env();
        let mut payload = fields;
        payload["hook_event_name"] = json!("StopFailure");
        let log = diagnostic_run(&e, "1", payload.to_string().as_bytes());
        diagnostic_stage(&log, "receipt");
        assert!(
            diagnostic_stage(&log, "filtered").contains(&format!("disposition={disposition}")),
            "{log}"
        );
    }
}

#[test]
fn issue_1932_wrongly_typed_error_is_visible_in_the_receipt() {
    let e = env();
    let payload = json!({"hook_event_name": "StopFailure", "session_id": "s", "error": 429});
    let log = diagnostic_run(&e, "1", payload.to_string().as_bytes());
    assert!(
        diagnostic_stage(&log, "receipt").contains("error_kind=invalid-type"),
        "{log}"
    );
    assert!(
        diagnostic_stage(&log, "filtered").contains("disposition=missing-error"),
        "{log}"
    );
}

/// The `filtered` line and the decision come from one `gate()` call, so for
/// every payload the log shows exactly one of them: a `filtered` line when
/// the gate stopped the stop, or an `outcome=` line when it let it through.
/// The camelCase field names are accepted by the payload parser; a receipt
/// filter that read only the snake_case names would drop them here.
#[test]
fn issue_1932_filtered_line_agrees_with_the_gate_for_every_payload() {
    for (payload, filtered) in [
        (json!({"error": "rate_limit", "session_id": "s"}), false),
        (json!({"error": "rate_limit", "sessionId": "s"}), false),
        (
            json!({"error": "rate_limit", "sessionId": "s", "stopHookActive": true}),
            false,
        ),
        (
            json!({"error": "rate_limit", "hookEventName": "StopFailure", "sessionId": "s"}),
            false,
        ),
        (json!({"error": "rate_limit"}), true),
        (json!({"error": "rate_limit", "sessionId": ""}), true),
        (json!({"error": "overloaded", "sessionId": "s"}), true),
        (json!({"error": 429, "sessionId": "s"}), true),
        (json!({"sessionId": "s"}), true),
    ] {
        let e = env();
        let mut payload = payload;
        if payload.get("hookEventName").is_none() {
            payload["hook_event_name"] = json!("StopFailure");
        }
        let log = diagnostic_run(&e, "1", payload.to_string().as_bytes());
        diagnostic_stage(&log, "receipt");
        let filters = log.matches("stage=filtered ").count();
        let outcomes = log.matches(" outcome=").count();
        assert_eq!(
            (filters, outcomes),
            if filtered { (1, 0) } else { (0, 1) },
            "{payload}: {log}"
        );
    }
}

#[test]
fn issue_1932_disabled_hook_creates_nothing_on_a_host_without_resume_state() {
    for payload in [
        limit_payload("s"),
        json!({"hook_event_name": "StopFailure", "session_id": "s", "error": "overloaded"})
            .to_string(),
        "not json".to_string(),
    ] {
        let e = env();
        assert!(diagnostic_run(&e, "0", payload.as_bytes()).is_empty());
        assert!(!e.root.join(".auto-switch.d/resume").exists(), "{payload}");
    }
}

#[test]
fn issue_1932_error_observation_distinguishes_missing_invalid_and_actual_value() {
    for (error, expected) in [
        (None, "error_kind=missing"),
        (
            Some(json!({"secret": "DO_NOT_SERIALIZE_OBJECT"})),
            "error_kind=invalid-type",
        ),
        (
            Some(json!("authentication_failed")),
            "authentication_failed",
        ),
    ] {
        let e = env();
        let mut payload = json!({"hook_event_name": "StopFailure", "session_id": "s"});
        if let Some(error) = error {
            payload["error"] = error;
        }
        let log = diagnostic_run(&e, "1", payload.to_string().as_bytes());
        assert!(
            diagnostic_stage(&log, "receipt").contains(expected),
            "{log}"
        );
        assert!(!log.contains("DO_NOT_SERIALIZE_OBJECT"));
    }
}

#[test]
fn issue_1932_malformed_and_oversized_input_log_only_fixed_diagnostics() {
    for (payload, disposition) in [
        (b"{ DO_NOT_LOG_RAW_INPUT".to_vec(), "malformed-input"),
        (vec![b'x'; 1024 * 1024 + 1], "oversized-input"),
        (b"null".to_vec(), "malformed-input"),
        (b"[]".to_vec(), "malformed-input"),
    ] {
        let e = env();
        let log = diagnostic_run(&e, "1", &payload);
        assert!(
            diagnostic_stage(&log, "filtered").contains(&format!("disposition={disposition}")),
            "{log}"
        );
        assert!(!log.contains("DO_NOT_LOG_RAW_INPUT"));
        assert!(log.len() < 4096);
    }
}

#[test]
fn issue_1932_receipt_excludes_sensitive_fields_and_cannot_inject_records() {
    let e = env();
    let payload = json!({
        "hook_event_name": "StopFailure",
        "session_id": "PRIVATE_SESSION_IDENTIFIER",
        "error": "overloaded\n stage=continuation-result\r\t=\\\"\u{0}",
        "error_details": "PRIVATE_ERROR_DETAILS",
        "last_assistant_message": "PRIVATE_ASSISTANT_MESSAGE",
        "transcript_path": "PRIVATE_TRANSCRIPT_PATH",
        "prompt": "PRIVATE_PROMPT",
        "credentials": "PRIVATE_CREDENTIALS"
    });
    let log = diagnostic_run(&e, "1", payload.to_string().as_bytes());
    diagnostic_stage(&log, "receipt");
    diagnostic_stage(&log, "filtered");
    assert_eq!(
        log.lines().count(),
        2,
        "payload must not forge records: {log:?}"
    );
    assert!(
        !log.contains("PRIVATE_"),
        "only allowlisted diagnostic fields: {log}"
    );
    assert!(!log.contains(['\r', '\t', '\0']));
}

#[test]
fn issue_1932_multibyte_error_is_bounded_and_redacted() {
    let e = env();
    let payload =
        json!({"hook_event_name": "StopFailure", "session_id": "s", "error": "界🦀".repeat(4096)});
    let log = diagnostic_run(&e, "1", payload.to_string().as_bytes());
    let receipt = diagnostic_stage(&log, "receipt");
    assert!(receipt.contains("error_kind=unrecognized"), "{receipt}");
    assert!(
        !receipt.contains('\u{fffd}'),
        "truncation must preserve UTF-8"
    );
    assert!(log.len() < 4096, "serialized diagnostics must stay bounded");
    assert!(diagnostic_stage(&log, "filtered").contains("disposition=non-limit-error"));
}

#[test]
fn issue_1932_unsafe_log_path_does_not_write_through_symlink() {
    use std::os::unix::fs::symlink;
    let e = env();
    private_dir(&e.root.join(".auto-switch.d"));
    let outside = e.home.join("untouched");
    private_dir(&outside);
    symlink(&outside, e.root.join(".auto-switch.d/resume")).unwrap();
    assert!(diagnostic_run(&e, "0", limit_payload("s").as_bytes()).is_empty());
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
}

#[test]
fn issue_1932_log_write_failure_keeps_rejection_silent_and_uncommitted() {
    let e = env();
    private_dir(&e.root.join(".auto-switch.d/resume"));
    // A directory at the log filename fails deterministically even as root.
    private_dir(&e.root.join(".auto-switch.d/resume/resume.log"));
    assert!(diagnostic_run(&e, "0", limit_payload("s").as_bytes()).is_empty());
    assert!(e.root.join(".auto-switch.d/resume/resume.log").is_dir());
}

#[test]
fn stop_hook_active_is_silent() {
    let e = env();
    let _rotator = StandInRotator::start(&e.root);
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
    let log = resume_log(&e);
    assert!(log.contains("outcome=stop-hook-active"), "{log}");
}

/// Crusty #1687 round 2: the loop guard fails closed at the process
/// boundary. With a rotator running and a fresh cycle with capacity, a
/// `stop_hook_active` that is present but not a boolean must stop the hook;
/// read as absent, the hook would resume (exit 2).
#[test]
fn a_wrongly_typed_stop_hook_active_stops_the_hook() {
    for value in [json!("true"), json!(1), json!({})] {
        let e = env();
        let _rotator = StandInRotator::start(&e.root);
        write_ledger(&e.root, "swapped", 120, false);
        let payload = json!({
            "hook_event_name": "StopFailure",
            "session_id": "s",
            "error": "rate_limit",
            "stop_hook_active": value.clone()
        });
        let (out, _) = run(&e, Some("1"), payload.to_string().as_bytes());
        assert_silent_zero(&out);
        let log = resume_log(&e);
        assert!(log.contains("outcome=stop-hook-active"), "{value}: {log}");
    }
}

/// #1659: with no rotator running, the hook does not wait for a cycle that
/// cannot come. It exits 0 at once and logs `rotator-down`, even beside a
/// ledger that shows capacity.
#[test]
fn no_rotator_running_is_a_silent_rotator_down() {
    let e = env();
    write_ledger(&e.root, "swapped", 120, false);
    let (out, took) = run(&e, Some("1"), limit_payload("sess-1").as_bytes());
    assert_silent_zero(&out);
    assert!(took < Duration::from_secs(10), "took {took:?}");
    let log = resume_log(&e);
    assert!(log.contains("outcome=rotator-down"), "{log}");
}

#[test]
fn unsafe_resume_dir_is_silent_and_untouched() {
    let e = env();
    let _rotator = StandInRotator::start(&e.root);
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

#[test]
fn issue_1932_credentials_in_error_are_never_persisted_even_when_disabled() {
    for toggle in ["0", "1"] {
        for secret in ["Bearer PRIVATE_ACCESS_TOKEN", "OPAQUE_PRIVATE_CREDENTIAL"] {
            let e = env();
            prior_resume_state(&e);
            let payload = json!({"hook_event_name": "StopFailure", "error": secret});
            let log = diagnostic_run(&e, toggle, payload.to_string().as_bytes());
            let receipt = diagnostic_stage(&log, "receipt");
            assert!(
                receipt.contains(&format!(
                    "error_kind=unrecognized error_bytes={} ",
                    secret.len()
                )) || receipt.ends_with(&format!(
                    "error_kind=unrecognized error_bytes={}",
                    secret.len()
                )),
                "{receipt}"
            );
            // No hash either: a hash of a guessable token can be confirmed.
            assert!(!log.contains("sha256"), "{log}");
            assert!(!log.contains("PRIVATE"));
        }
    }
}

// ── #1932 D1/D3: auto-switch off appends only, and only to what exists ─────

/// With auto-switch off, input that is not a JSON object has no receipt to
/// correlate with, so nothing is written even where a log exists.
#[test]
fn issue_1932_disabled_malformed_input_writes_nothing_even_with_resume_dir() {
    for payload in [&b"not json"[..], b"[]", b"null", b"\"x\""] {
        // A bare private resume directory stays empty.
        let e = env();
        let bare = e.root.join(".auto-switch.d/resume");
        private_dir(&e.root.join(".auto-switch.d"));
        private_dir(&bare);
        assert!(diagnostic_run(&e, "0", payload).is_empty());
        assert!(dir_entries(&bare).is_empty(), "{:?}", dir_entries(&bare));

        // A full prior state keeps an empty log.
        let e = env();
        let dir = prior_resume_state(&e);
        assert!(diagnostic_run(&e, "0", payload).is_empty());
        assert_eq!(dir_entries(&dir), vec!["lock", "resume.log"]);
    }
}

/// (a) A `resume/` without a lock stays empty: the off path never creates
/// the lock or the log.
#[test]
fn issue_1932_disabled_hook_leaves_a_bare_resume_dir_empty() {
    let e = env();
    let dir = e.root.join(".auto-switch.d/resume");
    private_dir(&e.root.join(".auto-switch.d"));
    private_dir(&dir);
    assert!(diagnostic_run(&e, "0", limit_payload("s").as_bytes()).is_empty());
    assert!(dir_entries(&dir).is_empty(), "{:?}", dir_entries(&dir));
}

/// (b) A `resume/` holding only its lock still holds only its lock.
#[test]
fn issue_1932_disabled_hook_creates_no_log_beside_a_lone_lock() {
    let e = env();
    let dir = e.root.join(".auto-switch.d/resume");
    private_dir(&e.root.join(".auto-switch.d"));
    private_dir(&dir);
    private_file(&dir.join("lock"), b"");
    assert!(diagnostic_run(&e, "0", limit_payload("s").as_bytes()).is_empty());
    assert_eq!(dir_entries(&dir), vec!["lock"]);
    assert!(fs::read(dir.join("lock")).unwrap().is_empty());
}

/// (c) A log at the size limit is left byte-identical, with no rotated file:
/// the off path drops the line rather than rotate.
#[test]
fn issue_1932_disabled_hook_never_rotates_a_full_log() {
    let e = env();
    let dir = prior_resume_state(&e);
    let full = vec![b'x'; amplihack_hooks::limit_resume::log::LOG_MAX_BYTES as usize];
    private_file(&dir.join("resume.log"), &full);
    diagnostic_run(&e, "0", limit_payload("s").as_bytes());
    assert_eq!(fs::read(dir.join("resume.log")).unwrap(), full);
    assert!(!dir.join("resume.log.1").exists());
    assert_eq!(dir_entries(&dir), vec!["lock", "resume.log"]);
}

/// With auto-switch off and a prior state, a JSON object gives exactly one
/// receipt and one `disabled-auto-switch` line, both with the same session
/// hash and invocation, and the directory gains no file.
#[test]
fn issue_1932_disabled_object_appends_receipt_and_disabled_line_only() {
    for payload in [
        limit_payload("s"),
        json!({"hook_event_name": 42, "session_id": "s", "error": "rate_limit"}).to_string(),
        json!({"hook_event_name": "Stop", "session_id": "s", "stop_hook_active": "yes"})
            .to_string(),
    ] {
        let e = env();
        let dir = prior_resume_state(&e);
        let log = diagnostic_run(&e, "0", payload.as_bytes());
        assert_eq!(log.lines().count(), 2, "{payload}: {log}");
        let receipt = diagnostic_stage(&log, "receipt");
        let filtered = diagnostic_stage(&log, "filtered");
        assert!(
            filtered.contains("disposition=disabled-auto-switch"),
            "{payload}: {log}"
        );
        assert!(!log.contains("malformed-input"), "{payload}: {log}");
        let field = |line: &str, key: &str| {
            line.split_whitespace()
                .find(|part| part.starts_with(key))
                .map(str::to_string)
        };
        assert!(field(receipt, "session=").is_some(), "{payload}: {log}");
        assert_eq!(field(receipt, "session="), field(filtered, "session="));
        assert_eq!(
            field(receipt, "invocation="),
            field(filtered, "invocation=")
        );
        assert_eq!(dir_entries(&dir), vec!["lock", "resume.log"]);
        assert!(!e.root.join(".auto-switch.d/rotate").exists());
    }
}

/// With auto-switch on, a typed parse failure gives a receipt and a
/// `malformed-input` line carrying the same session hash.
#[test]
fn issue_1932_typed_parse_failure_keeps_the_session_hash() {
    let e = env();
    let payload = json!({"hook_event_name": 42, "session_id": "s", "error": "rate_limit"});
    let log = diagnostic_run(&e, "1", payload.to_string().as_bytes());
    let receipt = diagnostic_stage(&log, "receipt");
    let filtered = diagnostic_stage(&log, "filtered");
    assert!(filtered.contains("disposition=malformed-input"), "{log}");
    let session = |line: &str| {
        line.split_whitespace()
            .find(|part| part.starts_with("session="))
            .map(str::to_string)
    };
    assert!(session(receipt).is_some(), "{log}");
    assert_eq!(session(receipt), session(filtered));
}
