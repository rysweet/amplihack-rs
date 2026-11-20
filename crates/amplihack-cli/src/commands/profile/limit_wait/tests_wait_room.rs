//! `wait` and the room gate (issue #2857), over the `Fake` `WaitDeps` of
//! `tests_wait.rs`.
//!
//! A wait may end early on three kinds of change to the rotator's state. The
//! rotator's own fresh capacity is already a reading of room (the real
//! `rotator_capacity` is `fresh_room`), and another login's capacity is
//! unchanged (the known residual). The other two ends are *changes*: the login
//! fingerprint differing, and the named login key differing. They say the login
//! moved, not that the login it moved to can be used, so each is held back
//! until `WaitDeps::change_may_end` says the login now live has room.
//!
//! The contract these tests pin, and the real `WaitDeps` is held to in
//! `tests_room_gate.rs`:
//!
//! - `WaitDeps::change_may_end(&mut self) -> bool` is a required method with no
//!   default body: an implementor that forgot it would fail open, so it does
//!   not compile.
//! - it is asked only after a change has been seen, last in the conjunction,
//!   so a wait that sees no change makes no extra reading;
//! - a change that is held back is not forgotten: the wait keeps polling and
//!   ends on the change at the first poll where the gate opens, so a gate that
//!   closed for good on the first change would pass every "does not end" test
//!   and fail the "ends when room appears" ones;
//! - nothing else moves: the signal, the deadline, the session's end, the
//!   rotator's capacity and another login's capacity keep their order and
//!   their meaning, and `RotatorCapacity` stays ahead of `LoginChanged`.
//! - `End::said_try_now` splits the ends into those that claim the retry
//!   should work and those that merely ran out (part 1).
//!
//! `wait` has no opinion about login-change waits (#2731): it asks
//! `change_may_end` for every plan, and the exemption is the real `WaitDeps`'
//! to make from the plan, which `tests_room_gate.rs` pins. The pre-existing
//! `a_login_change_wait_still_ends_when_the_login_changes` in
//! `tests_wait_ends.rs` runs that path through the new conjunction.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_wait_room;`.

use super::plan::Bound;
use super::tests_wait::{Fake, capacity_plan_in, plan_in};
use super::wait::{End, wait};

// --- the fingerprint route -------------------------------------------------------

#[test]
fn a_fingerprint_change_onto_a_login_without_room_does_not_end_the_wait() {
    let mut f = Fake::new();
    f.prints = vec![(0, Some(1)), (45, Some(2))];
    f.change_room_from = None;
    assert_eq!(
        wait(&plan_in(300, Bound::ResetMargin), &mut f),
        End::ResetReached,
        "the login moved to one with no room: nothing to wake for"
    );
    assert!(f.elapsed() >= 300, "ended early at {}", f.elapsed());
}

/// The positive control: the same fingerprint schedule with room ends the wait
/// at the second poll that agrees, as it always did.
#[test]
fn the_same_fingerprint_change_onto_a_login_with_room_still_ends_the_wait() {
    let mut f = Fake::new();
    f.prints = vec![(0, Some(1)), (45, Some(2))];
    f.change_room_from = Some(0);
    assert_eq!(
        wait(&plan_in(300, Bound::ResetMargin), &mut f),
        End::LoginChanged
    );
    assert_eq!(f.elapsed(), 90, "the poll two agreeing probes in");
}

#[test]
fn a_fingerprint_change_ends_the_wait_at_the_first_poll_where_room_appears() {
    let mut f = Fake::new();
    f.prints = vec![(0, Some(1)), (45, Some(2))];
    f.change_room_from = Some(200);
    assert_eq!(
        wait(&plan_in(3000, Bound::ResetMargin), &mut f),
        End::LoginChanged
    );
    assert_eq!(
        f.elapsed(),
        210,
        "the change was seen at 90 and kept; the room reading at 210 is the \
         first poll at or after 200"
    );
}

// --- the login-key route ---------------------------------------------------------

#[test]
fn a_login_key_change_onto_a_login_without_room_does_not_end_the_wait() {
    let mut f = Fake::new();
    f.login_changed_from = Some(10);
    f.change_room_from = None;
    assert_eq!(
        wait(&plan_in(300, Bound::ResetMargin), &mut f),
        End::ResetReached
    );
    assert!(f.elapsed() >= 300, "ended early at {}", f.elapsed());
}

#[test]
fn the_same_login_key_change_onto_a_login_with_room_still_ends_the_wait() {
    let mut f = Fake::new();
    f.login_changed_from = Some(10);
    f.change_room_from = Some(0);
    assert_eq!(
        wait(&plan_in(300, Bound::ResetMargin), &mut f),
        End::LoginChanged
    );
    assert_eq!(f.elapsed(), 30, "at the first poll");
}

#[test]
fn a_login_key_change_ends_the_wait_at_the_first_poll_where_room_appears() {
    let mut f = Fake::new();
    f.login_changed_from = Some(10);
    f.change_room_from = Some(100);
    assert_eq!(
        wait(&plan_in(3000, Bound::ResetMargin), &mut f),
        End::LoginChanged
    );
    assert_eq!(f.elapsed(), 120, "the first poll at or after 100");
}

#[test]
fn the_gate_holds_a_capacity_wait_back_the_same_way() {
    let mut f = Fake::new();
    f.login_changed_from = Some(10);
    f.change_room_from = None;
    assert_eq!(
        wait(&capacity_plan_in(300, Bound::MaxWait), &mut f),
        End::CapacityExpired,
        "a capacity wait ends on room or on its own deadline, not on a login that moved"
    );
}

// --- the gate is asked only when there is a change to ask about -------------------

#[test]
fn no_change_means_no_question_about_room() {
    let mut f = Fake::new();
    f.change_room_from = None;
    assert_eq!(
        wait(&plan_in(100, Bound::ResetMargin), &mut f),
        End::ResetReached
    );
    assert_eq!(
        f.change_probes,
        Vec::<i64>::new(),
        "a wait that sees no change reads nothing extra"
    );
}

#[test]
fn the_question_is_asked_at_the_poll_that_sees_the_change_and_at_each_poll_after() {
    let mut f = Fake::new();
    f.prints = vec![(0, Some(1)), (45, Some(2))];
    f.change_room_from = None;
    wait(&plan_in(150, Bound::ResetMargin), &mut f);
    assert_eq!(
        f.change_probes,
        vec![90, 120],
        "the fingerprint needs two agreeing probes (60, 90) to call it a change; \
         the question is asked there and at each later poll, on the poll cadence"
    );
    let mut f = Fake::new();
    f.login_changed_from = Some(10);
    f.change_room_from = None;
    wait(&plan_in(100, Bound::ResetMargin), &mut f);
    assert_eq!(
        f.change_probes,
        vec![30, 60, 90],
        "once per poll, no faster"
    );
}

// --- what the gate does not touch ---------------------------------------------------

/// The rotator's capacity is already a reading of room (`fresh_room`), so it is
/// not asked again, and a change the gate would hold back does not hold it
/// back. The change is armed and the gate shut, so a capacity source that had
/// lost its place to the change, or that read the gate itself, would fail here.
#[test]
fn rotator_capacity_ends_the_wait_whatever_the_change_gate_says() {
    let mut f = Fake::new();
    f.capacity_from = Some(30);
    f.login_changed_from = Some(10);
    f.change_room_from = None;
    assert_eq!(
        wait(&plan_in(300, Bound::ResetMargin), &mut f),
        End::RotatorCapacity
    );
    assert_eq!(
        f.change_probes,
        Vec::<i64>::new(),
        "the capacity sources are read first, so the gate is never asked"
    );
}

/// The known residual, stated so it is a decision and not an oversight: another
/// login's capacity is read at the 100% rule and is not gated by this change.
/// As above, the change is armed and the gate shut.
#[test]
fn another_logins_capacity_is_not_held_back_by_the_room_gate() {
    let mut f = Fake::new();
    f.other_from = Some(30);
    f.login_changed_from = Some(10);
    f.change_room_from = None;
    assert_eq!(
        wait(&plan_in(300, Bound::ResetMargin), &mut f),
        End::OtherCapacity
    );
    assert_eq!(f.change_probes, Vec::<i64>::new());
}

#[test]
fn a_held_back_change_never_holds_back_a_signal_the_deadline_or_the_sessions_end() {
    let mut f = Fake::new();
    f.login_changed_from = Some(0);
    f.change_room_from = None;
    f.signal_from = Some((40, libc::SIGTERM));
    assert_eq!(
        wait(&plan_in(3000, Bound::ResetMargin), &mut f),
        End::Signal(libc::SIGTERM)
    );
    let mut f = Fake::new();
    f.login_changed_from = Some(0);
    f.change_room_from = None;
    f.parent_gone_from = Some(40);
    assert_eq!(
        wait(&plan_in(3000, Bound::ResetMargin), &mut f),
        End::SessionEnded
    );
    let mut f = Fake::new();
    f.login_changed_from = Some(0);
    f.change_room_from = None;
    assert_eq!(wait(&plan_in(75, Bound::MaxWait), &mut f), End::MaxReached);
}

// --- what an end claims -----------------------------------------------------------------

#[test]
fn an_end_that_said_try_now_is_one_a_limit_can_contradict() {
    for end in [
        End::ResetReached,
        End::RotatorCapacity,
        End::LoginChanged,
        End::OtherCapacity,
    ] {
        assert!(end.said_try_now(), "{}", end.as_str());
    }
}

#[test]
fn an_end_that_merely_ran_out_said_nothing() {
    for end in [
        End::MaxReached,
        End::CapacityExpired,
        End::StepTimeout,
        End::SessionEnded,
        End::Signal(libc::SIGTERM),
    ] {
        assert!(
            !end.said_try_now(),
            "{}: nothing said to try, so a limit after it is no contradiction",
            end.as_str()
        );
    }
}
