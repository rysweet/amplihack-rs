//! Issue #2018: classification (D8) through a fake probe, and the system
//! probe's reading of `/proc`. See `tests_support` for the contract.

#![cfg(unix)]

use super::tests_support::*;
use crate::step_record::{Probe, StepRecord, StepState, SystemProbe, classify};
use std::collections::HashMap;

/// A pid above any `pid_max` the kernel allows (2^22), so it never exists.
const NEVER_A_PID: i32 = 2_000_000_000;

/// Live pids and their current start times.
#[derive(Default)]
struct FakeProbe(HashMap<i32, u64>);

impl FakeProbe {
    fn with(live: &[(i32, u64)]) -> Self {
        Self(live.iter().copied().collect())
    }
}

impl Probe for FakeProbe {
    fn exists(&self, pid: i32) -> bool {
        self.0.contains_key(&pid)
    }

    fn start_ticks(&self, pid: i32) -> Option<u64> {
        self.0.get(&pid).copied()
    }
}

const LAUNCHER: (i32, u64) = (10, 100);
const AGENT: (i32, u64) = (11, 110);
const PARENT: (i32, u64) = (9, 90);

/// Launcher 10 (start 100), agent 11 (start 110), parent 9 (start 90).
fn rec() -> StepRecord {
    StepRecord {
        launcher: proc_id(10, Some(100)),
        agent: proc_id(11, Some(110)),
        parent: Some(proc_id(9, Some(90))),
        ..record(RUN, 10)
    }
}

fn state(record: &StepRecord, live: &[(i32, u64)]) -> StepState {
    classify(record, &FakeProbe::with(live))
}

// ---------------------------------------------------------------------
// The D8 table
// ---------------------------------------------------------------------

#[test]
fn launcher_and_parent_alive_is_live_whatever_the_agent() {
    assert_eq!(state(&rec(), &[LAUNCHER, AGENT, PARENT]), StepState::Live);
    assert_eq!(state(&rec(), &[LAUNCHER, PARENT]), StepState::Live);
}

#[test]
fn launcher_alive_without_its_parent_is_an_orphan() {
    assert_eq!(state(&rec(), &[LAUNCHER, AGENT]), StepState::Orphan);
    assert_eq!(state(&rec(), &[LAUNCHER]), StepState::Orphan);
}

#[test]
fn agent_alive_without_its_launcher_is_an_orphan_whatever_the_parent() {
    assert_eq!(state(&rec(), &[AGENT, PARENT]), StepState::Orphan);
    assert_eq!(state(&rec(), &[AGENT]), StepState::Orphan);
}

#[test]
fn neither_launcher_nor_agent_alive_is_stale_whatever_the_parent() {
    assert_eq!(state(&rec(), &[PARENT]), StepState::Stale);
    assert_eq!(state(&rec(), &[]), StepState::Stale);
}

// ---------------------------------------------------------------------
// Start times and pid reuse
// ---------------------------------------------------------------------

#[test]
fn a_reused_launcher_pid_with_another_start_time_is_not_the_launcher() {
    assert_eq!(state(&rec(), &[(10, 5000), PARENT]), StepState::Stale);
    assert_eq!(
        state(&rec(), &[(10, 5000), AGENT, PARENT]),
        StepState::Orphan
    );
}

#[test]
fn a_reused_parent_pid_does_not_keep_a_step_live() {
    assert_eq!(
        state(&rec(), &[LAUNCHER, AGENT, (9, 7777)]),
        StepState::Orphan
    );
}

#[test]
fn a_reused_agent_pid_does_not_make_a_stale_step_an_orphan() {
    assert_eq!(state(&rec(), &[(11, 8888), PARENT]), StepState::Stale);
}

/// `start_ticks: null` (a non-Linux record) checks existence only.
#[test]
fn a_null_start_time_checks_existence_only() {
    let mut r = rec();
    r.launcher.start_ticks = None;
    r.parent.as_mut().unwrap().start_ticks = None;
    assert_eq!(state(&r, &[(10, 123_456), (9, 654_321)]), StepState::Live);
}

/// A live pid whose start time cannot be read does not match a recorded
/// start time.
#[test]
fn a_known_start_time_that_cannot_be_read_counts_as_gone() {
    struct NoTicks;
    impl Probe for NoTicks {
        fn exists(&self, _pid: i32) -> bool {
            true
        }
        fn start_ticks(&self, _pid: i32) -> Option<u64> {
            None
        }
    }
    assert_eq!(classify(&rec(), &NoTicks), StepState::Stale);
}

// ---------------------------------------------------------------------
// parent: null
// ---------------------------------------------------------------------

/// A parent outside the launcher's PID namespace is recorded as `null` and
/// counts as alive, so it never makes a running step look orphaned.
#[test]
fn a_null_parent_counts_as_alive() {
    let mut r = rec();
    r.parent = None;
    assert_eq!(state(&r, &[LAUNCHER, AGENT]), StepState::Live);
    assert_eq!(state(&r, &[LAUNCHER]), StepState::Live);
    assert_eq!(state(&r, &[AGENT]), StepState::Orphan);
    assert_eq!(state(&r, &[]), StepState::Stale);
}

#[test]
fn state_names_are_lowercase() {
    assert_eq!(StepState::Live.as_str(), "live");
    assert_eq!(StepState::Orphan.as_str(), "orphan");
    assert_eq!(StepState::Stale.as_str(), "stale");
}

// ---------------------------------------------------------------------
// The system probe
// ---------------------------------------------------------------------

/// `kill(0, 0)` and `kill(-1, 0)` succeed for any process, so these must be
/// refused before any system call.
#[test]
fn the_system_probe_never_reports_pid_zero_or_below() {
    for pid in [0, -1, -2, i32::MIN] {
        assert!(!SystemProbe.exists(pid), "pid {pid}");
        assert_eq!(SystemProbe.start_ticks(pid), None, "pid {pid}");
    }
}

#[test]
fn the_system_probe_does_not_find_a_pid_that_cannot_exist() {
    assert!(!SystemProbe.exists(NEVER_A_PID));
    assert!(!SystemProbe.exists(i32::MAX));
}

#[cfg(target_os = "linux")]
#[test]
fn the_system_probe_finds_this_process_with_its_start_time() {
    let me = std::process::id() as i32;
    assert!(SystemProbe.exists(me));
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap();
    let rest = &stat[stat.rfind(')').unwrap() + 1..];
    let want: u64 = rest.split_whitespace().nth(19).unwrap().parse().unwrap();
    assert_eq!(SystemProbe.start_ticks(me), Some(want));
}

/// A zombie is not alive and has no start time to match.
#[cfg(target_os = "linux")]
#[test]
fn the_system_probe_counts_a_zombie_as_gone() {
    let mut child = std::process::Command::new("/bin/sh")
        .args(["-c", "exit 0"])
        .spawn()
        .unwrap();
    let pid = child.id() as i32;
    // Blocks until the child has exited. WNOWAIT leaves it unreaped, so it
    // is a zombie from here until `child.wait()` below.
    // SAFETY: `info` is a zeroed siginfo_t the kernel fills in; `pid` is a
    // child of this process.
    let waited = unsafe {
        let mut info: libc::siginfo_t = std::mem::zeroed();
        libc::waitid(
            libc::P_PID,
            pid as libc::id_t,
            &mut info,
            libc::WEXITED | libc::WNOWAIT,
        )
    };
    assert_eq!(waited, 0, "waitid: {}", std::io::Error::last_os_error());
    let exists = SystemProbe.exists(pid);
    let ticks = SystemProbe.start_ticks(pid);
    child.wait().unwrap();
    assert!(!exists, "a zombie is not alive");
    assert_eq!(ticks, None, "a zombie has no start time to match");
}

/// End to end through the system probe: this process as launcher and
/// parent, a pid that cannot exist as agent.
#[cfg(target_os = "linux")]
#[test]
fn a_record_of_this_process_classifies_live() {
    let me = std::process::id() as i32;
    let ticks = SystemProbe.start_ticks(me);
    let r = StepRecord {
        launcher: proc_id(me, ticks),
        agent: proc_id(NEVER_A_PID, Some(1)),
        parent: Some(proc_id(me, ticks)),
        ..record(RUN, me)
    };
    assert_eq!(classify(&r, &SystemProbe), StepState::Live);
    let stale = StepRecord {
        launcher: proc_id(me, ticks.map(|t| t + 1)),
        ..r
    };
    assert_eq!(classify(&stale, &SystemProbe), StepState::Stale);
}
