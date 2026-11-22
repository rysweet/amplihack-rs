//! Issue #2931 (INUSE): is a recorded session holder still running?
//!
//! The decision is two small pure functions, so these tests supply their own
//! inputs and ask nothing of the host's process table:
//!
//! - `pid_for_signal` turns the recorded `u32` into a pid that is safe to hand
//!   to `kill(2)`, or `None`. Zero would signal the caller's process group, and
//!   a value above `i32::MAX` would wrap to a negative number, which
//!   addresses a process group too.
//! - `live_from_kill` reads the outcome of `kill(pid, 0)`: `None` is success,
//!   `Some(errno)` is the error. Only `ESRCH` says the process is gone.

use super::{SessionEntry, SessionStatus, holder_is_live, live_from_kill, pid_for_signal};

#[test]
fn a_recorded_pid_reaches_kill_only_when_it_is_a_positive_pid_t() {
    let cases: [(u32, Option<i32>); 7] = [
        (0, None),
        (1, Some(1)),
        (4242, Some(4242)),
        (i32::MAX as u32, Some(i32::MAX)),
        (i32::MAX as u32 + 1, None),
        (u32::MAX - 1, None),
        (u32::MAX, None),
    ];
    for (recorded, expected) in cases {
        assert_eq!(
            pid_for_signal(recorded),
            expected,
            "recorded pid {recorded}"
        );
    }
}

#[test]
fn only_a_missing_process_reads_as_dead() {
    let cases: [(Option<i32>, bool, &str); 5] = [
        (None, true, "kill succeeded: the process exists"),
        (Some(libc::ESRCH), false, "no such process"),
        (
            Some(libc::EPERM),
            true,
            "the process exists and belongs to someone else",
        ),
        (
            Some(libc::EINVAL),
            true,
            "an unexpected error is not proof of death",
        ),
        (
            Some(0),
            true,
            "an error number of zero is not proof of death",
        ),
    ];
    for (outcome, live, why) in cases {
        assert_eq!(live_from_kill(outcome), live, "{outcome:?}: {why}");
    }
}

fn entry(pid: Option<u32>) -> SessionEntry {
    SessionEntry {
        depth: 0,
        parent: None,
        status: SessionStatus::Active,
        started_at: 0,
        completed_at: None,
        children: Vec::new(),
        pid,
    }
}

#[test]
fn a_holder_with_no_recorded_pid_counts_as_live() {
    assert!(holder_is_live(&entry(None)));
}

#[test]
fn a_recorded_pid_that_cannot_be_a_process_counts_as_dead_without_asking_the_host() {
    // The conversion check alone decides these: neither value reaches kill.
    assert!(!holder_is_live(&entry(Some(0))));
    assert!(!holder_is_live(&entry(Some(u32::MAX - 1))));
}
