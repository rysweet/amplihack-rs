//! `wait` (issue #1618), continued from `tests_wait.rs` (whose `Fake`
//! `WaitDeps` this uses): a stop signal, which end wins when several hold
//! at once, the end of the step's session, and fresh rotator capacity. Also
//! how a capacity wait ends (issue #1801), and how another login's
//! capacity ends a wait of any kind (issue #2694).
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_wait_ends;`.

use super::plan::Bound;
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

// --- another login's capacity ends any wait (issue #2694) -----------------

/// The issue's own reproduction: a step under a stored profile, a stated
/// reset 3 h 45 min away, so the plan is a reset wait, and a reading taken
/// 40 s in showing another login has room. The wait must end at the next
/// 30 s poll instead of sleeping to the reset.
#[test]
fn another_logins_capacity_also_ends_a_reset_wait() {
    let mut f = Fake::new();
    f.other_from = Some(40);
    assert_eq!(
        wait(&plan_in(13_500, Bound::ResetMargin), &mut f),
        End::OtherCapacity,
        "a near reset must not keep the step on an exhausted login"
    );
    assert_eq!(f.elapsed(), 60, "the first poll after the reading");
}

/// Requirement 4: with nothing to move to, the reset wait is as it was.
#[test]
fn a_reset_wait_with_no_other_login_still_reaches_its_reset() {
    let mut f = Fake::new();
    assert_eq!(
        wait(&plan_in(13_500, Bound::ResetMargin), &mut f),
        End::ResetReached
    );
    assert!(f.elapsed() >= 13_500, "ended early at {}", f.elapsed());
}

/// The same deadline bounds the wait whatever its end, so a maximum reached
/// first still ends the wait there.
#[test]
fn a_reset_wait_reaches_its_maximum_before_a_later_reading() {
    let mut f = Fake::new();
    f.other_from = Some(120);
    assert_eq!(wait(&plan_in(90, Bound::MaxWait), &mut f), End::MaxReached);
}

#[test]
fn rotator_capacity_is_checked_first_on_a_reset_wait_too() {
    let mut f = Fake::new();
    f.capacity_from = Some(40);
    f.other_from = Some(40);
    assert_eq!(
        wait(&plan_in(13_500, Bound::ResetMargin), &mut f),
        End::RotatorCapacity
    );
}

#[test]
fn a_signal_and_a_passed_deadline_outrank_other_capacity_on_a_reset_wait() {
    let mut f = Fake::new();
    f.signal_from = Some((30, libc::SIGTERM));
    f.other_from = Some(0);
    assert_eq!(
        wait(&plan_in(13_500, Bound::ResetMargin), &mut f),
        End::Signal(libc::SIGTERM)
    );
    let mut f = Fake::new();
    f.other_from = Some(0);
    assert_eq!(
        wait(&plan_in(30, Bound::ResetMargin), &mut f),
        End::ResetReached,
        "the deadline is read before the 30 s probes"
    );
}

/// Within one poll the login fingerprint is still read first: a login that
/// changed under the step ends the wait before any reading does.
#[test]
fn a_changed_login_outranks_another_logins_capacity_on_a_reset_wait() {
    let mut f = Fake::new();
    f.prints = vec![(0, Some(1)), (45, Some(2))];
    f.other_from = Some(61);
    assert_eq!(
        wait(&plan_in(13_500, Bound::ResetMargin), &mut f),
        End::LoginChanged
    );
    assert_eq!(f.elapsed(), 90, "the poll two agreeing probes in");
}

/// The reading rides the login check the wait already makes and happens
/// nowhere else, on a reset wait as on a capacity wait: issue #2694 widens
/// which waits consult it and adds no timer, interval or retry of its own.
///
/// Stated against the login checks themselves, not against the interval
/// between them, so this says where the reading is taken and leaves what
/// that interval is set to for `the_fingerprint_is_taken_at_the_start_and_
/// every_thirty_seconds` to pin. The first check is the baseline taken
/// before the loop and no reading accompanies it, so the readings are
/// exactly the tail of the checks.
#[test]
fn another_logins_capacity_is_read_on_the_existing_login_check() {
    for plan in [
        plan_in(100, Bound::ResetMargin),
        capacity_plan_in(100, Bound::MaxWait),
    ] {
        let mut f = Fake::new();
        wait(&plan, &mut f);
        let mut checks = f.probes.clone();
        checks.dedup();
        assert!(
            !f.other_probes.is_empty(),
            "{:?}: a wait of this length does read another login's capacity",
            plan.kind
        );
        assert_eq!(
            f.other_probes,
            checks[1..],
            "{:?}: every reading falls on a login check, and the wait makes \
             no check of its own; checks were {checks:?}",
            plan.kind
        );
    }
}

/// The reading is not taken at all before the first poll, so a wait that
/// ends inside its first 30 seconds makes none of the probe's reads.
#[test]
fn a_wait_shorter_than_one_poll_never_reads_another_logins_capacity() {
    let mut f = Fake::new();
    f.other_from = Some(0);
    assert_eq!(
        wait(&plan_in(20, Bound::ResetMargin), &mut f),
        End::ResetReached
    );
    assert!(f.other_probes.is_empty(), "{:?}", f.other_probes);
}

#[test]
fn the_end_of_the_steps_session_outranks_other_capacity_on_a_reset_wait() {
    let mut f = Fake::new();
    f.parent_gone_from = Some(30);
    f.other_from = Some(0);
    assert_eq!(
        wait(&plan_in(13_500, Bound::ResetMargin), &mut f),
        End::SessionEnded,
        "nobody is left to use the retry's result"
    );
}

#[test]
fn a_step_timeout_outranks_other_capacity_on_a_reset_wait() {
    let mut f = Fake::new();
    f.other_from = Some(0);
    assert_eq!(
        wait(&plan_in(30, Bound::StepTimeout), &mut f),
        End::StepTimeout
    );
}
