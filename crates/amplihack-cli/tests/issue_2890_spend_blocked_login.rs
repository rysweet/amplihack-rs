//! Issue #2890: a step's spend-limit failure records the login it ran under
//! as blocked until the reset the message states, as data in the rotate
//! ledger (`overflow_blocked`), and `rotate status` shows it.
//!
//! - A message stating a parsable reset in range writes one block keyed on
//!   the login, `reason=spend_limit`, `until - at` equal to the stated
//!   distance, with one ALERT line in the rotate log.
//! - A message stating no reset writes nothing and keeps #2731.
//! - A stated reset out of range is reported on stderr and not stored; the
//!   ledger still loads (`rotate status` exits 0).
//! - A step with no rotate ledger creates none.
//! - Recording never changes the step's own outcome or exit code.
//! - `rotate status` prints `blocked logins: <name> (blocked until <time>)`.

#![cfg(unix)]

use crate::issue_1618_fixture::{
    Fixture, SPEND_TEXT, SPEND_WEEKLY_TEXT, TEST_NOW, describe, events, text,
};
use crate::issue_1618_wait_profiles::with_profiles;
use crate::issue_1801_capacity_wait::{rotator_running, x_is_live};
use crate::issue_2882_spend_limit_capacity_wait::SPEND_FAR;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// `SPEND_FAR`'s reset, `TEST_NOW + 6 days 6 hours`, as a distance.
const FAR_DISTANCE: i64 = 6 * 86_400 + 6 * 3600;

fn ledger_path(f: &Fixture) -> PathBuf {
    f.profiles
        .join(".auto-switch.d")
        .join("rotate")
        .join("ledger.json")
}

fn ledger(f: &Fixture) -> serde_json::Value {
    serde_json::from_str(&fs::read_to_string(ledger_path(f)).unwrap()).unwrap()
}

fn home_step(f: &Fixture, message: &str) -> Command {
    fs::create_dir_all(f.home.join(".claude")).unwrap();
    let mut c = f.step();
    c.env_remove("CLAUDE_CONFIG_DIR")
        .env("LIMIT_MESSAGE", message)
        .env("AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS", "0");
    c
}

fn rotate_log(f: &Fixture) -> String {
    fs::read_to_string(
        f.profiles
            .join(".auto-switch.d")
            .join("rotate")
            .join("rotate.log"),
    )
    .unwrap_or_default()
}

#[test]
fn a_spend_limit_with_a_stated_reset_blocks_the_login_the_step_ran_under() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    let out = f.run(&mut home_step(&f, SPEND_FAR));
    let l = ledger(&f);
    let b = &l["overflow_blocked"]["X"];
    assert!(b.is_object(), "no block for X: {l}\n{}", describe(&out));
    assert_eq!(b["reason"], "spend_limit");
    let (at, until) = (b["at"].as_i64().unwrap(), b["until"].as_i64().unwrap());
    assert_eq!(
        until - at,
        FAR_DISTANCE,
        "the stated reset, on the real clock"
    );
    assert!(
        at > TEST_NOW,
        "recorded on the real clock, not the test clock"
    );
    let log = rotate_log(&f);
    assert_eq!(
        log.matches("ALERT: login X hit its spend limit; blocked until ")
            .count(),
        1,
        "{log}"
    );
}

#[test]
fn a_second_failing_step_neither_extends_the_block_nor_alerts_again() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    f.run(&mut home_step(&f, SPEND_FAR));
    let first = ledger(&f)["overflow_blocked"]["X"].clone();
    f.run(&mut home_step(&f, SPEND_FAR));
    assert_eq!(ledger(&f)["overflow_blocked"]["X"], first);
    assert_eq!(
        rotate_log(&f)
            .matches("hit its spend limit; blocked until")
            .count(),
        1
    );
}

#[test]
fn a_spend_limit_with_no_reset_writes_no_block() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    let out = f.run(&mut home_step(&f, SPEND_TEXT));
    let l = ledger(&f);
    let blocked = l.get("overflow_blocked").and_then(|b| b.as_object());
    assert!(blocked.is_none_or(|b| b.is_empty()), "{l}");
    assert!(
        !rotate_log(&f).contains("spend limit"),
        "{}",
        rotate_log(&f)
    );
    assert!(!out.status.success(), "{}", describe(&out));
}

#[test]
fn a_stated_reset_out_of_range_is_reported_and_not_stored() {
    // `SPEND_WEEKLY_TEXT` reads at `TEST_NOW` as next October: far beyond
    // the 62-day cap on a block.
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    let out = f.run(&mut home_step(&f, SPEND_WEEKLY_TEXT));
    let l = ledger(&f);
    let blocked = l.get("overflow_blocked").and_then(|b| b.as_object());
    assert!(blocked.is_none_or(|b| b.is_empty()), "{l}");
    assert!(
        text(&out.stderr)
            .contains("ALERT: login X spend limit not blocked (stated reset out of range)"),
        "{}",
        describe(&out)
    );
    // The ledger still loads for the rotator's own reader.
    let status = f.run(&mut f.cmd(&["profile", "rotate", "status"]));
    assert!(status.status.success(), "{}", describe(&status));
}

/// The record fails open, and the documented row says so: when the block
/// cannot be written the wait is planned exactly as it would have been.
///
/// This is the one place the two halves of the change could have been tied
/// together by a check — "only wait if the block was stored" — and they are
/// deliberately not. `note_spend_block` returns `()` so that no caller can
/// read its outcome, and the planner never asks whether a block exists.
/// What follows from that is stated rather than hidden: the blocked login
/// is not recorded, so the takeover count and the wake condition still
/// include it, as they did before this change.
#[test]
fn a_record_that_could_not_be_written_still_plans_the_wait() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    // Out of the 62-day span, so the writer refuses it; the message is
    // otherwise an ordinary far spend reset.
    let out = f.run(&mut home_step(&f, SPEND_WEEKLY_TEXT));
    let l = ledger(&f);
    assert!(
        l.get("overflow_blocked")
            .and_then(|b| b.as_object())
            .is_none_or(|b| b.is_empty()),
        "nothing stored: {l}"
    );
    let ev = events(&out);
    assert!(
        ev[0].starts_with("start reason=usage_limit wait=capacity harness=claude"),
        "the wait is planned whatever happened to the record: {ev:?}: {}",
        describe(&out)
    );
    assert!(
        !ev.iter().any(|l| l.contains("reason=reset_too_far")),
        "a refused record never turns the wait back into a skip: {ev:?}"
    );
}

#[test]
fn a_step_with_no_rotate_ledger_creates_none() {
    let f = with_profiles();
    let out = f.run(&mut home_step(&f, SPEND_FAR));
    assert!(!ledger_path(&f).exists(), "{}", describe(&out));
}

#[test]
fn recording_the_block_does_not_change_the_steps_exit_code() {
    let f = with_profiles();
    x_is_live(&f);
    let out = f.run(&mut home_step(&f, SPEND_FAR));
    // No rotator running: the skip, and the step's own exit code, stand.
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
}

#[test]
fn rotate_status_shows_the_block_in_one_extra_field() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    f.run(&mut home_step(&f, SPEND_FAR));
    let status = f.run(&mut f.cmd(&["profile", "rotate", "status"]));
    assert!(status.status.success(), "{}", describe(&status));
    let s = text(&status.stdout);
    let line = s
        .lines()
        .find(|l| l.starts_with("blocked logins:"))
        .unwrap_or_else(|| panic!("no `blocked logins:` field in {s}"));
    assert!(
        line.starts_with("blocked logins: X (blocked until "),
        "{line}"
    );
    let at = s.find("overflow:").unwrap();
    let blocked = s.find("blocked logins:").unwrap();
    let on = s.find("on overflow:").unwrap();
    assert!(at < blocked && blocked < on, "field order: {s}");
}

#[test]
fn rotate_status_prints_a_dash_when_nothing_is_blocked() {
    let f = with_profiles();
    x_is_live(&f);
    let status = f.run(&mut f.cmd(&["profile", "rotate", "status"]));
    assert!(
        text(&status.stdout).contains("blocked logins: -\n"),
        "{}",
        describe(&status)
    );
}

#[test]
fn overflow_clear_removes_a_step_written_block() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    f.run(&mut home_step(&f, SPEND_FAR));
    let out = f.run(&mut f.cmd(&["profile", "overflow", "clear", "X"]));
    assert!(out.status.success(), "{}", describe(&out));
    let l = ledger(&f);
    let blocked = l.get("overflow_blocked").and_then(|b| b.as_object());
    assert!(blocked.is_none_or(|b| !b.contains_key("X")), "{l}");
}
