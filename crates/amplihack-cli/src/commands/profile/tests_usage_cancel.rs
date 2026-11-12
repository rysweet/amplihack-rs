//! Issue #1710: a cancelled probe starts nothing.
//!
//! `workstreams status` stops `profile usage` at its deadline by setting
//! `PROBE_CANCEL`. A probe still queued behind a full pool must then be
//! skipped, not spawned and killed. The smoke test cannot tell the two
//! apart, because a stub killed on the first poll rarely gets as far as
//! logging that it started. Here the command names a file that does not
//! exist: spawning it fails with `StartFailed`, so `Cancelled` can only come
//! from the check made before the spawn.
//!
//! Each test gives `attempt_until` its own flag, so no other probe in this
//! process sees the cancel.

use super::{Ended, ErrorReason, attempt_until};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const DEADLINE: Duration = Duration::from_secs(30);

fn missing_binary(dir: &tempfile::TempDir) -> Command {
    Command::new(dir.path().join("no-such-claude"))
}

#[test]
fn a_command_that_cannot_start_fails_to_start_without_a_cancel() {
    let dir = tempfile::tempdir().unwrap();
    let cancel = AtomicBool::new(false);
    let a = attempt_until(
        missing_binary(&dir),
        None,
        Instant::now() + DEADLINE,
        &cancel,
    );
    assert!(
        matches!(a.ended, Ended::Failed(ErrorReason::StartFailed)),
        "{:?}",
        a.ended
    );
}

#[test]
fn a_cancel_set_before_the_attempt_spawns_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let cancel = AtomicBool::new(true);
    for input in [None, Some(&b"request\n"[..])] {
        let a = attempt_until(
            missing_binary(&dir),
            input,
            Instant::now() + DEADLINE,
            &cancel,
        );
        assert!(matches!(a.ended, Ended::Cancelled), "{:?}", a.ended);
        assert!(a.output.is_none(), "no stream was read");
    }
}

#[test]
fn a_cancel_set_while_the_child_runs_stops_it_before_the_deadline() {
    let cancel = AtomicBool::new(false);
    let mut cmd = Command::new("/bin/sh");
    cmd.args(["-c", "exec sleep 600"]);
    let start = Instant::now();
    let a = std::thread::scope(|s| {
        s.spawn(|| {
            std::thread::sleep(Duration::from_millis(200));
            cancel.store(true, Ordering::Relaxed);
        });
        attempt_until(cmd, None, Instant::now() + DEADLINE, &cancel)
    });
    assert!(matches!(a.ended, Ended::Cancelled), "{:?}", a.ended);
    assert!(start.elapsed() < DEADLINE, "took {:?}", start.elapsed());
}
