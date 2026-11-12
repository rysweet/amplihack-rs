//! Issue #2019: the debug-only probe-wait seam of `launch_target`.
//!
//! Contract (see docs/howto/keep-tests-passing-under-host-load.md, "Wait for
//! the version probe to exit"):
//!
//! - `PROBE_WAIT_ENV` is `"AMPLIHACK_TEST_PROBE_WAIT"` and, like every item
//!   of the seam, exists only under `cfg(debug_assertions)`.
//! - `probe_wait_mode_from(raw)` gives `ProbeWait::Exit` for exactly
//!   `Some("exit")` and `ProbeWait::Deadline` for anything else.
//!   `probe_wait_mode()` reads the variable once per call.
//! - `probe_limit(mode, budget)` is the wait one probe gets:
//!   `Some(PER_CANDIDATE_PROBE_TIMEOUT.min(budget))` under `Deadline`, and
//!   `None` (wait for the candidate to exit) under `Exit`.
//! - `resolve_from_candidates_with(tool, candidates, mode)` is what
//!   `resolve_from_candidates` calls with `probe_wait_mode()`; under `Exit`
//!   the `TOTAL_PROBE_BUDGET` check is skipped, and the candidate limit, byte
//!   cap and rejection reasons still apply.
//! - `binary_finder::run_capped_output_to_exit(cmd, limit)` waits for the
//!   child to exit, keeps the byte cap and returns its real exit status.
//! - The 3 s and 10 s limits stay the production defaults.
//!
//! None of these tests measures real time.

use super::*;
use crate::binary_finder::{PROBE_CAPTURE_LIMIT, run_capped_output_to_exit};
use std::process::Command;
use std::time::Duration;

#[cfg(unix)]
#[path = "../../amplihack-cli/tests/support/unix_process.rs"]
mod unix_process;
#[cfg(unix)]
#[path = "../../amplihack-cli/tests/support/wait.rs"]
mod wait;

#[test]
fn the_knob_is_named_amplihack_test_probe_wait() {
    assert_eq!(PROBE_WAIT_ENV, "AMPLIHACK_TEST_PROBE_WAIT");
    assert!(
        PROBE_WAIT_ENV.starts_with("AMPLIHACK_TEST_")
            && !PROBE_WAIT_ENV.starts_with("AMPLIHACK_ROTATE"),
        "the knob must stay outside the rotate daemon's AMPLIHACK_ROTATE* strip"
    );
}

#[test]
fn probe_wait_mode_accepts_exactly_exit() {
    assert_eq!(probe_wait_mode_from(Some("exit")), ProbeWait::Exit);
}

#[test]
fn probe_wait_mode_rejects_every_other_value() {
    for raw in [
        None,
        Some(""),
        Some("Exit"),
        Some("EXIT"),
        Some("exit "),
        Some(" exit"),
        Some("exit\n"),
        Some("exit\0"),
        Some("1"),
        Some("true"),
        Some("600"),
        Some("exited"),
        Some("ｅｘｉｔ"),
    ] {
        assert_eq!(
            probe_wait_mode_from(raw),
            ProbeWait::Deadline,
            "{raw:?} must leave the 3 s / 10 s limits in place"
        );
    }
}

#[test]
fn the_production_probe_limits_are_unchanged() {
    assert_eq!(PER_CANDIDATE_PROBE_TIMEOUT, Duration::from_secs(3));
    assert_eq!(TOTAL_PROBE_BUDGET, Duration::from_secs(10));
}

#[test]
fn a_deadline_probe_gets_the_per_candidate_limit_capped_by_the_budget() {
    assert_eq!(
        probe_limit(ProbeWait::Deadline, TOTAL_PROBE_BUDGET),
        Some(PER_CANDIDATE_PROBE_TIMEOUT)
    );
    assert_eq!(
        probe_limit(ProbeWait::Deadline, Duration::from_secs(1)),
        Some(Duration::from_secs(1))
    );
    assert_eq!(
        probe_limit(ProbeWait::Deadline, Duration::ZERO),
        Some(Duration::ZERO)
    );
}

#[test]
fn an_exit_probe_has_no_deadline_whatever_the_budget() {
    // The knob replaces the deadline with the candidate's exit. It never
    // stands for a longer deadline.
    for budget in [
        Duration::ZERO,
        Duration::from_secs(1),
        TOTAL_PROBE_BUDGET,
        Duration::MAX,
    ] {
        assert_eq!(probe_limit(ProbeWait::Exit, budget), None, "{budget:?}");
    }
}

#[test]
fn resolving_no_candidates_under_either_mode_finds_nothing() {
    // Pins the signature of the seam and that the mode does not invent a
    // candidate.
    let none: &[(PathBuf, TargetSource)] = &[];
    let a = resolve_from_candidates_with("claude", none, ProbeWait::Deadline);
    let b = resolve_from_candidates_with("claude", none, ProbeWait::Exit);
    assert_eq!(format!("{a:?}"), format!("{b:?}"));
}

#[cfg(unix)]
#[test]
fn run_to_exit_returns_the_childs_real_exit_status() {
    let mut cmd = Command::new("/bin/sh");
    cmd.args(["-c", "printf 'claude 1.2.3\\n'; exit 3"]);
    let out = run_capped_output_to_exit(cmd, PROBE_CAPTURE_LIMIT).expect("spawn sh");
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(out.stdout, b"claude 1.2.3\n");
}

#[cfg(unix)]
#[test]
fn run_to_exit_keeps_the_byte_cap() {
    // 3 × the cap on stdout: the child is drained to exit (no SIGPIPE), and
    // only `limit` bytes are kept.
    let limit = 1024;
    let mut cmd = Command::new("/bin/sh");
    cmd.args(["-c", "head -c 3072 /dev/zero; exit 0"]);
    let out = run_capped_output_to_exit(cmd, limit).expect("spawn sh");
    assert!(out.status.success(), "{:?}", out.status);
    assert_eq!(
        out.stdout.len(),
        limit,
        "stdout must be capped at the limit"
    );
}

#[cfg(unix)]
#[test]
fn run_to_exit_keeps_the_byte_cap_on_stderr() {
    let limit = 512;
    let mut cmd = Command::new("/bin/sh");
    cmd.args(["-c", "head -c 4096 /dev/zero >&2; exit 0"]);
    let out = run_capped_output_to_exit(cmd, limit).expect("spawn sh");
    assert_eq!(out.stderr.len(), limit);
}

#[cfg(unix)]
#[test]
fn run_to_exit_reports_a_spawn_failure_as_an_error() {
    let cmd = Command::new("/nonexistent/amplihack-2019-probe");
    assert!(run_capped_output_to_exit(cmd, PROBE_CAPTURE_LIMIT).is_err());
}

#[cfg(unix)]
#[test]
fn run_to_exit_returns_when_the_child_exits_even_if_a_grandchild_holds_the_pipe() {
    with_held_grandchild(|out, _| assert!(out.status.success()));
}

#[cfg(unix)]
fn with_held_grandchild(check: impl FnOnce(&std::process::Output, libc::pid_t)) {
    // SEC-4 still holds without a deadline: a candidate that backgrounds a
    // process holding stdout must not keep the probe waiting on the
    // grandchild. The grandchild blocks on a FIFO nobody opens for writing
    // until the process-group guard kills it, on success or assertion unwind.
    use std::os::unix::process::CommandExt;
    let dir = tempfile::tempdir().unwrap();
    let group = unix_process::ProcessGroup::new();
    let fifo = dir.path().join("hold");
    let pid_file = dir.path().join("grandchild.pid");
    let status = Command::new("mkfifo").arg(&fifo).status().unwrap();
    assert!(status.success());
    let mut cmd = Command::new("/bin/sh");
    cmd.args([
        "-c",
        "cat \"$1\" >/dev/null & echo $! >\"$2\"; printf 'claude 1.2.3\\n'; exit 0",
        "sh",
    ])
    .arg(&fifo)
    .arg(&pid_file)
    .process_group(group.id());
    // CEILING (600 s, tests/support/wait.rs) is the only bound: it ends a
    // hung run and never decides a pass.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(run_capped_output_to_exit(cmd, PROBE_CAPTURE_LIMIT));
    });
    let out = rx
        .recv_timeout(wait::CEILING)
        .expect("hung: run_capped_output_to_exit waited on the grandchild's pipe")
        .expect("spawn sh");
    let pid = std::fs::read_to_string(pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(
        unix_process::process_alive(pid),
        "the grandchild must still be held"
    );
    check(&out, pid);
}

#[cfg(target_os = "linux")]
#[test]
fn fifo_held_probe_grandchild_is_killed_on_assertion_unwind() {
    let mut pid = None;
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        with_held_grandchild(|_, grandchild| {
            pid = Some(grandchild);
            panic!("intentional assertion failure while the grandchild is parked");
        });
    }));
    assert!(failure.is_err());
    let pid = pid.expect("the held grandchild must have started");
    wait::wait_until("probe grandchild to exit on unwind", || {
        !unix_process::process_alive(pid)
    });
}

#[cfg(unix)]
#[test]
fn run_to_exit_returns_when_a_grandchild_never_stops_writing() {
    // `yes` inherits stdout and writes without pause after `sh` has exited.
    // The drain reads one chunk per wake and, after the exit, only the bytes
    // already in the pipe, so the flood cannot keep the probe running. The
    // test leaves nothing behind: `sh` joins an owned process group, which
    // the guard kills on drop (panics included), and `yes` also dies of SIGPIPE
    // once the capture closes its read end.
    use std::os::unix::process::CommandExt;
    let group = unix_process::ProcessGroup::new();
    let mut cmd = Command::new("/bin/sh");
    cmd.args(["-c", "yes & printf 'claude 1.2.3\\n' >&2; exit 0", "sh"])
        .process_group(group.id());
    // Keep group ownership on this thread so a hung capture cannot prevent
    // cleanup. CEILING only fails a hang; completion decides a pass.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(run_capped_output_to_exit(cmd, PROBE_CAPTURE_LIMIT));
    });
    let out = rx
        .recv_timeout(wait::CEILING)
        .expect("hung: run_capped_output_to_exit kept draining the continuous writer")
        .expect("spawn sh");
    assert!(out.status.success(), "{:?}", out.status);
    assert!(
        out.stdout.len() <= PROBE_CAPTURE_LIMIT,
        "the byte cap still holds: {}",
        out.stdout.len()
    );
    assert_eq!(
        out.stderr, b"claude 1.2.3\n",
        "what the child wrote before it exited is kept"
    );
}
