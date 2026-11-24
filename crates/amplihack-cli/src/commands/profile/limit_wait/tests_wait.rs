//! `wait` and `line` (issue #1618), driven by a fake `WaitDeps` with its own
//! virtual clock. The fake blocks the way the real wait does (issue #2911):
//! until the next moment something the wait watches changes, or until the
//! time the wait asked for, never in slices. The order of checks on each
//! wake, the fingerprint's baseline and failed reads, the 60-second progress
//! line, the one read as the wait begins, the absence of any other read
//! without a write, and a wait with no deadline that no clock ends are
//! pinned here. The
//! other ends (a stop signal, which end wins, the step's session ending,
//! rotator capacity) are in `tests_wait_ends.rs`, which uses `Fake` here.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_wait;`.

use super::plan::{Bound, Plan, PlanKind};
use super::wait::{End, WaitDeps, wait};

/// 2026-10-18 08:00:00 -07:00.
pub(super) const T0: i64 = 1_792_335_600;

/// Login fingerprint schedule: from virtual second `.0` (relative to T0) on,
/// the probe returns `.1`. `None` is a reading that could not be made.
type Schedule = Vec<(i64, Option<u8>)>;

pub(super) struct Fake {
    /// Virtual seconds since T0.
    secs: i64,
    pub(super) prints: Schedule,
    /// Virtual second from which a signal is pending.
    pub(super) signal_from: Option<(i64, i32)>,
    /// Each block, as (from, to) in virtual seconds.
    pub(super) blocks: Vec<(i64, i64)>,
    /// The virtual second of each login fingerprint.
    pub(super) probes: Vec<i64>,
    logs: Vec<String>,
    /// Virtual second from which the step's parent is gone.
    pub(super) parent_gone_from: Option<i64>,
    /// Virtual second from which the rotator reports fresh capacity.
    pub(super) capacity_from: Option<i64>,
    /// The virtual second of each reading of the rotator's cycle.
    pub(super) capacity_probes: Vec<i64>,
    /// Virtual second from which another stored login reports capacity.
    pub(super) other_from: Option<i64>,
    /// Virtual second from which the named login key differs from the one
    /// the attempt ran on (issue #2731). Unlike `prints` this needs no
    /// baseline: the anchor was read before the attempt was launched.
    pub(super) login_changed_from: Option<i64>,
    /// The virtual second of each reading of the login key.
    pub(super) login_probes: Vec<i64>,
    /// The virtual second of each reading of another login's capacity.
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
    /// Virtual seconds at which a watched file is written without anything
    /// the checks read changing.
    pub(super) writes: Vec<i64>,
}

impl Fake {
    pub(super) fn new() -> Self {
        Self {
            secs: 0,
            prints: vec![(0, Some(1))],
            signal_from: None,
            blocks: Vec::new(),
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
            writes: Vec::new(),
        }
    }

    pub(super) fn elapsed(&self) -> i64 {
        self.secs
    }

    /// Every virtual second at which a watched file is written: a change of
    /// fingerprint, of either capacity source, of the login key or of the
    /// room the rotator records, and the bare writes.
    fn file_writes(&self) -> Vec<i64> {
        let mut at: Vec<i64> = self.prints.iter().map(|(t, _)| *t).collect();
        at.extend(self.capacity_from);
        at.extend(self.other_from);
        at.extend(self.login_changed_from);
        at.extend(self.change_room_from);
        at.extend(self.writes.iter().copied());
        at
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
        T0 + self.secs
    }

    fn block(&mut self, until: i64) -> bool {
        let to = until - T0;
        let now = self.secs;
        assert!(
            to > now,
            "asked to block until {to} at {now}: the wait would spin"
        );
        assert!(self.blocks.len() < 200_000, "the wait never ended");
        let signal = self.signal_from.map(|(t, _)| t);
        let file = self.file_writes().into_iter().filter(|t| *t > now).min();
        let other = [signal, self.parent_gone_from]
            .into_iter()
            .flatten()
            .filter(|t| *t > now)
            .min();
        let at = [file, other, Some(to)]
            .into_iter()
            .flatten()
            .min()
            .unwrap_or(to);
        self.secs = at;
        self.blocks.push((now, at));
        file == Some(at)
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
        (self.secs >= from).then_some(sig)
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

/// A capacity plan whose limit stated no usable reset, so it has no
/// deadline (issue #2911), built by the real constructor.
pub(super) fn open_capacity_plan() -> Plan {
    super::plan::plan_capacity(T0, None)
}

/// A login-change plan (issue #2731), which has no deadline (issue #2911),
/// built by the real constructor rather than by hand: the flag that
/// suppresses the two capacity sources is `plan_login_change`'s to set, and
/// a test that could set it itself would stop proving that the constructor
/// does.
pub(super) fn login_change_plan() -> Plan {
    super::plan::plan_login_change()
}

#[test]
fn a_zero_length_wait_ends_at_once_without_blocking() {
    let mut f = Fake::new();
    assert_eq!(
        wait(&plan_in(0, Bound::ResetMargin), &mut f),
        End::ResetReached
    );
    assert!(f.blocks.is_empty(), "{:?}", f.blocks);
    assert!(f.progress().is_empty());
}

/// A deadline is a stated reset, so reaching it is the reset passing,
/// whatever kind of wait it is.
#[test]
fn a_deadline_reached_is_the_reset_reached() {
    for plan in [
        plan_in(75, Bound::ResetMargin),
        capacity_plan_in(75, Bound::ResetMargin),
    ] {
        let mut f = Fake::new();
        assert_eq!(wait(&plan, &mut f), End::ResetReached, "{:?}", plan.kind);
        assert_eq!(f.elapsed(), 75, "{:?} ended at {}", plan.kind, f.elapsed());
    }
}

/// Issue #2967: a deadline another login's reset set (`earliest_reset+margin`)
/// is a reset passing too, so reaching it is `reset reached`, on a reset wait
/// and a capacity wait alike; the retry then runs on that login. It is not a
/// login-change wait, so a login with room written before the deadline still
/// ends it early, and the wait reads its files no more often than any other.
#[test]
fn a_deadline_another_logins_reset_set_is_the_reset_reached() {
    for kind in [PlanKind::Reset, PlanKind::Capacity] {
        let plan = Plan::for_test(T0 + 75, Bound::EarliestReset, kind, Some(T0 + 3_600));
        let mut f = Fake::new();
        assert_eq!(wait(&plan, &mut f), End::ResetReached, "{kind:?}");
        assert_eq!(f.elapsed(), 75, "{kind:?} ended at {}", f.elapsed());
        assert_eq!(f.other_probes, vec![0], "{kind:?}: read once, as it began");
    }
    let plan = Plan::for_test(T0 + 600, Bound::EarliestReset, PlanKind::Reset, None);
    assert!(plan.capacity_may_end());
    let mut f = Fake::new();
    f.other_from = Some(40);
    assert_eq!(wait(&plan, &mut f), End::OtherCapacity);
    assert_eq!(f.elapsed(), 40, "ended on the write, not the deadline");
}

/// Issue #2911: a wait with no deadline is ended by no clock. With nothing
/// written it wakes only for its progress lines, reads no file at any of
/// them, and runs until something it waits on happens: here, six hours in,
/// the step's session ending. Its progress lines have nothing to count down
/// to, and say so.
#[test]
fn a_wait_with_no_deadline_is_ended_by_no_clock() {
    let six_hours = 6 * 3600;
    for plan in [open_capacity_plan(), login_change_plan()] {
        assert_eq!(plan.until, None);
        let mut f = Fake::new();
        f.parent_gone_from = Some(six_hours + 30);
        assert_eq!(wait(&plan, &mut f), End::SessionEnded);
        assert_eq!(f.elapsed(), six_hours + 30);
        let ends: Vec<i64> = f.blocks.iter().map(|(_, to)| *to).collect();
        let mut want: Vec<i64> = (1..=360).map(|m| m * 60).collect();
        want.push(six_hours + 30);
        assert_eq!(ends, want, "one wake a minute, for the progress line");
        assert_eq!(f.probes, vec![0, 0], "the baseline and the first read");
        assert_eq!(f.login_probes, vec![0]);
        let progress = f.progress();
        assert_eq!(progress.len(), 360);
        assert!(
            progress
                .iter()
                .all(|l| *l == "amplihack.recipe.usage_limit_wait progress remaining=none"),
            "{:?}",
            &progress[..3]
        );
    }
}

/// Issue #2911: with nothing written, a wait reads its files once as it
/// begins, then wakes for its progress lines and its deadline and for
/// nothing else, and reads no file at any of them.
#[test]
fn with_nothing_written_the_wait_reads_once_and_wakes_only_on_the_clock() {
    let mut f = Fake::new();
    assert_eq!(
        wait(&plan_in(3_600, Bound::ResetMargin), &mut f),
        End::ResetReached
    );
    let ends: Vec<i64> = f.blocks.iter().map(|(_, to)| *to).collect();
    let want: Vec<i64> = (1..=60).map(|m| m * 60).collect();
    assert_eq!(ends, want, "one wake a minute, for the progress line");
    assert_eq!(f.probes, vec![0, 0], "the baseline and the first read");
    assert_eq!(f.capacity_probes, vec![0]);
    assert_eq!(f.other_probes, vec![0]);
    assert_eq!(f.login_probes, vec![0]);
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

/// After the first read, the files are read at the moment of each write,
/// and at no other time.
#[test]
fn the_files_are_read_at_each_write_and_only_then() {
    let mut f = Fake::new();
    f.writes = vec![7, 41, 42, 200];
    wait(&plan_in(300, Bound::ResetMargin), &mut f);
    assert_eq!(f.probes, vec![0, 0, 7, 41, 42, 200]);
    assert_eq!(f.capacity_probes, vec![0, 7, 41, 42, 200]);
    assert_eq!(f.other_probes, vec![0, 7, 41, 42, 200]);
    assert_eq!(f.login_probes, vec![0, 7, 41, 42, 200]);
}

#[test]
fn a_changed_login_ends_the_wait_at_the_write_that_changed_it() {
    let mut f = Fake::new();
    f.prints = vec![(0, Some(1)), (45, Some(2))];
    assert_eq!(
        wait(&plan_in(1800, Bound::ResetMargin), &mut f),
        End::LoginChanged
    );
    assert_eq!(f.elapsed(), 45);
}

/// A read made part-way through a write is not a reading: it neither ends
/// the wait nor moves the baseline, and the write that completes the file
/// is read as the login it names.
#[test]
fn a_failed_read_between_two_readings_of_the_same_login_is_no_change() {
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
    assert_eq!(f.elapsed(), 200);
}

#[test]
fn a_new_login_after_a_failed_read_ends_the_wait_when_it_is_read() {
    let mut f = Fake::new();
    f.prints = vec![(0, Some(1)), (45, None), (75, Some(3))];
    assert_eq!(
        wait(&plan_in(1800, Bound::ResetMargin), &mut f),
        End::LoginChanged
    );
    assert_eq!(f.elapsed(), 75);
}

/// A part of the fingerprint that appears or disappears is a different
/// fingerprint (the real one compares each part), so it is a change.
#[test]
fn a_different_reading_after_the_baseline_is_a_change() {
    for prints in [
        vec![(0, Some(1)), (40, Some(2))],
        vec![(0, Some(2)), (40, Some(1))],
    ] {
        let mut f = Fake::new();
        f.prints = prints.clone();
        assert_eq!(
            wait(&plan_in(1800, Bound::ResetMargin), &mut f),
            End::LoginChanged,
            "{prints:?}"
        );
        assert_eq!(f.elapsed(), 40);
    }
}

/// A baseline that could not be read is taken from the first reading that
/// can be made, so a failed first read is never itself a change.
#[test]
fn a_failed_first_read_takes_the_baseline_from_the_next_reading() {
    let mut f = Fake::new();
    f.prints = vec![(0, None), (1, Some(1)), (90, Some(1))];
    assert_eq!(
        wait(&plan_in(300, Bound::ResetMargin), &mut f),
        End::ResetReached
    );
    let mut f = Fake::new();
    f.prints = vec![(0, None), (1, Some(1)), (90, Some(2))];
    assert_eq!(
        wait(&plan_in(300, Bound::ResetMargin), &mut f),
        End::LoginChanged
    );
    assert_eq!(f.elapsed(), 90);
}

#[test]
fn a_fingerprint_that_can_never_be_read_is_no_change() {
    let mut f = Fake::new();
    f.prints = vec![(0, None), (50, None)];
    assert_eq!(
        wait(&plan_in(200, Bound::ResetMargin), &mut f),
        End::ResetReached
    );
}
