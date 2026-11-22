//! `wait` and `line` (issue #1618), driven by a fake `WaitDeps` with its own
//! virtual clock: the order of checks in each pass (signal, deadline, login
//! fingerprint, progress), sleep slices of at most one second, the 30-second
//! fingerprint poll and its stable baseline, and the 60-second progress line.
//! The other ends (a stop signal, which end wins, the step's session ending,
//! rotator capacity) are in `tests_wait_ends.rs`, which uses `Fake` here.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_wait;`.

use super::plan::{Bound, Plan, PlanKind};
use super::wait::{End, WaitDeps, wait};
use std::time::Duration;

/// 2026-10-18 08:00:00 -07:00.
pub(super) const T0: i64 = 1_792_335_600;

/// Login fingerprint schedule: from virtual second `.0` (relative to T0) on,
/// the probe returns `.1`.
type Schedule = Vec<(i64, Option<u8>)>;

pub(super) struct Fake {
    ms: i64,
    pub(super) prints: Schedule,
    /// Virtual second from which a signal is pending.
    pub(super) signal_from: Option<(i64, i32)>,
    signal_seen_at_ms: Option<i64>,
    sleeps: Vec<Duration>,
    pub(super) sleeps_after_signal: usize,
    /// The virtual second of each login fingerprint. `tests_wait_ends`
    /// derives from this where another login's capacity may be read, so the
    /// reading's cadence is stated against the checks the wait already
    /// makes rather than against their interval (issue #2694).
    pub(super) probes: Vec<i64>,
    logs: Vec<String>,
    /// Virtual second from which the step's parent is gone.
    pub(super) parent_gone_from: Option<i64>,
    /// Virtual second from which the rotator reports fresh capacity.
    pub(super) capacity_from: Option<i64>,
    /// The virtual second of each reading of the rotator's cycle. The
    /// other-login source has `other_probes`; this is its counterpart, so a
    /// wait that must read *neither* can say so about both (issue #2731).
    pub(super) capacity_probes: Vec<i64>,
    /// Virtual second from which another stored login reports capacity.
    pub(super) other_from: Option<i64>,
    /// Virtual second from which the named login key differs from the one
    /// the attempt ran on (issue #2731). Unlike `prints` this needs no
    /// baseline: the anchor was read before the attempt was launched.
    pub(super) login_changed_from: Option<i64>,
    /// The virtual second of each reading of the login key.
    pub(super) login_probes: Vec<i64>,
    /// The virtual second of each reading of another login's capacity, so a
    /// test can show the wait adds no cadence of its own (issue #2694).
    pub(super) other_probes: Vec<i64>,
    /// Virtual second from which the login the retry would use has room, as
    /// the rotator's last cycle records it (issue #2857). It defaults to room
    /// from the start, so a test that is not about the room gate sees a wait
    /// that is never held back; the tests that are about it close it with
    /// `None` or open it later.
    pub(super) change_room_from: Option<i64>,
    /// The virtual second of each question "may this change end the wait",
    /// which is asked only after a login change has been seen.
    pub(super) change_probes: Vec<i64>,
}

impl Fake {
    pub(super) fn new() -> Self {
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
            capacity_probes: Vec::new(),
            other_from: None,
            other_probes: Vec::new(),
            login_changed_from: None,
            login_probes: Vec::new(),
            change_room_from: Some(0),
            change_probes: Vec::new(),
        }
    }

    pub(super) fn elapsed(&self) -> i64 {
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

    fn login_changed(&mut self) -> bool {
        self.login_probes.push(self.elapsed());
        self.login_changed_from.is_some_and(|t| self.elapsed() >= t)
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
        self.capacity_probes.push(self.elapsed());
        self.capacity_from.is_some_and(|t| self.elapsed() >= t)
    }

    fn change_may_end(&mut self) -> bool {
        self.change_probes.push(self.elapsed());
        self.change_room_from.is_some_and(|t| self.elapsed() >= t)
    }

    fn other_capacity(&mut self, start: i64) -> bool {
        assert_eq!(start, T0, "capacity is judged from the wait's start");
        self.other_probes.push(self.elapsed());
        self.other_from.is_some_and(|t| self.elapsed() >= t)
    }

    fn log(&mut self, line: &str) {
        self.logs.push(line.to_string());
    }
}

pub(super) fn plan_in(secs: i64, bound: Bound) -> Plan {
    Plan::for_test(T0 + secs, bound, PlanKind::Reset, Some(T0))
}

pub(super) fn capacity_plan_in(secs: i64, bound: Bound) -> Plan {
    Plan::for_test(T0 + secs, bound, PlanKind::Capacity, None)
}

/// A login-change plan ending `secs` after `T0` (issue #2731), built by the
/// real constructor rather than by hand: the flag that suppresses the two
/// capacity sources is `plan_login_change`'s to set, and a test that could
/// set it itself would stop proving that the constructor does.
pub(super) fn login_change_plan_in(secs: i64) -> Plan {
    let max = u64::try_from(secs).expect("a login-change wait's length is not negative");
    super::plan::plan_login_change(T0, Some(max))
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
