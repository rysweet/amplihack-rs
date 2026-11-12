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
    spawn_via(e, toggle, stdin, false)
}

/// [`spawn`], optionally through `/bin/sh -c` the way Claude Code runs a
/// hook command. Where `/bin/sh` is dash, the shell does not `exec` the
/// command, so the hook's parent is the short-lived shell, not the session
/// (crusty, PR #2295 round 3).
fn spawn_via(
    e: &Env,
    toggle: Option<&str>,
    stdin: &[u8],
    shell: bool,
) -> (std::process::Child, Instant) {
    let bin = env!("CARGO_BIN_EXE_amplihack-hooks");
    let mut cmd = if shell {
        let mut c = Command::new("/bin/sh");
        c.args(["-c", &format!("\"{bin}\" limit-resume")]);
        c
    } else {
        let mut c = Command::new(bin);
        c.arg("limit-resume");
        c
    };
    cmd.current_dir(&e.home)
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
    run_with_swap_record(e, toggle, stdin, json!({}), false)
}

/// [`run_with_swap`], with `extra` merged into the record written, and the
/// hook spawned through `/bin/sh -c` when `shell` is set.
#[cfg(target_os = "linux")]
fn run_with_swap_record(
    e: &Env,
    toggle: Option<&str>,
    stdin: &[u8],
    extra: Value,
    shell: bool,
) -> (Output, Duration) {
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
    let (child, start) = spawn_via(e, toggle, stdin, shell);
    let woke = watch.wait(start + CHILD_LIMIT).unwrap();
    assert_eq!(woke, Woke::File(0), "the hook never read swap.json");
    let at = now() + 120;
    let mut record = json!({
        "seq": 1,
        "live_login": "spare",
        "from_login": "work",
        "swapped_at": at,
        "reading": {"session": 10, "week": 20, "measured_at": at}
    });
    if let (Some(r), Some(x)) = (record.as_object_mut(), extra.as_object()) {
        r.extend(x.clone());
    }
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

/// Issue #2136, crusty PR #2295 round 3: the organisation warning goes out
/// with the resume that follows the swap, read from that swap's record, and
/// depends on no process identity. Run through `/bin/sh -c` as Claude Code
/// runs it, twice: the crossing record's warning reaches the first wake,
/// and the second wake, after a same-organisation swap, carries none. No
/// state is kept between the two calls, so nothing depends on which process
/// is the hook's parent.
#[test]
#[cfg(target_os = "linux")]
fn shell_wrapped_hook_delivers_the_warning_from_the_swap_record_alone() {
    let e = env();
    let _rotator = StandInRotator::start(&e.root);
    write_ledger(&e.root, "swapped", 120, false);
    let crossing = json!({"from_org": "org-a", "to_org": "org-b", "affinity": "crossing"});
    let (first, _) = run_with_swap_record(
        &e,
        Some("1"),
        limit_payload("sess-1").as_bytes(),
        crossing,
        true,
    );
    assert_eq!(first.status.code(), Some(2));
    assert!(first.stdout.is_empty());
    let text = String::from_utf8(first.stderr).unwrap();
    let warning = amplihack_utils::organisation_affinity::Affinity::Crossing
        .warning()
        .unwrap();
    assert_eq!(text, format!("{warning}\n{RESUME_MSG}\n"));

    let same = json!({"from_org": "org-a", "to_org": "org-a", "affinity": "same"});
    let (second, _) = run_with_swap_record(
        &e,
        Some("1"),
        limit_payload("sess-2").as_bytes(),
        same,
        true,
    );
    assert_eq!(second.status.code(), Some(2));
    assert_eq!(second.stderr, format!("{RESUME_MSG}\n").into_bytes());
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
    assert!(!e.root.join(".auto-switch.d/resume").exists());
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
    // A stop that is not a usage limit is decided before anything is
    // written or logged.
    assert!(!e.root.join(".auto-switch.d/resume").exists());
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
