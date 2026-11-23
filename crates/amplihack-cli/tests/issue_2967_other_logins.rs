//! Issue #2967: a step whose login reaches its usage limit continues on
//! another login the candidate rule allows, whether or not it has waited,
//! and waits only when none can run it. That wait also ends at the earliest
//! end of a limit block the rotate ledger records on another login the step
//! has run on, and the retry runs on that login. The wait line names the
//! step's own login, and a reset stated with no zone is named as the cause
//! of a wait rather than left silent. The waits themselves are the ones
//! #2911 made: no budget, no step timeout, and an ending only on an event.
//!
//! Cases (see `docs/reference/usage-limit-waits.md`, "Other logins"):
//!
//! - A: another login under the threshold takes the step with no wait.
//! - A3: the wait line names the login (`account=X`, not `unknown`).
//! - B: a reset stated with no zone is not read, and the wait says so
//!   (`reason=reset_without_zone`).
//! - E1: with no login left to move to, the wait ends at the earliest end of
//!   a block recorded on another login the step ran on, when that comes
//!   first, and the retry runs on that login. A wait with no deadline of its
//!   own takes one from there.
//! - C: a limit that arrives after the step has waited moves to a login that
//!   has room by then, instead of waiting again.
//! - T: two logins that each report a limit, on a host with no rotate
//!   ledger: the step moves once, then waits on its own reset, and does not
//!   cycle between them.
//!
//! Every case uses fake logins and the fixture's virtual clock (`TEST_NOW`),
//! which moves only while a wait blocks, and never real time. Readings and
//! blocks are stamped on the real clock; the wait moves an instant onto its
//! own clock by distance, so a test asserts a range for those, never an
//! exact instant. See `issue_1618_fixture`.

#![cfg(unix)]

use crate::issue_1618_fixture::{
    Fixture, LIMIT_TEXT, TEST_NOW, WAIT_PREFIX, describe, events, executable, private_file, text,
    wait_lines,
};
use crate::issue_1618_wait_profiles::{switch_on, with_profiles};
use crate::issue_1801_capacity_wait::{real_now, x_is_live};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};

const TOGGLE_ENV: &str = "AMPLIHACK_CLAUDE_AUTO_SWITCH";
const RECIPE_LOG_ENV: &str = "AMPLIHACK_RECIPE_LOG";

/// The wording seen on the host on 21 November 2026: the session window
/// named "5-hour limit", and its reset given as a bare clock time with no
/// zone, which is never read.
const SPEND_ZONELESS: &str = "You've hit your individual spend limit · run /usage-credits to ask your admin for a higher limit · your 5-hour limit resets 9:30am";

// --- helpers -----------------------------------------------------------------

fn private_dir(path: &Path) {
    fs::create_dir_all(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

/// One login's stored reading. `*_in` are seconds from the real clock.
struct Login {
    name: &'static str,
    session: f64,
    session_resets_in: i64,
    week: f64,
    week_resets_in: i64,
}

impl Login {
    /// Plenty of room: session 10%, week 5%.
    fn room(name: &'static str) -> Self {
        Self {
            name,
            session: 10.0,
            session_resets_in: 3600,
            week: 5.0,
            week_resets_in: 86_400,
        }
    }

    /// Session window at its limit, resetting `resets_in` seconds from now.
    fn out(name: &'static str, resets_in: i64) -> Self {
        Self {
            session: 100.0,
            session_resets_in: resets_in,
            ..Self::room(name)
        }
    }
}

/// The auto-switch readings file body for `logins`, stamped on the real
/// clock.
fn readings_body(logins: &[Login]) -> String {
    let at = real_now();
    let mut accounts = serde_json::Map::new();
    for l in logins {
        accounts.insert(
            l.name.to_string(),
            serde_json::json!({
                "five_hour": {"used_percentage": l.session, "resets_at": at + l.session_resets_in},
                "seven_day": {"used_percentage": l.week, "resets_at": at + l.week_resets_in},
                "observed_at": at,
                "source": "statusline",
            }),
        );
    }
    serde_json::json!({"version": 1, "accounts": accounts}).to_string()
}

fn usage_file(f: &Fixture) -> std::path::PathBuf {
    f.profiles.join(".auto-switch.d").join("usage.json")
}

fn write_readings(f: &Fixture, logins: &[Login]) {
    private_dir(&f.profiles.join(".auto-switch.d"));
    let tmp = usage_file(f).with_extension("tmp");
    private_file(&tmp, &readings_body(logins));
    fs::rename(&tmp, usage_file(f)).unwrap();
}

/// A step that inherited profile `X`, which alone is limited, once.
fn nested_x(f: &Fixture, message: &str, auto_switch: bool) -> Command {
    let mut c = f.step();
    c.env("AMPLIHACK_CLAUDE_PROFILE", "X")
        .env("CLAUDE_CONFIG_DIR", f.profiles.join("X"))
        .env(TOGGLE_ENV, if auto_switch { "1" } else { "0" })
        .env("LIMIT_PROFILE", "X")
        .env("LIMIT_CALLS", "1")
        .env("LIMIT_MESSAGE", message);
    c
}

fn recipe_log(f: &Fixture) -> std::path::PathBuf {
    let log = f.root.join("recipe-logs").join("run.log");
    private_file(&log, "");
    log
}

fn log_lines(log: &Path) -> Vec<String> {
    fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.strip_prefix(WAIT_PREFIX))
        .map(str::to_string)
        .collect()
}

fn started(out: &Output) -> bool {
    wait_lines(out).iter().any(|l| l.starts_with("start "))
}

/// The call's login and the directory it ran in.
fn ran_on(f: &Fixture, n: u32) -> (String, String) {
    let call = f.call("0", n);
    (call["PROFILE"].clone(), call["CLAUDE_CONFIG_DIR"].clone())
}

fn assert_ran_on_y(f: &Fixture, n: u32) {
    let (profile, dir) = ran_on(f, n);
    assert_eq!(profile, "Y", "call {n}");
    assert_eq!(dir, f.profiles.join("Y").display().to_string(), "call {n}");
}

/// `key=` value of a `start` line: the text up to the next ` <next>=`.
fn between<'a>(line: &'a str, key: &str, next: &str) -> &'a str {
    let from = line
        .find(&format!("{key}="))
        .unwrap_or_else(|| panic!("no {key}= in {line:?}"))
        + key.len()
        + 1;
    let to = line[from..]
        .find(&format!(" {next}="))
        .unwrap_or_else(|| panic!("no {next}= after {key}= in {line:?}"));
    &line[from..from + to]
}

fn instant(s: &str) -> i64 {
    chrono::DateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S %:z")
        .unwrap_or_else(|e| panic!("{s:?} is not a wait-line time: {e}"))
        .timestamp()
}

// --- A: another login is under the threshold, so there is no wait ----------------

#[test]
fn a_another_login_under_the_threshold_takes_the_step_with_no_wait() {
    let f = with_profiles();
    switch_on(&f);
    write_readings(&f, &[Login::room("Y")]);
    let out = f.run(&mut nested_x(&f, LIMIT_TEXT, true));
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2, "{}", describe(&out));
    assert_eq!(f.call("0", 1)["PROFILE"], "X");
    assert_ran_on_y(&f, 2);
    assert!(!started(&out), "no wait: {:?}", wait_lines(&out));
    f.assert_isolated("0");
}

#[test]
fn a_a_login_at_the_threshold_is_not_taken_and_the_step_waits() {
    // One rule decides: the auto-switch threshold (90). Y at 92% is over
    // it, so the step waits, and Y's reset (an hour on) is later than X's
    // own, so X's reset times the wait.
    let f = with_profiles();
    switch_on(&f);
    write_readings(
        &f,
        &[Login {
            session: 92.0,
            ..Login::room("Y")
        }],
    );
    let out = f.run(&mut nested_x(&f, LIMIT_TEXT, true));
    assert!(out.status.success(), "{}", describe(&out));
    assert!(started(&out), "{}", describe(&out));
    assert_eq!(f.call("0", 2)["PROFILE"], "X", "the retry stays on X");
}

// --- A3: the wait names the login ------------------------------------------

#[test]
fn a3_a_profile_step_with_auto_switch_off_waits_for_its_own_reset_and_names_its_login() {
    // D2: nothing may move the step. The wait is for its own reset, and the
    // line says `account=X`, where it used to say `account=unknown`.
    let f = with_profiles();
    write_readings(&f, &[Login::room("Y")]);
    let log = recipe_log(&f);
    let mut c = nested_x(&f, LIMIT_TEXT, false);
    c.env(RECIPE_LOG_ENV, &log);
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2);
    assert_eq!(f.call("0", 2)["PROFILE"], "X", "the retry stays on X");
    let lines = log_lines(&log);
    assert!(
        lines[0].starts_with("start reason=usage_limit account=X reset="),
        "{lines:?}"
    );
    assert!(
        !lines.iter().any(|l| l.contains("account=unknown")),
        "{lines:?}"
    );
    // The stderr form keeps `harness=` and names no account.
    assert!(
        events(&out)[0].starts_with("start reason=usage_limit harness=claude reset="),
        "{:?}",
        events(&out)
    );
}

#[test]
fn a3_the_account_in_the_log_is_a_plain_profile_name() {
    let f = with_profiles();
    let log = recipe_log(&f);
    let mut c = nested_x(&f, LIMIT_TEXT, false);
    c.env(RECIPE_LOG_ENV, &log);
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    let profiles = f.profiles.display().to_string();
    for l in log_lines(&log) {
        for word in ["@", "uuid-", "SENTINEL", profiles.as_str()] {
            assert!(!l.contains(word), "{word:?} in {l:?}");
        }
    }
}

// --- B: a reset stated with no zone is named, not silent ------------------------

#[test]
fn b_a_reset_with_no_zone_is_not_read_and_the_wait_says_why() {
    // No zone is guessed, so the reset is unread. The wait names the cause
    // on stderr and in the recipe log, and is a capacity wait: a spend limit
    // whose window reset could not be read is not one that states no reset,
    // so it does not take the login-change wait that only a login change
    // ends. Here nothing else can end it, and the login change does.
    let f = with_profiles();
    let log = recipe_log(&f);
    let mut c = nested_x(&f, SPEND_ZONELESS, false);
    c.env(RECIPE_LOG_ENV, &log);
    let out = f.run_with_a_login_change_in(&f.profiles.join("X"), &mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2, "one retry: {}", describe(&out));
    let ev = events(&out);
    assert_eq!(
        ev[0], "waiting_for_capacity reason=reset_without_zone harness=claude until=none",
        "{ev:?}"
    );
    assert!(
        ev[1].starts_with(
            "start reason=usage_limit wait=capacity harness=claude reset=none until=none "
        ),
        "{ev:?}"
    );
    assert_eq!(ev[2..], ["end reason=login changed", "resume attempt=2"]);
    let lines = log_lines(&log);
    assert_eq!(
        lines[0], "waiting_for_capacity reason=reset_without_zone account=X until=none",
        "{lines:?}"
    );
}

// --- E1: no login left, so the earliest reset across logins -------------------

/// Y's limit, resetting at 08:15, fifteen minutes before X's own 08:30.
const Y_SOONER: &str = "You've hit your session limit · resets 8:15am (America/Los_Angeles)";

/// Y's limit, resetting at 09:30, an hour after X's own.
const Y_LATER: &str = "You've hit your session limit · resets 9:30am (America/Los_Angeles)";

/// A step that inherited X on a host whose rotator has run (so a limit
/// records a block), with Y under the threshold: X meets `x_message`, the
/// step moves to Y, Y meets `y_message`, and no login is left to move to.
fn x_then_y(f: &Fixture, x_message: &str, y_message: &str) -> Command {
    switch_on(f);
    x_is_live(f);
    write_readings(f, &[Login::room("Y")]);
    let mut c = nested_x(f, x_message, true);
    c.env("LIMIT_PROFILE", "")
        .env("LIMIT_CALLS", "2")
        .env("LIMIT_MESSAGE_Y", y_message);
    c
}

#[test]
fn e1_with_no_login_left_the_wait_ends_at_the_earliest_reset_across_logins() {
    // X's own reset is 08:30. Y's limit recorded a block until 08:15, so that
    // is when the step can run again, on Y, and not 08:32 on X. `reset=`
    // keeps the reset X's own message stated; `until=` and `bound=` say
    // where the deadline came from.
    let f = with_profiles();
    let out = f.run(&mut x_then_y(&f, LIMIT_TEXT, Y_SOONER));
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 3, "X, Y, then Y again: {}", describe(&out));
    assert_eq!(f.call("0", 1)["PROFILE"], "X");
    assert_ran_on_y(&f, 2);
    let ev = events(&out);
    assert_eq!(ev.len(), 3, "{ev:?}");
    let start = &ev[0];
    assert!(
        start.starts_with(
            "start reason=usage_limit harness=claude reset=2026-10-18 08:30:00 -07:00 "
        ),
        "{start}"
    );
    assert!(start.ends_with(" bound=earliest_reset+margin"), "{start}");
    let until = instant(between(start, "until", "bound"));
    assert!(
        (TEST_NOW + 900 + 120 - 5..=TEST_NOW + 900 + 120).contains(&until),
        "until={until} is Y's block end plus the margin, not X's 08:32: {start}"
    );
    assert_eq!(ev[1..], ["end reason=reset reached", "resume attempt=3"]);
    assert!(
        text(&out.stderr).contains(
            "the earliest recorded reset among this step's logins has passed; rerunning this \
             step under profile \"Y\" (auto-switch)"
        ),
        "{}",
        describe(&out)
    );
    assert_ran_on_y(&f, 3);
}

#[test]
fn e1_a_wait_with_no_deadline_of_its_own_takes_the_block_end() {
    // X's message states no reset that parses, so its own wait would have
    // no deadline (#2911). Y's block ends at 08:15: that is the deadline,
    // and the retry runs on Y.
    let f = with_profiles();
    let out = f.run(&mut x_then_y(
        &f,
        "You've hit your session limit · resets later",
        Y_SOONER,
    ));
    assert!(out.status.success(), "{}", describe(&out));
    let ev = events(&out);
    assert_eq!(ev.len(), 4, "{ev:?}");
    assert!(
        ev[0].starts_with("waiting_for_capacity reason=no_reset harness=claude until=2"),
        "a deadline, not until=none: {ev:?}"
    );
    let start = &ev[1];
    assert!(
        start.starts_with(
            "start reason=usage_limit wait=capacity harness=claude reset=none until=2"
        ),
        "{start}"
    );
    assert!(start.ends_with(" bound=earliest_reset+margin"), "{start}");
    assert_eq!(ev[2..], ["end reason=reset reached", "resume attempt=3"]);
    assert_ran_on_y(&f, 3);
}

#[test]
fn e1_a_later_block_on_another_login_leaves_the_wait_on_its_own_reset() {
    // Y's block ends an hour after X's reset: X's own reset times the wait,
    // as it did, and the retry runs on X.
    let f = with_profiles();
    let out = f.run(&mut x_then_y(&f, LIMIT_TEXT, Y_LATER));
    assert!(out.status.success(), "{}", describe(&out));
    let start = &events(&out)[0];
    assert!(start.ends_with(" bound=reset+margin"), "{start}");
    assert!(
        start.contains("reset=2026-10-18 08:30:00 -07:00"),
        "the message's reset: {start}"
    );
    assert_eq!(f.call("0", 3)["PROFILE"], "X");
}

// --- C: a limit after the wait was used ----------------------------------------

/// A `claude` that makes login Y usable just before the second call, then
/// runs the fixture's stub: Y is out at the first limit (so the step waits)
/// and has room by the second.
const HEALING_CLAUDE: &str = r#"#!/bin/sh
case "$1" in --version) exec "$REAL_CLAUDE" "$@" ;; esac
n=$(cat "$RECORD_DIR/count.${STEP:-0}" 2>/dev/null || echo 0)
if [ "$n" = "1" ] && [ ! -e "$RECORD_DIR/healed" ]; then
  : > "$RECORD_DIR/healed"
  cp "$HEAL_FROM" "$HEAL_TO.tmp" && mv "$HEAL_TO.tmp" "$HEAL_TO"
fi
exec "$REAL_CLAUDE" "$@"
"#;

fn healing_claude(f: &Fixture) -> std::path::PathBuf {
    let dir = f.root.join("wrapper");
    fs::create_dir_all(&dir).unwrap();
    let wrapper = dir.join("claude");
    executable(&wrapper, HEALING_CLAUDE);
    wrapper
}

#[test]
fn c_a_limit_after_the_wait_moves_to_a_login_that_is_usable_by_then() {
    // Before #2967 the step's wait set `used`, and `next_profile` returned
    // at once for every later limit: the step waited again on X with Y
    // usable.
    let f = with_profiles();
    switch_on(&f);
    write_readings(&f, &[Login::out("Y", 7200)]);
    let usable = f.root.join("usable.json");
    private_file(&usable, &readings_body(&[Login::room("Y")]));
    let wrapper = healing_claude(&f);
    let mut c = nested_x(&f, LIMIT_TEXT, true);
    c.env("LIMIT_CALLS", "2")
        .env("AMPLIHACK_CLAUDE_BINARY_PATH", &wrapper)
        .env("REAL_CLAUDE", f.tools.join("claude"))
        .env("HEAL_FROM", &usable)
        .env("HEAL_TO", usage_file(&f));
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        f.count("0"),
        3,
        "X, X again after the wait, then Y: {}",
        describe(&out)
    );
    assert_eq!(f.call("0", 1)["PROFILE"], "X");
    assert_eq!(f.call("0", 2)["PROFILE"], "X", "the retry the wait earned");
    assert_ran_on_y(&f, 3);
    let starts = events(&out)
        .into_iter()
        .filter(|l| l.starts_with("start "))
        .count();
    assert_eq!(
        starts,
        1,
        "the move needs no second wait: {:?}",
        events(&out)
    );
}

// --- T: no cycle between logins ------------------------------------------------

#[test]
fn t_two_logins_that_each_report_a_limit_move_once_then_wait() {
    // X and Y each report a limit. The step moves from X to Y once; Y is
    // then in the tried set with X, so no move is left and the step waits
    // on X's own reset, then retries on X. It never moves back and forth:
    // each move adds a login to the tried set. This host's rotator has never
    // run, so no block is recorded and nothing moves the deadline.
    let f = with_profiles();
    switch_on(&f);
    let mut c = nested_x(&f, LIMIT_TEXT, true);
    c.env("LIMIT_PROFILE", "").env("LIMIT_CALLS", "2");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        f.count("0"),
        3,
        "X, Y, then X after the wait: {}",
        describe(&out)
    );
    assert_eq!(f.call("0", 1)["PROFILE"], "X");
    assert_ran_on_y(&f, 2);
    assert_eq!(f.call("0", 3)["PROFILE"], "X", "the retry the wait earned");
    let ev = events(&out);
    assert_eq!(ev.len(), 3, "one wait and nothing else: {ev:?}");
    assert!(
        ev[0].starts_with("start reason=usage_limit harness=claude reset="),
        "{ev:?}"
    );
    assert_eq!(ev[1..], ["end reason=reset reached", "resume attempt=3"]);
}
