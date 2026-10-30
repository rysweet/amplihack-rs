//! `wait` (issues #1618, #1761), continued from `tests_wait.rs` (whose
//! `Fake` `WaitDeps` this uses): a stop signal, which end wins when several
//! hold at once, the end of the step's session, fresh rotator capacity, and
//! the run budget's deadline.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_wait_ends;`.

use super::plan::Bound;
use super::tests_wait::{Fake, plan_in};
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

#[test]
fn the_budget_deadline_ends_the_wait_with_budget_spent() {
    let mut f = Fake::new();
    assert_eq!(wait(&plan_in(600, Bound::Budget), &mut f), End::BudgetSpent);
    assert_eq!(f.elapsed(), 600);
    assert_eq!(End::BudgetSpent.as_str(), "budget spent");
}

#[test]
fn an_empty_budget_ends_the_wait_at_once() {
    let mut f = Fake::new();
    assert_eq!(wait(&plan_in(0, Bound::Budget), &mut f), End::BudgetSpent);
    assert_eq!(f.elapsed(), 0);
}

#[test]
fn rotator_capacity_still_ends_a_budget_bound_wait_at_the_next_poll() {
    let mut f = Fake::new();
    f.capacity_from = Some(10);
    assert_eq!(
        wait(&plan_in(600, Bound::Budget), &mut f),
        End::RotatorCapacity
    );
    assert_eq!(f.elapsed(), 30);
}
