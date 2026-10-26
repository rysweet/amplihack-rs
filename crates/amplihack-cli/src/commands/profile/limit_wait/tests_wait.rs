//! `wait` and `line` (issue #1618), driven by a fake `WaitDeps` with its own
//! virtual clock: the order of checks in each pass (signal, deadline, login
//! fingerprint, progress), sleep slices of at most one second, the 30-second
//! fingerprint poll and its stable baseline, and the 60-second progress line.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_wait;`.

use super::plan::{Bound, Plan};
use super::wait::{End, WaitDeps, wait};
use std::time::Duration;

/// 2026-10-18 08:00:00 -07:00.
const T0: i64 = 1_792_335_600;

/// Login fingerprint schedule: from virtual second `.0` (relative to T0) on,
/// the probe returns `.1`.
type Schedule = Vec<(i64, Option<u8>)>;

struct Fake {
    ms: i64,
    prints: Schedule,
    /// Virtual second from which a signal is pending.
    signal_from: Option<(i64, i32)>,
    signal_seen_at_ms: Option<i64>,
    sleeps: Vec<Duration>,
    sleeps_after_signal: usize,
    probes: Vec<i64>,
    logs: Vec<String>,
    /// Virtual second from which the step's parent is gone.
    parent_gone_from: Option<i64>,
    /// Virtual second from which the rotator reports fresh capacity.
    capacity_from: Option<i64>,
}

impl Fake {
    fn new() -> Self {
        Self {
            ms: 0,
            prints: vec![(0, Some(1))],
            signal_from: None,
            signal_seen_at_ms: None,
            sleeps: Vec::new(),
            sleeps_after_signal: 0,
            probes: Vec::new(),
            logs: Vec::new(),
            parent_gone_from: None,
            capacity_from: None,
        }
    }

    fn elapsed(&self) -> i64 {
        self.ms.div_euclid(1000)
    }

    fn progress(&self) -> Vec<&str> {
        self.logs
            .iter()
            .map(String::as_str)
            .filter(|l| l.contains(" progress "))
            .collect()
    }
}

impl WaitDeps for Fake {
    type Print = u8;

    fn now(&mut self) -> i64 {
        T0 + self.elapsed()
    }

    fn sleep(&mut self, d: Duration) {
        assert!(
            d <= Duration::from_secs(1),
            "slice of {d:?} is over one second"
        );
        assert!(!d.is_zero(), "a zero sleep would spin");
        assert!(self.sleeps.len() < 200_000, "the wait never ended");
        if self.signal_seen_at_ms.is_some() {
            self.sleeps_after_signal += 1;
        }
        self.sleeps.push(d);
        self.ms += d.as_millis() as i64;
    }

    fn fingerprint(&mut self) -> Option<u8> {
        let t = self.elapsed();
        self.probes.push(t);
        self.prints
            .iter()
            .rev()
            .find(|(from, _)| *from <= t)
            .and_then(|(_, p)| *p)
    }

    fn signal(&mut self) -> Option<i32> {
        let (from, sig) = self.signal_from?;
        if self.ms >= from * 1000 {
            self.signal_seen_at_ms.get_or_insert(self.ms);
            Some(sig)
        } else {
            None
        }
    }

    fn session_ended(&mut self) -> bool {
        self.parent_gone_from.is_some_and(|t| self.elapsed() >= t)
    }

    fn rotator_capacity(&mut self, start: i64) -> bool {
        assert_eq!(start, T0, "capacity is judged from the wait's start");
        self.capacity_from.is_some_and(|t| self.elapsed() >= t)
    }

    fn log(&mut self, line: &str) {
        self.logs.push(line.to_string());
    }
}

fn plan_in(secs: i64, bound: Bound) -> Plan {
    Plan {
        until: T0 + secs,
        bound,
        reset: T0,
    }
}

#[test]
fn a_zero_length_wait_ends_at_once_without_sleeping() {
    let mut f = Fake::new();
    assert_eq!(
        wait(&plan_in(0, Bound::ResetMargin), &mut f),
        End::ResetReached
    );
    assert!(f.sleeps.is_empty(), "{:?}", f.sleeps);
    assert!(f.progress().is_empty());
}

#[test]
fn each_bound_maps_to_its_end_reason() {
    for (bound, want) in [
        (Bound::ResetMargin, End::ResetReached),
        (Bound::MaxWait, End::MaxReached),
        (Bound::StepTimeout, End::StepTimeout),
    ] {
        let mut f = Fake::new();
        assert_eq!(wait(&plan_in(75, bound), &mut f), want, "{bound:?}");
        assert!(
            f.elapsed() >= 75,
            "{bound:?} ended early at {}",
            f.elapsed()
        );
        assert!(f.elapsed() <= 76, "{bound:?} overslept to {}", f.elapsed());
        let mut f = Fake::new();
        assert_eq!(wait(&plan_in(0, bound), &mut f), want, "{bound:?} zero");
    }
}

#[test]
fn sleeps_are_slices_of_at_most_one_second() {
    let mut f = Fake::new();
    wait(&plan_in(300, Bound::ResetMargin), &mut f);
    assert!(!f.sleeps.is_empty());
    // The assertions in `Fake::sleep` cover each slice; the total is the wait.
    let total: Duration = f.sleeps.iter().sum();
    assert!(total >= Duration::from_secs(300) && total < Duration::from_secs(302));
}

#[test]
fn a_progress_line_is_written_every_sixty_seconds() {
    let mut f = Fake::new();
    wait(&plan_in(150, Bound::ResetMargin), &mut f);
    assert_eq!(
        f.progress(),
        vec![
            "amplihack.recipe.usage_limit_wait progress remaining=90s",
            "amplihack.recipe.usage_limit_wait progress remaining=30s",
        ]
    );
    // `wait` writes only progress lines; start, end and resume are the
    // caller's.
    assert_eq!(f.logs.len(), 2, "{:?}", f.logs);
}

#[test]
fn the_fingerprint_is_taken_at_the_start_and_every_thirty_seconds() {
    let mut f = Fake::new();
    wait(&plan_in(100, Bound::ResetMargin), &mut f);
    let mut distinct = f.probes.clone();
    distinct.dedup();
    assert_eq!(distinct, vec![0, 30, 60, 90]);
}

#[test]
fn a_changed_login_ends_the_wait_at_the_second_differing_poll() {
    let mut f = Fake::new();
    f.prints = vec![(0, Some(1)), (45, Some(2))];
    assert_eq!(
        wait(&plan_in(1800, Bound::ResetMargin), &mut f),
        End::LoginChanged
    );
    assert_eq!(f.elapsed(), 90);
}

#[test]
fn one_differing_poll_followed_by_a_matching_one_keeps_waiting() {
    let mut f = Fake::new();
    f.prints = vec![
        (0, Some(1)),
        (45, None),
        (75, Some(1)),
        (105, None),
        (135, Some(1)),
    ];
    assert_eq!(
        wait(&plan_in(200, Bound::ResetMargin), &mut f),
        End::ResetReached
    );
    assert!(f.elapsed() >= 200, "ended early at {}", f.elapsed());
}

#[test]
fn a_new_login_must_read_the_same_twice() {
    // A logout seen at 60 s, then a new login at 90 s and 120 s.
    let mut f = Fake::new();
    f.prints = vec![(0, Some(1)), (45, None), (75, Some(3))];
    assert_eq!(
        wait(&plan_in(1800, Bound::ResetMargin), &mut f),
        End::LoginChanged
    );
    assert_eq!(f.elapsed(), 120);
}

#[test]
fn a_part_that_appears_or_disappears_after_the_baseline_counts_as_a_change() {
    for prints in [
        vec![(0, None), (40, Some(1))],
        vec![(0, Some(1)), (40, None)],
    ] {
        let mut f = Fake::new();
        f.prints = prints.clone();
        assert_eq!(
            wait(&plan_in(1800, Bound::ResetMargin), &mut f),
            End::LoginChanged,
            "{prints:?}"
        );
        assert_eq!(f.elapsed(), 90);
    }
}

#[test]
fn a_torn_or_failed_first_read_is_not_a_change() {
    // The first probe fails or reads a torn file; every later read agrees.
    for first in [None, Some(9)] {
        let mut f = Fake::new();
        f.prints = vec![(0, first), (1, Some(1))];
        assert_eq!(
            wait(&plan_in(300, Bound::ResetMargin), &mut f),
            End::ResetReached,
            "first read {first:?}"
        );
    }
}

#[test]
fn an_absent_fingerprint_that_stays_absent_is_no_change() {
    let mut f = Fake::new();
    f.prints = vec![(0, None)];
    assert_eq!(
        wait(&plan_in(200, Bound::ResetMargin), &mut f),
        End::ResetReached
    );
}

#[test]
fn a_signal_ends_the_wait_within_one_slice() {
    for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        let mut f = Fake::new();
        f.signal_from = Some((10, sig));
        assert_eq!(
            wait(&plan_in(1800, Bound::ResetMargin), &mut f),
            End::Signal(sig)
        );
        assert_eq!(f.sleeps_after_signal, 0, "slept after seeing the signal");
        assert!(f.elapsed() <= 11, "noticed at {}", f.elapsed());
    }
}

#[test]
fn a_pending_signal_wins_over_a_passed_deadline_and_a_changed_login() {
    let mut f = Fake::new();
    f.signal_from = Some((0, libc::SIGTERM));
    assert_eq!(
        wait(&plan_in(0, Bound::ResetMargin), &mut f),
        End::Signal(libc::SIGTERM)
    );
    let mut f = Fake::new();
    f.signal_from = Some((30, libc::SIGINT));
    f.prints = vec![(0, Some(1)), (20, Some(2))];
    assert_eq!(
        wait(&plan_in(1800, Bound::ResetMargin), &mut f),
        End::Signal(libc::SIGINT)
    );
}

#[test]
fn a_passed_deadline_wins_over_a_changed_login() {
    let mut f = Fake::new();
    f.prints = vec![(0, Some(1)), (45, Some(2))];
    assert_eq!(wait(&plan_in(60, Bound::MaxWait), &mut f), End::MaxReached);
}

#[test]
fn the_end_of_the_steps_session_ends_the_wait_within_one_slice() {
    let mut f = Fake::new();
    f.parent_gone_from = Some(10);
    assert_eq!(
        wait(&plan_in(5 * 3600, Bound::ResetMargin), &mut f),
        End::SessionEnded
    );
    assert!(f.elapsed() <= 11, "noticed at {}", f.elapsed());
}

#[test]
fn fresh_rotator_capacity_ends_the_wait_at_the_next_poll() {
    let mut f = Fake::new();
    f.capacity_from = Some(40);
    assert_eq!(
        wait(&plan_in(1800, Bound::ResetMargin), &mut f),
        End::RotatorCapacity
    );
    assert_eq!(f.elapsed(), 60);
}
