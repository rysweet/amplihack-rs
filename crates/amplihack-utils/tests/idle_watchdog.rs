//! TDD red-phase contract tests for `amplihack_utils::idle_watchdog`.
//!
//! Tracking issue: GitHub #867 — replace wall-clock, kill-on-expiry timeouts
//! with idle/liveness detection so a child that keeps producing output is never
//! killed, while a genuinely hung child is reaped after an idle window.
//!
//! These tests exercise the public API surface only. They DELIBERATELY fail
//! until the `idle_watchdog` functions are implemented per the design note; the
//! implementation PR turns them green.
//!
//! Run with:
//!     cargo test -p amplihack-utils --test idle_watchdog
//!
//! The two behaviours the issue requires are asserted for BOTH the async and
//! the sync watchdog:
//!   (a) a child that keeps producing output past the old deadline is NOT killed
//!   (b) a genuinely idle child IS killed after the idle window elapses

use std::process::Stdio;
use std::sync::Mutex;
use std::time::Duration;

use amplihack_utils::idle_watchdog::{
    DEFAULT_IDLE_TIMEOUT_SECS, DEFAULT_POLL_MS, ENV_IDLE_POLL_MS, ENV_IDLE_TIMEOUT_SECS,
    IdleConfig, file_idle_since, wait_with_idle_watchdog, wait_with_idle_watchdog_sync,
};

/// Serializes tests that read or mutate the process environment. Behavioural
/// tests build `IdleConfig` via struct literals and do NOT take this lock.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Build a config directly (env-independent) for deterministic behavioural tests.
fn cfg(idle: Duration, poll: Duration) -> IdleConfig {
    IdleConfig {
        idle_timeout: idle,
        poll,
    }
}

// ---------------------------------------------------------------------------
// IdleConfig
// ---------------------------------------------------------------------------

#[test]
fn config_from_env_defaults() {
    let _guard = ENV_LOCK.lock().unwrap();
    unsafe {
        std::env::remove_var(ENV_IDLE_TIMEOUT_SECS);
        std::env::remove_var(ENV_IDLE_POLL_MS);
    }
    let c = IdleConfig::from_env();
    assert_eq!(
        c.idle_timeout,
        Duration::from_secs(DEFAULT_IDLE_TIMEOUT_SECS)
    );
    assert_eq!(c.poll, Duration::from_millis(DEFAULT_POLL_MS));
}

#[test]
fn config_from_env_override() {
    let _guard = ENV_LOCK.lock().unwrap();
    unsafe {
        std::env::set_var(ENV_IDLE_TIMEOUT_SECS, "42");
        std::env::set_var(ENV_IDLE_POLL_MS, "250");
    }
    let c = IdleConfig::from_env();
    unsafe {
        std::env::remove_var(ENV_IDLE_TIMEOUT_SECS);
        std::env::remove_var(ENV_IDLE_POLL_MS);
    }
    assert_eq!(c.idle_timeout, Duration::from_secs(42));
    assert_eq!(c.poll, Duration::from_millis(250));
}

#[test]
fn config_from_env_ignores_garbage_and_zero() {
    let _guard = ENV_LOCK.lock().unwrap();
    unsafe {
        std::env::set_var(ENV_IDLE_TIMEOUT_SECS, "not-a-number");
        std::env::set_var(ENV_IDLE_POLL_MS, "0");
    }
    let c = IdleConfig::from_env();
    unsafe {
        std::env::remove_var(ENV_IDLE_TIMEOUT_SECS);
        std::env::remove_var(ENV_IDLE_POLL_MS);
    }
    // Non-parsable / zero values must fall back to the defaults, never 0.
    assert_eq!(
        c.idle_timeout,
        Duration::from_secs(DEFAULT_IDLE_TIMEOUT_SECS)
    );
    assert_eq!(c.poll, Duration::from_millis(DEFAULT_POLL_MS));
}

#[test]
fn config_with_idle_overrides_idle_keeps_default_poll() {
    let _guard = ENV_LOCK.lock().unwrap();
    unsafe {
        std::env::remove_var(ENV_IDLE_TIMEOUT_SECS);
        std::env::remove_var(ENV_IDLE_POLL_MS);
    }
    let c = IdleConfig::with_idle(Duration::from_secs(7));
    assert_eq!(c.idle_timeout, Duration::from_secs(7));
    assert_eq!(c.poll, Duration::from_millis(DEFAULT_POLL_MS));
}

// ---------------------------------------------------------------------------
// Async watchdog — wait_with_idle_watchdog
// ---------------------------------------------------------------------------

/// Behaviour (a): a child that keeps emitting output well past the old
/// wall-clock deadline must run to completion, NOT be killed.
#[tokio::test]
async fn async_producing_child_survives_past_old_deadline() {
    // Prints once per second for ~6 s. The historical wall-clock cap was ~5 s.
    let mut child = tokio::process::Command::new("bash")
        .args(["-c", "for i in $(seq 1 6); do echo tick $i; sleep 1; done"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn producing child");
    let (out, err) = (child.stdout.take(), child.stderr.take());

    // Idle window (2 s) is shorter than total runtime (6 s) but longer than the
    // gap between outputs (1 s), so the child must NOT be killed.
    let outcome = wait_with_idle_watchdog(
        &mut child,
        out,
        err,
        cfg(Duration::from_secs(2), Duration::from_millis(100)),
    )
    .await;

    assert!(
        !outcome.killed_for_idle,
        "a continuously-producing child must not be killed"
    );
    assert_eq!(outcome.status.unwrap().code(), Some(0));
    assert!(
        outcome.stdout.contains("tick 6"),
        "all streamed output must be captured, got: {:?}",
        outcome.stdout
    );
}

/// Behaviour (b): a genuinely idle child IS killed after the idle window.
#[tokio::test]
async fn async_idle_child_is_killed_after_window() {
    // Emits nothing while it sleeps → genuinely hung.
    let mut child = tokio::process::Command::new("bash")
        .args(["-c", "sleep 20"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn idle child");
    let (out, err) = (child.stdout.take(), child.stderr.take());

    let start = std::time::Instant::now();
    let outcome = wait_with_idle_watchdog(
        &mut child,
        out,
        err,
        cfg(Duration::from_secs(1), Duration::from_millis(100)),
    )
    .await;

    assert!(
        outcome.killed_for_idle,
        "a silent child must be killed once the idle window elapses"
    );
    assert!(
        start.elapsed() < Duration::from_secs(15),
        "kill must happen shortly after the idle window, not at process end"
    );
}

/// A child that produces output and THEN goes silent is allowed to run while it
/// streams, and is killed only once it has been idle for the window.
#[tokio::test]
async fn async_child_killed_only_after_it_stops_producing() {
    // Three ticks (≈3 s of activity), then silent for 20 s.
    let mut child = tokio::process::Command::new("bash")
        .args([
            "-c",
            "for i in 1 2 3; do echo tick $i; sleep 1; done; sleep 20",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn produce-then-idle child");
    let (out, err) = (child.stdout.take(), child.stderr.take());

    let outcome = wait_with_idle_watchdog(
        &mut child,
        out,
        err,
        cfg(Duration::from_secs(1), Duration::from_millis(100)),
    )
    .await;

    assert!(
        outcome.killed_for_idle,
        "child must be killed after it goes idle"
    );
    assert!(
        outcome.stdout.contains("tick 3"),
        "output produced before going idle must be captured, got: {:?}",
        outcome.stdout
    );
}

/// Progress on stderr alone must reset the idle timer, so a child that logs only
/// to stderr is NOT killed.
#[tokio::test]
async fn async_stderr_output_counts_as_progress() {
    let mut child = tokio::process::Command::new("bash")
        .args(["-c", "for i in 1 2 3 4; do echo err $i 1>&2; sleep 1; done"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn stderr-only child");
    let (out, err) = (child.stdout.take(), child.stderr.take());

    let outcome = wait_with_idle_watchdog(
        &mut child,
        out,
        err,
        cfg(Duration::from_secs(2), Duration::from_millis(100)),
    )
    .await;

    assert!(
        !outcome.killed_for_idle,
        "stderr activity must reset the idle timer"
    );
    assert_eq!(outcome.status.unwrap().code(), Some(0));
    assert!(
        outcome.stderr.contains("err 4"),
        "stderr must be captured, got: {:?}",
        outcome.stderr
    );
}

/// A fast, quiet child that exits on its own before the idle window is reported
/// with its real exit status and is not flagged as idle-killed.
#[tokio::test]
async fn async_quick_exit_is_not_idle_killed() {
    let mut child = tokio::process::Command::new("bash")
        .args(["-c", "echo done"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn quick child");
    let (out, err) = (child.stdout.take(), child.stderr.take());

    let outcome = wait_with_idle_watchdog(
        &mut child,
        out,
        err,
        cfg(Duration::from_secs(5), Duration::from_millis(100)),
    )
    .await;

    assert!(!outcome.killed_for_idle);
    assert_eq!(outcome.status.unwrap().code(), Some(0));
    assert!(outcome.stdout.contains("done"));
}

// ---------------------------------------------------------------------------
// Sync watchdog — wait_with_idle_watchdog_sync
// ---------------------------------------------------------------------------

/// Behaviour (a), sync path: a continuously-producing child is NOT killed.
#[test]
fn sync_producing_child_survives_past_old_deadline() {
    let mut child = std::process::Command::new("bash")
        .args(["-c", "for i in $(seq 1 5); do echo tick $i; sleep 1; done"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn producing child");
    let (out, err) = (child.stdout.take(), child.stderr.take());

    let outcome = wait_with_idle_watchdog_sync(
        &mut child,
        out,
        err,
        cfg(Duration::from_secs(2), Duration::from_millis(100)),
    );

    assert!(
        !outcome.killed_for_idle,
        "a continuously-producing child must not be killed"
    );
    assert_eq!(outcome.status.unwrap().code(), Some(0));
    assert!(
        outcome.stdout.contains("tick 5"),
        "streamed output must be captured, got: {:?}",
        outcome.stdout
    );
}

/// Behaviour (b), sync path: a genuinely idle child IS killed after the window.
#[test]
fn sync_idle_child_is_killed_after_window() {
    let mut child = std::process::Command::new("bash")
        .args(["-c", "sleep 20"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn idle child");
    let (out, err) = (child.stdout.take(), child.stderr.take());

    let start = std::time::Instant::now();
    let outcome = wait_with_idle_watchdog_sync(
        &mut child,
        out,
        err,
        cfg(Duration::from_secs(1), Duration::from_millis(100)),
    );

    assert!(
        outcome.killed_for_idle,
        "a silent child must be killed once the idle window elapses"
    );
    assert!(
        start.elapsed() < Duration::from_secs(15),
        "kill must happen shortly after the idle window, not at process end"
    );
}

// ---------------------------------------------------------------------------
// File-mtime idle probe — file_idle_since
// ---------------------------------------------------------------------------

/// `file_idle_since` reports idle only once the log file's mtime has aged past
/// the window: false immediately after a write, true after the mtime is aged.
#[test]
fn file_idle_since_tracks_mtime_age() {
    use std::fs::File;
    use std::io::Write;
    use std::time::SystemTime;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run.log");
    let mut f = File::create(&path).unwrap();
    f.write_all(b"progress\n").unwrap();
    f.flush().unwrap();

    // Fresh write: not idle within a generous 60 s window.
    assert!(
        !file_idle_since(&path, Duration::from_secs(60)).unwrap(),
        "a just-written file must not be reported idle"
    );

    // Age the mtime 10 s into the past.
    let aged = SystemTime::now() - Duration::from_secs(10);
    f.set_modified(aged).unwrap();

    // A 5 s window now reports idle (10 s of no growth ≥ 5 s window).
    assert!(
        file_idle_since(&path, Duration::from_secs(5)).unwrap(),
        "a file untouched past the window must be reported idle"
    );

    // The same aged file with a 60 s window is still NOT idle.
    assert!(
        !file_idle_since(&path, Duration::from_secs(60)).unwrap(),
        "10 s of idleness must not trip a 60 s window"
    );
}

#[test]
fn file_idle_since_missing_file_is_error() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("does-not-exist.log");
    assert!(
        file_idle_since(&missing, Duration::from_secs(1)).is_err(),
        "probing a nonexistent log file must surface an I/O error, not a bool"
    );
}
