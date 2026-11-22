//! Issue #1801: a weekly limit, or one with no readable reset, on a step
//! whose login another login could replace waits for capacity, then runs
//! once more on the login that has it. It no longer ends the run at once.
//!
//! - AC1, AC13: a capacity wait ends when a login shows capacity, and the retry
//!   runs on that login, never the exhausted one.
//! - AC3, AC8: no second login, a store that cannot be read, an explicit
//!   `CLAUDE_CONFIG_DIR`, auto-switch off or a spend limit keep today's skip.
//! - AC14: `max_wait_secs = 0` is immediate `usage_limit`, one attempt.
//! - AC15: a retry that hits the limit again gets no second wait.
//! - S24: the wait never runs the stub to probe a login.
//! - Issue #2694: the same exit now ends a reset wait, so a step whose
//!   stored profile states a near reset no longer sleeps to it while
//!   another login has room.
//! - Issue #2694: a step auto-switch cannot move, one on an explicit
//!   `CLAUDE_CONFIG_DIR` or on `$HOME/.claude`, still sleeps to its reset
//!   with the same readings in front of it, and a spend limit still skips.
//!
//! Tests that need something to happen during a wait run on the real clock,
//! scaled, with `TEST_NOW` removed: a reading or rotator cycle is stamped with
//! real time, and the wait's `start` must be on the same clock. The other
//! tests use the fixture's virtual clock, so their log times are exact. See
//! `issue_1618_fixture`.

#![cfg(unix)]

use crate::issue_1618_fixture::{
    Fixture, LIMIT_TEXT, SPEND_TEXT, SPEND_WEEKLY_TEXT, TEST_NOW, WAIT_PREFIX, WEEKLY_TEXT,
    claude_stdout, describe, events, private_file, text, wait_lines,
};
use crate::issue_1618_wait_profiles::{switch_on, with_profiles};
use crate::support::profile_default::set_default;
use crate::support::wait::{CEILING, wait_until};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

const TOGGLE_ENV: &str = "AMPLIHACK_CLAUDE_AUTO_SWITCH";
const MAX_WAIT_ENV: &str = "AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS";
const NO_RESET_TEXT: &str = "You've hit your session limit · try again later";
/// `WEEKLY_TEXT` read at `TEST_NOW`: October 8 has passed, so it is next year's.
pub(crate) const WEEKLY_RESET: &str = "2027-10-08 00:00:00 -07:00";
const FAILED_EXPIRED: &str = "failed reason=capacity_expired: usage limit not lifted";

pub(crate) fn real_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn private_dirs(profiles: &Path) {
    for dir in [
        profiles.join(".auto-switch.d"),
        profiles.join(".auto-switch.d").join("rotate"),
    ] {
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    }
}

fn rotate_ledger(profiles: &Path) -> PathBuf {
    profiles
        .join(".auto-switch.d")
        .join("rotate")
        .join("ledger.json")
}

/// Replaces `path` in one step, so the wait never reads half a file.
fn replace(path: &Path, body: &str) {
    let tmp = path.with_extension("tmp");
    private_file(&tmp, body);
    fs::rename(&tmp, path).unwrap();
}

/// The rotate ledger: `live` is live, its last cycle measured at `at`. Since
/// issue #2857 the cycle also records `live_room: true`, as the rotator writes it
/// for a login below its own thresholds: a wait wakes on the rotator's capacity
/// only with it (`issue_2857_wakes_only_on_room.rs` covers a cycle without).
pub(crate) fn write_cycle(profiles: &Path, live: &str, at: i64) {
    private_dirs(profiles);
    replace(
        &rotate_ledger(profiles),
        &format!(
            r#"{{"version":1,"live":"{live}","alert":null,"last_cycle":{{"at":{at},"measured_at":{at},"outcome":"swapped","live_room":true}}}}"#
        ),
    );
}

/// A cycle from before the step on either clock: `TEST_NOW` is in the past.
pub(crate) const OLD_CYCLE: i64 = TEST_NOW - 3600;

/// A rotate ledger naming X as live, with a cycle from before the step.
pub(crate) fn x_is_live(f: &Fixture) {
    write_cycle(&f.profiles, "X", OLD_CYCLE);
}

/// The auto-switch readings with X at its session limit and Y with room,
/// both stamped `at`. Written in one step, so a wait taking a reading
/// never reads half the file.
///
/// Used both during a wait, stamped with real time, where it moves the
/// step onto Y, and before a run, stamped on the fixture's virtual clock,
/// where the question is whether the step moves at all. The two uses share
/// the body so that a reading shape the probe would reject could not leave
/// the second sort passing on readings that move nothing.
fn readings_with_room(profiles: &Path, at: i64) {
    private_dirs(profiles);
    let reading = |pct: f64| {
        serde_json::json!({
            "five_hour": {"used_percentage": pct, "resets_at": at + 3600},
            "seven_day": {"used_percentage": 5.0, "resets_at": at + 86_400},
            "observed_at": at,
            "source": "statusline",
        })
    };
    let body = serde_json::json!({
        "version": 1,
        "accounts": {"X": reading(100.0), "Y": reading(10.0)},
    });
    replace(
        &profiles.join(".auto-switch.d").join("usage.json"),
        &body.to_string(),
    );
}

/// Holds the external rotator's lock, as a running prototype rotator does,
/// until the returned file is dropped. A step takes a capacity wait only
/// while a rotator runs (#1801).
pub(crate) fn rotator_running(f: &Fixture) -> fs::File {
    use std::os::fd::AsRawFd;
    let dir = f.profiles.join(".rotator");
    fs::create_dir_all(&dir).unwrap();
    let lock = dir.join("lock");
    fs::write(&lock, "").unwrap();
    let holder = fs::File::open(&lock).unwrap();
    // SAFETY: valid open descriptor.
    assert_eq!(
        unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    holder
}

/// A step using `$HOME/.claude`.
fn home_step(f: &Fixture) -> Command {
    fs::create_dir_all(f.home.join(".claude")).unwrap();
    let mut c = f.step();
    c.env_remove("CLAUDE_CONFIG_DIR")
        .env("LIMIT_MESSAGE", WEEKLY_TEXT);
    c
}

/// A step that inherited profile X, with auto-switch on.
fn nested_x(f: &Fixture) -> Command {
    switch_on(f);
    let mut c = f.step();
    c.env("AMPLIHACK_CLAUDE_PROFILE", "X")
        .env("CLAUDE_CONFIG_DIR", f.profiles.join("X"))
        .env(TOGGLE_ENV, "1")
        .env("LIMIT_PROFILE", "")
        .env("LIMIT_MESSAGE", WEEKLY_TEXT);
    c
}

/// Runs on the real clock, sped up: a 30 s poll takes 3 s, and the
/// longest wait here, the default six hours, 36 minutes. Issue #1973: a
/// faster clock could end a wait at its maximum before a loaded host ran
/// the test's change; every wait here ends on that change instead. The
/// scale does not bound a failing run: if the change never ends the wait,
/// `run_watching` kills the step at the shared 10-minute ceiling and the
/// test fails there.
pub(crate) fn real_clock(c: &mut Command) {
    c.env_remove("AMPLIHACK_USAGE_LIMIT_WAIT_TEST_NOW")
        .env("AMPLIHACK_USAGE_LIMIT_WAIT_TEST_SCALE", "10");
}

/// Runs the step `c` and, once its usage-limit wait has started, runs
/// `then` on another thread, in a later second than the wait's `start`
/// line, so what it stamps is after the wait's start. Waits on the line,
/// not on a delay (issue #1973). A wait that never starts fails the test;
/// the writer's panic is propagated.
fn run_during_wait(f: &Fixture, c: &mut Command, then: impl FnOnce() + Send + 'static) -> Output {
    use std::sync::mpsc;
    let (started_tx, started_rx) = mpsc::channel::<i64>();
    let writer = std::thread::spawn(move || {
        let start = started_rx
            .recv_timeout(CEILING)
            .unwrap_or_else(|e| panic!("the wait never started within {CEILING:?}: {e}"));
        wait_until("the second after the wait's start", || real_now() > start);
        then();
    });
    // Any wait's start line, whatever its reason: a login-change wait's
    // reads `reason=spend_limit` (#2731), and a matcher pinned to
    // `usage_limit` would never fire and the test would fail at the shared
    // ten-minute ceiling rather than on an assertion.
    let start_line = format!("{WAIT_PREFIX}start ");
    let mut started_tx = Some(started_tx);
    let out = f.run_watching(c, |line| {
        if line.starts_with(&start_line)
            && let Some(tx) = started_tx.take()
        {
            tx.send(real_now()).expect("the writer is waiting");
        }
    });
    // Without a start line the writer fails on its own ceiling.
    drop(started_tx);
    if let Err(panic) = writer.join() {
        std::panic::resume_unwind(panic);
    }
    out
}

pub(crate) fn run_with_rotator_cycle_during_wait(f: &Fixture, c: &mut Command) -> Output {
    let profiles = f.profiles.clone();
    run_during_wait(f, c, move || {
        write_cycle(&profiles, "Y", real_now());
    })
}

/// No wait started, and `reason` is the skip.
fn assert_skipped(f: &Fixture, out: &Output, reason: &str) {
    assert_eq!(out.status.code(), Some(1), "{reason}: {}", describe(out));
    assert_eq!(f.count("0"), 1, "{reason}: no rerun");
    assert_eq!(
        events(out),
        [format!("skipped reason={reason}")],
        "{reason}"
    );
}

/// No wait line may name a login.
pub(crate) fn assert_no_account_detail(f: &Fixture, out: &Output) {
    let profiles = f.profiles.display().to_string();
    for l in wait_lines(out) {
        for word in ["@", "uuid-", "SENTINEL", profiles.as_str()] {
            assert!(!l.contains(word), "{word:?} in {l:?}");
        }
    }
}

// --- a capacity wait starts and ends -----------------------------------------

#[test]
fn ac1_a_home_step_waits_on_a_weekly_limit_and_the_rotator_ends_the_wait() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f);
    real_clock(&mut c);
    let out = run_with_rotator_cycle_during_wait(&f, &mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2, "one retry");
    let ev = events(&out);
    assert_eq!(ev.len(), 3, "{ev:?}");
    assert!(
        ev[0].starts_with("start reason=usage_limit wait=capacity harness=claude reset=2"),
        "{ev:?}"
    );
    assert!(ev[0].ends_with(" bound=max_wait"), "{ev:?}");
    assert_eq!(ev[1..], ["end reason=rotator capacity", "resume attempt=2"]);
    assert_eq!(
        f.call("0", 2)["PROFILE"],
        "<unset>",
        "the live $HOME/.claude"
    );
    assert_eq!(claude_stdout(&out), "attempt 2 output\n");
    assert_no_account_detail(&f, &out);
}

#[test]
fn ac1_no_readable_reset_also_waits_and_says_none() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f);
    c.env("LIMIT_MESSAGE", NO_RESET_TEXT).env(MAX_WAIT_ENV, "0");
    let out = f.run(&mut c);
    assert_eq!(
        events(&out)[0],
        "start reason=usage_limit wait=capacity harness=claude reset=none \
         until=2026-10-18 08:00:00 -07:00 bound=max_wait",
        "{}",
        describe(&out)
    );
}

#[test]
fn ac13_an_inherited_profile_step_retries_on_the_login_with_capacity() {
    let f = with_profiles();
    let _rotator = rotator_running(&f);
    let mut c = nested_x(&f);
    // X, then Y, then the retry.
    c.env("LIMIT_CALLS", "2");
    real_clock(&mut c);
    let profiles = f.profiles.clone();
    let count = f.records.join("count.0");
    let out = run_during_wait(&f, &mut c, move || {
        // The reading must change during the wait that follows Y's limit
        // hit (call 2), not during one after X's. Checked after the write,
        // so a wrong order fails here rather than at the ceiling.
        let calls = fs::read_to_string(&count).unwrap_or_default();
        readings_with_room(&profiles, real_now());
        assert_eq!(
            calls.trim(),
            "2",
            "the wait started after call {calls:?}; expected X, then Y, then the retry"
        );
    });
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 3, "{}", describe(&out));
    assert_eq!(f.call("0", 1)["PROFILE"], "X");
    assert_eq!(f.call("0", 2)["PROFILE"], "Y");
    assert_eq!(
        f.call("0", 3)["PROFILE"],
        "Y",
        "the login with capacity, not the exhausted inherited one"
    );
    assert_eq!(
        f.call("0", 3)["CLAUDE_CONFIG_DIR"],
        f.profiles.join("Y").display().to_string()
    );
    let ev = events(&out);
    assert!(ev[0].contains(" wait=capacity "), "{ev:?}");
    assert_eq!(ev[1..], ["end reason=login capacity", "resume attempt=3"]);
    assert!(
        text(&out.stderr).contains(
            "amplihack: a login has capacity again; rerunning this step under profile \"Y\" (auto-switch)"
        ),
        "the run log must name the login the retry moved to: {}",
        describe(&out)
    );
    f.assert_isolated("0");
    assert_no_account_detail(&f, &out);
}

/// Claude's other limit wording, which states the reset as an epoch. The
/// tests that need something to happen during a wait run on the real clock,
/// where a fixed clock time such as `LIMIT_TEXT`'s 8:30am is between zero
/// and twenty-four hours away and so would not always give a reset wait.
/// 3 h 45 min is inside the five-hour ceiling, so the plan is `Reset`.
fn near_reset_message() -> String {
    format!("Claude AI usage limit reached|{}", real_now() + 13_500)
}

/// Issue #2694: a step that inherited profile X, limited with a reset under
/// five hours away, takes a reset wait. While it waits, a reading shows Y
/// has room, so the wait ends there and the retry runs under Y. No rotator
/// runs: a reset wait needs none, and the move is auto-switch's, not the
/// rotator's.
#[test]
fn issue_2694_a_reset_wait_under_a_profile_ends_on_another_logins_capacity() {
    let f = with_profiles();
    let mut c = nested_x(&f);
    // X, then Y, then the retry.
    c.env("LIMIT_CALLS", "2")
        .env("LIMIT_MESSAGE", near_reset_message());
    real_clock(&mut c);
    let profiles = f.profiles.clone();
    let count = f.records.join("count.0");
    let out = run_during_wait(&f, &mut c, move || {
        // The reading must change during the wait that follows Y's limit
        // hit (call 2), not during one after X's. Checked after the write,
        // so a wrong order fails here rather than at the ceiling.
        let calls = fs::read_to_string(&count).unwrap_or_default();
        readings_with_room(&profiles, real_now());
        assert_eq!(
            calls.trim(),
            "2",
            "the wait started after call {calls:?}; expected X, then Y, then the retry"
        );
    });
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 3, "{}", describe(&out));
    assert_eq!(f.call("0", 1)["PROFILE"], "X");
    assert_eq!(f.call("0", 2)["PROFILE"], "Y");
    assert_eq!(
        f.call("0", 3)["PROFILE"],
        "Y",
        "the login with capacity, not the exhausted inherited one"
    );
    assert_eq!(
        f.call("0", 3)["CLAUDE_CONFIG_DIR"],
        f.profiles.join("Y").display().to_string()
    );
    let ev = events(&out);
    assert_eq!(ev.len(), 3, "{ev:?}");
    assert!(
        ev[0].starts_with("start reason=usage_limit harness=claude reset="),
        "a reset wait, not a capacity wait: {ev:?}"
    );
    assert!(
        !ev[0].contains(" wait=capacity"),
        "a reset wait, not a capacity wait: {ev:?}"
    );
    assert!(ev[0].ends_with(" bound=reset+margin"), "{ev:?}");
    assert_eq!(ev[1..], ["end reason=login capacity", "resume attempt=3"]);
    assert!(
        text(&out.stderr).contains(
            "amplihack: a login has capacity again; rerunning this step under profile \"Y\" (auto-switch)"
        ),
        "the run log must name the login the retry moved to: {}",
        describe(&out)
    );
    f.assert_isolated("0");
    assert_no_account_detail(&f, &out);
}

/// Issue #2694: the no-room line belongs to a wait a reading ended early,
/// and to no other wait. A step that inherited profile X is limited with a
/// reset 5 h 30 min away: past the five-hour reset-wait ceiling, so the plan
/// is a capacity wait, and inside the six-hour maximum, so that reset bounds
/// it. No reading is ever written, so nothing shows room and the wait runs to
/// its own reset, where X has recovered and the retry runs on X. That retry
/// gave up nothing, so it must say nothing: the line was once printed after
/// any capacity wait, which told every such step it had lost room it never
/// had.
#[test]
fn issue_2694_a_capacity_wait_that_reaches_its_reset_says_nothing_about_no_room() {
    let f = with_profiles();
    let _rotator = rotator_running(&f);
    let mut c = nested_x(&f);
    // X, then Y, then the retry after the wait. The reset is stated as an
    // epoch on the fixture's virtual clock, so the log times are exact.
    c.env("LIMIT_CALLS", "2").env(
        "LIMIT_MESSAGE",
        format!("Claude AI usage limit reached|{}", TEST_NOW + 19_800),
    );
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        f.count("0"),
        3,
        "X, then Y, then one retry: {}",
        describe(&out)
    );
    assert_eq!(
        f.call("0", 3)["PROFILE"],
        "X",
        "no login had room, so the retry stays on the limited profile"
    );
    let ev = events(&out);
    assert_eq!(ev.len(), 3, "{ev:?}");
    assert!(
        ev[0].starts_with("start reason=usage_limit wait=capacity harness=claude reset="),
        "a reset past the five-hour ceiling is a capacity wait: {ev:?}"
    );
    assert!(
        ev[0].ends_with(" bound=reset+margin"),
        "a reset inside the six-hour maximum bounds the wait: {ev:?}"
    );
    assert_eq!(ev[1..], ["end reason=reset reached", "resume attempt=3"]);
    assert!(
        !text(&out.stderr).contains("no login has room now"),
        "this wait reached its own reset and never saw room, so the retry must \
         not report room lost: {}",
        describe(&out)
    );
}

#[test]
fn a_near_reset_stays_a_reset_wait_with_a_second_login() {
    let f = with_profiles();
    x_is_live(&f);
    let mut c = home_step(&f);
    c.env("LIMIT_MESSAGE", LIMIT_TEXT);
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        events(&out),
        vec![
            "start reason=usage_limit harness=claude reset=2026-10-18 08:30:00 -07:00 \
             until=2026-10-18 08:32:00 -07:00 bound=reset+margin",
            "end reason=reset reached",
            "resume attempt=2",
        ]
    );
}

// --- a capacity wait that nothing lifts ---------------------------------------

#[test]
fn ac14_a_zero_maximum_is_immediate_usage_limit_with_one_attempt() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f);
    c.env(MAX_WAIT_ENV, "0");
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(
        f.count("0"),
        1,
        "S24: no retry, and no call of the stub to probe a login"
    );
    assert_eq!(
        events(&out),
        vec![
            format!(
                "start reason=usage_limit wait=capacity harness=claude reset={WEEKLY_RESET} \
                 until=2026-10-18 08:00:00 -07:00 bound=max_wait"
            ),
            "end reason=capacity expired".to_string(),
            FAILED_EXPIRED.to_string(),
        ]
    );
    assert_eq!(claude_stdout(&out), "attempt 1 output\n");
}

#[test]
fn reaching_the_maximum_ends_with_no_retry() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f);
    c.env(MAX_WAIT_ENV, "120");
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(f.count("0"), 1, "no retry");
    let ev = events(&out);
    assert!(
        ev[0].ends_with("until=2026-10-18 08:02:00 -07:00 bound=max_wait"),
        "{ev:?}"
    );
    assert_eq!(ev[1..], ["end reason=capacity expired", FAILED_EXPIRED]);
    let lines = wait_lines(&out);
    assert_eq!(
        lines.last().map(String::as_str),
        Some(FAILED_EXPIRED),
        "the closing line comes after the last progress line"
    );
}

// G3, AC4: the maximum from the user config file, with no variable, ends a
// capacity wait that nothing lifts, and the step is classed usage_limit.
#[test]
fn ac4_a_config_file_maximum_ends_the_wait_as_a_usage_limit() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    f.write_config("[recipe]\nusage_limit_max_wait_secs = 300\n");
    let mut c = home_step(&f);
    c.env_remove(MAX_WAIT_ENV);
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(f.count("0"), 1, "no retry after the maximum");
    let ev = events(&out);
    assert!(
        ev[0].starts_with("start reason=usage_limit wait=capacity "),
        "{ev:?}"
    );
    assert!(
        ev[0].ends_with("until=2026-10-18 08:05:00 -07:00 bound=max_wait"),
        "{ev:?}"
    );
    assert_eq!(ev[1..], ["end reason=capacity expired", FAILED_EXPIRED]);
    assert!(
        text(&out.stderr).contains("You've hit your weekly limit"),
        "the runner's classifier must still see the limit, so the step is usage_limit"
    );
    assert_no_account_detail(&f, &out);
}

#[test]
fn the_default_maximum_is_six_hours() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    let out = f.run(&mut home_step(&f));
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert!(
        events(&out)[0].ends_with("until=2026-10-18 14:00:00 -07:00 bound=max_wait"),
        "{:?}",
        events(&out)
    );
}

#[test]
fn a_user_maximum_above_six_hours_is_honoured() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f);
    c.env(MAX_WAIT_ENV, "28800");
    let out = f.run(&mut c);
    assert!(
        events(&out)[0].ends_with("until=2026-10-18 16:00:00 -07:00 bound=max_wait"),
        "{:?}",
        events(&out)
    );
}

#[test]
fn a_step_timeout_on_a_capacity_wait_writes_the_closing_line() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f);
    c.env("AMPLIHACK_STEP_TIMEOUT", "30");
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(f.count("0"), 1, "no retry");
    let ev = events(&out);
    assert!(ev[0].ends_with(" bound=step_timeout"), "{ev:?}");
    assert_eq!(
        ev[1..],
        [
            "end reason=step timeout",
            "failed reason=step_timeout: usage limit not lifted",
        ]
    );
}

// Progress lines can push the limit message out of the runner's 40-line
// evidence tail, so a reset wait ended by the step timeout also closes with
// the failed line, and the step is still classed usage_limit.
#[test]
fn a_reset_wait_at_a_step_timeout_writes_the_closing_line() {
    let f = Fixture::new();
    let mut c = f.step();
    c.env("AMPLIHACK_STEP_TIMEOUT", "30");
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    let ev = events(&out);
    assert!(ev[0].ends_with(" bound=step_timeout"), "{ev:?}");
    assert_eq!(
        ev[1..],
        [
            "end reason=step timeout",
            "failed reason=step_timeout: usage limit not lifted",
        ]
    );
    assert_eq!(
        wait_lines(&out).last().map(String::as_str),
        Some("failed reason=step_timeout: usage limit not lifted"),
        "the closing line is the wait's last line"
    );
}

#[test]
fn the_exit_code_passes_through_unchanged() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f);
    c.env(MAX_WAIT_ENV, "0").env("LIMIT_EXIT", "7");
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(7), "{}", describe(&out));
}

/// Issue #2731 split this test in two. AC15's rule is now one wait per
/// *login*, not one per step, so what the retry runs on decides whether a
/// second wait starts. Here the rotator's cycle keeps X live, so the retry
/// runs on the login the step already waited on and AC15 holds unchanged.
///
/// Issue #2857 changed it again and kept it. The wait ended on the rotator's
/// fresh capacity, which says *try now*; the retry ran on X, the login that
/// already had its wait, and was limited, so the signal was wrong. That is the
/// case the same-login refusal was never meant to cover: the step waits again
/// and says so once. The second wait is bounded by what the first left of
/// `max_wait_secs` (120 here, so about 90 virtual seconds), and nothing lifts
/// X's limit, so it ends as a capacity wait does, at its deadline. The refusal
/// for a wait that ended at its own ceiling is in
/// `issue_2857_limit_after_a_wait.rs`.
#[test]
fn ac15_a_retry_on_the_same_login_after_a_wake_waits_again_inside_the_budget() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f);
    c.env("LIMIT_CALLS", "5").env(MAX_WAIT_ENV, "120");
    real_clock(&mut c);
    let profiles = f.profiles.clone();
    let out = run_during_wait(&f, &mut c, move || {
        // A fresh cycle with capacity, but X is still live: the wait ends
        // and the retry runs on the login that already had its wait.
        write_cycle(&profiles, "X", real_now());
    });
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(f.count("0"), 2, "one wait, one retry, one more wait");
    let ev = events(&out);
    assert_eq!(ev.iter().filter(|l| l.starts_with("start ")).count(), 2);
    assert_eq!(
        ev[ev.len() - 2..],
        ["end reason=capacity expired", FAILED_EXPIRED],
        "{ev:?}"
    );
    assert_eq!(
        ev.iter().filter(|l| l.starts_with("reentered ")).count(),
        1,
        "reported once: {ev:?}"
    );
    assert!(!ev.iter().any(|l| l.contains("already_waited")), "{ev:?}");
    assert!(
        text(&out.stderr).contains("You've hit your weekly limit"),
        "the runner's classifier must still see the limit"
    );
}

/// The other half: the rotator made Y live, so the retry runs on a login
/// this step has not waited on and a second wait starts, keyed to it. That
/// second wait is bounded by what the first left of `max_wait_secs`, and
/// when nothing lifts Y's limit either the step ends as it always did.
#[test]
fn ac15_a_retry_on_a_login_this_step_has_not_waited_on_waits_again() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f);
    c.env("LIMIT_CALLS", "5").env(MAX_WAIT_ENV, "120");
    real_clock(&mut c);
    let out = run_with_rotator_cycle_during_wait(&f, &mut c);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(f.count("0"), 2, "X, then the retry on Y");
    let ev = events(&out);
    assert_eq!(
        ev.iter().filter(|l| l.starts_with("start ")).count(),
        2,
        "the login the step moved to gets a wait of its own: {ev:?}"
    );
    assert!(!ev.iter().any(|l| l.contains("already_waited")), "{ev:?}");
    assert_eq!(
        ev[ev.len() - 2..],
        [
            "end reason=capacity expired".to_string(),
            FAILED_EXPIRED.to_string()
        ],
        "{ev:?}"
    );
}

// --- what never starts a capacity wait ----------------------------------------

#[test]
fn ac3_one_stored_login_skips_even_with_rotator_cycles() {
    let f = with_profiles();
    fs::remove_dir_all(f.profiles.join("Y")).unwrap();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    let out = f.run(&mut home_step(&f));
    assert_skipped(&f, &out, "reset_too_far");
}

// With no rotator nothing can end a `$HOME/.claude` step's wait but a login
// change, so a far reset fails at once as it did before #1801.
#[test]
fn a_home_step_with_two_logins_and_no_rotator_skips() {
    let f = with_profiles();
    x_is_live(&f);
    let out = f.run(&mut home_step(&f));
    assert_skipped(&f, &out, "reset_too_far");
}

// A rotator with auto-switch off keeps running but skips every cycle, so it
// records no cycle and makes no swap. It cannot end a `$HOME/.claude` step's
// wait, so it does not count: a far reset fails at once, not after six hours.
#[test]
fn a_home_step_with_a_running_rotator_and_auto_switch_off_skips() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    let off = f.run(&mut f.cmd(&["profile", "auto-switch", "off"]));
    assert!(off.status.success(), "auto-switch off: {}", describe(&off));
    let out = f.run(&mut home_step(&f));
    assert_skipped(&f, &out, "reset_too_far");
}

// The same holds for an inherited-profile step: no `/usage` probe is made, and
// in an unattended run nothing records a reading without a rotator, so the
// wait would run to its six-hour maximum. A far reset fails at once instead.
#[test]
fn an_inherited_profile_step_with_two_logins_and_no_rotator_skips() {
    let f = with_profiles();
    let mut c = nested_x(&f);
    // X, then the auto-switch rerun on Y, are both limited.
    c.env("LIMIT_CALLS", "2");
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(f.count("0"), 2, "X, then Y, and no retry after them");
    assert_eq!(events(&out), ["skipped reason=reset_too_far"]);
}

#[test]
fn ac3_one_stored_login_with_no_reset_skips() {
    let f = with_profiles();
    fs::remove_dir_all(f.profiles.join("Y")).unwrap();
    x_is_live(&f);
    let mut c = home_step(&f);
    c.env("LIMIT_MESSAGE", NO_RESET_TEXT);
    let out = f.run(&mut c);
    assert_skipped(&f, &out, "no_reset");
}

/// A spend limit never starts a **capacity** wait: it can apply to a whole
/// organisation, so another login of the same one would not lift it. Issue
/// #2731 gave the no-reset case a login-change wait instead, covered in
/// `issue_2731_login_change_wait`; here the weekly-reset case and the
/// no-rotator case keep their skips.
#[test]
fn ac3_a_spend_limit_with_no_reset_never_starts_a_capacity_wait() {
    // Issue #2882: a spend limit that states a reset now starts a capacity
    // wait (`issue_2882_spend_limit_capacity_wait`). One that states none
    // keeps its skip, or #2731's login-change wait with a rotator.
    // No rotator: nothing could move another login in, so it ends at once.
    let g = with_profiles();
    x_is_live(&g);
    let mut c = home_step(&g);
    c.env("LIMIT_MESSAGE", SPEND_TEXT);
    let out = g.run(&mut c);
    assert_skipped(&g, &out, "spend_limit");

    // A stated reset with no rotator is still `reset_too_far`.
    let h = with_profiles();
    x_is_live(&h);
    let mut c = home_step(&h);
    c.env("LIMIT_MESSAGE", SPEND_WEEKLY_TEXT);
    let out = h.run(&mut c);
    assert_skipped(&h, &out, "reset_too_far");
}

#[test]
fn ac8_no_profile_root_skips() {
    let f = Fixture::new();
    fs::remove_dir_all(&f.profiles).unwrap();
    let out = f.run(&mut home_step(&f));
    assert_skipped(&f, &out, "reset_too_far");
}

#[test]
fn ac8_an_empty_store_skips() {
    let f = Fixture::new();
    let out = f.run(&mut home_step(&f));
    assert_skipped(&f, &out, "reset_too_far");
}

#[test]
fn ac8_a_rotate_ledger_others_can_write_skips() {
    let f = with_profiles();
    x_is_live(&f);
    fs::set_permissions(
        rotate_ledger(&f.profiles),
        fs::Permissions::from_mode(0o622),
    )
    .unwrap();
    let out = f.run(&mut home_step(&f));
    assert_skipped(&f, &out, "reset_too_far");
}

#[test]
fn ac8_an_explicit_config_dir_skips_with_a_second_login() {
    let f = with_profiles();
    x_is_live(&f);
    let mut c = f.step();
    c.env("LIMIT_MESSAGE", WEEKLY_TEXT);
    let out = f.run(&mut c);
    assert_skipped(&f, &out, "reset_too_far");
}

#[test]
fn an_inherited_profile_with_auto_switch_off_skips() {
    let f = with_profiles();
    let mut c = f.step();
    c.env("AMPLIHACK_CLAUDE_PROFILE", "X")
        .env("CLAUDE_CONFIG_DIR", f.profiles.join("X"))
        .env(TOGGLE_ENV, "0")
        .env("LIMIT_MESSAGE", WEEKLY_TEXT);
    let out = f.run(&mut c);
    assert_skipped(&f, &out, "reset_too_far");
}

#[test]
fn a_default_pointer_that_is_not_the_live_profile_skips() {
    let f = with_profiles();
    write_cycle(&f.profiles, "Y", OLD_CYCLE);
    set_default(&f.profiles, "X");
    let out = f.run(&mut home_step(&f));
    assert_skipped(&f, &out, "reset_too_far");
}

/// A step with no inherited profile and the wait off is not supervised, so
/// it runs once and writes no wait line at all.
fn assert_ran_once_unsupervised(f: &Fixture, out: &Output) {
    assert_eq!(out.status.code(), Some(1), "{}", describe(out));
    assert_eq!(f.count("0"), 1, "no rerun");
    assert!(wait_lines(out).is_empty(), "{:?}", wait_lines(out));
}

#[test]
fn the_wait_turned_off_refuses_first() {
    let f = with_profiles();
    x_is_live(&f);
    let mut c = home_step(&f);
    c.env("AMPLIHACK_USAGE_LIMIT_WAIT", "0");
    let out = f.run(&mut c);
    assert_ran_once_unsupervised(&f, &out);
}

#[test]
fn ci_refuses_first() {
    let f = with_profiles();
    x_is_live(&f);
    let mut c = home_step(&f);
    c.env("CI", "true");
    let out = f.run(&mut c);
    assert_ran_once_unsupervised(&f, &out);
}

// --- a step with nowhere to go still sleeps to its reset (issue #2694) --------

/// The readings written before the run are still the ones on disk: the run
/// replaced none of them, so what the wait had in front of it the whole
/// time was a login with room.
fn assert_room_still_shown(f: &Fixture, at: i64) {
    let body = fs::read_to_string(f.profiles.join(".auto-switch.d").join("usage.json")).unwrap();
    let got: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(got["accounts"]["Y"]["observed_at"], at, "{body}");
    assert_eq!(
        got["accounts"]["Y"]["five_hour"]["used_percentage"], 10.0,
        "{body}"
    );
}

/// The wait ran to its reset and the step never changed login.
fn assert_slept_to_its_reset(f: &Fixture, out: &Output) {
    assert!(out.status.success(), "{}", describe(out));
    assert_eq!(f.count("0"), 2, "one wait and one retry: {}", describe(out));
    assert_eq!(
        events(out),
        vec![
            "start reason=usage_limit harness=claude reset=2026-10-18 08:30:00 -07:00 \
             until=2026-10-18 08:32:00 -07:00 bound=reset+margin",
            "end reason=reset reached",
            "resume attempt=2",
        ],
        "{}",
        describe(out)
    );
    assert!(
        !text(&out.stderr).contains("a login has capacity again"),
        "the step must not move login: {}",
        describe(out)
    );
    assert_eq!(
        f.call("0", 2)["PROFILE"],
        "<unset>",
        "no profile was chosen"
    );
}

/// Issue #2694, the other half of the change: the reading ends a wait only
/// where auto-switch has somewhere to move the step. A step given an
/// explicit `CLAUDE_CONFIG_DIR` inherited no profile, so nothing may
/// substitute its login, and its wait runs to its reset however much room
/// another stored login has.
#[test]
fn issue_2694_an_explicit_login_sleeps_to_its_reset_though_another_login_has_room() {
    let f = with_profiles();
    switch_on(&f);
    readings_with_room(&f.profiles, TEST_NOW + 60);
    let mut c = f.step();
    c.env(TOGGLE_ENV, "1").env("LIMIT_MESSAGE", LIMIT_TEXT);
    let out = f.run(&mut c);
    assert_slept_to_its_reset(&f, &out);
    assert_room_still_shown(&f, TEST_NOW + 60);
    assert_eq!(
        f.call("0", 2)["CLAUDE_CONFIG_DIR"],
        f.config.display().to_string(),
        "the retry keeps the login it was given"
    );
}

/// The same for a step on `$HOME/.claude` whose live login cannot be named:
/// the rotator may change that login under it, but auto-switch may not, so
/// the reading does not end this wait either.
///
/// Issue #2967 keeps this expectation by a different route. With no rotator
/// a step that *can* name its login moves to another one before waiting (see
/// `issue_2967_other_logins`, A1); this step has no live login on record, so
/// it cannot be marked, and it fails closed to the wait for its own reset.
#[test]
fn issue_2694_a_home_step_sleeps_to_its_reset_though_another_login_has_room() {
    let f = with_profiles();
    switch_on(&f);
    readings_with_room(&f.profiles, TEST_NOW + 60);
    let mut c = home_step(&f);
    c.env(TOGGLE_ENV, "1").env("LIMIT_MESSAGE", LIMIT_TEXT);
    let out = f.run(&mut c);
    assert_slept_to_its_reset(&f, &out);
    assert_room_still_shown(&f, TEST_NOW + 60);
}

/// Issue #2967: the auto-switch-off variant of the case above, with the
/// live login named. Auto-switch is the user's standing instruction, so with
/// it off no mechanism may move the step: the eligible set is its own login
/// and the wait is for its own reset, as before #2967. The environment
/// setting overrides the file that `switch_on` wrote.
#[test]
fn issue_2967_a_home_step_with_auto_switch_off_sleeps_to_its_reset_though_a_login_has_room() {
    let f = with_profiles();
    switch_on(&f);
    x_is_live(&f);
    readings_with_room(&f.profiles, TEST_NOW + 60);
    let mut c = home_step(&f);
    c.env(TOGGLE_ENV, "0").env("LIMIT_MESSAGE", LIMIT_TEXT);
    let out = f.run(&mut c);
    assert_slept_to_its_reset(&f, &out);
    assert_room_still_shown(&f, TEST_NOW + 60);
}

/// Requirement 14 / D8: the profiled half, with a rotator running.
///
/// Issue #2731 promotes a no-reset spend limit to a login-change wait only
/// on a `$HOME/.claude` step, and two separate things refuse the promotion
/// here, either of which would be enough on its own. `rerun` filters the
/// spend harness through `Waiter::is_home`, which is false for a step with
/// `CLAUDE_CONFIG_DIR` set; and `supervise_gate` builds an inherited
/// profile's `Waiter` with `home_rotator` hardcoded to `|| false`, so
/// `upgrade_spend` is told no rotator is running however the host is really
/// set up. A real rotator does run in this test and changes nothing, which
/// is the property worth asserting: the rotator swaps logins into
/// `~/.claude` and nowhere else, so a step pinned to profile X would not see
/// the new login even if one arrived, and another login of the same
/// organisation would not lift the limit anyway.
///
/// Because either refusal is sufficient, removing one of them alone leaves
/// this test passing. It fails when both go, which is what makes it a check
/// on the pair rather than on `is_home` by itself.
///
/// The sibling below is the `$HOME/.claude` step with no rotator, where
/// `upgrade_spend` does ask a live rotator question and gets `false`. The
/// case where a promotion is actually allowed — a home step with a rotator
/// — is the login-change wait itself, in
/// `issue_2731_login_change_wait.rs`.
#[test]
fn issue_2731_an_inherited_profile_spend_limit_skips_though_a_rotator_runs() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    readings_with_room(&f.profiles, TEST_NOW + 60);
    let mut c = nested_x(&f);
    // X, then the auto-switch rerun on Y, both hit the spend limit.
    c.env("LIMIT_MESSAGE", SPEND_TEXT).env("LIMIT_CALLS", "2");
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(f.count("0"), 2, "X, then Y, and no retry after them");
    assert_eq!(
        events(&out),
        ["skipped reason=spend_limit"],
        "a profiled step gets no login-change wait, rotator or not: {}",
        describe(&out)
    );
}

/// A spend limit is unchanged by #2694: another login of the same
/// organisation would not lift it, so one with room starts no wait and
/// gives no login-capacity exit. The step is passed up as it was.
///
/// Stated here on a `$HOME/.claude` step with no rotator running, which is
/// the case where nothing at all can replace the exhausted credit: only the
/// rotator moves a login into `~/.claude`, so with none there is nowhere for
/// the step to go and nothing to wait for, and the room another login has
/// changes none of that.
///
/// The two cases this one does not cover are next to it. With a rotator
/// running the same message starts a login-change wait rather than ending
/// the step:
/// `issue_2731_a_shared_home_spend_limit_waits_for_the_swap_instead` in
/// `issue_2731_login_change_wait.rs`, which asserts the same readings end
/// nothing. On a step with an inherited profile the skip stands even with a
/// rotator running:
/// `issue_2731_an_inherited_profile_spend_limit_skips_though_a_rotator_runs`
/// above.
#[test]
fn issue_2694_a_spend_limit_still_skips_with_room_on_another_login() {
    let f = with_profiles();
    x_is_live(&f);
    readings_with_room(&f.profiles, TEST_NOW + 60);
    let mut c = home_step(&f);
    c.env(TOGGLE_ENV, "1").env("LIMIT_MESSAGE", SPEND_TEXT);
    let out = f.run(&mut c);
    assert_skipped(&f, &out, "spend_limit");
    assert_room_still_shown(&f, TEST_NOW + 60);
    assert!(
        !text(&out.stderr).contains("a login has capacity again"),
        "the step must not move login: {}",
        describe(&out)
    );
}
