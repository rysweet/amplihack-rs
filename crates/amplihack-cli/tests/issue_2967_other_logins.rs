//! Issue #2967: a step whose login reaches its usage limit continues on
//! another login the candidate rule allows, whether or not it has waited,
//! and waits only when none can run it. That wait also ends at the earliest
//! end of a limit block the rotate ledger records on any other login the
//! step could move to, whether or not it has run there, and the retry runs
//! on that login. The wait line names the step's own login. The waits
//! themselves are the ones #2911 made: no budget, no step timeout, and an
//! ending only on an event.
//!
//! Cases (see `docs/reference/usage-limit-waits.md`, "Other logins"):
//!
//! - A: another login under the threshold takes the step with no wait.
//! - A3: the wait line names the login (`account=X`, not `unknown`).
//! - E1: with no login left to move to, the wait ends at the earliest end of
//!   a block recorded on another login the step ran on, when that comes
//!   first, and the retry runs on that login. A wait with no deadline of its
//!   own takes one from there.
//! - E2: the incident's shape. The step has run on one login only, and
//!   several logins it never ran on are blocked: the wait ends at the
//!   earliest of their block ends, and the retry runs there. A login the
//!   candidate rule would never take (excluded, or with no login of its
//!   own) is passed over, and with auto-switch off no other login's block
//!   ends the wait.
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

// --- E2: the incident's shape: one login run on, others blocked -------------

/// Adds a stored login `name` with its own login file and account, as
/// `with_profiles` does for X and Y.
fn add_login(f: &Fixture, name: &str) {
    let out = f.run(&mut f.cmd(&["profile", "add", name]));
    assert!(
        out.status.success(),
        "profile add {name}: {}",
        describe(&out)
    );
    let dir = f.profiles.join(name);
    private_file(
        &dir.join(".credentials.json"),
        &format!(r#"{{"claudeAiOauth":{{"accessToken":"SENTINEL-{name}"}}}}"#),
    );
    private_file(
        &dir.join(".claude.json"),
        &format!(
            r#"{{"oauthAccount":{{"emailAddress":"{}@example.invalid","accountUuid":"uuid-{name}"}}}}"#,
            name.to_lowercase()
        ),
    );
}

/// A rotate ledger naming X as live, with a cycle from before the step, and
/// a standing limit block on each of `blocks`, ending the given number of
/// seconds from the real clock: what the rotator and other steps record.
fn blocks_on(f: &Fixture, blocks: &[(&str, i64)]) {
    let at = real_now();
    let rotate = f.profiles.join(".auto-switch.d").join("rotate");
    private_dir(&f.profiles.join(".auto-switch.d"));
    private_dir(&rotate);
    let blocked: serde_json::Map<String, serde_json::Value> = blocks
        .iter()
        .map(|(name, ends_in)| {
            (
                (*name).to_string(),
                serde_json::json!({"at": at - 60, "until": at + ends_in, "reason": "spend_limit"}),
            )
        })
        .collect();
    let cycle = TEST_NOW - 3600;
    let body = serde_json::json!({
        "version": 1,
        "live": "X",
        "alert": null,
        "last_cycle": {"at": cycle, "measured_at": cycle, "outcome": "swapped", "live_room": true},
        "overflow_blocked": blocked,
    });
    let ledger = rotate.join("ledger.json");
    let tmp = ledger.with_extension("tmp");
    private_file(&tmp, &body.to_string());
    fs::rename(&tmp, &ledger).unwrap();
}

/// The incident's shape: X, Y, Z and W are stored, the step inherited X and
/// has run on nothing else, and Y, Z and W are each at their session limit
/// with a standing block. Z's ends first, 15 minutes on (08:15), before X's
/// own reset at 08:30; Y's at 08:25 and W's at 09:00. Only X is limited.
fn one_login_run_on(f: &Fixture, auto_switch: bool) -> Command {
    add_login(f, "Z");
    add_login(f, "W");
    if auto_switch {
        switch_on(f);
    }
    blocks_on(f, &[("Y", 1500), ("Z", 900), ("W", 3600)]);
    write_readings(
        f,
        &[
            Login::out("Y", 1500),
            Login::out("Z", 900),
            Login::out("W", 3600),
        ],
    );
    nested_x(f, LIMIT_TEXT, auto_switch)
}

/// The deadline of a wait's `start` line, asserted to be `ends_in` seconds
/// after `TEST_NOW` plus the margin. Blocks are stamped on the real clock and
/// moved onto the wait's by distance, so a few seconds may be lost.
fn assert_ends_at_block(start: &str, ends_in: i64) {
    assert!(start.ends_with(" bound=earliest_reset+margin"), "{start}");
    let until = instant(between(start, "until", "bound"));
    let want = TEST_NOW + ends_in + 120;
    assert!(
        (want - 5..=want).contains(&until),
        "until={until}, want {want}: {start}"
    );
}

#[test]
fn e2_with_one_login_run_on_the_wait_ends_at_the_earliest_block_among_logins_never_run_on() {
    // Before, only logins the step had run on counted: here that is X alone,
    // so the wait read no block and ran to X's own 08:32. Z, which the step
    // never ran on, is free again at 08:15, so the step runs there.
    let f = with_profiles();
    let out = f.run(&mut one_login_run_on(&f, true));
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2, "X, then Z: {}", describe(&out));
    assert_eq!(f.call("0", 1)["PROFILE"], "X");
    let ev = events(&out);
    assert_eq!(ev.len(), 3, "{ev:?}");
    assert!(
        ev[0].starts_with(
            "start reason=usage_limit harness=claude reset=2026-10-18 08:30:00 -07:00 "
        ),
        "the reset X's own message stated: {ev:?}"
    );
    assert_ends_at_block(&ev[0], 900);
    assert_eq!(ev[1..], ["end reason=reset reached", "resume attempt=2"]);
    assert!(
        text(&out.stderr).contains(
            "the earliest recorded reset among this step's logins has passed; rerunning this \
             step under profile \"Z\" (auto-switch)"
        ),
        "{}",
        describe(&out)
    );
    let (profile, dir) = ran_on(&f, 2);
    assert_eq!(profile, "Z");
    assert_eq!(dir, f.profiles.join("Z").display().to_string());
}

#[test]
fn e2_an_excluded_login_and_one_with_no_login_of_its_own_are_passed_over() {
    // Z's block ends first, but auto-switch excludes Z; Y's comes next, but
    // Y holds no login file, so no move could ever run on it. W's 09:00 is
    // after X's own 08:30, so X's reset times the wait.
    let f = with_profiles();
    let mut c = one_login_run_on(&f, true);
    let out = f.run(&mut f.cmd(&["profile", "auto-switch", "exclude", "Z"]));
    assert!(out.status.success(), "exclude Z: {}", describe(&out));
    fs::remove_file(f.profiles.join("Y").join(".credentials.json")).unwrap();
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    let start = &events(&out)[0];
    assert!(start.ends_with(" bound=reset+margin"), "{start}");
    assert_eq!(f.count("0"), 2, "{}", describe(&out));
    assert_eq!(f.call("0", 2)["PROFILE"], "X", "the retry stays on X");

    // With Y's login back, Y's 08:25 is the earliest block a move could use.
    let f = with_profiles();
    let mut c = one_login_run_on(&f, true);
    let out = f.run(&mut f.cmd(&["profile", "auto-switch", "exclude", "Z"]));
    assert!(out.status.success(), "exclude Z: {}", describe(&out));
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_ends_at_block(&events(&out)[0], 1500);
    assert_ran_on_y(&f, 2);
}

#[test]
fn e2_with_auto_switch_off_no_other_logins_block_ends_the_wait() {
    // The step may not move, so another login's block end would only rerun
    // it on X while X is still limited. X's own reset times the wait.
    let f = with_profiles();
    let out = f.run(&mut one_login_run_on(&f, false));
    assert!(out.status.success(), "{}", describe(&out));
    let start = &events(&out)[0];
    assert!(start.ends_with(" bound=reset+margin"), "{start}");
    assert_eq!(f.count("0"), 2, "{}", describe(&out));
    assert_eq!(f.call("0", 2)["PROFILE"], "X", "the retry stays on X");
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
