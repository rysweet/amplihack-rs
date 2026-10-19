//! Issue #1618: how the usage-limit wait ends other than at the reset.
//!
//! - AC4: `max_wait_secs = 0` waits zero seconds and still retries once;
//!   a step timeout of 60 s or less ends the wait at once with no retry.
//! - AC5: a login that changes during the wait ends it with
//!   `login changed`, then the retry runs.
//!
//! See `issue_1618_fixture`.

#![cfg(unix)]

use crate::issue_1618_fixture::{Fixture, claude_stdout, describe, events};
use std::time::{Duration, Instant};

#[test]
fn ac4_a_zero_maximum_waits_zero_seconds_and_retries_once() {
    let f = Fixture::new();
    let mut c = f.step();
    c.env("AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS", "0");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        events(&out),
        vec![
            "start reason=usage_limit harness=claude reset=2026-10-18 08:30:00 -07:00 \
             until=2026-10-18 08:00:00 -07:00 bound=max_wait",
            "end reason=max reached",
            "resume attempt=2",
        ]
    );
    assert_eq!(f.count("0"), 2);
}

#[test]
fn ac4_a_smaller_maximum_names_the_bound() {
    let f = Fixture::new();
    let mut c = f.step();
    c.env("AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS", "600");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        events(&out)[0],
        "start reason=usage_limit harness=claude reset=2026-10-18 08:30:00 -07:00 \
         until=2026-10-18 08:10:00 -07:00 bound=max_wait"
    );
}

#[test]
fn ac4_a_larger_maximum_changes_nothing() {
    let f = Fixture::new();
    let mut c = f.step();
    c.env("AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS", "86400");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert!(
        events(&out)[0].ends_with("until=2026-10-18 08:32:00 -07:00 bound=reset+margin"),
        "{:?}",
        events(&out)
    );
}

#[test]
fn ac4_a_step_timeout_of_sixty_seconds_or_less_ends_the_step_without_a_retry() {
    let f = Fixture::new();
    let mut c = f.step();
    c.env("AMPLIHACK_STEP_TIMEOUT", "30");
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(f.count("0"), 1, "no retry after a step timeout");
    let ev = events(&out);
    assert_eq!(ev.len(), 2, "{ev:?}");
    assert!(
        ev[0].starts_with(
            "start reason=usage_limit harness=claude reset=2026-10-18 08:30:00 -07:00 until="
        ),
        "{ev:?}"
    );
    assert!(ev[0].ends_with(" bound=step_timeout"), "{ev:?}");
    assert_eq!(ev[1], "end reason=step timeout");
    assert_eq!(
        claude_stdout(&out),
        "attempt 1 output\n",
        "the failed attempt's held-back stdout is printed"
    );
    assert!(f.stdin_copies().is_empty());
}

#[test]
fn a_zero_step_timeout_does_not_bound_the_wait() {
    let f = Fixture::new();
    let mut c = f.step();
    c.env("AMPLIHACK_STEP_TIMEOUT", "0");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert!(
        events(&out)[0].ends_with("bound=reset+margin"),
        "{:?}",
        events(&out)
    );
}

#[test]
fn ac5_a_login_moved_into_place_during_the_wait_ends_it() {
    let f = Fixture::new();
    let mut c = f.step();
    // 100x: a 30 s virtual poll is 0.3 s real, the 1920 s wait 19.2 s real.
    // The credentials file appears 1 s (real) after the first attempt.
    c.env("AMPLIHACK_USAGE_LIMIT_WAIT_TEST_SCALE", "100")
        .env("CHANGE_LOGIN_AFTER", "1");
    let started = Instant::now();
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    let ev = events(&out);
    assert_eq!(
        ev[1..],
        ["end reason=login changed", "resume attempt=2"],
        "{ev:?}"
    );
    assert_eq!(f.count("0"), 2);
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "the wait should end at the login change, not the reset"
    );
}
