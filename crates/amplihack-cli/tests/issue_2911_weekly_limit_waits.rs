//! Issue #2911: a weekly limit whose reset is days away, with no other login
//! that has room, waits and continues when a stand-in login gains room. It
//! used to end the run as `skipped reason=reset_too_far`.

#![cfg(unix)]

use crate::issue_1618_fixture::{WEEKLY_TEXT, describe, events};
use crate::issue_1618_wait_profiles::with_profiles;
use crate::issue_1801_capacity_wait::{
    assert_no_account_detail, real_clock, rotator_running, run_with_rotator_cycle_during_wait,
    x_is_live,
};
use std::fs;

fn home_step(f: &crate::issue_1618_fixture::Fixture) -> std::process::Command {
    fs::create_dir_all(f.home.join(".claude")).unwrap();
    let mut c = f.step();
    c.env_remove("CLAUDE_CONFIG_DIR")
        .env("LIMIT_MESSAGE", WEEKLY_TEXT);
    c
}

#[test]
fn a_weekly_limit_with_no_room_elsewhere_waits_then_continues_on_the_login_that_gains_room() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f);
    real_clock(&mut c);
    let out = run_with_rotator_cycle_during_wait(&f, &mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2, "one retry: {}", describe(&out));
    let ev = events(&out);
    assert!(
        ev.iter()
            .any(|l| l.starts_with("waiting_for_capacity reason=reset_too_far")),
        "the wait says plainly that it is waiting for capacity and why: {ev:?}"
    );
    assert!(
        !ev.iter()
            .any(|l| l.starts_with("skipped") || l.starts_with("failed")),
        "a limit never ends the run: {ev:?}"
    );
    assert!(
        ev.last().is_some_and(|l| l.starts_with("resume attempt=2")),
        "{ev:?}"
    );
    assert_no_account_detail(&f, &out);
}

/// Starting the wait no longer depends on a rotator running. Nothing moves a
/// login into `~/.claude` without one, so there is less that could end the
/// wait early, which is a reason to wait rather than a reason to end the run:
/// `plan::upgrade`'s old `takeover()` gate is what wrote `reset_too_far`.
#[test]
fn the_same_limit_waits_with_no_rotator_running() {
    let f = with_profiles();
    x_is_live(&f);
    let mut c = home_step(&f);
    real_clock(&mut c);
    let out = run_with_rotator_cycle_during_wait(&f, &mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2, "one retry: {}", describe(&out));
    let ev = events(&out);
    assert!(
        ev.iter()
            .any(|l| l.starts_with("waiting_for_capacity reason=reset_too_far")),
        "the wait still announces itself: {ev:?}"
    );
    assert!(
        ev.iter()
            .any(|l| l.starts_with("start reason=usage_limit wait=capacity")),
        "and it really is a capacity wait: {ev:?}"
    );
    assert!(
        !ev.iter()
            .any(|l| l.starts_with("skipped") || l.starts_with("failed")),
        "a limit never ends the run: {ev:?}"
    );
}

/// The hardest case the issue reports: one stored login, so there is no other
/// login that could ever take over, and no rotator. Every start gate that
/// used to be consulted would refuse here, and the run ended `reset_too_far`.
/// Now the step waits its one wait out and retries, and says why it waited.
#[test]
fn with_one_stored_login_a_weekly_limit_waits_rather_than_ending() {
    let f = crate::issue_1618_fixture::Fixture::new();
    let mut c = home_step(&f);
    real_clock(&mut c);
    // Nothing here can give a login room, so the wait runs its own length.
    // 60 seconds is the floor a configured length is raised to, and the
    // virtual clock `real_clock` sets runs it in a fraction of that.
    c.env("AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS", "60");
    let out = f.run(&mut c);
    let ev = events(&out);
    assert!(
        ev.iter()
            .any(|l| l.starts_with("waiting_for_capacity reason=reset_too_far")),
        "{ev:?}: {}",
        describe(&out)
    );
    assert_eq!(
        ev.iter()
            .filter(|l| l.starts_with("end reason=capacity expired"))
            .count(),
        1,
        "the wait reached its own deadline: {ev:?}"
    );
    assert!(
        ev.iter().any(|l| l.starts_with("resume attempt=2")),
        "and the step retried rather than ending: {ev:?}"
    );
    assert!(
        !ev.iter()
            .any(|l| l.starts_with("skipped") || l.starts_with("failed")),
        "{ev:?}: {}",
        describe(&out)
    );
    assert_eq!(f.count("0"), 2, "{}", describe(&out));
}
