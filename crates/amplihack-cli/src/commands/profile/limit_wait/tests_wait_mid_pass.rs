//! `wait` when the rotator's write lands part-way through one read pass
//! (issues #3124 and #3324), over the `Fake` `WaitDeps` of `tests_wait.rs`.
//!
//! The rotator writes its ledger to a temporary file and renames it over the
//! ledger. The wait watches the whole rotate folder, so the temporary file
//! wakes it, and the rename can land between two of the pass's readings of
//! the ledger. `Fake::capacity_from_read` models that: the rotator's
//! capacity reads false up to a given reading and true from it on, all at
//! one virtual second.
//!
//! A write that both swaps the login and records room after the wait's
//! start ends the pass `rotator capacity`, wherever its rename falls. A
//! login change with no room recorded after the start still ends the pass
//! `login changed`, and a login-change wait still reads no capacity.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_wait_mid_pass;`.

use super::plan::{Bound, Plan};
use super::tests_wait::{Fake, capacity_plan_in, login_change_plan, open_capacity_plan, plan_in};
use super::wait::{End, wait};

/// Every kind of wait that reads capacity: a reset wait, a capacity wait
/// with a deadline, and a capacity wait with none.
fn capacity_reading_plans() -> [Plan; 3] {
    [
        plan_in(1800, Bound::ResetMargin),
        capacity_plan_in(1800, Bound::ResetMargin),
        open_capacity_plan(),
    ]
}

/// The pass at second 30 reads no capacity first (reading 1; reading 0 was
/// the read as the wait began), then sees the fingerprint move. By then the
/// rename has landed, so the ledger records room after the start.
#[test]
fn a_rotator_write_seen_first_as_a_new_fingerprint_ends_the_wait_with_rotator_capacity() {
    for plan in capacity_reading_plans() {
        let mut f = Fake::new();
        f.prints = vec![(0, Some(1)), (30, Some(2))];
        f.capacity_from_read = Some(2);
        assert_eq!(wait(&plan, &mut f), End::RotatorCapacity, "{:?}", plan.kind);
        assert_eq!(f.elapsed(), 30, "{:?} ended at {}", plan.kind, f.elapsed());
    }
}

/// The same write seen through the named login key: the fingerprint does
/// not move, and the key does.
#[test]
fn a_rotator_write_seen_first_as_a_new_login_key_ends_the_wait_with_rotator_capacity() {
    for plan in capacity_reading_plans() {
        let mut f = Fake::new();
        f.login_changed_from = Some(30);
        f.capacity_from_read = Some(2);
        assert_eq!(wait(&plan, &mut f), End::RotatorCapacity, "{:?}", plan.kind);
        assert_eq!(f.elapsed(), 30, "{:?} ended at {}", plan.kind, f.elapsed());
    }
}

/// A login that moved to one with room, where the rotator recorded no room
/// after the wait's start, ends the wait `login changed`, as it did before.
#[test]
fn a_login_change_with_no_rotator_room_after_the_start_still_ends_login_changed() {
    for plan in capacity_reading_plans() {
        let mut f = Fake::new();
        f.prints = vec![(0, Some(1)), (30, Some(2))];
        assert_eq!(wait(&plan, &mut f), End::LoginChanged, "{:?}", plan.kind);
        assert_eq!(f.elapsed(), 30, "{:?} ended at {}", plan.kind, f.elapsed());

        let mut f = Fake::new();
        f.login_changed_from = Some(30);
        assert_eq!(wait(&plan, &mut f), End::LoginChanged, "{:?}", plan.kind);
        assert_eq!(f.elapsed(), 30, "{:?} ended at {}", plan.kind, f.elapsed());
    }
}

/// A login-change wait reads no capacity, so a ledger that records room
/// does not rename its end, and the rotator's cycle is never read.
#[test]
fn a_login_change_wait_still_ends_login_changed_and_reads_no_capacity() {
    let mut f = Fake::new();
    f.prints = vec![(0, Some(1)), (30, Some(2))];
    f.capacity_from_read = Some(0);
    assert_eq!(wait(&login_change_plan(), &mut f), End::LoginChanged);
    assert_eq!(f.elapsed(), 30);
    assert!(
        f.capacity_probes.is_empty(),
        "the rotator's cycle was read: {:?}",
        f.capacity_probes
    );
}
