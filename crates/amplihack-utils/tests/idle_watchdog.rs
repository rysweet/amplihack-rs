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
    DEFAULT_POLL_MS, ENV_IDLE_POLL_MS, ENV_IDLE_TIMEOUT_SECS, IDLE_TIMEOUT_WARNING, IdleConfig,
    file_idle_since, parse_positive_secs, positive_secs_is_malformed, wait_with_idle_watchdog,
    wait_with_idle_watchdog_sync,
};

/// Serializes tests that read or mutate the process environment. Behavioural
/// tests build `IdleConfig` via struct literals and do NOT take this lock.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Build a config directly (env-independent) for deterministic behavioural tests.
fn cfg(idle: Duration, poll: Duration) -> IdleConfig {
    IdleConfig {
        idle_timeout: Some(idle),
        poll,
    }
}

// ---------------------------------------------------------------------------
// IdleConfig
// ---------------------------------------------------------------------------

/// Issue #2022: with the variable unset there is no idle limit at all.
#[test]
fn config_from_env_defaults_to_no_idle_limit() {
    let _guard = ENV_LOCK.lock().unwrap();
    let saved = (
        std::env::var_os(ENV_IDLE_TIMEOUT_SECS),
        std::env::var_os(ENV_IDLE_POLL_MS),
    );
    unsafe {
        std::env::remove_var(ENV_IDLE_TIMEOUT_SECS);
        std::env::remove_var(ENV_IDLE_POLL_MS);
    }
    let c = IdleConfig::from_env();
    restore(saved);
    assert_eq!(c.idle_timeout, None);
    assert_eq!(c.poll, Duration::from_millis(DEFAULT_POLL_MS));
}

#[test]
fn config_from_env_override() {
    let _guard = ENV_LOCK.lock().unwrap();
    let saved = (
        std::env::var_os(ENV_IDLE_TIMEOUT_SECS),
        std::env::var_os(ENV_IDLE_POLL_MS),
    );
    unsafe {
        std::env::set_var(ENV_IDLE_TIMEOUT_SECS, "42");
        std::env::set_var(ENV_IDLE_POLL_MS, "250");
    }
    let c = IdleConfig::from_env();
    restore(saved);
    assert_eq!(c.idle_timeout, Some(Duration::from_secs(42)));
    assert_eq!(c.poll, Duration::from_millis(250));
}

/// Issue #2022: `0`, empty and garbage mean no idle limit, never 0 s and
/// never a hidden default.
#[test]
fn config_from_env_zero_empty_and_garbage_mean_no_idle_limit() {
    let _guard = ENV_LOCK.lock().unwrap();
    let saved = (
        std::env::var_os(ENV_IDLE_TIMEOUT_SECS),
        std::env::var_os(ENV_IDLE_POLL_MS),
    );
    for raw in ["0", "", "not-a-number", "-5", "99999999999999999999"] {
        unsafe {
            std::env::set_var(ENV_IDLE_TIMEOUT_SECS, raw);
            std::env::set_var(ENV_IDLE_POLL_MS, "0");
        }
        let c = IdleConfig::from_env();
        assert_eq!(c.idle_timeout, None, "{raw:?}");
        assert_eq!(c.poll, Duration::from_millis(DEFAULT_POLL_MS), "{raw:?}");
    }
    restore(saved);
}

#[test]
fn config_none_has_no_idle_limit_and_the_default_poll() {
    // `none()` reads the poll interval from the environment.
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let saved = (
        std::env::var_os(ENV_IDLE_TIMEOUT_SECS),
        std::env::var_os(ENV_IDLE_POLL_MS),
    );
    unsafe { std::env::remove_var(ENV_IDLE_POLL_MS) };
    let c = IdleConfig::none();
    restore(saved);
    assert_eq!(c.idle_timeout, None);
    assert_eq!(c.poll, Duration::from_millis(DEFAULT_POLL_MS));
}

fn restore(saved: (Option<std::ffi::OsString>, Option<std::ffi::OsString>)) {
    for (key, value) in [
        (ENV_IDLE_TIMEOUT_SECS, saved.0),
        (ENV_IDLE_POLL_MS, saved.1),
    ] {
        match value {
            Some(v) => unsafe { std::env::set_var(key, v) },
            None => unsafe { std::env::remove_var(key) },
        }
    }
}

// ---------------------------------------------------------------------------
// parse_positive_secs: the one digits-only parser (issue #2022, AC2, S24)
// ---------------------------------------------------------------------------

#[test]
fn parse_positive_secs_rejects_everything_but_a_positive_whole_number() {
    for raw in [
        None,
        Some(""),
        Some("  "),
        Some("0"),
        Some("00"),
        Some("abc"),
        Some("-5"),
        Some("+7"),
        Some("7s"),
        Some("0x10"),
        Some("7.0"),
        Some("1e3"),
        Some("\u{0}7"),
        Some("7 8"),
        Some("99999999999999999999"),
    ] {
        assert_eq!(parse_positive_secs(raw), None, "{raw:?}");
    }
}

// ---------------------------------------------------------------------------
// positive_secs_is_malformed: when a limit variable warns (issue #2022, S5)
// ---------------------------------------------------------------------------

#[test]
fn malformed_limit_values_warn() {
    for raw in [
        "300s",
        "+7",
        "0x10",
        "-5",
        "7.0",
        "abc",
        "99999999999999999999",
    ] {
        assert!(positive_secs_is_malformed(Some(raw)), "{raw:?} must warn");
    }
}

#[test]
fn unset_empty_zero_and_valid_limit_values_do_not_warn() {
    for raw in [
        None,
        Some(""),
        Some("  "),
        Some("0"),
        Some("000"),
        Some("30"),
        Some(" 30 "),
    ] {
        assert!(!positive_secs_is_malformed(raw), "{raw:?} must not warn");
    }
}

#[test]
fn idle_timeout_warning_names_the_variable_and_no_value() {
    assert!(IDLE_TIMEOUT_WARNING.contains(ENV_IDLE_TIMEOUT_SECS));
    // The only digit is the "0" of "above 0": the message is a fixed string
    // and never echoes the rejected value.
    let without_rule = IDLE_TIMEOUT_WARNING.replacen("above 0", "", 1);
    assert!(
        !without_rule.bytes().any(|b| b.is_ascii_digit()),
        "the warning must not echo any value"
    );
}

#[test]
fn parse_positive_secs_accepts_trimmed_digits() {
    assert_eq!(parse_positive_secs(Some("7")), Some(Duration::from_secs(7)));
    assert_eq!(
        parse_positive_secs(Some(" 7 ")),
        Some(Duration::from_secs(7))
    );
    assert_eq!(
        parse_positive_secs(Some("7\n")),
        Some(Duration::from_secs(7))
    );
    assert_eq!(
        parse_positive_secs(Some("007")),
        Some(Duration::from_secs(7))
    );
    assert_eq!(
        parse_positive_secs(Some("18446744073709551615")),
        Some(Duration::from_secs(u64::MAX))
    );
}

#[test]
fn config_with_idle_overrides_idle_keeps_default_poll() {
    let _guard = ENV_LOCK.lock().unwrap();
    unsafe {
        std::env::remove_var(ENV_IDLE_TIMEOUT_SECS);
        std::env::remove_var(ENV_IDLE_POLL_MS);
    }
    let c = IdleConfig::with_idle(Duration::from_secs(7));
    assert_eq!(c.idle_timeout, Some(Duration::from_secs(7)));
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
///
/// Timing (issue #1535): the gap between ticks must sit well inside the idle
/// window, or a loaded host stretches a gap past the window and the watchdog
/// correctly kills the child mid-stream. Ticks come every 0.25 s against a 3 s
/// window, so a gap has to stall by more than 2.75 s to trip it. Streaming
/// lasts ≈4 s, longer than the window, so surviving to "tick 16" proves output
/// resets the timer. The 20 s silent tail leaves the watchdog 17 s to act
/// before the child would exit on its own.
///
/// The tail is `exec sleep 20`, so the process the watchdog kills is the one
/// holding the pipes. Bash 5.1 and later exec a trailing command of `bash -c`
/// on their own, but older bash (macOS's /bin/bash 3.2) forks it. The kill then
/// reaches only bash, the orphaned `sleep` keeps both pipes open, and each
/// drainer join waits out its 5 s grace, ≈17 s in all.
#[tokio::test]
async fn async_child_killed_only_after_it_stops_producing() {
    // Sixteen ticks 0.25 s apart (≈4 s of activity), then silent for 20 s.
    let mut child = tokio::process::Command::new("bash")
        .args([
            "-c",
            "for i in $(seq 1 16); do echo tick $i; sleep 0.25; done; exec sleep 20",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn produce-then-idle child");
    let (out, err) = (child.stdout.take(), child.stderr.take());

    let start = std::time::Instant::now();
    let outcome = wait_with_idle_watchdog(
        &mut child,
        out,
        err,
        cfg(Duration::from_secs(3), Duration::from_millis(100)),
    )
    .await;

    assert!(
        outcome.killed_for_idle,
        "child must be killed after it goes idle"
    );
    // ≈4 s of streaming plus the 3 s window is ≈7 s. A watchdog that waited
    // much longer than its window (but still under the 20 s tail) would pass
    // the assertion above, so bound the kill time too.
    assert!(
        start.elapsed() < Duration::from_secs(15),
        "kill must follow the idle window, not lag it (elapsed {:?})",
        start.elapsed()
    );
    assert!(
        outcome.stdout.contains("tick 16"),
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

/// Regression (issue #867), async path: a child that exits while a reparented
/// descendant keeps the stdout pipe open must NOT hang the watchdog. The old
/// wall-clock backstop was removed, so an unbounded drainer join would block
/// until the grandchild exits; the bounded join must return well before that.
#[tokio::test]
async fn async_lingering_grandchild_does_not_hang_join() {
    // `bash` echoes then exits, but backgrounds `sleep 30`, which inherits the
    // stdout fd and holds the pipe open (no EOF for the drainer) for 30 s.
    let mut child = tokio::process::Command::new("bash")
        .args(["-c", "echo hello; sleep 30 & exit 0"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn child with lingering grandchild");
    let (out, err) = (child.stdout.take(), child.stderr.take());

    let start = std::time::Instant::now();
    let outcome = wait_with_idle_watchdog(
        &mut child,
        out,
        err,
        cfg(Duration::from_secs(30), Duration::from_millis(100)),
    )
    .await;

    assert!(
        start.elapsed() < Duration::from_secs(20),
        "watchdog must not block on a pipe held open by a lingering descendant \
         (elapsed {:?})",
        start.elapsed()
    );
    assert!(
        !outcome.killed_for_idle,
        "the child exited normally; it was not idle-killed"
    );
    assert_eq!(outcome.status.unwrap().code(), Some(0));
    assert!(
        outcome.stdout.contains("hello"),
        "output buffered before the grace bound must still be captured, got {:?}",
        outcome.stdout
    );
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

/// Regression (issue #867), sync path: a lingering descendant holding the pipe
/// open must not hang the blocking drainer join.
#[test]
fn sync_lingering_grandchild_does_not_hang_join() {
    let mut child = std::process::Command::new("bash")
        .args(["-c", "echo hello; sleep 30 & exit 0"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn child with lingering grandchild");
    let (out, err) = (child.stdout.take(), child.stderr.take());

    let start = std::time::Instant::now();
    let outcome = wait_with_idle_watchdog_sync(
        &mut child,
        out,
        err,
        cfg(Duration::from_secs(30), Duration::from_millis(100)),
    );

    assert!(
        start.elapsed() < Duration::from_secs(20),
        "sync watchdog must not block on a pipe held open by a lingering \
         descendant (elapsed {:?})",
        start.elapsed()
    );
    assert!(
        !outcome.killed_for_idle,
        "the child exited normally; it was not idle-killed"
    );
    assert_eq!(outcome.status.unwrap().code(), Some(0));
    assert!(
        outcome.stdout.contains("hello"),
        "output buffered before the grace bound must still be captured, got {:?}",
        outcome.stdout
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

// ---------------------------------------------------------------------------
// Issue #2022: with no idle limit the watchdog never kills
// ---------------------------------------------------------------------------

fn no_limit() -> IdleConfig {
    IdleConfig {
        idle_timeout: None,
        poll: Duration::from_millis(100),
    }
}

/// With `idle_timeout: None` the watchdog takes the no-kill path; the child
/// exits only when released over stdin.
#[test]
fn sync_silent_child_is_not_killed_without_an_idle_limit() {
    use std::io::Write as _;
    let mut child = std::process::Command::new("bash")
        .args(["-c", "read -r _; echo done"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn silent child");
    let mut stdin = child.stdin.take().expect("child stdin");
    let release = std::thread::spawn(move || {
        // The exit-status asserts below decide the result.
        let _ = stdin.write_all(b"go\n");
    });
    let (out, err) = (child.stdout.take(), child.stderr.take());
    let outcome = wait_with_idle_watchdog_sync(&mut child, out, err, no_limit());
    release.join().expect("release thread");
    assert!(!outcome.killed_for_idle);
    assert_eq!(outcome.status.unwrap().code(), Some(0));
    assert!(outcome.stdout.contains("done"), "{:?}", outcome.stdout);
}

/// With `idle_timeout: None` the watchdog takes the no-kill path; the child
/// exits only when released over stdin.
#[tokio::test]
async fn async_silent_child_is_not_killed_without_an_idle_limit() {
    use tokio::io::AsyncWriteExt as _;
    let mut child = tokio::process::Command::new("bash")
        .args(["-c", "read -r _; echo done"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn silent child");
    let mut stdin = child.stdin.take().expect("child stdin");
    drop(tokio::spawn(async move {
        // The exit-status asserts below decide the result.
        let _ = stdin.write_all(b"go\n").await;
    }));
    let (out, err) = (child.stdout.take(), child.stderr.take());
    let outcome = wait_with_idle_watchdog(&mut child, out, err, no_limit()).await;
    assert!(!outcome.killed_for_idle);
    assert_eq!(outcome.status.unwrap().code(), Some(0));
    assert!(outcome.stdout.contains("done"), "{:?}", outcome.stdout);
}

/// The unbounded path still reaps a child that exits with an error.
#[test]
fn sync_unbounded_wait_reports_a_failing_exit() {
    let mut child = std::process::Command::new("bash")
        .args(["-c", "echo oops >&2; exit 4"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn failing child");
    let (out, err) = (child.stdout.take(), child.stderr.take());
    let outcome = wait_with_idle_watchdog_sync(&mut child, out, err, no_limit());
    assert!(!outcome.killed_for_idle);
    assert_eq!(outcome.status.unwrap().code(), Some(4));
    assert!(outcome.stderr.contains("oops"));
}
