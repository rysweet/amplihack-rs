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
//!
//! Tests that need something to happen during a wait run on the real clock,
//! scaled, with `TEST_NOW` removed: a reading or rotator cycle is stamped with
//! real time, and the wait's `start` must be on the same clock. The other
//! tests use the fixture's virtual clock, so their log times are exact. See
//! `issue_1618_fixture`.

#![cfg(unix)]

use crate::issue_1618_fixture::{
    Fixture, LIMIT_TEXT, SPEND_TEXT, SPEND_WEEKLY_TEXT, TEST_NOW, WEEKLY_TEXT, claude_stdout,
    describe, events, private_file, text, wait_lines,
};
use crate::issue_1618_wait_profiles::{switch_on, with_profiles};
use crate::support::profile_default::set_default;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const TOGGLE_ENV: &str = "AMPLIHACK_CLAUDE_AUTO_SWITCH";
const MAX_WAIT_ENV: &str = "AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS";
const NO_RESET_TEXT: &str = "You've hit your session limit · try again later";
/// `WEEKLY_TEXT` read at `TEST_NOW`: October 8 has passed, so it is next year's.
pub(crate) const WEEKLY_RESET: &str = "2027-10-08 00:00:00 -07:00";
const FAILED_EXPIRED: &str = "failed reason=capacity_expired: usage limit not lifted";
const FAILED_AGAIN: &str = "failed reason=already_waited: usage limit not lifted";

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

/// The rotate ledger: `live` is live, its last cycle measured at `at`.
pub(crate) fn write_cycle(profiles: &Path, live: &str, at: i64) {
    private_dirs(profiles);
    replace(
        &rotate_ledger(profiles),
        &format!(
            r#"{{"version":1,"live":"{live}","alert":null,"last_cycle":{{"at":{at},"measured_at":{at},"outcome":"swapped"}}}}"#
        ),
    );
}

/// A cycle from before the step on either clock: `TEST_NOW` is in the past.
pub(crate) const OLD_CYCLE: i64 = TEST_NOW - 3600;

/// A rotate ledger naming X as live, with a cycle from before the step.
pub(crate) fn x_is_live(f: &Fixture) {
    write_cycle(&f.profiles, "X", OLD_CYCLE);
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

/// Runs on the real clock, sped up: a 30 s poll takes 30 ms, and the longest
/// wait here, the default six hours, 21.6 s.
fn real_clock(c: &mut Command) {
    c.env_remove("AMPLIHACK_USAGE_LIMIT_WAIT_TEST_NOW")
        .env("AMPLIHACK_USAGE_LIMIT_WAIT_TEST_SCALE", "1000");
}

/// Runs `then` once the stub has been called `calls` times and the second
/// has turned over, so what it stamps is after the wait's start.
fn during_wait(f: &Fixture, calls: u32, then: impl FnOnce() + Send + 'static) -> JoinHandle<()> {
    let count = f.records.join("count.0");
    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            let seen = fs::read_to_string(&count)
                .ok()
                .and_then(|s| s.trim().parse::<u32>().ok());
            if seen == Some(calls) {
                std::thread::sleep(Duration::from_millis(1200));
                then();
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    })
}

fn rotator_cycle_during_wait(f: &Fixture) -> JoinHandle<()> {
    let profiles = f.profiles.clone();
    during_wait(f, 1, move || {
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
    let writer = rotator_cycle_during_wait(&f);
    let out = f.run(&mut c);
    writer.join().unwrap();
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
    let usage = f.profiles.join(".auto-switch.d").join("usage.json");
    let writer = during_wait(&f, 2, move || {
        let now = real_now();
        let reading = |pct: f64, source: &str| {
            serde_json::json!({
                "five_hour": {"used_percentage": pct, "resets_at": now + 3600},
                "seven_day": {"used_percentage": 5.0, "resets_at": now + 86_400},
                "observed_at": now,
                "source": source,
            })
        };
        let body = serde_json::json!({
            "version": 1,
            "accounts": {"X": reading(100.0, "statusline"), "Y": reading(10.0, "statusline")},
        });
        replace(&usage, &body.to_string());
    });
    let out = f.run(&mut c);
    writer.join().unwrap();
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

#[test]
fn ac15_a_retry_that_hits_the_limit_again_gets_no_second_wait() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f);
    c.env("LIMIT_CALLS", "5");
    real_clock(&mut c);
    let writer = rotator_cycle_during_wait(&f);
    let out = f.run(&mut c);
    writer.join().unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(f.count("0"), 2, "one wait, one retry, then give up");
    let ev = events(&out);
    assert_eq!(ev.iter().filter(|l| l.starts_with("start ")).count(), 1);
    assert_eq!(
        ev[ev.len() - 2..],
        ["skipped reason=already_waited", FAILED_AGAIN],
        "{ev:?}"
    );
    assert!(
        text(&out.stderr).contains("You've hit your weekly limit"),
        "the runner's classifier must still see the limit"
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

#[test]
fn ac3_a_spend_limit_skips_whatever_it_states_with_a_second_login() {
    for (message, reason) in [
        (SPEND_WEEKLY_TEXT, "reset_too_far"),
        (SPEND_TEXT, "spend_limit"),
    ] {
        let f = with_profiles();
        x_is_live(&f);
        let _rotator = rotator_running(&f);
        let mut c = home_step(&f);
        c.env("LIMIT_MESSAGE", message);
        let out = f.run(&mut c);
        assert_skipped(&f, &out, reason);
    }
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
