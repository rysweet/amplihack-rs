//! `wait` (issue #1618), continued from `tests_wait.rs` (whose `Fake`
//! `WaitDeps` this uses): a stop signal, which end wins when several hold
//! at once, the end of the step's session, and fresh rotator capacity. Also
//! how a capacity wait ends (issue #1801).
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
