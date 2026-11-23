//! Issue #2890, generalised by #2911: a step's limit records the login it
//! ran under as blocked until the reset the message states, as data in the
//! rotate ledger (`overflow_blocked`), and `rotate status` shows it. The
//! emitter is `note_limit_block(root, login, at, until, kind)`, called for
//! every limit kind the supervisor sees; the record itself is unchanged on
//! the wire, `reason=spend_limit` whatever the kind, so a host still running
//! the previous release loads a ledger this writer touched.
//!
//! - A message stating a parsable reset in range writes one block keyed on
//!   the login, `reason=spend_limit`, `until - at` equal to the stated
//!   distance, with one ALERT line in the rotate log.
//! - A spend limit stating no reset falls back to the month boundary the
//!   rotator already assumes for one (#2911); it used to write nothing.
//! - A stated reset out of range is reported on stderr and not stored; the
//!   ledger still loads (`rotate status` exits 0).
//! - A step with no rotate ledger creates none.
//! - Recording never delays, refuses or ends the step. Since #2911 the
//!   limit is waited out and the step's exit code is its retry's, so what
//!   is pinned is that the record changes neither it nor the wait's lines.
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
/// Every wait here is bounded by the `MIN_WAIT_SECS` floor on the fixture's
/// virtual clock, so its deadline is `TEST_NOW + 60 s` exactly. Since #2911
/// a configured `0` reads as that floor rather than as "do not wait".
const FLOORED_UNTIL: &str = "2026-10-18 08:01:00 -07:00";

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

/// The month boundary a spend limit that states no reset is blocked until:
/// the 1st of the next calendar month, 00:00 UTC, which is the assumption
/// the rotator has always made for one. `rotate::spend_blocks::spend_reset`
/// is the code, and `rotate/tests_step_block_kinds.rs` pins it; this is the
/// same boundary read from the other side.
fn next_month_utc(at: i64) -> i64 {
    use chrono::{Datelike, TimeZone, Utc};
    let t = chrono::DateTime::from_timestamp(at, 0).expect("the recorded instant");
    let (y, m) = if t.month() == 12 {
        (t.year() + 1, 1)
    } else {
        (t.year(), t.month() + 1)
    };
    Utc.with_ymd_and_hms(y, m, 1, 0, 0, 0)
        .single()
        .expect("a month boundary")
        .timestamp()
}

/// Issue #2911: a spend limit that states no reset is recorded as well,
/// against the month boundary the rotator already assumes for one. Until
/// #2911 the emitter needed a stated reset and this wrote nothing; only a
/// limit of another kind with no stated reset writes nothing now, because a
/// block with an invented end would keep a healthy login out of rotation.
///
/// The step itself keeps #2731: this is no capacity wait but a wait for the
/// login in `$HOME/.claude` to change.
#[test]
fn a_spend_limit_with_no_reset_blocks_until_the_monthly_reset() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    let out = f.run(&mut home_step(&f, SPEND_TEXT));
    let l = ledger(&f);
    let b = &l["overflow_blocked"]["X"];
    assert!(b.is_object(), "no block for X: {l}\n{}", describe(&out));
    let mut keys: Vec<&str> = b.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        ["at", "reason", "until"],
        "the record an older reader must still load: {b}"
    );
    assert_eq!(b["reason"], "spend_limit");
    let (at, until) = (b["at"].as_i64().unwrap(), b["until"].as_i64().unwrap());
    assert_eq!(
        until,
        next_month_utc(at),
        "the monthly reset assumed for it"
    );
    assert_eq!(
        rotate_log(&f)
            .matches("ALERT: login X hit its spend limit; blocked until ")
            .count(),
        1,
        "{}",
        rotate_log(&f)
    );
    let ev = events(&out);
    let why =
        format!("waiting_for_capacity reason=spend_limit harness=claude until={FLOORED_UNTIL}");
    assert_eq!(ev.first(), Some(&why), "{ev:?}: {}", describe(&out));
    assert!(
        ev.get(1)
            .is_some_and(|l| l.starts_with("start reason=spend_limit wait=login_change")),
        "{ev:?}: {}",
        describe(&out)
    );
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
/// together by a check ("only wait if the block was stored") and they are
/// deliberately not. `note_limit_block` returns `()` so that no caller can
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
    // Since #2911 the wait says why it is waiting for capacity, and a reset
    // a year away is `reset_too_far`: the word that used to end such a run.
    // The wait still ends at that reset, held to the 62-day span a spend
    // limit's record is held to.
    let why = "waiting_for_capacity reason=reset_too_far harness=claude \
               until=2026-12-19 07:02:00 -08:00"
        .to_string();
    assert_eq!(ev.first(), Some(&why), "{ev:?}: {}", describe(&out));
    assert!(
        ev.get(1)
            .is_some_and(|l| l.starts_with("start reason=usage_limit wait=capacity harness=claude")),
        "the wait is planned whatever happened to the record: {ev:?}: {}",
        describe(&out)
    );
    assert!(
        !ev.iter()
            .any(|l| l.starts_with("skipped ") || l.starts_with("failed ")),
        "a refused record never turns the wait back into a skip: {ev:?}"
    );
}

#[test]
fn a_step_with_no_rotate_ledger_creates_none() {
    let f = with_profiles();
    let out = f.run(&mut home_step(&f, SPEND_FAR));
    assert!(!ledger_path(&f).exists(), "{}", describe(&out));
}

/// The emitter cannot delay, refuse or end a step, and since #2911 there is
/// no terminal path left to read that off: the limit is waited out and the
/// step's exit code is its retry's. So the invariant is pinned by
/// comparison. The same step, once on a root holding a rotate ledger to
/// write a block into and once on a root holding none, ends with the same
/// code and writes the same wait lines.
///
/// It used to assert `Some(1)`, the code the limited attempt returned when
/// the supervisor skipped the wait instead of starting one.
#[test]
fn recording_the_block_does_not_change_the_steps_exit_code() {
    let recorded = with_profiles();
    x_is_live(&recorded);
    let with = recorded.run(&mut home_step(&recorded, SPEND_FAR));
    assert!(
        ledger(&recorded)["overflow_blocked"]["X"].is_object(),
        "the block was written, so the comparison means something: {}",
        describe(&with)
    );
    let plain = with_profiles();
    let without = plain.run(&mut home_step(&plain, SPEND_FAR));
    assert!(!ledger_path(&plain).exists(), "{}", describe(&without));
    assert_eq!(
        with.status.code(),
        without.status.code(),
        "{}\n{}",
        describe(&with),
        describe(&without)
    );
    assert_eq!(
        events(&with),
        events(&without),
        "{}\n{}",
        describe(&with),
        describe(&without)
    );
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
