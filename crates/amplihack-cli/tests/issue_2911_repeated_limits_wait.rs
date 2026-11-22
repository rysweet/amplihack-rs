//! Issue #2911: a second and a third limit in one step each wait. No
//! `already_waited` refusal and no `max_wait_secs` budget ends the chain.

#![cfg(unix)]

use crate::issue_1618_fixture::{Fixture, LIMIT_TEXT, claude_stdout, describe, events};

fn step(f: &Fixture) -> std::process::Command {
    let mut c = f.step();
    c.env("LIMIT_MESSAGE", LIMIT_TEXT);
    c
}

#[test]
fn three_limits_in_one_step_each_wait_and_the_fourth_attempt_succeeds() {
    let f = Fixture::new();
    let mut c = step(&f);
    c.env("LIMIT_CALLS", "3");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 4, "{}", describe(&out));
    let ev = events(&out);
    assert_eq!(
        ev.iter().filter(|l| l.starts_with("start ")).count(),
        3,
        "{ev:?}"
    );
    assert_eq!(claude_stdout(&out), "attempt 4 output\n");
}

#[test]
fn a_spent_max_wait_budget_does_not_end_the_chain() {
    let f = Fixture::new();
    let mut c = step(&f);
    // A budget of one wait length: under the old rule the second limit was
    // refused because the budget was spent.
    c.env("LIMIT_CALLS", "3")
        .env("AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS", "1800");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 4);
    let ev = events(&out);
    // Each of the three waits gets the configured length in full. Under the
    // old rule the value was a total, so the second wait got what the first
    // had left and the third got a refusal.
    assert_eq!(
        ev.iter().filter(|l| l.starts_with("start ")).count(),
        3,
        "{ev:?}"
    );
    assert!(
        !ev.iter().any(|l| l.starts_with("failed")),
        "no total ends the chain: {ev:?}"
    );
}

#[test]
fn no_refusal_or_budget_ending_is_ever_logged() {
    let f = Fixture::new();
    let mut c = step(&f);
    c.env("LIMIT_CALLS", "3");
    let out = f.run(&mut c);
    for l in events(&out) {
        for gone in [
            "already_waited",
            "capacity_expired",
            "failed reason=spend_limit",
        ] {
            assert!(!l.contains(gone), "{gone}: {l}");
        }
    }
}

/// A configured zero is raised to the 60-second floor, so it still buys a
/// real wait rather than a zero-length one.
///
/// This is the pacing rule at its sharpest point. The wait loop polls every
/// 30 seconds, so a wait shorter than one interval has no wake condition
/// that could end it and would simply return; a chain of those is a busy
/// loop, which is what a per-step budget used to be there to stop. The floor
/// is what makes "every plan ends strictly after now" true for every
/// configured value and not only for the default.
///
/// A zero therefore no longer means "do not wait";
/// `recipe.usage_limit_wait = false` is how to turn waiting off.
#[test]
fn a_configured_zero_wait_length_is_raised_to_the_floor_and_still_waits() {
    let f = Fixture::new();
    let mut c = step(&f);
    c.env("LIMIT_CALLS", "2")
        .env("AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS", "0");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 3, "{}", describe(&out));
    let ev = events(&out);
    let starts: Vec<&String> = ev.iter().filter(|l| l.starts_with("start ")).collect();
    assert_eq!(starts.len(), 2, "{ev:?}");
    // Every wait is bounded by the configured length, so by the floor: not
    // by `reset+margin`. No step timeout bounds a wait (issue #3037).
    for l in &starts {
        assert!(
            l.ends_with(" bound=max_wait"),
            "the configured length is what bounds the wait: {l}"
        );
    }
    // The first wait starts on the fixture's own clock, so its deadline is
    // exactly 60 seconds past `TEST_NOW` (08:00:00 -07:00): a real minute,
    // neither `now` nor the 21600-second default. The second wait starts
    // later by however long the retry took, so only its row is pinned.
    assert!(
        starts[0].ends_with("until=2026-10-18 08:01:00 -07:00 bound=max_wait"),
        "a zero must buy the 60-second floor, not a zero-length wait: {}",
        starts[0]
    );
}
