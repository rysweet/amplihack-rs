//! Issue #2731: how many times one step may wait, and for how long in total
//! ([`Marks`]).
//!
//! Before #2731 a step waited at most once. It may now wait again, but only
//! when the login it runs on has changed and only within the time its
//! earlier waits left unspent. Two rules carry that:
//!
//! - **every wait records a mark**, the login key of the attempt that
//!   failed, and a wait whose mark is already recorded is refused. A wait
//!   that ended before it began records its mark like any other. The marks
//!   are what bound the chain: the rotator can only move logins the store
//!   holds, so a repeat ends the step.
//! - `max_wait_secs` is a **per-step** budget. A second or later wait is
//!   refused once it is spent; the first wait of a step is never refused for
//!   it, so `max_wait_secs = 0` keeps its old meaning.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_marks;`.

use super::marks::Marks;

fn key(n: &str) -> Option<String> {
    Some(n.to_string())
}

/// No login name could be read: a step with no profile store, or a ledger
/// that could not be read at that moment.
const NONE: Option<String> = None;

// --- the first wait ------------------------------------------------------

#[test]
fn the_first_wait_of_a_step_is_never_refused() {
    for max in [None, Some(0), Some(1), Some(21_600)] {
        let m = Marks::new();
        assert!(!m.used(), "nothing waited yet");
        assert!(
            !m.refuses(&key("a"), max),
            "a first wait with max_wait_secs {max:?}"
        );
        assert!(
            !m.refuses(&NONE, max),
            "nor one on a step with no readable login name"
        );
    }
}

#[test]
fn a_zero_maximum_still_allows_the_first_wait_and_refuses_the_second() {
    let mut m = Marks::new();
    assert!(!m.refuses(&key("a"), Some(0)));
    m.record(key("a"));
    assert!(
        m.refuses(&key("b"), Some(0)),
        "max_wait_secs = 0 leaves no budget for a second wait, whatever the login"
    );
}

// --- one wait per login --------------------------------------------------

#[test]
fn a_login_already_waited_on_is_refused() {
    let mut m = Marks::new();
    m.record(key("a"));
    assert!(m.used());
    assert!(m.refuses(&key("a"), Some(21_600)));
}

#[test]
fn a_login_not_yet_waited_on_is_allowed() {
    let mut m = Marks::new();
    m.record(key("a"));
    assert!(!m.refuses(&key("b"), Some(21_600)));
}

#[test]
fn a_login_that_comes_round_again_ends_the_chain() {
    // The bound on a chain of waits: a rotator oscillating a -> b -> a finds
    // `a` already marked, so the step stops rather than waiting for ever.
    let mut m = Marks::new();
    m.record(key("a"));
    m.record(key("b"));
    assert!(m.refuses(&key("a"), Some(21_600)));
    assert!(m.refuses(&key("b"), Some(21_600)));
    assert!(!m.refuses(&key("c"), Some(21_600)));
}

#[test]
fn a_step_with_no_readable_login_name_waits_once() {
    // One `no login name` mark, recordable once: a step with no profile
    // store behaves exactly as it did before #2731.
    let mut m = Marks::new();
    assert!(!m.refuses(&NONE, Some(21_600)));
    m.record(NONE);
    assert!(m.refuses(&NONE, Some(21_600)));
}

#[test]
fn an_unreadable_name_neither_matches_nor_consumes_a_named_login() {
    let mut m = Marks::new();
    m.record(key("a"));
    assert!(
        !m.refuses(&NONE, Some(21_600)),
        "an absent name is not login a"
    );
    let mut m = Marks::new();
    m.record(NONE);
    assert!(
        !m.refuses(&key("a"), Some(21_600)),
        "and does not consume login a's wait"
    );
}

// --- the per-step budget -------------------------------------------------

#[test]
fn the_remaining_budget_is_the_maximum_less_what_this_step_has_spent() {
    let mut m = Marks::new();
    assert_eq!(m.remaining(Some(600)), Some(600));
    assert_eq!(m.remaining(None), None, "no maximum, no budget row");
    m.spend(100);
    assert_eq!(m.remaining(Some(600)), Some(500));
    assert_eq!(m.remaining(None), None);
    m.spend(500);
    assert_eq!(m.remaining(Some(600)), Some(0));
}

#[test]
fn spending_more_than_the_maximum_saturates_at_zero() {
    let mut m = Marks::new();
    m.spend(u64::MAX);
    m.spend(u64::MAX);
    assert_eq!(m.remaining(Some(600)), Some(0));
    assert_eq!(m.remaining(None), None);
}

#[test]
fn a_later_wait_is_refused_once_the_budget_is_spent() {
    let mut m = Marks::new();
    m.record(key("a"));
    m.spend(600);
    assert!(
        m.refuses(&key("b"), Some(600)),
        "a login this step has not waited on, but no time left to wait"
    );
    assert!(
        !m.refuses(&key("b"), Some(601)),
        "one second left is still a wait"
    );
    assert!(
        !m.refuses(&key("b"), None),
        "with no maximum set there is no budget to spend"
    );
}

#[test]
fn a_wait_that_ended_before_it_began_spends_a_mark_but_no_time() {
    // The short-circuit on a `$HOME/.claude` step whose login had already
    // changed: no time passes, so the next login keeps the whole budget.
    let mut m = Marks::new();
    m.record(key("a"));
    assert_eq!(m.remaining(Some(21_600)), Some(21_600));
    assert!(m.refuses(&key("a"), Some(21_600)));
    assert!(!m.refuses(&key("b"), Some(21_600)));
}

#[test]
fn the_budget_is_shared_by_every_wait_the_step_makes() {
    // Two waits across two logins last no longer in total than the one wait
    // the same step would have made before.
    let mut m = Marks::new();
    m.record(key("a"));
    m.spend(400);
    assert_eq!(m.remaining(Some(600)), Some(200));
    assert!(!m.refuses(&key("b"), Some(600)));
    m.record(key("b"));
    m.spend(200);
    assert_eq!(m.remaining(Some(600)), Some(0));
    assert!(m.refuses(&key("c"), Some(600)));
}

// --- what `used` still means ---------------------------------------------

#[test]
fn used_is_whether_this_step_has_waited_at_all() {
    // `auto_switch::rerun::next_profile` reads it to stop choosing another
    // candidate once the step has waited; #2731 must not change that.
    let mut m = Marks::new();
    assert!(!m.used());
    m.record(NONE);
    assert!(m.used());
    let mut m = Marks::new();
    m.record(key("a"));
    assert!(m.used());
}
