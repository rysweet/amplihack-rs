//! Issue #2967: a step whose login reaches its usage limit continues on
//! another login the candidate rule allows, waits only when none can run it,
//! and then also ends its wait at the earliest moment another login comes
//! back under the auto-switch threshold. The wait line names the step's own
//! login, and a reset stated with no zone is named as the cause of a wait
//! rather than left silent. The waits themselves are the ones #2911 made: no
//! budget, no step timeout, and an ending only on an event.
//!
//! Cases (see `docs/reference/usage-limit-waits.md`, "Other logins"):
//!
//! - A: another login under the threshold takes the step with no wait.
//! - A3: the wait line names the login (`account=X`, not `unknown`), with
//!   auto-switch off (a setting the user chose: the wait is for the own
//!   reset).
//! - B: a reset stated with no zone ("... your 5-hour limit resets 9:30am")
//!   is not read. The wait says so (`reason=reset_without_zone`) and is a
//!   capacity wait, which another login's reset ends when there is one.
//! - E1: with no login under the threshold, the wait ends at the earliest
//!   reset across the other logins when that is first, and the retry runs
//!   on the login that reset. A wait with no deadline of its own takes one
//!   from there.
//! - C: a limit that arrives after the step has waited moves to a login that
//!   has room by then, instead of waiting again.
//! - T: two logins that each report a limit: the step moves once, then
//!   waits, and does not cycle between them.
//! - U: the other logins cannot be considered (here, auto-switch turned
//!   off while the step ran). The step still waits for its own reset, as it
//!   must, but says why on stderr and in the recipe log before the wait's
//!   `start`, so the quiet wait of #2967 cannot recur unexplained. An
//!   unreadable ledger or readings file reaches the same lines (unit tests in
//!   `auto_switch/tests_rerun_unread.rs` and `tests_limit_choice.rs`). A
//!   ledger unreadable from the start fails the launch before any attempt
//!   runs, so only these tests can reach that case.
//!
//! Every case uses fake logins and the fixture's virtual clock (`TEST_NOW`),
//! which moves only while a wait blocks, and never real time. Readings are
//! stamped on the real clock, because the pre-wait choice reads
//! `fsx::now()`; the wait moves a reset onto its own clock by distance, so a
//! test asserts a range for those, never an exact instant. See
//! `issue_1618_fixture`.

#![cfg(unix)]

use crate::issue_1618_fixture::{
    Fixture, LIMIT_TEXT, TEST_NOW, WAIT_PREFIX, describe, events, executable, private_file, text,
    wait_lines,
};
use crate::issue_1618_wait_profiles::{switch_on, with_profiles};
use crate::issue_1801_capacity_wait::real_now;
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

#[test]
fn b_a_reset_with_no_zone_takes_the_earliest_reset_of_another_login() {
    // The same message with auto-switch on and Y out for about 15 minutes:
    // the wait is not left without a deadline; Y's reset ends it and the
    // retry runs on Y.
    let f = with_profiles();
    switch_on(&f);
    write_readings(&f, &[Login::out("Y", 900)]);
    let out = f.run(&mut nested_x(&f, SPEND_ZONELESS, true));
    assert!(out.status.success(), "{}", describe(&out));
    let ev = events(&out);
    assert!(
        ev[0].starts_with("waiting_for_capacity reason=reset_without_zone harness=claude until=2"),
        "{ev:?}"
    );
    assert!(ev[1].ends_with(" bound=earliest_reset+margin"), "{ev:?}");
    assert_eq!(ev[2..], ["end reason=reset reached", "resume attempt=2"]);
    assert_ran_on_y(&f, 2);
}

// --- E1: no login usable, so the earliest reset across logins -------------------

#[test]
fn e1_with_no_login_usable_the_wait_ends_at_the_earliest_reset_across_logins() {
    // X's own reset is 08:30. Y is out until about 08:15, so that is when
    // the step can run again, on Y, and not 08:32 on X. `reset=` keeps the
    // reset X's own message stated; `until=` and `bound=` say where the
    // deadline came from.
    let f = with_profiles();
    switch_on(&f);
    write_readings(&f, &[Login::out("Y", 900)]);
    let out = f.run(&mut nested_x(&f, LIMIT_TEXT, true));
    assert!(out.status.success(), "{}", describe(&out));
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
        (TEST_NOW + 600 + 120..=TEST_NOW + 900 + 120).contains(&until),
        "until={until} is Y's reset plus the margin, not X's 08:32: {start}"
    );
    assert_eq!(ev[1..], ["end reason=reset reached", "resume attempt=2"]);
    assert!(
        text(&out.stderr).contains(
            "the earliest reset among this step's logins has arrived; rerunning this step under profile \"Y\" (auto-switch)"
        ),
        "{}",
        describe(&out)
    );
    assert_ran_on_y(&f, 2);
}

#[test]
fn e1_a_wait_with_no_deadline_takes_the_other_logins_reset_however_far() {
    // X's message states no reset that parses, so its own wait would have
    // no deadline (#2911). Y resets six hours on: there is no five-hour
    // ceiling, so that is the deadline, and the retry runs on Y.
    let f = with_profiles();
    switch_on(&f);
    write_readings(&f, &[Login::out("Y", 6 * 3600)]);
    let out = f.run(&mut nested_x(
        &f,
        "You've hit your session limit · resets later",
        true,
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
    let until = instant(between(start, "until", "bound"));
    assert!(
        (TEST_NOW + 6 * 3600 - 300 + 120..=TEST_NOW + 6 * 3600 + 120).contains(&until),
        "until={until}: {start}"
    );
    assert_eq!(ev[2..], ["end reason=reset reached", "resume attempt=2"]);
    assert_ran_on_y(&f, 2);
}

#[test]
fn e1_a_later_reset_on_another_login_leaves_the_wait_on_its_own_reset() {
    // Y resets a day on, after X: X's own reset times the wait, as it did.
    let f = with_profiles();
    switch_on(&f);
    write_readings(&f, &[Login::out("Y", 86_400)]);
    let out = f.run(&mut nested_x(&f, LIMIT_TEXT, true));
    assert!(out.status.success(), "{}", describe(&out));
    let start = &events(&out)[0];
    assert!(start.ends_with(" bound=reset+margin"), "{start}");
    assert_eq!(f.call("0", 2)["PROFILE"], "X");
}

#[test]
fn e1_the_failed_logins_own_earlier_reset_stays_a_plain_reset_wait() {
    // Y resets after X: nothing changes.
    let f = with_profiles();
    switch_on(&f);
    write_readings(&f, &[Login::out("Y", 3500)]);
    let out = f.run(&mut nested_x(&f, LIMIT_TEXT, true));
    let start = &events(&out)[0];
    assert!(start.ends_with(" bound=reset+margin"), "{start}");
    assert!(
        start.contains("reset=2026-10-18 08:30:00 -07:00"),
        "the message's reset: {start}"
    );
    assert_eq!(f.call("0", 2)["PROFILE"], "X");
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
    // each move adds a login to the tried set.
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

// --- U: the other logins cannot be considered -------------------------------------

/// A `claude` that turns auto-switch off during the step's first attempt,
/// by putting a prepared `off` toggle file in place, then runs the fixture's
/// stub. The supervisor reads the setting again when the limit arrives.
const SWITCH_OFF_CLAUDE: &str = r#"#!/bin/sh
case "$1" in --version) exec "$REAL_CLAUDE" "$@" ;; esac
n=$(cat "$RECORD_DIR/count.${STEP:-0}" 2>/dev/null || echo 0)
if [ "$n" = "0" ]; then
  cp "$OFF_FROM" "$OFF_TO.tmp" && mv "$OFF_TO.tmp" "$OFF_TO"
fi
exec "$REAL_CLAUDE" "$@"
"#;

#[test]
fn u_auto_switch_turned_off_during_the_step_is_named_before_the_wait() {
    // Y is out for about 15 minutes, so with auto-switch on its reset ends
    // the wait (E1). Auto-switch is turned off while the first attempt runs:
    // the step then waits for X's own reset, with no retry and no gate
    // added, and both logs say why. Before, the lines here were the same as
    // for a step with no other login at all.
    let f = with_profiles();
    let toggle = f.profiles.join(".auto-switch");
    let out = f.run(&mut f.cmd(&["profile", "auto-switch", "off"]));
    assert!(out.status.success(), "auto-switch off: {}", describe(&out));
    let off = f.root.join("auto-switch.off");
    private_file(&off, &fs::read_to_string(&toggle).unwrap());
    switch_on(&f);
    write_readings(&f, &[Login::out("Y", 900)]);
    let dir = f.root.join("switch-off-wrapper");
    fs::create_dir_all(&dir).unwrap();
    let wrapper = dir.join("claude");
    executable(&wrapper, SWITCH_OFF_CLAUDE);
    let log = recipe_log(&f);
    let mut c = nested_x(&f, LIMIT_TEXT, true);
    // The setting comes from the file alone, so turning it off there counts.
    c.env_remove(TOGGLE_ENV)
        .env(RECIPE_LOG_ENV, &log)
        .env("AMPLIHACK_CLAUDE_BINARY_PATH", &wrapper)
        .env("REAL_CLAUDE", f.tools.join("claude"))
        .env("OFF_FROM", &off)
        .env("OFF_TO", &toggle);
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2, "{}", describe(&out));
    assert_eq!(f.call("0", 2)["PROFILE"], "X", "the retry stays on X");

    assert!(
        text(&out.stderr).contains(
            "amplihack: this step's other logins were not read for its wait \
             (reason=auto_switch_off): auto-switch was turned off while this step ran; \
             no other login's reset can end this wait early (auto-switch)"
        ),
        "{}",
        describe(&out)
    );
    let ev = events(&out);
    assert_eq!(
        ev[0], "other_logins_unread reason=auto_switch_off",
        "{ev:?}"
    );
    assert!(
        ev[1].starts_with(
            "start reason=usage_limit harness=claude reset=2026-10-18 08:30:00 -07:00 "
        ),
        "{ev:?}"
    );
    assert!(
        ev[1].ends_with(" bound=reset+margin"),
        "X's own reset, not Y's: {ev:?}"
    );
    assert_eq!(ev[2..], ["end reason=reset reached", "resume attempt=2"]);

    let lines = log_lines(&log);
    assert_eq!(
        lines[0], "other_logins_unread reason=auto_switch_off account=X",
        "{lines:?}"
    );
    assert!(
        lines[1].starts_with("start reason=usage_limit account=X "),
        "{lines:?}"
    );
}
