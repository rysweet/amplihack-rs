//! Issue #2857 as #2911 leaves it: a limit on the retry a wait earned sends
//! the step back into a wait, and never ends it.
//!
//! #2857 built that out of two pieces: `plan::upgrade_passed_reset`, which
//! lifted a limit whose stated reset the clock had passed into a capacity
//! wait, and `Waiter::ordinarily_refused`, the reader the supervisor used to
//! ask whether the ordinary one-wait-per-login rule would have refused it.
//! Both are gone. The rule they narrowed - a step waits at most once per
//! login, within one budget - ended runs that a limit should only have
//! delayed, so #2911 removed it, and with no refusal to lift there is
//! nothing left to narrow.
//!
//! What they produced survives, unconditionally. `wait_for_message` makes a
//! reset already behind the clock a capacity wait for every limit, not only
//! for a refused re-entry, so no limit is ever planned as a wait that ends
//! the moment it begins. The tests below pin that, and that `after_limit`
//! waits for each of a chain of limits.
//!
//! The tests this file used to hold for `ordinarily_refused` and the budget
//! are deleted rather than rewritten: their subject was the refusals
//! themselves.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_passed_reset;`.

use super::events::Met;
use super::plan::{WaitFor, Waiting, wait_for_message};
use super::scratch::Replay;
use super::tests_fingerprint::{Env, env};
use super::tests_room_gate::{guarded, readings_written, signals, waiter};
use super::{AfterLimit, Resumed, Waiter};
use crate::commands::recipe::run::limit_patterns::{self, Harness};
use std::cell::Cell;

const SESSION: &str = "You've hit your session limit · resets 1pm (America/Los_Angeles)";

fn capacity_for(reset: Option<i64>, why: Waiting) -> WaitFor {
    WaitFor::Capacity {
        harness: Harness::Claude,
        reset,
        why,
    }
}

/// The rotator's own record naming `login` live, with room.
fn live(e: &Env, login: &str) {
    e.rotator_cycle_room(login, 2, None, "swapped", false, Some(true));
}

/// One `after_limit` call as the retry a wait that said *try now* earned.
fn after_a_signal(w: &mut Waiter, wait: WaitFor, probe: &dyn Fn(i64, i64) -> bool) -> AfterLimit {
    w.after_limit(
        wait,
        2,
        &signals(),
        probe,
        &mut || Replay::Ready,
        Resumed::AfterASignal(Met::UsageLimit),
    )
}

// --- the planner: a passed reset is a capacity wait, always ----------------

/// 2026-10-18 13:00:00 -07:00, the reset the fixture message states.
const LA_1300: i64 = 1_792_353_600;

#[test]
fn a_reset_the_clock_has_passed_is_a_capacity_wait() {
    // 13:02 local: "1pm" is two minutes ago, inside the ten minutes a bare
    // clock time is still read as today's.
    let late = LA_1300 + 120;
    let message = limit_patterns::limit_message(SESSION.as_bytes(), b"", late);
    assert_eq!(
        wait_for_message(message, late),
        capacity_for(Some(LA_1300), Waiting::PassedReset),
        "the stated reset is kept, so the start line can print what was claimed"
    );
}

#[test]
fn the_boundary_is_the_clock_itself() {
    // A reset at `now` has passed: a reset wait for it would end at once.
    let message = limit_patterns::limit_message(SESSION.as_bytes(), b"", LA_1300);
    assert_eq!(
        wait_for_message(message, LA_1300),
        capacity_for(Some(LA_1300), Waiting::PassedReset)
    );
    // A second earlier it is still ahead, and an ordinary reset wait.
    let early = LA_1300 - 1;
    let message = limit_patterns::limit_message(SESSION.as_bytes(), b"", early);
    assert_eq!(
        wait_for_message(message, early),
        WaitFor::Reset {
            harness: Harness::Claude,
            reset: LA_1300,
        }
    );
}

/// Issue #2911: the lift is unconditional. It used to apply only to a retry
/// the one-wait-per-login rule would have refused, which meant the *first*
/// limit of a step whose reset had passed was planned as a reset wait with a
/// deadline of `now`. Nothing distinguishes the cases now, so neither does
/// the planner: there is no third argument it could read.
#[test]
fn issue_2911_the_lift_does_not_depend_on_how_the_attempt_came_to_run() {
    let late = LA_1300 + 120;
    let message = limit_patterns::limit_message(SESSION.as_bytes(), b"", late);
    // The same message, judged by the same function, at the same instant: it
    // can only give one answer, and that answer is a wait with real length.
    for _ in 0..2 {
        assert_eq!(
            wait_for_message(message, late),
            capacity_for(Some(LA_1300), Waiting::PassedReset)
        );
    }
}

// --- the supervisor: a limit after a wait waits again ----------------------

/// The capacity wait a passed reset earns runs, and another login's room ends
/// it. The login this step has already waited on is no obstacle: there is no
/// same-login refusal left for the wait to be lifted past.
#[test]
fn a_capacity_wait_for_a_passed_reset_runs_on_a_login_already_waited_on() {
    guarded(|| {
        let e = env();
        let _writes = readings_written(&e);
        live(&e, "a");
        let mut w = waiter(&e);
        w.note_attempt_login();
        w.marks.record(Some("a".to_string()));
        let probed = Cell::new(0);
        let wait = capacity_for(Some(w.now() - 120), Waiting::PassedReset);
        let got = after_a_signal(&mut w, wait, &|_, _| {
            probed.set(probed.get() + 1);
            true
        });
        assert!(
            matches!(got, AfterLimit::Retry),
            "the re-entry is waited out"
        );
        assert!(
            probed.get() >= 1,
            "the wait ran and was ended by another login's capacity"
        );
    });
}

/// Issue #2911: a capacity wait for a passed reset has no deadline, so no
/// clock runs the step again on the login whose limit it met. It is ended
/// by an event - here the rotator recording room after it began - and a
/// further re-entry is a further wait, not an ending. Before, a second
/// re-entry on the same login wrote `skipped reason=already_waited` and the
/// run finished on the limit it had.
#[test]
fn issue_2911_a_capacity_wait_for_a_passed_reset_waits_for_an_event_each_time() {
    guarded(|| {
        let e = env();
        live(&e, "a");
        let mut w = waiter(&e);
        w.note_attempt_login();
        w.marks.record(Some("a".to_string()));
        for n in 0..2 {
            let now = w.now();
            let wait = capacity_for(Some(now - 120), Waiting::PassedReset);
            // The rotator's next cycle, measured after the limit.
            e.rotator_cycle_room("a", now + 1, Some(now + 1), "stayed", false, Some(true));
            let got = after_a_signal(&mut w, wait, &|_, _| false);
            assert!(
                matches!(got, AfterLimit::Retry),
                "re-entry {n} is waited out to an event, not ended"
            );
            assert!(
                w.resumed_on_a_signal(),
                "re-entry {n}: the end was the rotator's room, which says try now"
            );
        }
    });
}

/// A reset still ahead is an ordinary reset wait, on a login the step has
/// waited on or not.
#[test]
fn a_reset_still_ahead_is_an_ordinary_reset_wait() {
    guarded(|| {
        let e = env();
        let _writes = readings_written(&e);
        live(&e, "a");
        let mut w = waiter(&e);
        w.note_attempt_login();
        w.marks.record(Some("a".to_string()));
        let probed = Cell::new(0);
        let wait = WaitFor::Reset {
            harness: Harness::Claude,
            reset: w.now() + 1800,
        };
        let got = after_a_signal(&mut w, wait, &|_, _| {
            probed.set(probed.get() + 1);
            true
        });
        assert!(matches!(got, AfterLimit::Retry));
        assert!(probed.get() >= 1, "the wait ran");
    });
}
