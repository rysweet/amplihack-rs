//! `wait` (issue #1618), continued from `tests_wait.rs` (whose `Fake`
//! `WaitDeps` this uses): a stop signal, which end wins when several hold
//! at once, the end of the step's session, and fresh rotator capacity. Also
//! how a capacity wait ends (issue #1801), and how another login's
//! capacity ends a wait of any kind (issue #2694). Every end is seen at the
//! moment of the write or signal that causes it (issue #2911): there is no
//! poll for it to wait for.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_wait_ends;`.

use super::plan::Bound;
use super::tests_wait::{Fake, capacity_plan_in, login_change_plan, open_capacity_plan, plan_in};
use super::wait::{End, wait};

#[test]
fn a_signal_ends_the_wait_when_it_arrives() {
    for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        let mut f = Fake::new();
        f.signal_from = Some((10, sig));
        assert_eq!(
            wait(&plan_in(1800, Bound::ResetMargin), &mut f),
            End::Signal(sig)
        );
        assert_eq!(f.elapsed(), 10, "noticed at {}", f.elapsed());
        assert_eq!(f.blocks.last(), Some(&(0, 10)), "{:?}", f.blocks);
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
    // A write and a signal at the same moment: the signal is read first.
    let mut f = Fake::new();
    f.signal_from = Some((30, libc::SIGINT));
    f.prints = vec![(0, Some(1)), (30, Some(2))];
    assert_eq!(
        wait(&plan_in(1800, Bound::ResetMargin), &mut f),
        End::Signal(libc::SIGINT)
    );
}

#[test]
fn a_passed_deadline_wins_over_a_changed_login() {
    // A write at the deadline: the deadline is read first.
    let mut f = Fake::new();
    f.prints = vec![(0, Some(1)), (60, Some(2))];
    assert_eq!(
        wait(&plan_in(60, Bound::ResetMargin), &mut f),
        End::ResetReached
    );
}

#[test]
fn the_end_of_the_steps_session_ends_the_wait_when_it_happens() {
    let mut f = Fake::new();
    f.parent_gone_from = Some(10);
    assert_eq!(
        wait(&plan_in(5 * 3600, Bound::ResetMargin), &mut f),
        End::SessionEnded
    );
    assert_eq!(f.elapsed(), 10, "noticed at {}", f.elapsed());
}

#[test]
fn fresh_rotator_capacity_ends_the_wait_at_the_rotators_write() {
    let mut f = Fake::new();
    f.capacity_from = Some(40);
    assert_eq!(
        wait(&plan_in(1800, Bound::ResetMargin), &mut f),
        End::RotatorCapacity
    );
    assert_eq!(f.elapsed(), 40);
}

// --- capacity waits (issue #1801) ----------------------------------------

#[test]
fn a_capacity_wait_that_reaches_the_failed_logins_reset_retries() {
    let mut f = Fake::new();
    assert_eq!(
        wait(&capacity_plan_in(90, Bound::ResetMargin), &mut f),
        End::ResetReached
    );
}

/// Issue #2911: a capacity wait whose limit stated no usable reset has no
/// deadline, so with nothing to end it, it is still waiting long after the
/// six hours the old outer bound gave it, and nothing ran the step again.
#[test]
fn a_capacity_wait_with_no_reset_is_still_waiting_after_any_length_of_time() {
    let mut f = Fake::new();
    f.signal_from = Some((3 * 86_400, libc::SIGTERM));
    assert_eq!(
        wait(&open_capacity_plan(), &mut f),
        End::Signal(libc::SIGTERM),
        "only the signal, three days in, ended it"
    );
    assert_eq!(f.elapsed(), 3 * 86_400);
}

#[test]
fn another_login_with_capacity_ends_the_wait_at_the_readings_write() {
    let mut f = Fake::new();
    f.other_from = Some(40);
    assert_eq!(wait(&open_capacity_plan(), &mut f), End::OtherCapacity);
    assert_eq!(f.elapsed(), 40);
}

#[test]
fn rotator_capacity_is_checked_before_another_logins_capacity() {
    let mut f = Fake::new();
    f.capacity_from = Some(40);
    f.other_from = Some(40);
    assert_eq!(wait(&open_capacity_plan(), &mut f), End::RotatorCapacity);
}

#[test]
fn a_signal_and_a_passed_deadline_outrank_other_capacity() {
    let mut f = Fake::new();
    f.signal_from = Some((30, libc::SIGTERM));
    f.other_from = Some(30);
    assert_eq!(
        wait(&open_capacity_plan(), &mut f),
        End::Signal(libc::SIGTERM)
    );
    let mut f = Fake::new();
    f.other_from = Some(30);
    assert_eq!(
        wait(&capacity_plan_in(30, Bound::ResetMargin), &mut f),
        End::ResetReached,
        "the deadline is read before the files"
    );
}

/// The files are read once as the wait begins, so a record already there
/// is judged then, and not again until a write.
#[test]
fn a_record_already_there_is_read_once_as_the_wait_begins() {
    let mut f = Fake::new();
    f.other_from = Some(0);
    assert_eq!(wait(&open_capacity_plan(), &mut f), End::OtherCapacity);
    assert_eq!(f.elapsed(), 0);
    assert_eq!(f.other_probes, vec![0]);
}

// --- another login's capacity ends any wait (issue #2694) -----------------

/// The issue's own reproduction: a step under a stored profile, a stated
/// reset 3 h 45 min away, so the plan is a reset wait, and a reading written
/// 40 s in showing another login has room. The wait ends at that write
/// instead of sleeping to the reset.
#[test]
fn another_logins_capacity_also_ends_a_reset_wait() {
    let mut f = Fake::new();
    f.other_from = Some(40);
    assert_eq!(
        wait(&plan_in(13_500, Bound::ResetMargin), &mut f),
        End::OtherCapacity,
        "a near reset must not keep the step on an exhausted login"
    );
    assert_eq!(f.elapsed(), 40, "at the reading's write");
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

/// The same deadline bounds the wait whatever its end, so a reset reached
/// first still ends the wait there.
#[test]
fn a_reset_wait_reaches_its_reset_before_a_later_reading() {
    let mut f = Fake::new();
    f.other_from = Some(120);
    assert_eq!(
        wait(&plan_in(90, Bound::ResetMargin), &mut f),
        End::ResetReached
    );
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
    f.other_from = Some(30);
    assert_eq!(
        wait(&plan_in(13_500, Bound::ResetMargin), &mut f),
        End::Signal(libc::SIGTERM)
    );
    let mut f = Fake::new();
    f.other_from = Some(30);
    assert_eq!(
        wait(&plan_in(30, Bound::ResetMargin), &mut f),
        End::ResetReached,
        "the deadline is read before the files"
    );
}

/// Within one wake the login fingerprint is still read first: a login that
/// changed under the step ends the wait before any reading does.
#[test]
fn a_changed_login_outranks_another_logins_capacity_on_a_reset_wait() {
    let mut f = Fake::new();
    f.prints = vec![(0, Some(1)), (45, Some(2))];
    f.other_from = Some(45);
    assert_eq!(
        wait(&plan_in(13_500, Bound::ResetMargin), &mut f),
        End::LoginChanged
    );
    assert_eq!(f.elapsed(), 45);
}

/// The reading rides the login check the wait already makes after a write
/// and happens nowhere else, on a reset wait as on a capacity wait: issue
/// #2694 widens which waits consult it and adds no timer, interval or retry
/// of its own. The first check is the baseline taken before the loop and no
/// reading accompanies it, so the readings are exactly the tail of the
/// checks.
#[test]
fn another_logins_capacity_is_read_on_the_existing_login_check() {
    for plan in [plan_in(100, Bound::ResetMargin), open_capacity_plan()] {
        let mut f = Fake::new();
        f.writes = vec![20, 50, 80];
        f.parent_gone_from = Some(100);
        wait(&plan, &mut f);
        assert_eq!(f.probes, vec![0, 0, 20, 50, 80], "{:?}", plan.kind);
        assert_eq!(
            f.other_probes,
            f.probes[1..],
            "{:?}: every reading falls on a login check",
            plan.kind
        );
    }
}

/// With no write, the reading is taken once, as the wait begins.
#[test]
fn a_wait_with_no_write_reads_another_logins_capacity_once() {
    let mut f = Fake::new();
    assert_eq!(
        wait(&plan_in(200, Bound::ResetMargin), &mut f),
        End::ResetReached
    );
    assert_eq!(f.other_probes, vec![0]);
}

#[test]
fn the_end_of_the_steps_session_outranks_other_capacity_on_a_reset_wait() {
    let mut f = Fake::new();
    f.parent_gone_from = Some(30);
    f.other_from = Some(30);
    assert_eq!(
        wait(&plan_in(13_500, Bound::ResetMargin), &mut f),
        End::SessionEnded,
        "nobody is left to use the retry's result"
    );
}

// --- which waits a capacity reading may end (issues #2694, #2717, #2731) ---
//
// #2717 made another login's room end a wait of any kind, not only a
// capacity wait: a step whose own reset is hours away should not sleep to it
// while a login with room is sitting there. That rule is pinned by the cases
// above. A login-change wait is the one exception, and it is an exception
// about meaning rather than about timing. It is waiting for an organisation
// spend limit to be replaced by another organisation's credit, and no
// capacity reading reports credit: an exhausted credit leaves the session
// and weekly windows looking perfectly healthy, which is exactly when a
// reading would end the wait and send the step back onto the same spent
// login. So both sources are suppressed, not merely the no-swap case the
// field report named.

#[test]
fn a_login_change_wait_ignores_another_logins_capacity() {
    let mut f = Fake::new();
    f.other_from = Some(10);
    f.parent_gone_from = Some(90);
    assert_eq!(
        wait(&login_change_plan(), &mut f),
        End::SessionEnded,
        "a reading of another login's windows says nothing about credit"
    );
    assert_eq!(f.elapsed(), 90, "ended early at {}", f.elapsed());
}

/// The deputy's point 4: the rotator starting a fresh cycle is not the
/// rotator swapping the login. A cycle that looks at the live login, finds
/// its session window has room and stays put records fresh capacity, and
/// that is the ordinary reading on a host whose credit has run out. Ending
/// the wait there retries on the same spend-limited login and ends the step.
#[test]
fn a_fresh_rotator_cycle_without_a_swap_does_not_end_a_login_change_wait() {
    let mut f = Fake::new();
    f.capacity_from = Some(10);
    f.parent_gone_from = Some(90);
    assert_eq!(
        wait(&login_change_plan(), &mut f),
        End::SessionEnded,
        "only the login changing ends this wait"
    );
    assert_eq!(f.elapsed(), 90, "ended early at {}", f.elapsed());
}

#[test]
fn a_login_change_wait_still_ends_when_the_login_changes() {
    // The suppression is of the capacity sources alone: the fingerprint is
    // what this wait is for, and it still ends the wait at the write that
    // changed it.
    let mut f = Fake::new();
    f.prints = vec![(0, Some(1)), (45, Some(2))];
    f.capacity_from = Some(20);
    f.other_from = Some(20);
    assert_eq!(
        wait(&login_change_plan(), &mut f),
        End::LoginChanged,
        "the swap, and nothing else, ends it"
    );
    assert_eq!(f.elapsed(), 45);
}

/// A swap moments after the wait began is seen at its write by the
/// fingerprint: the baseline was read before it, and there is no settling
/// interval for it to land inside.
#[test]
fn a_swap_just_after_the_wait_began_is_seen_at_its_write() {
    let mut f = Fake::new();
    f.prints = vec![(0, Some(1)), (10, Some(2))];
    assert_eq!(wait(&login_change_plan(), &mut f), End::LoginChanged);
    assert_eq!(f.elapsed(), 10);
}

/// The named login key is anchored before the attempt was launched, so it
/// catches a swap the fingerprint's baseline absorbed: one that landed
/// before the wait read its baseline (issue #2731). A login-change wait
/// suppresses both capacity sources, so the key is what ends it.
#[test]
fn the_login_key_catches_a_swap_the_baseline_absorbed() {
    let mut f = Fake::new();
    f.login_changed_from = Some(0);
    assert_eq!(
        wait(&login_change_plan(), &mut f),
        End::LoginChanged,
        "the named login key has no baseline to settle"
    );
    assert_eq!(f.elapsed(), 0, "at the first read");
}

/// The login key renames no end that already has a name. One rotator
/// write both swaps the login and records fresh capacity, so on a reset or
/// capacity wait the two fire on the same poll; `rotator capacity` is the
/// end such a wait has always reported, and auto-drive reads these reasons.
/// Checking the key before the capacity sources would relabel it.
#[test]
fn a_swap_that_also_reports_capacity_still_ends_as_rotator_capacity() {
    let mut f = Fake::new();
    f.capacity_from = Some(30);
    f.login_changed_from = Some(30);
    assert_eq!(
        wait(&plan_in(1800, Bound::ResetMargin), &mut f),
        End::RotatorCapacity,
        "the capacity sources keep first refusal"
    );
}

/// One rotator write that swaps the login changes the fingerprint and
/// records the cycle's room together. The wait reports it as the rotator's
/// capacity, as it always has: that source is read before the fingerprint.
#[test]
fn a_swapping_rotator_write_that_changes_the_fingerprint_ends_as_rotator_capacity() {
    let mut f = Fake::new();
    f.prints = vec![(0, Some(1)), (30, Some(2))];
    f.capacity_from = Some(30);
    f.login_changed_from = Some(30);
    assert_eq!(
        wait(&plan_in(1800, Bound::ResetMargin), &mut f),
        End::RotatorCapacity
    );
    assert_eq!(f.elapsed(), 30);
}

/// The same for another login's capacity, which is checked after the
/// rotator's and still before the key.
#[test]
fn a_swap_that_also_shows_another_login_with_room_still_ends_as_login_capacity() {
    let mut f = Fake::new();
    f.other_from = Some(30);
    f.login_changed_from = Some(30);
    assert_eq!(
        wait(&plan_in(1800, Bound::ResetMargin), &mut f),
        End::OtherCapacity
    );
}

/// The key is read once per write and at no other time: it reads two files.
#[test]
fn the_login_key_is_read_once_per_write() {
    let mut f = Fake::new();
    f.writes = vec![30, 60, 90];
    wait(&plan_in(100, Bound::ResetMargin), &mut f);
    assert_eq!(f.login_probes, vec![0, 30, 60, 90]);
}

/// A step whose login the rotator never changes reports no change, so the
/// key adds no exit to the waits that are not about a swap.
#[test]
fn a_login_key_that_never_changes_ends_nothing() {
    let mut f = Fake::new();
    assert_eq!(
        wait(&plan_in(100, Bound::ResetMargin), &mut f),
        End::ResetReached
    );
}

#[test]
fn a_login_change_wait_still_ends_on_a_signal_and_on_the_sessions_end() {
    let mut f = Fake::new();
    f.signal_from = Some((10, libc::SIGTERM));
    assert_eq!(
        wait(&login_change_plan(), &mut f),
        End::Signal(libc::SIGTERM)
    );
    let mut f = Fake::new();
    f.parent_gone_from = Some(10);
    assert_eq!(wait(&login_change_plan(), &mut f), End::SessionEnded);
}

/// Issue #2911: a login-change wait has no deadline, so with no swap it is
/// still waiting a week on, and nothing ran the step again on the spent
/// login in the meantime.
#[test]
fn a_login_change_wait_has_no_deadline_to_reach() {
    let mut f = Fake::new();
    f.parent_gone_from = Some(7 * 86_400);
    assert_eq!(wait(&login_change_plan(), &mut f), End::SessionEnded);
    assert_eq!(f.elapsed(), 7 * 86_400);
}

/// A login-change wait never asks either capacity source at all, rather
/// than asking and ignoring the answer: a wait that is not going to act on
/// a reading has no business taking one.
///
/// Both sources are named here. Pinning only the other-login one would let
/// a wait that still read the rotate ledger on every write pass, while the
/// module header above claims both are suppressed.
#[test]
fn a_login_change_wait_takes_no_capacity_reading() {
    let mut f = Fake::new();
    f.writes = vec![30, 60, 90];
    f.parent_gone_from = Some(100);
    wait(&login_change_plan(), &mut f);
    assert!(
        f.other_probes.is_empty(),
        "another login's capacity was read: {:?}",
        f.other_probes
    );
    assert!(
        f.capacity_probes.is_empty(),
        "the rotator's cycle was read: {:?}",
        f.capacity_probes
    );
}

/// The counterpart, so the test above cannot pass by the sources never
/// being read at all: a reset wait reads both, at each write.
#[test]
fn a_reset_wait_reads_both_capacity_sources() {
    let mut f = Fake::new();
    f.writes = vec![30, 60, 90];
    wait(&plan_in(100, Bound::ResetMargin), &mut f);
    assert_eq!(f.capacity_probes, vec![0, 30, 60, 90]);
    assert_eq!(f.other_probes, vec![0, 30, 60, 90]);
}
