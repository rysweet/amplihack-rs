//! `wait` (issue #1618), continued from `tests_wait.rs` (whose `Fake`
//! `WaitDeps` this uses): a stop signal, which end wins when several hold
//! at once, the end of the step's session, and fresh rotator capacity. Also
//! how a capacity wait ends (issue #1801), and how another login's
//! capacity ends a wait of any kind (issue #2694).
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_wait_ends;`.

use super::plan::Bound;
use super::tests_wait::{Fake, capacity_plan_in, login_change_plan_in, plan_in};
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
    f.other_from = Some(0);
    assert_eq!(
        wait(&login_change_plan_in(90), &mut f),
        End::MaxReached,
        "a reading of another login's windows says nothing about credit"
    );
    assert!(f.elapsed() >= 90, "ended early at {}", f.elapsed());
}

/// The deputy's point 4: the rotator starting a fresh cycle is not the
/// rotator swapping the login. A cycle that looks at the live login, finds
/// its session window has room and stays put records fresh capacity, and
/// that is the ordinary reading on a host whose credit has run out. Ending
/// the wait there retries on the same spend-limited login and ends the step.
#[test]
fn a_fresh_rotator_cycle_without_a_swap_does_not_end_a_login_change_wait() {
    let mut f = Fake::new();
    f.capacity_from = Some(0);
    assert_eq!(
        wait(&login_change_plan_in(90), &mut f),
        End::MaxReached,
        "only the login changing ends this wait"
    );
    assert!(f.elapsed() >= 90, "ended early at {}", f.elapsed());
}

#[test]
fn a_login_change_wait_still_ends_when_the_login_changes() {
    // The suppression is of the capacity sources alone: the fingerprint is
    // what this wait is for, and it still ends the wait at the second poll
    // that agrees.
    let mut f = Fake::new();
    f.prints = vec![(0, Some(1)), (45, Some(2))];
    f.capacity_from = Some(0);
    f.other_from = Some(0);
    assert_eq!(
        wait(&login_change_plan_in(1800), &mut f),
        End::LoginChanged,
        "the swap, and nothing else, ends it"
    );
    assert_eq!(f.elapsed(), 90);
}

/// The fingerprint's blind spot, stated so the fix below has something to
/// be a fix *for*. A swap landing before two probes have agreed is absorbed
/// into the baseline: the first reading and the stable one that follows
/// differ, which is also what a torn first read looks like, and the
/// fingerprint cannot tell the two apart. Left to the fingerprint alone this
/// wait sleeps to its deadline with a swapped-in login sitting there.
#[test]
fn the_fingerprint_alone_absorbs_a_swap_inside_the_first_poll_interval() {
    let mut f = Fake::new();
    f.prints = vec![(0, Some(1)), (10, Some(2))];
    assert_eq!(
        wait(&login_change_plan_in(300), &mut f),
        End::MaxReached,
        "the baseline settles on the post-swap value and never differs again"
    );
}

/// Issue #2731's own failure, shifted thirty seconds later: a login-change
/// wait suppresses both capacity sources, so the login key is what has to
/// catch a swap this early. Same fingerprint schedule as the test above,
/// and now the wait ends at the first poll.
#[test]
fn a_swap_inside_the_first_poll_interval_ends_a_login_change_wait() {
    let mut f = Fake::new();
    f.prints = vec![(0, Some(1)), (10, Some(2))];
    f.login_changed_from = Some(10);
    assert_eq!(
        wait(&login_change_plan_in(300), &mut f),
        End::LoginChanged,
        "the named login key is anchored before the attempt, so it has no \
         baseline to settle"
    );
    assert_eq!(f.elapsed(), 30, "at the first poll, not the second");
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

/// The key is read on the poll cadence and no faster: it reads two files.
#[test]
fn the_login_key_is_read_once_per_poll() {
    let mut f = Fake::new();
    wait(&plan_in(100, Bound::ResetMargin), &mut f);
    assert_eq!(f.login_probes, vec![30, 60, 90]);
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
        wait(&login_change_plan_in(1800), &mut f),
        End::Signal(libc::SIGTERM)
    );
    let mut f = Fake::new();
    f.parent_gone_from = Some(10);
    assert_eq!(wait(&login_change_plan_in(1800), &mut f), End::SessionEnded);
}

#[test]
fn a_login_change_wait_that_reaches_its_deadline_reads_as_max_reached() {
    // `mod.rs` retries on this (issue #2911), and `max reached` is the one
    // end that claims nothing about the login, so the mapping from the
    // deadline to the end reason must not move with the flag.
    let mut f = Fake::new();
    assert_eq!(wait(&login_change_plan_in(0), &mut f), End::MaxReached);
    assert_eq!(f.elapsed(), 0, "a zero-length wait never sleeps");
}

/// A login-change wait never asks either capacity source at all, rather
/// than asking and ignoring the answer: a wait that is not going to act on
/// a reading has no business taking one.
///
/// Both sources are named here. Pinning only the other-login one would let
/// a wait that still read the rotate ledger every 30 s pass, while the
/// module header above claims both are suppressed.
#[test]
fn a_login_change_wait_takes_no_capacity_reading() {
    let mut f = Fake::new();
    wait(&login_change_plan_in(100), &mut f);
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
/// being read at all: a reset wait reads both, on the poll cadence.
#[test]
fn a_reset_wait_reads_both_capacity_sources() {
    let mut f = Fake::new();
    wait(&plan_in(100, Bound::ResetMargin), &mut f);
    assert_eq!(f.capacity_probes, vec![30, 60, 90]);
    assert_eq!(f.other_probes, vec![30, 60, 90]);
}
