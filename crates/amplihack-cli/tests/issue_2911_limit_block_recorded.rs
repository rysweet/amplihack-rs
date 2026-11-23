//! Issues #2911 and #2909 (emitter): a step's limit failure of any kind
//! writes `overflow_blocked[login]` with the reset from the message, in the
//! shape older readers accept. A session block ends at most five hours after
//! it is written and a weekly block at most seven days after, whatever reset
//! the message states.

#![cfg(unix)]

use crate::issue_1618_fixture::{Fixture, LIMIT_TEXT, TEST_NOW, describe};
use crate::issue_1618_wait_profiles::with_profiles;
use crate::issue_1801_capacity_wait::{rotator_running, x_is_live};
use crate::issue_2882_spend_limit_capacity_wait::SPEND_FAR;
use std::fs;
use std::process::Command;

const WEEKLY_NEAR: &str = "You've hit your weekly limit · resets Oct 20, 2pm (America/Los_Angeles)";

/// `WEEKLY_NEAR`'s reset, 21:00Z on 20 October, measured from `TEST_NOW`
/// (15:00Z on 18 October): two days and six hours, inside the weekly window.
const WEEKLY_NEAR_SECS: i64 = 2 * 86_400 + 6 * 3_600;

/// A message in the harness's own shape stating a reset 30 days away, which
/// no session or weekly window has. The step cannot tell such a line from
/// one a model printed as its last line of output.
const SESSION_FAR: &str =
    "You've hit your session limit · resets Nov 17, 2pm (America/Los_Angeles)";
const WEEKLY_FAR: &str = "You've hit your weekly limit · resets Nov 17, 2pm (America/Los_Angeles)";

fn span(b: &serde_json::Value) -> i64 {
    b["until"].as_i64().unwrap() - b["at"].as_i64().unwrap()
}

fn ledger(f: &Fixture) -> serde_json::Value {
    let p = f.profiles.join(".auto-switch.d/rotate/ledger.json");
    serde_json::from_str(&fs::read_to_string(p).unwrap()).unwrap()
}

fn home_step(f: &Fixture, message: &str) -> Command {
    fs::create_dir_all(f.home.join(".claude")).unwrap();
    let mut c = f.step();
    c.env_remove("CLAUDE_CONFIG_DIR")
        .env("LIMIT_MESSAGE", message);
    c
}

fn run_and_block(message: &str) -> (Fixture, serde_json::Value) {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    let out = f.run(&mut home_step(&f, message));
    let l = ledger(&f);
    assert!(
        l["overflow_blocked"]["X"].is_object(),
        "no block for X: {l}\n{}",
        describe(&out)
    );
    let b = l["overflow_blocked"]["X"].clone();
    (f, b)
}

#[test]
fn a_session_limit_records_a_block_with_the_stated_reset() {
    let (_f, b) = run_and_block(LIMIT_TEXT);
    assert_eq!(b["reason"], "spend_limit");
    assert_eq!(
        b["until"].as_i64().unwrap() - b["at"].as_i64().unwrap(),
        1800
    );
    assert!(b["at"].as_i64().unwrap() > TEST_NOW, "real clock");
}

#[test]
fn a_weekly_limit_records_a_block_with_the_stated_reset() {
    let (_f, b) = run_and_block(WEEKLY_NEAR);
    assert_eq!(b["reason"], "spend_limit");
    assert_eq!(span(&b), WEEKLY_NEAR_SECS, "inside the window: unchanged");
}

#[test]
fn a_session_limit_stating_a_far_reset_blocks_for_at_most_five_hours() {
    let (_f, b) = run_and_block(SESSION_FAR);
    assert_eq!(b["reason"], "spend_limit");
    assert_eq!(span(&b), 5 * 3_600, "{b}");
}

#[test]
fn a_weekly_limit_stating_a_far_reset_blocks_for_at_most_seven_days() {
    let (_f, b) = run_and_block(WEEKLY_FAR);
    assert_eq!(b["reason"], "spend_limit");
    assert_eq!(span(&b), 7 * 86_400, "{b}");
}

#[test]
fn the_record_has_only_the_fields_every_reader_knows() {
    let (_f, b) = run_and_block(SPEND_FAR);
    let mut keys: Vec<_> = b.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(keys, ["at", "reason", "until"]);
}

#[test]
fn a_limit_with_no_stated_reset_other_than_spend_writes_no_block() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    // The wait has no deadline (issue #2911): the test changes the login once
    // it is under way, so the step can finish.
    let out = f.run_with_a_login_change_in(
        &f.home.join(".claude"),
        &mut home_step(&f, "You've hit your session limit · try again later"),
    );
    assert!(out.status.success(), "{}", describe(&out));
    let l = ledger(&f);
    assert!(
        l.get("overflow_blocked")
            .and_then(|b| b.as_object())
            .is_none_or(|b| b.is_empty()),
        "{l}"
    );
}

#[test]
fn the_step_is_unaffected_when_there_is_no_ledger() {
    let f = Fixture::new();
    let out = f.run(&mut home_step(&f, LIMIT_TEXT));
    assert!(out.status.success(), "{}", describe(&out));
    assert!(
        !f.profiles
            .join(".auto-switch.d/rotate/ledger.json")
            .exists()
    );
}
