//! Issue #1801: a weekly limit, or one with no readable reset, on a step
//! whose login another login could replace waits for capacity, then runs
//! once more on the login that has it. It no longer ends the run at once.
//!
//! Issue #2911 widened that to every limit: a usage, spend or credit limit
//! never ends a step. Every terminal path a limit used to take is gone, so
//! the scenarios here that once ended the run at once now wait and carry on.
//!
//! - AC1, AC13: a capacity wait ends when a login shows capacity, and the retry
//!   runs on that login, never the exhausted one.
//! - AC3, AC8: no second login, no profile root, an empty store, a store that
//!   cannot be read, an explicit `CLAUDE_CONFIG_DIR`, auto-switch off, no
//!   rotator or a spend limit each used to end the step at once. Each now
//!   waits: refusing to wait because no login has room *this instant* is
//!   exactly the `reset_too_far` failure issue #2911 reports, and a login can
//!   gain room while the step waits.
//! - AC4, AC14 (issue #2911): a wait whose limit stated no usable reset has
//!   no deadline and ends only on an event, here a login change the test
//!   makes (docs/reference/usage-limit-waits.md, "A wait with no
//!   deadline"); no setting gives it a length. A weekly limit's reset is
//!   waited for to that reset, held to the seven-day window.
//!   `recipe.usage_limit_wait = false` is how to turn waiting off.
//! - AC15: a retry that hits the limit again gets a wait of its own.
//! - S24: the wait never runs the stub to probe a login.
//! - Issue #2694: the same exit now ends a reset wait, so a step whose
//!   stored profile states a near reset no longer sleeps to it while
//!   another login has room.
//! - Issue #2694: a step auto-switch cannot move, one on an explicit
//!   `CLAUDE_CONFIG_DIR` or on `$HOME/.claude`, still sleeps to its reset
//!   with the same readings in front of it, and no reading ends the
//!   login-change wait a spend limit now takes.
//! - Issue #2911: a wait with no reset it can end on writes a
//!   `waiting_for_capacity` line naming its reason immediately before its
//!   own `start`; a reset wait writes none. The only endings left are the
//!   step's own timeout, a stop signal, the end of its session, output with
//!   no limit message in it and a prompt that cannot be replayed.
//!
//! Tests that need something to happen during a wait run on the real clock,
//! scaled, with `TEST_NOW` removed: a reading or rotator cycle is stamped with
//! real time, and the wait's `start` must be on the same clock. A wait with
//! no deadline is ended instead by a login change the fixture makes once the
//! wait is under way (`Fixture::run_with_a_login_change_in`), which needs no
//! clock. The other tests use the fixture's virtual clock, so their log
//! times are exact. See `issue_1618_fixture`.

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
const NO_RESET_TEXT: &str = "You've hit your session limit · try again later";
/// `WEEKLY_TEXT` read at `TEST_NOW`: October 8 has passed, so it is next
/// year's, and the wait holds it to the seven-day window from `TEST_NOW`.
const WEEKLY_HELD: &str = "2026-10-25 08:00:00 -07:00";
/// That held reset plus the two-minute margin: a weekly wait's deadline.
const WEEKLY_UNTIL: &str = "2026-10-25 08:02:00 -07:00";

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
/// until the returned file is dropped. Issue #1801 let a step take a
/// capacity wait only while a rotator ran; since issue #2911 the wait starts
/// either way, and a running rotator is what can end one early with a cycle
/// that reports room.
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

/// Where a `$HOME/.claude` step's login lives, for a login change.
fn home_login(f: &Fixture) -> PathBuf {
    f.home.join(".claude")
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

/// The `until=` value of a wait line: the rest of the line, or the part
/// before ` bound=` on a `start` line.
fn until_of(line: &str) -> &str {
    let rest = line
        .split_once(" until=")
        .unwrap_or_else(|| panic!("no until= field in {line:?}"))
        .1;
    rest.split_once(" bound=").map_or(rest, |(t, _)| t)
}

/// Issue #2911: a wait with no reset it can end on says so plainly, naming
/// why, on the line immediately before its own `start`. `at` is where that
/// pair begins in `ev`.
fn assert_waits_for_capacity(ev: &[String], at: usize, why: &str) {
    assert!(
        ev[at].starts_with(&format!(
            "waiting_for_capacity reason={why} harness=claude until="
        )),
        "{ev:?}"
    );
    assert!(
        ev[at + 1].starts_with("start reason="),
        "the line comes immediately before the wait's own start: {ev:?}"
    );
    assert_eq!(
        until_of(&ev[at]),
        until_of(&ev[at + 1]),
        "both lines name this one wait's deadline: {ev:?}"
    );
}

/// The step waited for capacity, naming `why`, and ran again when the wait
/// ended; `calls` is how many times the stub ran in all. A far reset
/// (`reset_too_far`) ends at that reset, held to its window; a limit with no
/// usable reset has no deadline and ends on the login change the test made.
///
/// Issue #2911: each scenario asserted with this used to end the run at once
/// with `skipped reason=<why>`, because no login had room at the moment the
/// limit was seen. A login can gain room during the wait, so the wait starts
/// regardless and the step carries on.
fn assert_waited_then_retried(f: &Fixture, out: &Output, why: &str, calls: u32) {
    assert!(out.status.success(), "{why}: {}", describe(out));
    assert_eq!(f.count("0"), calls, "{why}: {}", describe(out));
    let ev = events(out);
    assert_waits_for_capacity(&ev, 0, why);
    let end = if why == "reset_too_far" {
        "end reason=reset reached"
    } else {
        "end reason=login changed"
    };
    assert_eq!(
        ev[2..],
        [end.to_string(), format!("resume attempt={calls}")],
        "{why}: {}",
        describe(out)
    );
    assert!(
        !ev.iter()
            .any(|l| l.starts_with("skipped ") || l.starts_with("failed ")),
        "{why}: no limit ends a step now: {ev:?}"
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

/// AC1: a weekly limit on a `$HOME/.claude` step waits for capacity, and the
/// rotator's next cycle ends the wait.
///
/// Issue #2911 added one line to the sequence: `waiting_for_capacity
/// reason=reset_too_far` before its own `start`. The wait's deadline is the
/// weekly reset, held to its window, not the outer bound.
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
    assert_eq!(ev.len(), 4, "{ev:?}");
    assert_waits_for_capacity(&ev, 0, "reset_too_far");
    assert!(
        ev[1].starts_with("start reason=usage_limit wait=capacity harness=claude reset=2"),
        "{ev:?}"
    );
    assert!(ev[1].ends_with(" bound=reset+margin"), "{ev:?}");
    assert_eq!(ev[2..], ["end reason=rotator capacity", "resume attempt=2"]);
    assert_eq!(
        f.call("0", 2)["PROFILE"],
        "<unset>",
        "the live $HOME/.claude"
    );
    assert_eq!(claude_stdout(&out), "attempt 2 output\n");
    assert_no_account_detail(&f, &out);
}

/// AC1: a limit that states no reset also waits, and its `start` line says
/// `reset=none`. Issue #2911 names the reason on the line before, and the
/// wait has no deadline: `until=none bound=none`. A login change ends it.
#[test]
fn ac1_no_readable_reset_also_waits_and_says_none() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f);
    c.env("LIMIT_MESSAGE", NO_RESET_TEXT);
    let out = f.run_with_a_login_change_in(&home_login(&f), &mut c);
    let ev = events(&out);
    assert_waits_for_capacity(&ev, 0, "no_reset");
    assert_eq!(
        ev[1],
        "start reason=usage_limit wait=capacity harness=claude reset=none until=none \
         bound=none",
        "{}",
        describe(&out)
    );
    assert_eq!(ev[2..], ["end reason=login changed", "resume attempt=2"]);
}

/// AC13: a step that inherited profile X waits for capacity and the retry
/// runs on the login that gained room, never the exhausted one. Issue #2911
/// put the `waiting_for_capacity` line in front of the wait's `start`.
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
    assert_waits_for_capacity(&ev, 0, "reset_too_far");
    assert!(ev[1].contains(" wait=capacity "), "{ev:?}");
    assert_eq!(ev[2..], ["end reason=login capacity", "resume attempt=3"]);
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
///
/// Issue #2911: the wait still has no reset it can sleep to, so it opens
/// with `waiting_for_capacity reason=reset_too_far`. That line is about the
/// wait, not about the retry, which is why saying nothing about lost room
/// afterwards is still the thing being pinned here.
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
    assert_eq!(ev.len(), 4, "{ev:?}");
    assert_waits_for_capacity(&ev, 0, "reset_too_far");
    assert!(
        ev[1].starts_with("start reason=usage_limit wait=capacity harness=claude reset="),
        "a reset past the five-hour ceiling is a capacity wait: {ev:?}"
    );
    assert!(
        ev[1].ends_with(" bound=reset+margin"),
        "a reset inside the six-hour maximum bounds the wait: {ev:?}"
    );
    assert_eq!(ev[2..], ["end reason=reset reached", "resume attempt=3"]);
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

/// Issue #2911: a capacity wait with no usable reset has no deadline, so
/// nothing timed runs the step again. It used to end the step with `failed
/// reason=capacity_expired` at its maximum, and later to retry there; now
/// the event that ends it - here a login change - is what earns the retry,
/// which is announced after the last progress line. S24 still holds: the
/// wait never runs the stub to probe a login.
#[test]
fn a_capacity_wait_with_no_reset_retries_on_an_event_instead_of_ending_the_step() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f);
    c.env("LIMIT_MESSAGE", NO_RESET_TEXT);
    let out = f.run_with_a_login_change_in(&home_login(&f), &mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2, "S24: one wait, one retry, no probe call");
    let ev = events(&out);
    assert_waits_for_capacity(&ev, 0, "no_reset");
    assert!(ev[1].ends_with(" until=none bound=none"), "{ev:?}");
    assert_eq!(ev[2..], ["end reason=login changed", "resume attempt=2"]);
    assert_eq!(
        wait_lines(&out).last().map(String::as_str),
        Some("resume attempt=2"),
        "the retry comes after the last progress line"
    );
    assert_eq!(claude_stdout(&out), "attempt 2 output\n");
}

/// G3, AC4 (issue #2911): a user config file that still sets the removed
/// `recipe.usage_limit_max_wait_secs` changes nothing about the wait. The
/// wait still has no deadline and still ends on its event.
#[test]
fn a_config_file_that_still_sets_the_removed_length_changes_nothing() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    f.write_config("[recipe]\nusage_limit_max_wait_secs = 300\n");
    let mut c = home_step(&f);
    c.env("LIMIT_MESSAGE", NO_RESET_TEXT);
    let out = f.run_with_a_login_change_in(&home_login(&f), &mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2, "one wait, then the retry");
    let ev = events(&out);
    assert_waits_for_capacity(&ev, 0, "no_reset");
    assert!(
        ev[1].starts_with("start reason=usage_limit wait=capacity "),
        "{ev:?}"
    );
    assert!(ev[1].ends_with(" until=none bound=none"), "{ev:?}");
    assert_eq!(ev[2..], ["end reason=login changed", "resume attempt=2"]);
    assert!(
        text(&out.stderr).contains(NO_RESET_TEXT),
        "the limited attempt's own message still reaches the step's stderr"
    );
    assert_no_account_detail(&f, &out);
}

/// A weekly limit is waited for to its reset, held to the seven-day window:
/// the step runs again at the reset, once, and not before.
#[test]
fn a_weekly_limit_is_waited_for_to_its_reset() {
    let f = with_profiles();
    x_is_live(&f);
    let mut c = home_step(&f);
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2, "the limited attempt and one at the reset");
    assert_eq!(
        events(&out),
        [
            format!(
                "waiting_for_capacity reason=reset_too_far harness=claude until={WEEKLY_UNTIL}"
            ),
            format!(
                "start reason=usage_limit wait=capacity harness=claude reset={WEEKLY_HELD} \
                 until={WEEKLY_UNTIL} bound=reset+margin"
            ),
            "end reason=reset reached".to_string(),
            "resume attempt=2".to_string(),
        ],
        "{}",
        describe(&out)
    );
}

/// Issue #3037: a step timeout does not end a capacity
/// wait. This step used to stop at `failed reason=step_timeout` with no
/// retry while its limit still stood. Nothing enforces that variable on a
/// step - recipe-runner-rs enforces only a step's YAML `timeout:` - so the
/// wait now ends on its own terms and the step retries.
#[test]
fn issue_3037_a_step_timeout_does_not_end_a_capacity_wait() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f);
    c.env("AMPLIHACK_STEP_TIMEOUT", "30");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2, "the step retried after its wait");
    let ev = events(&out);
    assert_waits_for_capacity(&ev, 0, "reset_too_far");
    assert!(!ev[1].contains("step_timeout"), "{ev:?}");
    assert!(
        ev.iter().any(|e| e == "resume attempt=2"),
        "the wait earned a retry: {ev:?}"
    );
    assert!(
        ev.iter()
            .all(|e| !e.contains("step timeout") && !e.contains("failed")),
        "{ev:?}"
    );
}

/// The limited attempt's own exit code is what the step exits with, wherever
/// the step ends on a limit at all.
///
/// Issues #2911 and #3037 took away the endings this test used to reach, a
/// maximum of `0` and then the step's own timeout: a limit never ends a
/// step now. What is left is an operator turning the wait off, which passes
/// the limited attempt's result straight up, and its code untouched.
#[test]
fn the_exit_code_passes_through_unchanged() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f);
    c.env("AMPLIHACK_USAGE_LIMIT_WAIT", "0")
        .env("LIMIT_EXIT", "7");
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(7), "{}", describe(&out));
    assert_eq!(f.count("0"), 1, "no retry with the wait turned off");
    assert!(
        events(&out).iter().all(|e| !e.starts_with("start ")),
        "no wait was started: {}",
        describe(&out)
    );
}

/// The `start` lines of `ev`, in order.
fn starts(ev: &[String]) -> Vec<&String> {
    ev.iter().filter(|l| l.starts_with("start ")).collect()
}

/// Issue #2731 split this test in two. AC15's rule is one wait per *login*,
/// not one per step, so what the retry runs on decides whether a second wait
/// starts. Here the rotator's cycle keeps X live, so the retry runs on the
/// login the step already waited on.
///
/// Issue #2857 kept the second wait: the first ended on the rotator's fresh
/// capacity, which says *try now*; the retry ran on X, the login that already
/// had its wait, and was limited, so the signal was wrong. The step waits
/// again and says so once, on a `reentered` line.
///
/// Issue #2911: the limit states no reset, so neither wait has a deadline,
/// and nothing ends the step when X's limit is met a second time. The cycle
/// written during the first wait is stamped in real time, after the virtual
/// clock's start of either wait, so it is fresh room for both, and the
/// second wait ends on it at its first read.
#[test]
fn ac15_a_retry_on_the_same_login_after_a_wake_waits_again() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f);
    // X, then the retry on X, then the attempt the second wait earns.
    c.env("LIMIT_CALLS", "2")
        .env("LIMIT_MESSAGE", NO_RESET_TEXT);
    let profiles = f.profiles.clone();
    let out = f.run_with_an_event_per_open_wait(&mut c, |n| {
        if n == 1 {
            // A fresh cycle with room, but X is still live: the wait ends
            // and the retry runs on the login that already had its wait.
            write_cycle(&profiles, "X", real_now());
        }
    });
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 3, "two waits, then the attempt that works");
    let ev = events(&out);
    let s = starts(&ev);
    assert_eq!(s.len(), 2, "{ev:?}");
    for l in &s {
        assert!(l.ends_with(" until=none bound=none"), "{ev:?}");
    }
    assert_eq!(
        ev.iter().filter(|l| l.starts_with("reentered ")).count(),
        1,
        "reported once: {ev:?}"
    );
    assert!(!ev.iter().any(|l| l.contains("already_waited")), "{ev:?}");
    assert!(
        !ev.iter().any(|l| l.starts_with("failed ")),
        "no limit ends the step now: {ev:?}"
    );
    assert_eq!(
        ev.last().map(String::as_str),
        Some("resume attempt=3"),
        "{ev:?}"
    );
    assert!(
        text(&out.stderr).contains(NO_RESET_TEXT),
        "the runner's classifier must still see the limit"
    );
}

/// The other half: the rotator made Y live, so the retry runs on a login
/// this step has not waited on, which reads as that login's first wait and
/// writes no `reentered` line.
///
/// Issue #2911: that second wait is a wait of its own, with no deadline, and
/// the step retries when it ends rather than ending with `failed
/// reason=capacity_expired`.
#[test]
fn ac15_a_retry_on_a_login_this_step_has_not_waited_on_waits_again() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f);
    // X, then the retry on Y, then the attempt the second wait earns.
    c.env("LIMIT_CALLS", "2")
        .env("LIMIT_MESSAGE", NO_RESET_TEXT);
    let profiles = f.profiles.clone();
    let out = f.run_with_an_event_per_open_wait(&mut c, |n| {
        if n == 1 {
            write_cycle(&profiles, "Y", real_now());
        }
    });
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        f.count("0"),
        3,
        "X, the retry on Y, then the one that works"
    );
    let ev = events(&out);
    let s = starts(&ev);
    assert_eq!(
        s.len(),
        2,
        "the login the step moved to gets a wait of its own: {ev:?}"
    );
    assert!(
        !ev.iter().any(|l| l.starts_with("reentered ")),
        "a first wait on this login, not a re-entry: {ev:?}"
    );
    assert!(!ev.iter().any(|l| l.contains("already_waited")), "{ev:?}");
    assert!(
        !ev.iter().any(|l| l.starts_with("failed ")),
        "no limit ends the step now: {ev:?}"
    );
    assert_eq!(
        ev.last().map(String::as_str),
        Some("resume attempt=3"),
        "{ev:?}"
    );
}

// --- what used to end the step at once, and now waits (issue #2911) -----------
//
// Every test in this section once asserted a terminal `skipped reason=...`.
// The scenarios are kept, because each of them is the case issue #2911 says
// must wait: a wait was refused because nothing could end it *at the moment
// the limit was seen*, which is a judgement about the next instant, not about
// the next six hours. A login can gain room, a rotator can be started and a
// store can be repaired while the step waits, and the wait's own wake
// conditions see any of those. A weekly limit's wait ends at its held reset,
// which the fixture's virtual clock reaches at once; a wait with no reset is
// ended by a login change the test makes.

/// One stored login, so nothing could take over, and a rotator whose cycles
/// cannot help either. It used to end the step with
/// `skipped reason=reset_too_far`; it now waits.
#[test]
fn ac3_one_stored_login_still_waits_even_with_rotator_cycles() {
    let f = with_profiles();
    fs::remove_dir_all(f.profiles.join("Y")).unwrap();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f);
    let out = f.run(&mut c);
    assert_waited_then_retried(&f, &out, "reset_too_far", 2);
}

/// No rotator runs, so nothing but a login change could end this step's wait
/// early. That used to end the step at once, as it did before #1801; the wait
/// now runs its length, because a rotator can be started during it, and the
/// step retries afterwards.
#[test]
fn a_home_step_with_two_logins_and_no_rotator_still_waits() {
    let f = with_profiles();
    x_is_live(&f);
    let mut c = home_step(&f);
    let out = f.run(&mut c);
    assert_waited_then_retried(&f, &out, "reset_too_far", 2);
}

/// A rotator with auto-switch off keeps running but skips every cycle, so it
/// records no cycle and makes no swap, and used not to count: the step ended
/// at once rather than waiting six hours for a swap that was not coming.
/// Auto-switch can be turned back on during the wait, so the wait starts
/// anyway and the step retries at its maximum.
#[test]
fn a_home_step_with_a_running_rotator_and_auto_switch_off_still_waits() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    let off = f.run(&mut f.cmd(&["profile", "auto-switch", "off"]));
    assert!(off.status.success(), "auto-switch off: {}", describe(&off));
    let mut c = home_step(&f);
    let out = f.run(&mut c);
    assert_waited_then_retried(&f, &out, "reset_too_far", 2);
}

/// The same for a step that inherited a profile: no `/usage` probe is made,
/// and in an unattended run with no rotator nothing records a reading, so the
/// wait runs to its maximum. That used to be the reason for ending the step
/// instead; the wait now runs, and the attempt after it is the one that
/// works.
#[test]
fn an_inherited_profile_step_with_two_logins_and_no_rotator_still_waits() {
    let f = with_profiles();
    let mut c = nested_x(&f);
    // X, then the auto-switch rerun on Y, are both limited.
    c.env("LIMIT_CALLS", "2");
    let out = f.run(&mut c);
    assert_waited_then_retried(&f, &out, "reset_too_far", 3);
}

/// One stored login and a message with no readable reset: it used to end the
/// step with `skipped reason=no_reset`, and the reason is now the wait's own.
#[test]
fn ac3_one_stored_login_with_no_reset_still_waits() {
    let f = with_profiles();
    fs::remove_dir_all(f.profiles.join("Y")).unwrap();
    x_is_live(&f);
    let mut c = home_step(&f);
    c.env("LIMIT_MESSAGE", NO_RESET_TEXT);
    let out = f.run_with_a_login_change_in(&home_login(&f), &mut c);
    assert_waited_then_retried(&f, &out, "no_reset", 2);
}

/// A spend limit still never starts a **capacity** wait: it can apply to a
/// whole organisation, so another login of the same one would not lift it,
/// and no capacity reading reports credit.
///
/// Issue #2911 inverted what happens instead. The no-reset case used to end
/// the step with `skipped reason=spend_limit` unless a rotator was running;
/// it now takes #2731's login-change wait whether one runs or not, which has
/// no deadline and ends when the login changes. A spend limit
/// that states a far reset used to end the step with
/// `skipped reason=reset_too_far`; it now takes the capacity wait any other
/// far reset takes (issue #2882).
#[test]
fn ac3_a_spend_limit_with_no_reset_waits_for_a_login_change_not_capacity() {
    // No rotator runs in either half, so nothing moves another login in: the
    // test changes the login by hand, as a person would.
    let g = with_profiles();
    x_is_live(&g);
    let mut c = home_step(&g);
    c.env("LIMIT_MESSAGE", SPEND_TEXT);
    let out = g.run_with_a_login_change_in(&home_login(&g), &mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(g.count("0"), 2, "one wait, then the retry");
    assert_eq!(
        events(&out),
        [
            "waiting_for_capacity reason=spend_limit harness=claude until=none",
            "start reason=spend_limit wait=login_change harness=claude reset=none \
             until=none bound=none",
            "end reason=login changed",
            "resume attempt=2",
        ],
        "a spend limit with no reset waits for the login itself to change: {}",
        describe(&out)
    );

    // A stated reset far away is a capacity wait, as any far reset is.
    let h = with_profiles();
    x_is_live(&h);
    let mut c = home_step(&h);
    c.env("LIMIT_MESSAGE", SPEND_WEEKLY_TEXT);
    let out = h.run(&mut c);
    assert_waited_then_retried(&h, &out, "reset_too_far", 2);
}

/// No profile root at all: there is no store to read, so no login could have
/// taken over and the step used to end at once. The root can be created
/// during the wait, so the wait starts regardless.
#[test]
fn ac8_no_profile_root_still_waits() {
    let f = Fixture::new();
    fs::remove_dir_all(&f.profiles).unwrap();
    let mut c = home_step(&f);
    let out = f.run(&mut c);
    assert_waited_then_retried(&f, &out, "reset_too_far", 2);
}

/// An empty store: the same as no root, and the same inversion.
#[test]
fn ac8_an_empty_store_still_waits() {
    let f = Fixture::new();
    let mut c = home_step(&f);
    let out = f.run(&mut c);
    assert_waited_then_retried(&f, &out, "reset_too_far", 2);
}

/// A rotate ledger others can write is not trusted, so the rotator's record
/// says nothing that could end the wait early, and the step used to end at
/// once. The permissions can be repaired during the wait, and a wait that
/// nothing ends still costs only its own length, so it starts.
#[test]
fn ac8_a_rotate_ledger_others_can_write_still_waits() {
    let f = with_profiles();
    x_is_live(&f);
    fs::set_permissions(
        rotate_ledger(&f.profiles),
        fs::Permissions::from_mode(0o622),
    )
    .unwrap();
    let mut c = home_step(&f);
    let out = f.run(&mut c);
    assert_waited_then_retried(&f, &out, "reset_too_far", 2);
}

/// A step given an explicit `CLAUDE_CONFIG_DIR` inherited no profile, so
/// nothing may substitute its login, however many are stored. It used to end
/// at once for that reason; the login in that directory can still be replaced
/// by whoever runs the step, so it waits and retries.
#[test]
fn ac8_an_explicit_config_dir_still_waits_with_a_second_login() {
    let f = with_profiles();
    x_is_live(&f);
    let mut c = f.step();
    c.env("LIMIT_MESSAGE", WEEKLY_TEXT);
    let out = f.run(&mut c);
    assert_waited_then_retried(&f, &out, "reset_too_far", 2);
}

/// An inherited profile with auto-switch off: no other profile may be
/// substituted, which used to end the step at once. Auto-switch can be turned
/// back on during the wait, so the wait starts and the step retries.
#[test]
fn an_inherited_profile_with_auto_switch_off_still_waits() {
    let f = with_profiles();
    let mut c = f.step();
    c.env("AMPLIHACK_CLAUDE_PROFILE", "X")
        .env("CLAUDE_CONFIG_DIR", f.profiles.join("X"))
        .env(TOGGLE_ENV, "0")
        .env("LIMIT_MESSAGE", WEEKLY_TEXT);
    let out = f.run(&mut c);
    assert_waited_then_retried(&f, &out, "reset_too_far", 2);
}

/// A `.default` pointer naming a profile the rotator does not have live: the
/// launch chose that login, so no takeover was possible and the step used to
/// end at once. It waits now, as every other limited step does.
#[test]
fn a_default_pointer_that_is_not_the_live_profile_still_waits() {
    let f = with_profiles();
    write_cycle(&f.profiles, "Y", OLD_CYCLE);
    set_default(&f.profiles, "X");
    let mut c = home_step(&f);
    let out = f.run(&mut c);
    assert_waited_then_retried(&f, &out, "reset_too_far", 2);
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

/// Issue #2911: `CI` used to refuse the wait as `skipped reason=ci` before
/// anything else was checked. It is not a setting any more, so a limited
/// step under `CI=true` waits and retries like any other.
#[test]
fn issue_2911_ci_does_not_refuse_the_wait() {
    let f = with_profiles();
    x_is_live(&f);
    let mut c = home_step(&f);
    c.env("CI", "true");
    let out = f.run(&mut c);
    assert_waited_then_retried(&f, &out, "reset_too_far", 2);
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

/// The same for a step on `$HOME/.claude`: the rotator may change that
/// login under it, but auto-switch may not, so the reading does not end
/// this wait either.
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

/// Requirement 14 / D8: the profiled half, with a rotator running.
///
/// Issue #2731 gave a no-reset spend limit a login-change wait, and refused
/// the promotion for a step that inherited a profile: `rerun` filtered the
/// spend harness through `Waiter::is_home`, and an inherited profile's
/// `Waiter` was built as though no rotator were running, so such a step
/// ended with `skipped reason=spend_limit` however the host was set up.
///
/// Issue #2911 removed every refusal, this one included: the limit's own
/// reading decides the wait, so a pinned step takes the same login-change
/// wait. What has not changed is that nothing here can end that wait early,
/// and that is the property worth keeping. The rotator swaps logins into
/// `~/.claude` and nowhere else, so a step pinned to profile X would not see
/// a new login even if one arrived, and a reading never ends a login-change
/// wait, because no reading reports an organisation's credit. The run starts
/// with a reading that shows Y has room, and each limited attempt replaces
/// the readings with its own exhausted ones; neither ends the wait. It has
/// no deadline (issue #2911), so it ends when the login in X's own directory
/// changes, which the test makes, and the step runs again: the limit delayed
/// the step rather than ending the run.
#[test]
fn issue_2731_an_inherited_profile_spend_limit_waits_though_nothing_can_lift_it() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    readings_with_room(&f.profiles, TEST_NOW + 60);
    let mut c = nested_x(&f);
    // X, then the auto-switch rerun on Y, both hit the spend limit.
    c.env("LIMIT_MESSAGE", SPEND_TEXT).env("LIMIT_CALLS", "2");
    let out = f.run_with_a_login_change_in(&f.profiles.join("X"), &mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        f.count("0"),
        3,
        "X, then Y, then the attempt after the wait"
    );
    let ev = events(&out);
    assert_waits_for_capacity(&ev, 0, "spend_limit");
    assert!(
        ev[1].contains(" wait=login_change "),
        "a spend limit with no reset waits for the login to change: {ev:?}"
    );
    assert_eq!(
        ev[2..],
        ["end reason=login changed", "resume attempt=3"],
        "no reading ended the wait, only the login change: {}",
        describe(&out)
    );
    assert_eq!(
        f.call("0", 3)["PROFILE"],
        "X",
        "the retry runs under the inherited profile, not on another login"
    );
}

/// A spend limit is unchanged by #2694 in the part that matters: another
/// login of the same organisation would not lift it, so a reading showing
/// room ends no wait of its and gives no login-capacity exit. The step does
/// not move login.
///
/// Issue #2911 changed what it does instead of moving. With no rotator
/// running nothing at all can replace the exhausted credit, and the step used
/// to end at once with `skipped reason=spend_limit`; it now waits for the
/// login in `$HOME/.claude` to change, because a rotator can be started or a
/// login swapped in by hand while it waits, and runs again when it does. The
/// readings with room are the ones written before the run throughout, so
/// what the wait had in front of it the whole time was a login with room,
/// and only the login change the test made ended it.
///
/// The case with a rotator running is
/// `issue_2731_a_shared_home_spend_limit_waits_for_the_swap_instead` in
/// `issue_2731_login_change_wait.rs`, which asserts the same readings end
/// nothing; the pinned-step case is
/// `issue_2731_an_inherited_profile_spend_limit_waits_though_nothing_can_lift_it`
/// above.
#[test]
fn issue_2694_a_spend_limit_waits_for_a_login_change_with_room_on_another_login() {
    let f = with_profiles();
    x_is_live(&f);
    readings_with_room(&f.profiles, TEST_NOW + 60);
    let mut c = home_step(&f);
    c.env(TOGGLE_ENV, "1").env("LIMIT_MESSAGE", SPEND_TEXT);
    let out = f.run_with_a_login_change_in(&home_login(&f), &mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2, "one wait, then the retry");
    let ev = events(&out);
    assert_waits_for_capacity(&ev, 0, "spend_limit");
    assert!(ev[1].contains(" wait=login_change "), "{ev:?}");
    assert_eq!(
        ev[2..],
        ["end reason=login changed", "resume attempt=2"],
        "a reading of another login's room ends no login-change wait: {}",
        describe(&out)
    );
    assert_room_still_shown(&f, TEST_NOW + 60);
    assert!(
        !text(&out.stderr).contains("a login has capacity again"),
        "the step must not move login: {}",
        describe(&out)
    );
    assert_eq!(
        f.call("0", 2)["PROFILE"],
        "<unset>",
        "the retry keeps the live $HOME/.claude login"
    );
}
