//! Issue #1973: the shared wait helpers in `support::wait`.
//!
//! Tests wait on a condition, not on a fixed delay, under one ceiling of
//! 10 minutes. On timeout a helper fails with a message naming the
//! condition and the time spent, plus at most the last 4 KiB of the child's
//! stderr, and never the child's environment. A child that outlives the
//! ceiling is killed and reaped.
//!
//! Each public helper uses `CEILING`; the `*_with_ceiling` forms take the
//! ceiling as a parameter so that the timeout paths can be tested here in
//! well under a second.

#![cfg(unix)]

use crate::support::wait::{
    CEILING, output_with_ceiling, output_within, wait_child, wait_child_with_ceiling, wait_until,
    wait_until_with_ceiling,
};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

const SHORT: Duration = Duration::from_millis(300);

fn panic_text(f: impl FnOnce()) -> String {
    let payload = catch_unwind(AssertUnwindSafe(f)).expect_err("the helper must panic");
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_string()))
        .expect("panic payload is a string")
}

fn sh(script: &str) -> Command {
    let mut c = Command::new("/bin/sh");
    c.args(["-c", script]);
    c
}

/// A pid that has been reaped has no `/proc` entry; a zombie still has one.
fn assert_reaped(pid_file: &Path) {
    let pid = std::fs::read_to_string(pid_file)
        .expect("the child wrote its pid")
        .trim()
        .to_string();
    assert!(
        !Path::new("/proc").join(&pid).exists(),
        "child {pid} must be killed and reaped at the ceiling"
    );
}

#[test]
fn the_ceiling_is_ten_minutes() {
    assert_eq!(CEILING, Duration::from_secs(600));
}

#[test]
fn wait_until_returns_once_the_condition_holds() {
    let mut calls = 0;
    wait_until("the third poll", || {
        calls += 1;
        calls >= 3
    });
    assert_eq!(calls, 3, "polling stops at the first true result");
}

#[test]
fn wait_until_checks_before_sleeping() {
    let started = std::time::Instant::now();
    wait_until("an already true condition", || true);
    // Not a timing assertion on the product: an immediately true condition
    // is never slept on, so even a loaded host returns well inside 600 s.
    assert!(started.elapsed() < CEILING);
}

#[test]
fn wait_until_names_the_condition_and_the_time_on_timeout() {
    let msg = panic_text(|| wait_until_with_ceiling("the marker appears", SHORT, || false));
    assert!(
        msg.contains("the marker appears"),
        "message must name the condition: {msg}"
    );
    assert!(
        msg.contains("timed out after "),
        "message must give the time spent: {msg}"
    );
}

#[test]
fn output_within_drains_large_stdout_and_stderr() {
    // 1 MiB on each stream: a helper that read only after exit would
    // deadlock on a full pipe.
    let out = output_within(
        "a chatty child",
        &mut sh("head -c 1048576 /dev/zero; head -c 1048576 /dev/zero >&2; exit 3"),
    );
    assert_eq!(out.stdout.len(), 1 << 20);
    assert_eq!(out.stderr.len(), 1 << 20);
    assert_eq!(out.status.code(), Some(3), "the exit status is kept");
}

#[test]
fn output_within_ignores_the_callers_stdio_settings_for_capture() {
    let mut cmd = sh("echo out; echo err >&2");
    cmd.stdout(Stdio::null()).stderr(Stdio::null());
    let out = output_within("a child with null stdio", &mut cmd);
    assert_eq!(out.stdout, b"out\n");
    assert_eq!(out.stderr, b"err\n");
}

/// The child must write its pid and stderr before the ceiling for the
/// assertions to mean anything. On a loaded host it may not manage that in
/// a short ceiling, so each attempt doubles the ceiling (a condition wait on
/// a test input) until one attempt's message shows the stderr tail.
#[test]
fn output_kills_and_reaps_a_hung_child_and_reports_only_the_stderr_tail() {
    let dir = tempfile::tempdir().unwrap();
    let mut ceiling = SHORT;
    let mut spent = Duration::ZERO;
    let msg = loop {
        let pid_file = dir.path().join(format!("pid-{}", ceiling.as_millis()));
        let mut cmd = sh(
            "echo $$ > \"$PID_FILE\"; head -c 10240 /dev/zero | tr '\\0' x >&2; \
             echo TAIL-MARK >&2; exec sleep 600",
        );
        cmd.env("PID_FILE", &pid_file)
            .env("SECRET_MARK", "env-value-must-not-be-printed");
        let msg = panic_text(|| {
            output_with_ceiling("the hung child exits", &mut cmd, ceiling);
        });
        assert!(msg.contains("the hung child exits"), "{msg}");
        assert!(msg.contains("timed out after "), "{msg}");
        assert!(
            !msg.contains("env-value-must-not-be-printed"),
            "the environment is never shown: {msg}"
        );
        if pid_file.exists() {
            assert_reaped(&pid_file);
        }
        if msg.contains("TAIL-MARK") {
            assert!(
                pid_file.exists(),
                "the child ran before its stderr was read"
            );
            break msg;
        }
        spent += ceiling;
        assert!(
            spent < CEILING,
            "the child never wrote its stderr tail within {spent:?}"
        );
        ceiling *= 2;
    };
    assert!(
        msg.len() < 4096 + 1024,
        "at most 4 KiB of stderr is shown, got {} bytes",
        msg.len()
    );
}

#[test]
fn output_within_fails_with_the_condition_when_the_program_cannot_start() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("no-such-program");
    let msg = panic_text(|| {
        output_within("the missing program runs", &mut Command::new(&missing));
    });
    assert!(msg.contains("the missing program runs"), "{msg}");
}

#[test]
fn wait_child_returns_the_exit_status() {
    let mut child = sh("exit 7").spawn().unwrap();
    let status = wait_child("a child that exits 7", &mut child);
    assert_eq!(status.code(), Some(7));
}

#[test]
fn wait_child_kills_and_reaps_at_the_ceiling() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("pid");
    let mut child = sh("echo $$ > \"$PID_FILE\"; exec sleep 600")
        .env("PID_FILE", &pid_file)
        .spawn()
        .unwrap();
    wait_until("the child writes its pid", || {
        std::fs::read_to_string(&pid_file).is_ok_and(|s| s.ends_with('\n'))
    });
    let msg = panic_text(|| {
        wait_child_with_ceiling("the sleeper exits", &mut child, SHORT);
    });
    assert!(msg.contains("the sleeper exits"), "{msg}");
    assert!(msg.contains("timed out after "), "{msg}");
    assert_reaped(&pid_file);
}
