//! Issue #2967: a step whose login reaches its usage limit continues on
//! another usable login, waits only when none is usable, and then also ends
//! its wait at the earliest reset across the other logins; it recognises the
//! zone-less spend-limit wording, names its login in the wait line, and does
//! not say `working` while it waits. The waits themselves are the ones #2911
//! made: no budget, no step timeout, and an ending only on an event.
//!
//! Cases (see `docs/reference/usage-limit-waits.md`, "Other logins"):
//!
//! - A1, A2: the observed run. The step's own login is out and another is
//!   usable, yet the run waited for its own reset. A1 is a `$HOME/.claude`
//!   step with no rotator and auto-switch on; A2 is a profile step whose
//!   other login is over the 90% candidate threshold but under the 95%
//!   "usable now" bound, so only the second capacity rule picks it.
//! - A3, A4: the wait line names the login (`account=X`, not `unknown`),
//!   with auto-switch off (a setting the user chose: the wait is for the own
//!   reset) on a profile step and on a `$HOME/.claude` step.
//! - B1, B2, B3: the second observed wording, "You've hit your individual
//!   spend limit ... your 5-hour limit resets 11:50pm", with no zone, in both
//!   middle-clause forms. B1: its reset is read and times a reset wait. B2:
//!   another usable login takes the step with no wait. B3: a `TZ` that names
//!   no zone leaves the reset unread, so it is a spend limit with no reset,
//!   which waits for the login to change (#2731, #2911).
//! - E1: with no login usable, the wait ends at the earliest reset across
//!   the other logins when that is first, and the retry runs on the login
//!   that reset. A wait with no deadline of its own takes one from there.
//! - C: a limit that arrives after the step has waited moves to a login that
//!   is usable by then, instead of waiting again.
//! - T: two logins that each report a limit: the step moves once, then
//!   waits, and does not cycle between them.
//! - H: while a step waits, the runner's heartbeat for it reads
//!   `waiting for usage limit until <until>`, and is untouched outside it.
//!
//! Every case uses fake logins, the fixture's virtual clock (`TEST_NOW`),
//! which moves only while a wait blocks, and never real time, except H. H
//! needs a wait that lasts while the heartbeat is printed and a record the
//! pump believes, so it runs on the scaled real clock, and the test itself
//! ends the wait with a login change once it has seen the rewritten lines.
//! Readings are stamped on the real clock, because the pre-wait choice reads
//! `fsx::now()`; the wait moves a reset onto its own clock by distance, so a
//! test asserts a range for those, never an exact instant. See
//! `issue_1618_fixture`.

#![cfg(unix)]

use crate::issue_1618_fixture::{
    Fixture, LIMIT_TEXT, TEST_NOW, WAIT_PREFIX, claude_stdout, describe, events, executable,
    private_file, text, toggle_login, wait_lines,
};
use crate::issue_1618_wait_profiles::{switch_on, with_profiles};
use crate::issue_1801_capacity_wait::{real_clock, real_now, rotator_running, x_is_live};
use crate::support::quiet_process::in_quiet_process;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};

const TOGGLE_ENV: &str = "AMPLIHACK_CLAUDE_AUTO_SWITCH";
const RECIPE_LOG_ENV: &str = "AMPLIHACK_RECIPE_LOG";

/// The wording seen on the host on 21 November 2026: no zone, and the
/// session window named "5-hour limit". The elided middle is one of two
/// clauses seen on that host.
const SPEND_ZONELESS_ADMIN: &str = "You've hit your individual spend limit · run /usage-credits to ask your admin for a higher limit · your 5-hour limit resets 9:30am";
const SPEND_ZONELESS_VISIT: &str = "You've hit your individual spend limit · run /usage-credits to raise it, or visit claude.ai/admin-settings/usage · your 5-hour limit resets 9:30am";
/// 09:30 at `TEST_NOW` in `America/Los_Angeles`: 90 minutes on.
const RESET_0930: &str = "2026-10-18 09:30:00 -07:00";
const UNTIL_0932: &str = "2026-10-18 09:32:00 -07:00";

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

    /// Over the candidate threshold (90%) and under the usable bound (95%).
    fn usable_only(name: &'static str) -> Self {
        Self {
            session: 92.0,
            week: 40.0,
            ..Self::room(name)
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

/// A step on `$HOME/.claude`, limited once.
fn home_step(f: &Fixture, message: &str, auto_switch: bool) -> Command {
    fs::create_dir_all(f.home.join(".claude")).unwrap();
    let mut c = f.step();
    c.env_remove("CLAUDE_CONFIG_DIR")
        .env(TOGGLE_ENV, if auto_switch { "1" } else { "0" })
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

// --- A1, A2: another login is usable, so there is no wait ---------------------

#[test]
fn a1_a_home_step_with_no_rotator_moves_to_a_usable_login_with_no_wait() {
    // The observed run: `account=unknown`, a two-hour wait, four logins usable.
    let f = with_profiles();
    switch_on(&f);
    x_is_live(&f);
    write_readings(&f, &[Login::room("Y")]);
    let out = f.run(&mut home_step(&f, LIMIT_TEXT, true));
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2, "{}", describe(&out));
    assert_ran_on_y(&f, 2);
    assert!(!started(&out), "no wait: {:?}", wait_lines(&out));
    assert!(
        text(&out.stderr).contains("rerunning this step under profile \"Y\" (auto-switch)"),
        "{}",
        describe(&out)
    );
    assert_eq!(claude_stdout(&out), "attempt 2 output\n");
    f.assert_isolated("0");
}

#[test]
fn a2_a_login_over_the_threshold_but_usable_takes_a_profile_step_with_no_wait() {
    // Rule 1 (threshold 90) rejects Y at 92%; rule 2 (usable: session < 95,
    // week < 100) accepts it.
    let f = with_profiles();
    switch_on(&f);
    write_readings(&f, &[Login::usable_only("Y")]);
    let out = f.run(&mut nested_x(&f, LIMIT_TEXT, true));
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2, "{}", describe(&out));
    assert_eq!(f.call("0", 1)["PROFILE"], "X");
    assert_ran_on_y(&f, 2);
    assert!(!started(&out), "no wait: {:?}", wait_lines(&out));
    f.assert_isolated("0");
}

#[test]
fn a2_a_login_at_the_usable_bound_is_not_usable() {
    // 95% session is not below 95: rule 2 refuses it, so the step waits.
    let f = with_profiles();
    switch_on(&f);
    write_readings(
        &f,
        &[Login {
            session: 95.0,
            ..Login::room("Y")
        }],
    );
    let out = f.run(&mut nested_x(&f, LIMIT_TEXT, true));
    assert!(out.status.success(), "{}", describe(&out));
    assert!(started(&out), "{}", describe(&out));
    assert_eq!(f.call("0", 2)["PROFILE"], "X", "the retry stays on X");
}

#[test]
fn a_login_with_no_reading_is_not_usable_by_the_second_rule() {
    // Rule 1 still takes a login with no reading (as ac9 of #1618 pins), so
    // this isolates rule 2: Y has a reading showing it out, Z has none and
    // is not a stored login. Only Y exists and it is out: the step waits.
    let f = with_profiles();
    switch_on(&f);
    write_readings(&f, &[Login::out("Y", 7200)]);
    let out = f.run(&mut nested_x(&f, LIMIT_TEXT, true));
    assert!(started(&out), "{}", describe(&out));
}

// --- A3, A4: the wait names the login ------------------------------------------

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

#[test]
fn a4_a_home_step_with_auto_switch_off_and_no_rotator_waits_and_names_its_login() {
    // D2 for `$HOME/.claude`: the setting is the user's standing instruction.
    let f = with_profiles();
    x_is_live(&f);
    write_readings(&f, &[Login::room("Y")]);
    let log = recipe_log(&f);
    let mut c = home_step(&f, LIMIT_TEXT, false);
    c.env(RECIPE_LOG_ENV, &log);
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2);
    assert_eq!(f.call("0", 2)["PROFILE"], "<unset>", "no move");
    assert!(started(&out), "{}", describe(&out));
    assert!(
        log_lines(&log)[0].starts_with("start reason=usage_limit account=X reset="),
        "{:?}",
        log_lines(&log)
    );
}

#[test]
fn a5_a_home_step_with_a_running_rotator_keeps_the_wait_for_its_own_reset() {
    // The rotator moves `~/.claude` itself; two processes never move one
    // login. The step does not switch itself and waits as before.
    let f = with_profiles();
    switch_on(&f);
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    write_readings(&f, &[Login::room("Y")]);
    let out = f.run(&mut home_step(&f, LIMIT_TEXT, true));
    assert!(started(&out), "{}", describe(&out));
    assert_eq!(f.call("0", 2)["PROFILE"], "<unset>", "no move by the step");
}

#[test]
fn a6_a_home_step_with_a_session_flag_keeps_the_wait_for_its_own_reset() {
    // A resumed session needs its transcript, which lives under `~/.claude`.
    for flag in [
        vec!["--resume", "abc"],
        vec!["--resume=abc"],
        vec!["--continue"],
        vec!["-c"],
    ] {
        let f = with_profiles();
        switch_on(&f);
        x_is_live(&f);
        write_readings(&f, &[Login::room("Y")]);
        fs::create_dir_all(f.home.join(".claude")).unwrap();
        let mut args = vec!["claude", "--subprocess-safe", "--"];
        args.extend(&flag);
        args.extend(["-p", "step"]);
        let mut c = f.cmd(&args);
        c.env_remove("CLAUDE_CONFIG_DIR")
            .env(TOGGLE_ENV, "1")
            .env("LIMIT_MESSAGE", LIMIT_TEXT);
        let out = f.run(&mut c);
        assert!(started(&out), "{flag:?}: {}", describe(&out));
        assert_eq!(f.call("0", 2)["PROFILE"], "<unset>", "{flag:?}");
    }
}

#[test]
fn a7_a_home_step_with_no_readable_live_name_keeps_the_wait_and_says_unknown() {
    // No rotate ledger: the step cannot be named, so it cannot be shown not
    // to have moved before. It fails closed.
    let f = with_profiles();
    switch_on(&f);
    write_readings(&f, &[Login::room("Y")]);
    let log = recipe_log(&f);
    let mut c = home_step(&f, LIMIT_TEXT, true);
    c.env(RECIPE_LOG_ENV, &log);
    let out = f.run(&mut c);
    assert!(started(&out), "{}", describe(&out));
    assert_eq!(f.call("0", 2)["PROFILE"], "<unset>");
    assert!(
        log_lines(&log)[0].contains(" account=unknown "),
        "{:?}",
        log_lines(&log)
    );
}

// --- B1, B2, B3: the spend-limit wording with no zone --------------------------

#[test]
fn b1_the_zoneless_spend_limit_is_a_usage_limit_and_its_reset_times_the_wait() {
    // Before #2967 the reset had no zone, so none was read, and the limit
    // was a spend limit stating no reset: a wait for the login to change,
    // which no other login's room or reset could end.
    for message in [SPEND_ZONELESS_ADMIN, SPEND_ZONELESS_VISIT] {
        let f = with_profiles();
        let out = f.run(&mut nested_x(&f, message, false));
        assert!(out.status.success(), "{message}: {}", describe(&out));
        assert_eq!(f.count("0"), 2, "{message}: {}", describe(&out));
        let ev = events(&out);
        assert_eq!(
            ev,
            vec![
                format!(
                    "start reason=usage_limit harness=claude reset={RESET_0930} \
                     until={UNTIL_0932} bound=reset+margin"
                ),
                "end reason=reset reached".to_string(),
                "resume attempt=2".to_string(),
            ],
            "{message}"
        );
    }
}

#[test]
fn b1_the_zoneless_reset_is_read_in_the_hosts_zone() {
    // The same wording under another zone is another instant: the zone is
    // the host's, not a guess. At `TEST_NOW` it is 05:00 in Honolulu, so
    // 9:30am is 4.5 hours on, inside the five-hour bound.
    use chrono::TimeZone;
    let f = with_profiles();
    let mut c = nested_x(&f, SPEND_ZONELESS_ADMIN, false);
    c.env("TZ", "Pacific/Honolulu");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    let start = events(&out)
        .into_iter()
        .find(|l| l.starts_with("start "))
        .unwrap_or_else(|| panic!("a wait starts: {}", describe(&out)));
    let want = chrono::Utc
        .with_ymd_and_hms(2026, 10, 18, 19, 30, 0)
        .unwrap()
        .timestamp();
    assert_eq!(instant(between(&start, "reset", "until")), want, "{start}");
    assert!(start.ends_with(" bound=reset+margin"), "{start}");
}

#[test]
fn b2_a_usable_login_takes_a_zoneless_spend_limit_step_with_no_wait() {
    // Y is at 92%: only the second rule finds it. The step is on X.
    for message in [SPEND_ZONELESS_ADMIN, SPEND_ZONELESS_VISIT] {
        let f = with_profiles();
        switch_on(&f);
        write_readings(&f, &[Login::usable_only("Y")]);
        let out = f.run(&mut nested_x(&f, message, true));
        assert!(out.status.success(), "{message}: {}", describe(&out));
        assert_eq!(f.count("0"), 2, "{message}: {}", describe(&out));
        assert_ran_on_y(&f, 2);
        assert!(!started(&out), "{message}: {:?}", wait_lines(&out));
    }
}

/// The lines of a spend limit that states no reset: a login-change wait with
/// no deadline (#2731, #2911), ended here by a login change in X's
/// directory.
fn login_change_wait() -> Vec<&'static str> {
    vec![
        "waiting_for_capacity reason=spend_limit harness=claude until=none",
        "start reason=spend_limit wait=login_change harness=claude reset=none until=none bound=none",
        "end reason=login changed",
        "resume attempt=2",
    ]
}

#[test]
fn b3_a_tz_that_names_no_zone_leaves_the_reset_unread() {
    // `TZ` as a path, or empty, names no zone: no file is read from it, no
    // zone is guessed, and the message is a spend limit that states no
    // reset, as it was before #2967.
    for tz in [":/etc/localtime", "/etc/localtime", "", "../x"] {
        let f = with_profiles();
        let mut c = nested_x(&f, SPEND_ZONELESS_ADMIN, false);
        c.env("TZ", tz);
        let out = f.run_with_a_login_change_in(&f.profiles.join("X"), &mut c);
        assert!(out.status.success(), "TZ={tz:?}: {}", describe(&out));
        assert_eq!(f.count("0"), 2, "TZ={tz:?}: one retry");
        assert_eq!(
            events(&out),
            login_change_wait(),
            "TZ={tz:?}: {}",
            describe(&out)
        );
    }
}

#[test]
fn b4_a_spend_limit_that_states_no_reset_keeps_its_present_handling() {
    let f = with_profiles();
    let message = "You've hit your individual spend limit · run /usage-credits to ask your admin for a higher limit";
    let mut c = nested_x(&f, message, false);
    let out = f.run_with_a_login_change_in(&f.profiles.join("X"), &mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(events(&out), login_change_wait(), "{}", describe(&out));
}

// --- D13: the message reader's line shape, on its own -------------------------
//
// The observed step's stderr cannot be recovered, so the PR names each path
// that could produce the symptom. This one is the reader's line shape: the
// message as a raw round log holds it, CRLF-terminated, or followed by more
// of the runner's own lines. Each message here states its zone, so the
// zone-less path (B1) cannot be what makes it pass or fail. A pass on `main`
// means this path is examined and excluded; the PR says which it was.

const SPEND_ZONED_ADMIN: &str = "You've hit your individual spend limit \u{b7} run /usage-credits to ask your admin for a higher limit \u{b7} your 5-hour limit resets 9:30am (America/Los_Angeles)";

#[test]
fn d13_a_crlf_terminated_spend_limit_line_is_read() {
    let f = with_profiles();
    let message = format!("{SPEND_ZONED_ADMIN}\r");
    let out = f.run(&mut nested_x(&f, &message, false));
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        events(&out),
        vec![
            format!(
                "start reason=usage_limit harness=claude reset={RESET_0930} \
                 until={UNTIL_0932} bound=reset+margin"
            ),
            "end reason=reset reached".to_string(),
            "resume attempt=2".to_string(),
        ]
    );
}

#[test]
fn d13_a_spend_limit_line_followed_by_more_runner_lines_is_read() {
    let f = with_profiles();
    let message = format!("{SPEND_ZONED_ADMIN}\n\nRun /help for more.\n");
    let out = f.run(&mut nested_x(&f, &message, false));
    assert!(out.status.success(), "{}", describe(&out));
    let ev = events(&out);
    assert!(
        ev.first()
            .is_some_and(|l| l.starts_with("start reason=usage_limit")),
        "{ev:?}: {}",
        describe(&out)
    );
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

// --- H: the heartbeat ----------------------------------------------------------

/// A recipe runner that runs one agent step in the background, as
/// `recipe-runner-rs` does, and prints its own heartbeat for it: `working`
/// lines labelled with the step's process id (elapsed 7 s), from the moment
/// the step's wait-state record exists for as long as it does, then one more
/// after the step ended (elapsed 99 s). The step's stderr is copied to the
/// runner's stderr last, so the wait's own lines are in the recipe run's
/// output.
///
/// Waiting on the record rather than on the wait's `start` line keeps the
/// first heartbeat from coming before the record is written. The wait's
/// deadline is an hour away, so only the test ends it.
const HEARTBEAT_RUNNER: &str = r#"#!/bin/sh
case "$*" in --capabilities) printf '%b\n' '\173"schema_version":1,"version":"fixture","capabilities":["codex_exec","durable_recovery_v1"]\175'; exit 0 ;; esac
case "$1" in
  --version) echo 'recipe-runner-rs 0.0.0 (stub)'; exit 0 ;;
esac
export AMPLIHACK_NONINTERACTIVE=1
err="$RECORD_DIR/step.err"
: > "$err"
STEP=1 "$AMPLIHACK_UNDER_TEST" claude --subprocess-safe -- -p "step 1" </dev/null >> "$RECORD_DIR/runner.out" 2> "$err" &
pid=$!
echo "$pid" > "$RECORD_DIR/step.pid"
rec="$AMPLIHACK_USAGE_LIMIT_WAIT_DIR/$pid"
until [ -e "$rec" ]; do
  kill -0 "$pid" 2>/dev/null || break
  sleep 0.1
done
while [ -e "$rec" ]; do
  echo "  [12:00:00] [amplihack:claude:$pid] ... working (7s elapsed, 7s since last output)" >&2
  sleep 0.1
done
wait "$pid"
rc=$?
cat "$err" >&2
echo "  [12:00:00] [amplihack:claude:$pid] ... working (99s elapsed, 3s since last output)" >&2
if [ "$rc" -ne 0 ]; then
  printf '{"recipe_name":"issue-2967","success":false,"step_results":[{"step_id":"step-1","status":"failed","output":"","error":"agent step failed"}],"context":{}}\n'
  exit 1
fi
printf '%s\n' '{"recipe_name":"issue-2967","success":true,"step_results":[],"context":{}}'
"#;

const HEARTBEAT_RECIPE: &str = r#"name: issue-2967
description: One agent step that hits a usage limit once
version: 1.0.0
context: {}
steps:
  - id: step-1
    type: bash
    command: "true"
"#;

/// The rewritten heartbeats the test waits to see before it ends the wait.
const HEARTBEATS_BEFORE_THE_END: usize = 3;

/// What `heartbeat_run` saw: the run's output, and how many stderr lines had
/// been read when the test ended the wait. Every heartbeat among those lines
/// was copied while the wait certainly lasted.
struct HeartbeatRun {
    out: Output,
    before_the_end: usize,
}

/// Runs the recipe with [`HEARTBEAT_RUNNER`]. The step's limit states a reset
/// an hour ahead of the real clock this run uses, and the test ends the wait
/// itself with a login change in the step's directory once it has read
/// [`HEARTBEATS_BEFORE_THE_END`] rewritten heartbeats: an event, not a delay
/// (issue #1973).
fn heartbeat_run(f: &Fixture) -> HeartbeatRun {
    let runner = f.tools.join("recipe-runner-rs");
    executable(&runner, HEARTBEAT_RUNNER);
    let recipe = f.root.join("recipe.yaml");
    fs::write(&recipe, HEARTBEAT_RECIPE).unwrap();
    let mut c = f.cmd(&["recipe", "run"]);
    c.arg(&recipe)
        .arg("-w")
        .arg(&f.work)
        .env("RECIPE_RUNNER_RS_PATH", &runner)
        .env("AMPLIHACK_AGENT_BINARY", "claude")
        .env("AMPLIHACK_HOME", f.home.join(".amplihack"))
        .env(
            "LIMIT_MESSAGE",
            format!("Claude AI usage limit reached|{}", real_now() + 3600),
        );
    real_clock(&mut c);
    let config = f.config.clone();
    let mut read = 0usize;
    let mut seen = 0usize;
    let mut before_the_end = None;
    let out = f.run_watching(&mut c, |line| {
        read += 1;
        if before_the_end.is_none()
            && line.contains("] ... waiting for usage limit until ")
            && line.contains("(7s elapsed")
        {
            seen += 1;
            if seen == HEARTBEATS_BEFORE_THE_END {
                before_the_end = Some(read);
                toggle_login(&config);
            }
        }
    });
    HeartbeatRun {
        before_the_end: before_the_end
            .unwrap_or_else(|| panic!("no rewritten heartbeat was seen: {}", describe(&out))),
        out,
    }
}

#[test]
fn h_the_heartbeat_of_a_waiting_step_says_it_is_waiting_and_until_when() {
    in_quiet_process(
        module_path!(),
        "h_the_heartbeat_of_a_waiting_step_says_it_is_waiting_and_until_when",
        || {
            let f = Fixture::new();
            let HeartbeatRun {
                out,
                before_the_end,
            } = heartbeat_run(&f);
            assert!(out.status.success(), "{}", describe(&out));
            let stderr = text(&out.stderr);
            let pid = fs::read_to_string(f.records.join("step.pid"))
                .unwrap()
                .trim()
                .to_string();
            let ev = events(&out);
            let start = ev
                .iter()
                .find(|l| l.starts_with("start "))
                .unwrap_or_else(|| panic!("a wait starts: {}", describe(&out)));
            let until = between(start, "until", "bound").to_string();
            assert!(
                ev.iter().any(|l| l == "end reason=login changed"),
                "the test's login change ended the wait: {ev:?}"
            );
            // Every heartbeat copied before the test ended the wait.
            let during: Vec<&str> = stderr
                .lines()
                .take(before_the_end)
                .filter(|l| l.contains("] ... ") && l.contains("(7s elapsed"))
                .collect();
            assert!(during.len() >= HEARTBEATS_BEFORE_THE_END, "{stderr}");
            let want = format!(
                "  [12:00:00] [amplihack:claude:{pid}] ... waiting for usage limit until {until} (7s elapsed, 7s since last output)"
            );
            for l in &during {
                assert_eq!(*l, want, "a heartbeat during the wait");
                assert!(!l.contains("working"), "{l}");
            }
        },
    );
}

#[test]
fn h_a_heartbeat_outside_a_wait_passes_byte_for_byte() {
    in_quiet_process(
        module_path!(),
        "h_a_heartbeat_outside_a_wait_passes_byte_for_byte",
        || {
            let f = Fixture::new();
            let HeartbeatRun { out, .. } = heartbeat_run(&f);
            let pid = fs::read_to_string(f.records.join("step.pid"))
                .unwrap()
                .trim()
                .to_string();
            let after = format!(
                "  [12:00:00] [amplihack:claude:{pid}] ... working (99s elapsed, 3s since last output)"
            );
            assert!(
                text(&out.stderr).lines().any(|l| l == after),
                "the wait is over, so the line is the runner's own: {}",
                describe(&out)
            );
        },
    );
}

#[test]
fn h_the_wait_state_directory_is_private_and_gone_after_the_run() {
    in_quiet_process(
        module_path!(),
        "h_the_wait_state_directory_is_private_and_gone_after_the_run",
        || {
            let f = Fixture::new();
            let HeartbeatRun { out, .. } = heartbeat_run(&f);
            assert!(out.status.success(), "{}", describe(&out));
            // Nothing the wait left behind: no `usage-limit-wait` directory
            // survives a run that created it.
            let left = walk(&f.home)
                .into_iter()
                .chain(walk(&f.work))
                .filter(|p| p.ends_with("usage-limit-wait"))
                .collect::<Vec<_>>();
            assert!(left.is_empty(), "left behind: {left:?}");
        },
    );
}

fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    let Ok(rd) = fs::read_dir(dir) else {
        return found;
    };
    for e in rd.flatten() {
        let p = e.path();
        found.push(p.clone());
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            found.extend(walk(&p));
        }
    }
    found
}
