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
//!   the same, for the stated reset, which earns the retry; the configured
//!   maximum does not cut that wait short. Both used to end
//!   the run with `skipped reason=reset_too_far`, which is the failure
//!   #2911 reports: a login can gain room while a step waits, and the
//!   wait's own wake conditions see that, so refusing to start one because
//!   none has room yet ends a run a limit should only have delayed.
//! - A spend limit that states no reset is still no capacity wait (#2731):
//!   it waits for the login in `$HOME/.claude` to change, and since #2911
//!   that wait needs no running rotator either.
//! - A limit on the retry a wait earned starts another wait (#2857), as any
//!   limit does; nothing counts a step's waits.
//! - Issue #2967: with auto-switch on and no rotator running, the same home
//!   step moves to another stored login before any wait starts; with a
//!   rotator running it does not move itself, and waits as above.
//!
//! See `issue_1618_fixture` and `issue_1801_capacity_wait`.

#![cfg(unix)]

use crate::issue_1618_fixture::{Fixture, SPEND_TEXT, claude_stdout, describe, events};
use crate::issue_1618_wait_profiles::{switch_on, with_profiles};
use crate::issue_1801_capacity_wait::{
    assert_no_account_detail, real_clock, rotator_running, run_with_rotator_cycle_during_wait,
    x_is_live,
};
use std::fs;
use std::process::{Command, Output};

/// Claude's individual spend limit with a weekly reset days after `TEST_NOW`.
pub(crate) const SPEND_FAR: &str = "You've hit your individual spend limit · run /usage-credits to raise it, or visit claude.ai/admin-settings/usage · your weekly limit resets Oct 24, 2pm (America/Los_Angeles)";

/// `SPEND_FAR`'s reset, read at the fixture's `TEST_NOW`.
const STATED_RESET: &str = "2026-10-24 14:00:00 -07:00";
/// That reset plus the two-minute margin: the deadline of a wait for it.
const STATED_UNTIL: &str = "2026-10-24 14:02:00 -07:00";
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

/// Where a `$HOME/.claude` step's login lives, for a login change.
fn home_login(f: &Fixture) -> std::path::PathBuf {
    f.home.join(".claude")
}

/// The lines one `SPEND_FAR` capacity wait writes when nothing ends it
/// early: why it is waiting, the wait itself, which ends at the stated
/// reset, and the retry it earns. Until #2911
/// there was one line, `skipped reason=reset_too_far`, and no retry.
fn capacity_wait(attempt: u32) -> Vec<String> {
    vec![
        format!("waiting_for_capacity reason=reset_too_far harness=claude until={STATED_UNTIL}"),
        format!(
            "start reason=usage_limit wait=capacity harness=claude reset={STATED_RESET} \
             until={STATED_UNTIL} bound=reset+margin"
        ),
        "end reason=reset reached".to_string(),
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
    assert_eq!(events(out), capacity_wait(2), "{}", describe(out));
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
    let out = f.run(&mut c);
    let ev = events(&out);
    let start = capacity_wait(2).swap_remove(1);
    assert_eq!(ev.get(1), Some(&start), "{ev:?}: {}", describe(&out));
    assert!(
        !ev.iter().any(|l| l.contains(FIVE_HOURS_ON)),
        "no line names a five-hour default: {ev:?}"
    );
}

/// With no running rotator the step waits all the same, reaches the stated
/// reset and retries.
///
/// Until #2911 this ended the run with `skipped reason=reset_too_far`,
/// because nothing could move another login in *yet*. That is the heart of
/// the issue: a rotator may start, and a login may gain room, while the
/// step waits, and the wait's own wake conditions notice either.
#[test]
fn without_a_running_rotator_the_step_still_waits() {
    // Issue #2967: with auto-switch off nothing may move the step, so it
    // waits on its own login. The setting is stated, not left to the
    // fixture's default.
    let f = with_profiles();
    x_is_live(&f);
    let mut c = home_step(&f, SPEND_FAR);
    c.env("AMPLIHACK_CLAUDE_AUTO_SWITCH", "0");
    let out = f.run(&mut c);
    assert_waited_then_retried(&f, &out);
}

/// Issue #2967: the same step with auto-switch on, no rotator running and a
/// login to move to is rerun there at once. No wait starts, so the step
/// writes no wait line at all. `Y` has no reading, which the candidate rule
/// takes as room.
#[test]
fn with_auto_switch_on_and_no_rotator_a_far_spend_reset_moves_to_another_login() {
    let f = with_profiles();
    switch_on(&f);
    x_is_live(&f);
    let mut c = home_step(&f, SPEND_FAR);
    c.env("AMPLIHACK_CLAUDE_AUTO_SWITCH", "1");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2, "one move: {}", describe(&out));
    assert_eq!(
        events(&out),
        Vec::<String>::new(),
        "no wait: {}",
        describe(&out)
    );
    assert_eq!(f.call("0", 2)["PROFILE"], "Y");
    assert_eq!(claude_stdout(&out), "attempt 2 output\n");
}

/// Issue #2967: a running rotator moves the login in `~/.claude` itself
/// (since #3012 at once when a limit is recorded), so the step does not
/// move itself even with auto-switch on, and makes the capacity wait the
/// other tests here make.
#[test]
fn with_auto_switch_on_a_rotator_running_keeps_the_far_spend_reset_capacity_wait() {
    let f = with_profiles();
    switch_on(&f);
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f, SPEND_FAR);
    c.env("AMPLIHACK_CLAUDE_AUTO_SWITCH", "1");
    let out = f.run(&mut c);
    assert_waited_then_retried(&f, &out);
}

/// With one stored login the step waits all the same, reaches the stated
/// reset and retries.
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
    let out = f.run(&mut c);
    assert_waited_then_retried(&f, &out);
}

/// A spend limit stating no reset is still no capacity wait: #2731 owns the
/// case, and it waits for the login in `$HOME/.claude` to change. Since
/// #2911 it does so with no rotator running, where it used to end the run
/// with `skipped reason=spend_limit`, and it has no deadline: the login
/// change the test makes ends it. Its `waiting_for_capacity` reason is
/// `spend_limit`, the same word the `start` line's `reason=` carries.
#[test]
fn a_spend_limit_with_no_reset_waits_for_the_login_to_change() {
    let f = with_profiles();
    x_is_live(&f);
    let mut c = home_step(&f, SPEND_TEXT);
    let out = f.run_with_a_login_change_in(&home_login(&f), &mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2, "one retry: {}", describe(&out));
    assert_eq!(
        events(&out),
        [
            "waiting_for_capacity reason=spend_limit harness=claude until=none",
            "start reason=spend_limit wait=login_change harness=claude reset=none until=none \
             bound=none",
            "end reason=login changed",
            "resume attempt=2",
        ],
        "{}",
        describe(&out)
    );
}

/// #2871 (#2857) kept: a limit on the retry a wait earned goes back into a
/// wait and does not end the step.
///
/// Nothing bounds the chain but the waits themselves (#2911): no per-step
/// budget refuses the second, and no retry count or timer either. The retry
/// meets the same message at the stated reset, when that reset has just
/// passed, so the second wait has no deadline and the login change the test
/// makes ends it.
#[test]
fn a_retry_that_meets_the_limit_again_waits_again() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f, SPEND_FAR);
    c.env("LIMIT_CALLS", "2");
    let out = f.run_with_a_login_change_in(&home_login(&f), &mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        f.count("0"),
        3,
        "two waits, two retries: {}",
        describe(&out)
    );
    let ev = events(&out);
    assert_eq!(ev[..4], capacity_wait(2), "{}", describe(&out));
    assert_eq!(
        ev.iter().filter(|l| l.starts_with("start ")).count(),
        2,
        "the retry's limit starts a second wait: {ev:?}"
    );
    assert_eq!(
        ev.last().map(String::as_str),
        Some("resume attempt=3"),
        "{ev:?}"
    );
    assert!(
        !ev.iter()
            .any(|l| l.starts_with("failed ") || l.starts_with("skipped ")),
        "{ev:?}"
    );
}
