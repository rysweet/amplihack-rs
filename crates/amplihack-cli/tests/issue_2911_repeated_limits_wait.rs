//! Issue #2911: a second and a third limit in one step each wait. No
//! `already_waited` refusal and no `max_wait_secs` budget ends the chain.
//!
//! What a wait that outlasts `max_wait_secs` with no event does depends on
//! the wait, and both halves are pinned at the end of this file: a wait for
//! a stated reset does not run the step again before its reset, and a wait
//! with no reset to wait for does, once, at its outer bound
//! (docs/reference/usage-limit-waits.md, "The timed parts that remain").

#![cfg(unix)]

use crate::issue_1618_fixture::{Fixture, LIMIT_TEXT, claude_stdout, describe, events, wait_lines};

/// A session limit that states no reset, so its wait is a capacity wait with
/// no reset row: the configured length is what bounds it.
const NO_RESET_TEXT: &str = "You've hit your session limit · try again later";

fn step(f: &Fixture) -> std::process::Command {
    let mut c = f.step();
    c.env("LIMIT_MESSAGE", LIMIT_TEXT);
    c
}

/// A step whose limit gives it no reset to wait for.
fn no_reset_step(f: &Fixture) -> std::process::Command {
    let mut c = f.step();
    c.env("LIMIT_MESSAGE", NO_RESET_TEXT);
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
    // A limit with no reset, so the configured length is what bounds each
    // wait: it does not bound a
    // reset wait at all.
    let mut c = no_reset_step(&f);
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
    let starts: Vec<&String> = ev.iter().filter(|l| l.starts_with("start ")).collect();
    assert_eq!(starts.len(), 3, "{ev:?}");
    for l in &starts {
        assert!(l.ends_with(" bound=max_wait"), "{l}");
    }
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
    // The floor applies to the outer bound, which only a wait with no reset
    // to wait for has.
    let mut c = no_reset_step(&f);
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

/// A wait that outlasts `max_wait_secs`
/// with no event does not run the step again when its limit stated a reset.
///
/// The reviewer read the configured length as a timer that re-entered every
/// wait. For a reset wait it was: a length below the reset's distance ended
/// the wait at that length and ran the agent again before the reset, so a
/// step under a short length re-asked the harness on a timer while the one
/// event it was waiting for was half an hour off. A reset wait now has no
/// timed bound. With the shortest outer bound the setting allows and a reset
/// half an hour away, the stub runs once before the wait and once after the
/// reset, and not at the one-minute mark; the progress lines show the wait
/// still running long after it.
#[test]
fn a_reset_wait_outlasting_max_wait_secs_does_not_run_the_step_again() {
    let f = Fixture::new();
    let mut c = step(&f);
    c.env("LIMIT_CALLS", "1")
        .env("AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS", "60");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        f.count("0"),
        2,
        "one attempt before the wait and one after its reset, none at 60 s: {}",
        describe(&out)
    );
    assert_eq!(
        events(&out),
        [
            "start reason=usage_limit harness=claude reset=2026-10-18 08:30:00 -07:00 \
             until=2026-10-18 08:32:00 -07:00 bound=reset+margin",
            "end reason=reset reached",
            "resume attempt=2",
        ],
        "the reset, not the configured length, ended the wait"
    );
    let remaining: Vec<u64> = wait_lines(&out)
        .iter()
        .filter_map(|l| l.strip_prefix("progress remaining="))
        .filter_map(|r| r.strip_suffix('s'))
        .filter_map(|r| r.parse().ok())
        .collect();
    assert!(
        remaining.iter().any(|r| *r < 120),
        "the wait was still running within two minutes of its reset, half an hour \
         past the configured 60 seconds: {remaining:?}"
    );
    assert_eq!(claude_stdout(&out), "attempt 2 output\n");
}

/// The other half: a wait whose limit gives it no
/// reset to wait for, and which no event ends, does run the step again once
/// its outer bound passes.
///
/// It does so because nothing records when such a limit lifts: a session
/// limit with no stated reset writes no block, and on this host there is no
/// rotator and no other login, so nothing but the outer bound would ever run
/// the step again without a person. The run is never ended by it, and a
/// limit on the next attempt starts another wait. This is the one timed
/// re-entry left (docs/reference/usage-limit-waits.md, "The timed parts
/// that remain").
#[test]
fn a_wait_with_no_reset_runs_the_step_again_at_its_outer_bound() {
    let f = Fixture::new();
    let mut c = no_reset_step(&f);
    c.env("LIMIT_CALLS", "1")
        .env("AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS", "60");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2, "{}", describe(&out));
    assert_eq!(
        events(&out),
        [
            "waiting_for_capacity reason=no_reset harness=claude \
             until=2026-10-18 08:01:00 -07:00",
            "start reason=usage_limit wait=capacity harness=claude reset=none \
             until=2026-10-18 08:01:00 -07:00 bound=max_wait",
            "end reason=capacity expired",
            "resume attempt=2",
        ],
        "the outer bound ran the step again, and nothing ended it"
    );
}
