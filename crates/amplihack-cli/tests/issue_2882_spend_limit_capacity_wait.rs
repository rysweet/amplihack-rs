//! Issue #2882: a spend limit that states a far weekly reset waits for a
//! login with room when another login could take over, instead of ending the
//! run with `skipped reason=reset_too_far`. The sequence of 20 Nov: the live
//! login hits an individual spend limit resetting days away, the rotator then
//! reports a second login with room, and the step continues on it.
//!
//! - A home step with a running rotator and a second login starts a capacity
//!   wait (`wait=capacity harness=claude reset=<stated>`), ends on the
//!   rotator's capacity and retries once.
//! - With no running rotator, or auto-switch off, the skip stays
//!   `reset_too_far`: nothing could move another login in.
//! - A spend limit that states no reset keeps #2731: no capacity wait.
//! - A limit after the wait starts another wait (#2857), as for any limit.
//!
//! See `issue_1618_fixture` and `issue_1801_capacity_wait`.

#![cfg(unix)]

use crate::issue_1618_fixture::{Fixture, SPEND_TEXT, claude_stdout, describe, events};
use crate::issue_1618_wait_profiles::with_profiles;
use crate::issue_1801_capacity_wait::{
    assert_no_account_detail, real_clock, rotator_running, run_with_rotator_cycle_during_wait,
    x_is_live,
};
use std::fs;
use std::process::{Command, Output};

/// Claude's individual spend limit with a weekly reset days after `TEST_NOW`.
pub(crate) const SPEND_FAR: &str = "You've hit your individual spend limit · run /usage-credits to raise it, or visit claude.ai/admin-settings/usage · your weekly limit resets Oct 24, 2pm (America/Los_Angeles)";

fn home_step(f: &Fixture, message: &str) -> Command {
    fs::create_dir_all(f.home.join(".claude")).unwrap();
    let mut c = f.step();
    c.env_remove("CLAUDE_CONFIG_DIR")
        .env("LIMIT_MESSAGE", message);
    c
}

fn assert_skipped(f: &Fixture, out: &Output, reason: &str) {
    assert_eq!(out.status.code(), Some(1), "{reason}: {}", describe(out));
    assert_eq!(f.count("0"), 1, "{reason}: no rerun");
    assert_eq!(
        events(out),
        [format!("skipped reason={reason}")],
        "{reason}"
    );
}

#[test]
fn a_far_spend_reset_starts_a_capacity_wait_and_resumes_on_the_login_with_room() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f, SPEND_FAR);
    real_clock(&mut c);
    let out = run_with_rotator_cycle_during_wait(&f, &mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2, "one retry: {}", describe(&out));
    let ev = events(&out);
    assert_eq!(ev.len(), 3, "{ev:?}");
    assert!(
        ev[0].starts_with("start reason=usage_limit wait=capacity harness=claude reset=2"),
        "{ev:?}"
    );
    assert_eq!(ev[1..], ["end reason=rotator capacity", "resume attempt=2"]);
    assert_eq!(claude_stdout(&out), "attempt 2 output\n");
    assert_no_account_detail(&f, &out);
}

#[test]
fn the_wait_names_the_stated_reset_not_the_five_hour_default() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f, SPEND_FAR);
    c.env("AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS", "0");
    let out = f.run(&mut c);
    let ev = events(&out);
    assert!(
        ev[0].starts_with(
            "start reason=usage_limit wait=capacity harness=claude reset=2026-10-24 14:00:00 -07:00"
        ),
        "{ev:?}: {}",
        describe(&out)
    );
}

#[test]
fn without_a_running_rotator_the_skip_stays_reset_too_far() {
    let f = with_profiles();
    x_is_live(&f);
    let out = f.run(&mut home_step(&f, SPEND_FAR));
    assert_skipped(&f, &out, "reset_too_far");
}

#[test]
fn with_one_stored_login_the_skip_stays_reset_too_far() {
    let f = Fixture::new();
    let _rotator = rotator_running(&f);
    let out = f.run(&mut home_step(&f, SPEND_FAR));
    assert_skipped(&f, &out, "reset_too_far");
}

#[test]
fn a_spend_limit_with_no_reset_still_starts_no_capacity_wait() {
    // #2731 owns this case: with no rotator it is `spend_limit`.
    let f = with_profiles();
    x_is_live(&f);
    let out = f.run(&mut home_step(&f, SPEND_TEXT));
    assert_skipped(&f, &out, "spend_limit");
}

#[test]
fn a_retry_that_meets_the_limit_again_waits_again() {
    // #2871 (#2857) kept: a limit on the retry a wait earned goes back into
    // a wait and does not end the step. The step's `max_wait_secs` budget,
    // and nothing else, bounds the chain: without it the second wait would
    // run to the six-hour default, which is what a retry count or a timer
    // would have been added for.
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f, SPEND_FAR);
    c.env("LIMIT_CALLS", "2")
        .env("AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS", "120");
    real_clock(&mut c);
    let out = run_with_rotator_cycle_during_wait(&f, &mut c);
    let ev = events(&out);
    assert!(
        ev[0].starts_with("start reason=usage_limit wait=capacity"),
        "{ev:?}: {}",
        describe(&out)
    );
    assert_eq!(f.count("0"), 2, "one retry: {}", describe(&out));
    assert!(
        !ev.iter().any(|l| l.contains("reason=reset_too_far")),
        "the stated reset never ends the run on its own: {ev:?}"
    );
}
