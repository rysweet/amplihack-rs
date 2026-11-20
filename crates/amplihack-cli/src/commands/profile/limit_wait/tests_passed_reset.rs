//! Issue #2857, part 1: the question the supervisor asks before it judges the
//! limit on a retry a wait earned, and what `after_limit` does with the wait
//! that answer allows.
//!
//! A limit on the retry whose stated reset the clock has already passed is
//! lifted into a capacity wait by `plan::upgrade_passed_reset`, and only for a
//! retry the ordinary one-wait-per-login rule would have refused. The
//! supervisor therefore needs that rule's answer *before* it calls
//! `after_limit`, and `after_limit` needs the same answer inside. They must be
//! one computation, so the supervisor reads it through
//! `Waiter::ordinarily_refused`, which `after_limit` also uses.
//!
//! Contract used here:
//! - `Waiter::ordinarily_refused(&self) -> bool`: whether a wait on the login
//!   the next limit would be judged against is refused by the ordinary rule,
//!   which is [`marks::Marks::refuses`] over that login and the step's
//!   `max_wait_secs`. False for a step with no settings. It reads state only;
//!   it records nothing.
//!
//! What stays as it is: `after_limit` still ends a re-entry for a *reset* wait
//! whose reset is behind the clock (`Plan::ends_at_once`, pinned in
//! `tests_room_gate.rs`), and still plans a *capacity* wait whatever reset it
//! carries. The upgrade happens upstream of both, so these tests need no new
//! parameter on `after_limit`.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_passed_reset;`.

use super::events::Met;
use super::fingerprint::Watch;
use super::plan::WaitFor;
use super::settings::WaitSettings;
use super::tests_fingerprint::{Env, env};
use super::tests_room_gate::{guarded, signals, waiter};
use super::{AfterLimit, Resumed, Waiter};
use crate::commands::recipe::run::limit_patterns::Harness;
use std::cell::Cell;

fn capacity_for(reset: Option<i64>) -> WaitFor {
    WaitFor::Capacity {
        harness: Harness::Claude,
        reset,
    }
}

/// The rotator's own record naming `login` live, with room.
fn live(e: &Env, login: &str) {
    e.rotator_cycle_room(login, 2, None, "swapped", false, Some(true));
}

/// One `after_limit` call for an attempt that was not resumed on a signal.
fn ordinarily(w: &mut Waiter, wait: WaitFor, probe: &dyn Fn(i64, i64) -> bool) -> AfterLimit {
    w.after_limit(
        wait,
        2,
        &signals(),
        probe,
        &mut || None,
        Resumed::Ordinarily,
    )
}

/// One `after_limit` call as the retry a wait that said *try now* earned.
fn after_a_signal(w: &mut Waiter, wait: WaitFor, probe: &dyn Fn(i64, i64) -> bool) -> AfterLimit {
    w.after_limit(
        wait,
        2,
        &signals(),
        probe,
        &mut || None,
        Resumed::AfterASignal(Met::UsageLimit),
    )
}

// --- the reader ------------------------------------------------------------------------

#[test]
fn a_step_that_has_not_waited_refuses_nothing() {
    guarded(|| {
        let e = env();
        live(&e, "a");
        // `max_wait_secs = 0` keeps the meaning it has always had: the first
        // wait of a step is never refused for the budget.
        for max in [None, Some(0), Some(600)] {
            let mut w = waiter(&e, max);
            w.note_attempt_login();
            assert!(
                !w.ordinarily_refused(),
                "max_wait_secs {max:?}: a step's first wait is never refused"
            );
        }
    });
}

#[test]
fn the_login_this_step_has_already_waited_on_is_refused() {
    guarded(|| {
        let e = env();
        live(&e, "a");
        let mut w = waiter(&e, None);
        w.note_attempt_login();
        w.marks.record(Some("a".to_string()));
        assert!(w.ordinarily_refused());
    });
}

#[test]
fn a_login_the_step_has_never_waited_on_is_not_refused_while_budget_remains() {
    guarded(|| {
        let e = env();
        live(&e, "a");
        let mut w = waiter(&e, Some(100));
        w.note_attempt_login();
        w.marks.record(Some("a".to_string()));
        w.marks.spend(99);
        // The rotator moved the step to `b` before the retry was launched.
        live(&e, "b");
        w.note_attempt_login();
        assert!(
            !w.ordinarily_refused(),
            "`b` is a login this step has not waited on, and one second is left"
        );
    });
}

#[test]
fn a_spent_budget_refuses_every_login() {
    guarded(|| {
        let e = env();
        live(&e, "a");
        let mut w = waiter(&e, Some(100));
        w.note_attempt_login();
        w.marks.record(Some("a".to_string()));
        w.marks.spend(100);
        live(&e, "b");
        w.note_attempt_login();
        assert!(
            w.ordinarily_refused(),
            "the budget is spent, so a later wait on any login is refused"
        );
    });
}

#[test]
fn a_waiter_with_no_settings_refuses_nothing() {
    guarded(|| {
        let e = env();
        live(&e, "a");
        let watch = Watch::new(Some(&e.store), None, None, Some(e.home.as_path()));
        let mut w = Waiter::new(
            None::<WaitSettings>,
            watch,
            super::claude_args::ClaudeArgs {
                print: true,
                prompt: true,
                stream_stdin: false,
            },
            Box::new(|| false),
            Box::new(|| true),
        );
        w.note_attempt_login();
        w.marks.record(Some("a".to_string()));
        assert!(
            !w.ordinarily_refused(),
            "the wait is not enabled, so there is no rule to have refused it"
        );
    });
}

#[test]
fn asking_records_nothing() {
    guarded(|| {
        let e = env();
        live(&e, "a");
        let mut w = waiter(&e, Some(100));
        w.note_attempt_login();
        for _ in 0..3 {
            assert!(!w.ordinarily_refused());
        }
        assert!(
            !w.used(),
            "reading the rule marks no login: only a wait that is planned does"
        );
    });
}

/// The reader and `after_limit` are one rule. For each state of a step, the
/// reader's answer is exactly whether `after_limit` ends an ordinary retry as
/// already waited, with the same capacity wait offered to both.
#[test]
fn the_reader_and_after_limit_judge_one_rule() {
    type Setup = Box<dyn Fn(&Env, &mut Waiter)>;
    guarded(|| {
        let e = env();
        let cases: Vec<(&str, Option<u64>, Setup)> = vec![
            ("a first wait", None, Box::new(|_: &Env, _: &mut Waiter| {})),
            (
                "the login already waited on",
                None,
                Box::new(|_: &Env, w: &mut Waiter| w.marks.record(Some("a".to_string()))),
            ),
            (
                "another login, budget left",
                Some(100),
                Box::new(|e: &Env, w: &mut Waiter| {
                    w.marks.record(Some("a".to_string()));
                    w.marks.spend(40);
                    live(e, "b");
                    w.note_attempt_login();
                }),
            ),
            (
                "another login, budget spent",
                Some(100),
                Box::new(|e: &Env, w: &mut Waiter| {
                    w.marks.record(Some("a".to_string()));
                    w.marks.spend(100);
                    live(e, "b");
                    w.note_attempt_login();
                }),
            ),
        ];
        let mut refused_somewhere = false;
        let mut allowed_somewhere = false;
        for (what, max, setup) in cases {
            live(&e, "a");
            let mut w = waiter(&e, max);
            w.note_attempt_login();
            setup(&e, &mut w);
            let refused = w.ordinarily_refused();
            // Another login's capacity ends the wait the moment it is read, so
            // a wait that is allowed to run is a `Retry` and a refusal is not.
            let got = ordinarily(&mut w, capacity_for(None), &|_, _| true);
            assert_eq!(
                matches!(got, AfterLimit::Finish(None)),
                refused,
                "{what}: the reader said refused={refused}"
            );
            assert_eq!(matches!(got, AfterLimit::Retry), !refused, "{what}");
            refused_somewhere |= refused;
            allowed_somewhere |= !refused;
        }
        assert!(
            refused_somewhere && allowed_somewhere,
            "the cases must exercise both answers"
        );
    });
}

// --- what after_limit does with the wait the upgrade allows ------------------------------

/// The upgrade hands `after_limit` a capacity wait that carries the stated,
/// passed reset. `Plan::ends_at_once` is true only for a reset wait, so the
/// re-entry's refusal for "nothing to wait on" does not apply, and the wait is
/// planned from capacity: it runs, and another login's room ends it.
#[test]
fn a_capacity_wait_that_carries_a_passed_reset_runs_on_a_login_already_waited_on() {
    guarded(|| {
        let e = env();
        live(&e, "a");
        let mut w = waiter(&e, Some(600));
        w.note_attempt_login();
        w.marks.record(Some("a".to_string()));
        assert!(
            w.ordinarily_refused(),
            "the precondition: a refused re-entry"
        );
        let probed = Cell::new(0);
        let wait = capacity_for(Some(w.now() - 120));
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

/// The same limit as a *reset* wait is what the upgrade replaces: the backstop
/// in `after_limit` still ends it, for a host where no capacity wait could
/// start. Pinned beside `tests_room_gate.rs`'s version so that the two shapes
/// are seen side by side.
#[test]
fn the_same_passed_reset_as_a_reset_wait_is_still_refused_as_already_waited() {
    guarded(|| {
        let e = env();
        live(&e, "a");
        let mut w = waiter(&e, Some(600));
        w.note_attempt_login();
        w.marks.record(Some("a".to_string()));
        let wait = WaitFor::Reset {
            harness: Harness::Claude,
            reset: w.now() - 120,
        };
        let probed = Cell::new(0);
        let got = after_a_signal(&mut w, wait, &|_, _| {
            probed.set(probed.get() + 1);
            true
        });
        assert!(matches!(got, AfterLimit::Finish(None)));
        assert_eq!(probed.get(), 0, "no wait ran");
    });
}

/// Why the budget bounds the chain. The capacity wait for a passed reset is not
/// a wait of no length: with no signal it lasts the whole budget the step has
/// left, on the wait's own clock, and the next re-entry is then refused. A
/// zero-cost wake cannot pass this: the clock is read, not the call count.
#[test]
fn a_capacity_wait_for_a_passed_reset_spends_the_budget_and_the_next_is_refused() {
    guarded(|| {
        let e = env();
        live(&e, "a");
        let budget = 90;
        let mut w = waiter(&e, Some(budget));
        w.note_attempt_login();
        w.marks.record(Some("a".to_string()));
        let before = w.now();
        let first = after_a_signal(&mut w, capacity_for(Some(before - 120)), &|_, _| false);
        assert!(
            matches!(first, AfterLimit::Finish(None)),
            "nothing lifted the limit, so the wait ran to its deadline and the step ends"
        );
        let spent = w.now() - before;
        assert_eq!(
            spent,
            i64::try_from(budget).unwrap(),
            "the wait lasted the budget the step had left, not nothing"
        );
        let wait = capacity_for(Some(w.now() - 120));
        let again = after_a_signal(&mut w, wait, &|_, _| true);
        assert!(
            matches!(again, AfterLimit::Finish(None)),
            "the budget is spent, so a further re-entry is refused"
        );
        assert_eq!(
            w.now() - before,
            spent,
            "and it waited no further: the clock did not move"
        );
    });
}
