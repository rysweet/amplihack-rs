//! What one step's record of its waits still says (issues #2731, #2911).
//!
//! [`Marks`] used to bound a chain of waits with two refusals: a login the
//! step had already waited on, and a spent `max_wait_secs` budget. Both
//! ended runs that a limit should only have delayed, and both are gone
//! (#2911). The tests that proved them are gone with them; what bounds the
//! chain now is that every wait costs time, which is `plan`'s invariant and
//! is tested there and in `tests_wait_ends.rs`.
//!
//! Two readers are left, and this file is their contract:
//!
//! - [`Marks::used`], which `auto_switch::rerun` reads to stop choosing
//!   another auto-switch candidate once the step has waited;
//! - [`Marks::waited_on`], which the `reentered` line reads to tell a
//!   second wait on the same login from a first wait on a new one.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_marks;`.

use super::marks::Marks;

fn key(n: &str) -> Option<String> {
    Some(n.to_string())
}

/// No login name could be read: a step with no profile store, or a ledger
/// that could not be read at that moment.
const NONE: Option<String> = None;

// --- `used`: has this step waited at all? --------------------------------

#[test]
fn used_is_whether_this_step_has_waited_at_all() {
    // `auto_switch::rerun::next_profile` reads it to stop choosing another
    // candidate once the step has waited; neither #2731 nor #2911 changed
    // that.
    let mut m = Marks::new();
    assert!(!m.used());
    m.record(NONE);
    assert!(m.used());
    let mut m = Marks::new();
    m.record(key("a"));
    assert!(m.used());
}

// --- `waited_on`: has this step waited on *this* login? ------------------

#[test]
fn waited_on_reports_a_login_this_step_has_waited_on() {
    let mut m = Marks::new();
    assert!(!m.waited_on(&key("a")), "nothing waited on yet");
    m.record(key("a"));
    assert!(m.waited_on(&key("a")));
    assert!(!m.waited_on(&key("b")), "a login not waited on");
}

#[test]
fn an_unreadable_name_is_its_own_mark_and_not_any_login() {
    let mut m = Marks::new();
    m.record(key("a"));
    assert!(!m.waited_on(&NONE), "an absent name is not login a");
    let mut m = Marks::new();
    m.record(NONE);
    assert!(!m.waited_on(&key("a")), "nor is login a an absent name");
    assert!(m.waited_on(&NONE));
}

#[test]
fn every_mark_is_kept_so_a_login_that_comes_round_again_is_recognised() {
    // A rotator oscillating a -> b -> a is the case the record exists for:
    // the step waits on `a` a second time, and the `reentered` line can say
    // so, because the first mark was never dropped.
    let mut m = Marks::new();
    m.record(key("a"));
    m.record(key("b"));
    assert!(m.waited_on(&key("a")));
    assert!(m.waited_on(&key("b")));
    assert!(!m.waited_on(&key("c")));
}

// --- issue #2911: the record refuses nothing ------------------------------

#[test]
fn issue_2911_recording_the_same_login_again_is_allowed_and_kept() {
    // Nothing here can refuse a wait any more. A step meeting limit after
    // limit on one login records a mark for each, and `used` and `waited_on`
    // go on answering their own questions.
    let mut m = Marks::new();
    for _ in 0..3 {
        m.record(key("a"));
        assert!(m.used());
        assert!(m.waited_on(&key("a")));
    }
    assert!(!m.waited_on(&key("b")), "still only login a");
}

#[test]
fn issue_2911_a_step_with_no_readable_login_name_waits_as_often_as_it_meets_a_limit() {
    // A step with no profile store records one `no login name` mark per
    // wait. Before #2911 the second such wait was refused and the step
    // ended; now there is nothing here to refuse it.
    let mut m = Marks::new();
    m.record(NONE);
    assert!(m.waited_on(&NONE));
    m.record(NONE);
    assert!(m.used());
    assert!(m.waited_on(&NONE));
}
