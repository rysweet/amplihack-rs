//! Issue #2882, as issue #2911 left it: a spend limit that states a far
//! weekly reset waits for a login with room instead of ending the run. The
//! sequence of 20 Nov: the live login hits an individual spend limit
//! resetting days away, the rotator then reports a second login with room,
//! and the step continues on it.
//!
//! - A home step with a running rotator and a second login says why it is
//!   waiting (`waiting_for_capacity reason=reset_too_far`), starts a
//!   capacity wait (`wait=capacity harness=claude reset=<stated>`), ends on
//!   the rotator's capacity and retries once.
//! - With no running rotator, and with one stored login, the step waits all
//!   the same and the wait's own deadline earns the retry. Both used to end
//!   the run with `skipped reason=reset_too_far`, which is the failure
//!   #2911 reports: a login can gain room while a step waits, and the
//!   wait's own wake conditions see that, so refusing to start one because
//!   none has room yet ends a run a limit should only have delayed.
//! - A spend limit that states no reset is still no capacity wait (#2731):
//!   it waits for the login in `$HOME/.claude` to change, and since #2911
//!   that wait needs no running rotator either.
//! - A limit on the retry a wait earned starts another wait (#2857), as any
//!   limit does; nothing counts a step's waits.
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

/// The length of one wait here, in seconds, as a string. Since #2911 this
/// is the shortest a configured value may ask for: `MIN_WAIT_SECS` floors
/// it where it is read, so a configured `0` would wait this long too.
const ONE_WAIT: &str = "60";
/// `SPEND_FAR`'s reset, read at the fixture's `TEST_NOW`.
const STATED_RESET: &str = "2026-10-24 14:00:00 -07:00";
/// `TEST_NOW + 60 s` and `TEST_NOW + 120 s`: the deadlines of the first and
/// of a second `ONE_WAIT` wait. The fixture's clock moves only as a wait
/// sleeps, so both are exact.
const FIRST_UNTIL: &str = "2026-10-18 08:01:00 -07:00";
const SECOND_UNTIL: &str = "2026-10-18 08:02:00 -07:00";
/// `TEST_NOW + 5 h`: the ceiling a reset wait is clamped to, which no line
/// about `SPEND_FAR` may name.
const FIVE_HOURS_ON: &str = "2026-10-18 13:00:00 -07:00";

fn home_step(f: &Fixture, message: &str) -> Command {
    fs::create_dir_all(f.home.join(".claude")).unwrap();
    let mut c = f.step();
    c.env_remove("CLAUDE_CONFIG_DIR")
        .env("LIMIT_MESSAGE", message);
    c
}

/// A step whose waits are one minute long on the fixture's virtual clock,
/// so the lines they write name exact times and nothing sleeps for real.
fn one_minute_waits(c: &mut Command) -> &mut Command {
    c.env("AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS", ONE_WAIT)
}

/// The lines one `SPEND_FAR` capacity wait writes when nothing ends it
/// early: why it is waiting, the wait itself, its deadline and the retry it
/// earns. Until #2911 there was one line, `skipped reason=reset_too_far`,
/// and no retry.
fn capacity_wait(until: &str, attempt: u32) -> Vec<String> {
    vec![
        format!("waiting_for_capacity reason=reset_too_far harness=claude until={until}"),
        format!(
            "start reason=usage_limit wait=capacity harness=claude reset={STATED_RESET} \
             until={until} bound=max_wait"
        ),
        "end reason=capacity expired".to_string(),
        format!("resume attempt={attempt}"),
    ]
}

/// One wait, its deadline and one retry, with no line that ends the step.
fn assert_waited_then_retried(f: &Fixture, out: &Output) {
    assert!(out.status.success(), "{}", describe(out));
    assert_eq!(
        f.count("0"),
        2,
        "the wait earned one retry: {}",
        describe(out)
    );
    assert_eq!(
        events(out),
        capacity_wait(FIRST_UNTIL, 2),
        "{}",
        describe(out)
    );
}

/// A far spend reset waits for capacity and the rotator's next cycle ends
/// the wait, so the retry runs on the login that has room. The
/// `waiting_for_capacity` line is new in #2911: a step that may wait a long
/// time says on stderr and in the recipe log that it is waiting, and why.
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
    assert_eq!(ev.len(), 4, "{ev:?}");
    assert!(
        ev[0].starts_with("waiting_for_capacity reason=reset_too_far harness=claude until=2"),
        "{ev:?}"
    );
    assert!(
        ev[1].starts_with("start reason=usage_limit wait=capacity harness=claude reset=2"),
        "{ev:?}"
    );
    assert_eq!(ev[2..], ["end reason=rotator capacity", "resume attempt=2"]);
    assert_eq!(claude_stdout(&out), "attempt 2 output\n");
    assert_no_account_detail(&f, &out);
}

/// The `start` line names the reset the message stated, not the five-hour
/// ceiling a reset wait is clamped to: a capacity wait has no such ceiling,
/// and the stated reset is the only reset it reports.
#[test]
fn the_wait_names_the_stated_reset_not_the_five_hour_default() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f, SPEND_FAR);
    // On the fixture's virtual clock a one-minute wait costs no real time.
    // Since #2911 a configured `0` reads as this same floor, so it would
    // really wait a minute; `recipe.usage_limit_wait = false` is how to
    // turn the wait off.
    one_minute_waits(&mut c);
    let out = f.run(&mut c);
    let ev = events(&out);
    let start = capacity_wait(FIRST_UNTIL, 2).swap_remove(1);
    assert_eq!(ev.get(1), Some(&start), "{ev:?}: {}", describe(&out));
    assert!(
        !ev.iter().any(|l| l.contains(FIVE_HOURS_ON)),
        "no line names a five-hour default: {ev:?}"
    );
}

/// With no running rotator the step waits all the same, reaches the wait's
/// deadline and retries.
///
/// Until #2911 this ended the run with `skipped reason=reset_too_far`,
/// because nothing could move another login in *yet*. That is the heart of
/// the issue: a rotator may start, and a login may gain room, while the
/// step waits, and the wait's own wake conditions notice either.
#[test]
fn without_a_running_rotator_the_step_still_waits() {
    let f = with_profiles();
    x_is_live(&f);
    let mut c = home_step(&f, SPEND_FAR);
    one_minute_waits(&mut c);
    let out = f.run(&mut c);
    assert_waited_then_retried(&f, &out);
}

/// With one stored login the step waits all the same, reaches the wait's
/// deadline and retries.
///
/// Until #2911 this ended the run with `skipped reason=reset_too_far`: no
/// second login could take over. A second login can be added, or the one
/// stored login's own window can recover, while the step waits, so the
/// store's contents no longer decide whether a wait starts.
#[test]
fn with_one_stored_login_the_step_still_waits() {
    let f = Fixture::new();
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f, SPEND_FAR);
    one_minute_waits(&mut c);
    let out = f.run(&mut c);
    assert_waited_then_retried(&f, &out);
}

/// A spend limit stating no reset is still no capacity wait: #2731 owns the
/// case, and it waits for the login in `$HOME/.claude` to change. Since
/// #2911 it does so with no rotator running, where it used to end the run
/// with `skipped reason=spend_limit`. Its `waiting_for_capacity` reason is
/// `spend_limit`, the same word the `start` line's `reason=` carries.
#[test]
fn a_spend_limit_with_no_reset_waits_for_the_login_to_change() {
    let f = with_profiles();
    x_is_live(&f);
    let mut c = home_step(&f, SPEND_TEXT);
    one_minute_waits(&mut c);
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2, "one retry: {}", describe(&out));
    assert_eq!(
        events(&out),
        [
            format!("waiting_for_capacity reason=spend_limit harness=claude until={FIRST_UNTIL}"),
            format!(
                "start reason=spend_limit wait=login_change harness=claude reset=none \
                 until={FIRST_UNTIL} bound=max_wait"
            ),
            "end reason=max reached".to_string(),
            "resume attempt=2".to_string(),
        ],
        "{}",
        describe(&out)
    );
}

/// #2871 (#2857) kept: a limit on the retry a wait earned goes back into a
/// wait and does not end the step.
///
/// Nothing bounds the chain but the waits themselves costing time (#2911).
/// The step's `max_wait_secs` is the length of *one* wait, so the second
/// wait is as long as the first and starts where the first ended; there is
/// no per-step budget left to refuse it, and no retry count or timer either.
#[test]
fn a_retry_that_meets_the_limit_again_waits_again() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f, SPEND_FAR);
    c.env("LIMIT_CALLS", "2");
    one_minute_waits(&mut c);
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        f.count("0"),
        3,
        "two waits, two retries: {}",
        describe(&out)
    );
    let expected: Vec<String> = capacity_wait(FIRST_UNTIL, 2)
        .into_iter()
        .chain(capacity_wait(SECOND_UNTIL, 3))
        .collect();
    assert_eq!(events(&out), expected, "{}", describe(&out));
}
