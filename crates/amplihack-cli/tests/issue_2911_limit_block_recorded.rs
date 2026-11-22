//! Issues #2911 and #2909 (emitter): a step's limit failure of any kind
//! writes `overflow_blocked[login]` with the reset from the message, in the
//! shape older readers accept.

#![cfg(unix)]

use crate::issue_1618_fixture::{Fixture, LIMIT_TEXT, TEST_NOW, describe};
use crate::issue_1618_wait_profiles::with_profiles;
use crate::issue_1801_capacity_wait::{rotator_running, x_is_live};
use crate::issue_2882_spend_limit_capacity_wait::SPEND_FAR;
use std::fs;
use std::process::Command;

const WEEKLY_NEAR: &str = "You've hit your weekly limit · resets Oct 20, 2pm (America/Los_Angeles)";

fn ledger(f: &Fixture) -> serde_json::Value {
    let p = f.profiles.join(".auto-switch.d/rotate/ledger.json");
    serde_json::from_str(&fs::read_to_string(p).unwrap()).unwrap()
}

fn home_step(f: &Fixture, message: &str) -> Command {
    fs::create_dir_all(f.home.join(".claude")).unwrap();
    let mut c = f.step();
    c.env_remove("CLAUDE_CONFIG_DIR")
        .env("LIMIT_MESSAGE", message)
        .env("AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS", "120");
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
    assert!(b["until"].as_i64().unwrap() > b["at"].as_i64().unwrap());
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
    f.run(&mut home_step(
        &f,
        "You've hit your session limit · try again later",
    ));
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
