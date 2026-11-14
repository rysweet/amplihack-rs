//! `wait` (issue #1618), continued from `tests_wait.rs` (whose `Fake`
//! `WaitDeps` this uses): a stop signal, which end wins when several hold
//! at once, the end of the step's session, and fresh rotator capacity. Also
//! how a capacity wait ends (issue #1801).
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_wait_ends;`.

use super::plan::{Bound, Plan, PlanKind};
use super::tests_wait::{Fake, capacity_plan_in, plan_in};
use super::wait::{End, wait};

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

// --- capacity waits (issue #1801) ----------------------------------------

#[test]
fn a_capacity_wait_expires_at_max_wait_where_a_reset_wait_reaches_max() {
    let mut f = Fake::new();
    assert_eq!(
        wait(&capacity_plan_in(90, Bound::MaxWait), &mut f),
        End::CapacityExpired
    );
    let mut f = Fake::new();
    assert_eq!(wait(&plan_in(90, Bound::MaxWait), &mut f), End::MaxReached);
}

#[test]
fn a_capacity_wait_that_reaches_the_failed_logins_reset_retries() {
    let mut f = Fake::new();
    assert_eq!(
        wait(&capacity_plan_in(90, Bound::ResetMargin), &mut f),
        End::ResetReached
    );
}

#[test]
fn a_zero_length_capacity_wait_expires_without_sleeping() {
    let mut f = Fake::new();
    assert_eq!(
        wait(&capacity_plan_in(0, Bound::MaxWait), &mut f),
        End::CapacityExpired
    );
    assert_eq!(f.elapsed(), 0);
}

#[test]
fn a_step_timeout_is_still_a_step_timeout_on_a_capacity_wait() {
    let mut f = Fake::new();
    assert_eq!(
        wait(&capacity_plan_in(90, Bound::StepTimeout), &mut f),
        End::StepTimeout
    );
}

#[test]
fn another_login_with_capacity_ends_the_wait_at_the_next_poll() {
    let mut f = Fake::new();
    f.other_from = Some(40);
    assert_eq!(
        wait(&capacity_plan_in(1800, Bound::MaxWait), &mut f),
        End::OtherCapacity
    );
    assert_eq!(f.elapsed(), 60);
}

#[test]
fn rotator_capacity_is_checked_before_another_logins_capacity() {
    let mut f = Fake::new();
    f.capacity_from = Some(40);
    f.other_from = Some(40);
    assert_eq!(
        wait(&capacity_plan_in(1800, Bound::MaxWait), &mut f),
        End::RotatorCapacity
    );
}

#[test]
fn a_signal_and_a_passed_deadline_outrank_other_capacity() {
    let mut f = Fake::new();
    f.signal_from = Some((30, libc::SIGTERM));
    f.other_from = Some(0);
    assert_eq!(
        wait(&capacity_plan_in(1800, Bound::MaxWait), &mut f),
        End::Signal(libc::SIGTERM)
    );
    let mut f = Fake::new();
    f.other_from = Some(0);
    assert_eq!(
        wait(&capacity_plan_in(30, Bound::MaxWait), &mut f),
        End::CapacityExpired,
        "the deadline is read before the 30 s probes"
    );
}

// --- issue #2022: an unbounded capacity wait ------------------------------

fn unbounded() -> Plan {
    Plan {
        until: None,
        bound: Bound::Unbounded,
        kind: PlanKind::Capacity,
        reset: None,
    }
}

#[test]
fn an_unbounded_wait_has_no_deadline_and_ends_on_another_logins_capacity() {
    let mut f = Fake::new();
    // Well past the former six-hour capacity default.
    f.other_from = Some(7 * 3600);
    assert_eq!(wait(&unbounded(), &mut f), End::OtherCapacity);
    assert!(
        f.elapsed() >= 7 * 3600,
        "ended early at {}: no default ceiling applies",
        f.elapsed()
    );
}

#[test]
fn an_unbounded_wait_ends_on_rotator_capacity_or_a_login_change() {
    let mut f = Fake::new();
    f.capacity_from = Some(9 * 3600);
    assert_eq!(wait(&unbounded(), &mut f), End::RotatorCapacity);

    let mut f = Fake::new();
    f.prints = vec![(0, Some(1)), (8 * 3600, Some(2))];
    assert_eq!(wait(&unbounded(), &mut f), End::LoginChanged);
}

// S11, S12: an unbounded wait still sleeps between polls and still stops on
// a signal or the end of the step's session.
#[test]
fn an_unbounded_wait_stops_on_a_signal_within_one_slice() {
    let mut f = Fake::new();
    // Fake clock: 6 h + 10 s is simulated; no real time passes.
    f.signal_from = Some((6 * 3600 + 10, libc::SIGTERM));
    assert_eq!(wait(&unbounded(), &mut f), End::Signal(libc::SIGTERM));
    assert_eq!(f.sleeps_after_signal, 0);
    assert!(f.elapsed() <= 6 * 3600 + 11, "noticed at {}", f.elapsed());

    let mut f = Fake::new();
    f.parent_gone_from = Some(10);
    assert_eq!(wait(&unbounded(), &mut f), End::SessionEnded);
}

#[test]
fn an_unbounded_wait_logs_progress_without_remaining() {
    let mut f = Fake::new();
    f.other_from = Some(300);
    assert_eq!(wait(&unbounded(), &mut f), End::OtherCapacity);
    let progress = f.progress();
    assert!(!progress.is_empty(), "progress lines are still written");
    for l in progress {
        assert!(l.ends_with(" progress"), "bare progress line: {l}");
        assert!(!l.contains("remaining="), "{l}");
    }
}

// A capacity wait with no deadline needs the rotator to end: once no rotator
// is running at two polls in a row, nothing could record capacity or swap
// the login, so the wait ends (issue #2022). Fake clock; no real time passes.
#[test]
fn an_unbounded_capacity_wait_ends_when_the_rotator_stays_stopped() {
    let mut f = Fake::new();
    f.rotator_gone = Some((7 * 3600, i64::MAX));
    assert_eq!(wait(&unbounded(), &mut f), End::RotatorStopped);
    let misses: Vec<i64> = f
        .rotator_checks
        .iter()
        .copied()
        .filter(|&t| t >= 7 * 3600)
        .collect();
    assert_eq!(
        misses.len(),
        2,
        "ends at the second miss in a row: {misses:?}"
    );
}

#[test]
fn one_missed_rotator_poll_does_not_end_the_wait() {
    let mut f = Fake::new();
    // Gone for one poll only, as during a rotator restart.
    f.rotator_gone = Some((3600, 3600 + 30));
    f.other_from = Some(2 * 3600);
    assert_eq!(wait(&unbounded(), &mut f), End::OtherCapacity);
    assert!(
        f.rotator_checks.contains(&3600),
        "the miss was polled: {:?}",
        f.rotator_checks
    );
}

#[test]
fn a_bounded_or_reset_wait_never_asks_for_the_rotator() {
    let mut f = Fake::new();
    f.rotator_gone = Some((0, i64::MAX));
    assert_eq!(wait(&plan_in(600, Bound::MaxWait), &mut f), End::MaxReached);
    assert!(f.rotator_checks.is_empty(), "{:?}", f.rotator_checks);

    let mut f = Fake::new();
    f.rotator_gone = Some((0, i64::MAX));
    let mut capacity = plan_in(600, Bound::MaxWait);
    capacity.kind = PlanKind::Capacity;
    assert_eq!(wait(&capacity, &mut f), End::CapacityExpired);
    assert!(f.rotator_checks.is_empty(), "{:?}", f.rotator_checks);
}
