//! Issue #2731: a `$HOME/.claude` step's limit wait ends when the rotator
//! moves another login into `~/.claude`, and the wait is visible in the run's
//! recipe log.
//!
//! On a host where every step shares one `~/.claude` login, the rotator
//! reacts to the same exhausted reading that failed the attempt, so the swap
//! usually lands **while the attempt is failing**. The step then took the new
//! login as its baseline, saw nothing change afterwards, and slept to the
//! **old** account's reset. These tests cover the three things the issue asks
//! for:
//!
//! - AC1: a swap ends the wait and the step retries on the login now live,
//!   whether the swap lands during the failing attempt or during the wait;
//! - AC2: an organisation spend limit on such a step waits for the login to
//!   change instead of ending the step at once;
//! - AC3: a swap to a login that is also limited starts a second wait, keyed
//!   to that login, and a login already waited on does not;
//! - AC5, AC6: the wait's `start` and `end` lines reach the run's recipe log,
//!   and a run without one behaves exactly as it did. The `start` line's
//!   `reason` names the limit that started the wait, so `spend_limit` for an
//!   organisation spend limit and `usage_limit` for the rest;
//! - a rotator cycle that swaps nothing ends no login-change wait: a cycle
//!   that finds room on the live login and stays put is the ordinary cycle
//!   on a host whose credit has run out, and it lifts nothing;
//! - a shared `$HOME/.claude` spend limit waits for the swap where a
//!   profiled one still skips, which is the second half of #2717's
//!   `issue_2694_a_spend_limit_still_skips_with_room_on_another_login`;
//! - AC7: an explicit `CLAUDE_CONFIG_DIR` step and an inherited-profile step
//!   are untouched by a `~/.claude` login change.
//!
//! Every test runs in a tempfile `HOME` and profile root. Nothing reads or
//! writes the real `~/.claude` or `~/.amplihack`. Tests that need something
//! to happen during a wait act on a log line, never on a delay (issue #1973).

#![cfg(unix)]

use crate::issue_1618_fixture::{
    Fixture, SPEND_TEXT, SPEND_WEEKLY_TEXT, TEST_NOW, WAIT_PREFIX, claude_stdout, describe, events,
    executable, private_file, text, wait_lines,
};
use crate::issue_1618_wait_profiles::with_profiles;
use crate::issue_1801_capacity_wait::{
    OLD_CYCLE, assert_no_account_detail, real_clock, real_now, rotator_running, write_cycle,
    x_is_live,
};
use crate::support::wait::{CEILING, wait_until};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const MAX_WAIT_ENV: &str = "AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS";
const WAIT_LEDGER_ENV: &str = "AMPLIHACK_USAGE_WAIT_LEDGER";
const RECIPE_LOG_ENV: &str = "AMPLIHACK_RECIPE_LOG";
const TOGGLE_ENV: &str = "AMPLIHACK_CLAUDE_AUTO_SWITCH";

/// The reset wait `LIMIT_TEXT` asks for, on the fixture's virtual clock.
const RESET_START: &str = "start reason=usage_limit harness=claude \
                           reset=2026-10-18 08:30:00 -07:00 \
                           until=2026-10-18 08:32:00 -07:00 bound=reset+margin";
const FAILED_SPEND: &str = "failed reason=spend_limit: usage limit not lifted";
const FAILED_AGAIN: &str = "failed reason=already_waited: usage limit not lifted";

/// The stub `claude` these tests use. It is the #1618 stub with one addition:
/// `SWAP_ON_CALL`, a space-separated list of `<call>:<login>`, runs the
/// rotator's swap script while that call is failing, so the login in
/// `~/.claude` changes during the attempt rather than during the wait.
const SWAP_STUB: &str = r#"#!/bin/sh
case "$1" in
  --version) echo '2.1.319 (Claude Code)'; exit 0 ;;
esac
step="${STEP:-0}"
n=$(( $(cat "$RECORD_DIR/count.$step" 2>/dev/null || echo 0) + 1 ))
echo "$n" > "$RECORD_DIR/count.$step"
{
  printf 'PWD=%s\n' "$(pwd -P)"
  printf 'HOME=%s\n' "$HOME"
  printf 'CLAUDE_CONFIG_DIR=%s\n' "${CLAUDE_CONFIG_DIR-<unset>}"
  printf 'PROFILE=%s\n' "${AMPLIHACK_CLAUDE_PROFILE-<unset>}"
} > "$RECORD_DIR/call.$step.$n"
echo "attempt $n output"
for spec in $SWAP_ON_CALL; do
  if [ "${spec%%:*}" = "$n" ]; then "$SWAP_SCRIPT" "${spec#*:}"; fi
done
limit="${LIMIT_CALLS:-0}"
if [ "$n" -le "$limit" ]; then
  eval "msg=\${LIMIT_MESSAGE_$n-\$LIMIT_MESSAGE}"
  printf '%s\n' "$msg" >&2
  exit "${LIMIT_EXIT:-1}"
fi
exit 0
"#;

/// Replaces the rotate ledger's `live` with `$1`, in one rename, as the
/// rotator does under its own lock. The cycle it records is an old one, so
/// nothing here is fresh rotator capacity: the only thing that changes is
/// which login is live.
const SWAP_SCRIPT: &str = r#"#!/bin/sh
umask 077
d="$LEDGER_DIR"
printf '{"version":1,"live":"%s","alert":null,"last_cycle":{"at":%s,"outcome":"stayed"}}' \
  "$1" "$OLD_AT" > "$d/ledger.json.new"
chmod 600 "$d/ledger.json.new"
mv "$d/ledger.json.new" "$d/ledger.json"
"#;

fn rotate_dir(f: &Fixture) -> PathBuf {
    f.profiles.join(".auto-switch.d").join("rotate")
}

/// The login the rotate ledger names live now, as the step would read it.
fn live_login(f: &Fixture) -> Option<String> {
    let raw = fs::read_to_string(rotate_dir(f).join("ledger.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    Some(v.get("live")?.as_str()?.to_string())
}

/// A step on `$HOME/.claude` with the swap-capable stub installed.
fn home_step(f: &Fixture) -> Command {
    fs::create_dir_all(f.home.join(".claude")).unwrap();
    executable(&f.tools.join("claude"), SWAP_STUB);
    let swap = f.tools.join("swap-login");
    executable(&swap, SWAP_SCRIPT);
    let mut c = f.step();
    // `issue_1618_fixture::step()` sets CLAUDE_CONFIG_DIR; without removing
    // it this would be a config-directory step and every test here would
    // pass without exercising anything.
    c.env_remove("CLAUDE_CONFIG_DIR")
        .env("SWAP_SCRIPT", &swap)
        .env("LEDGER_DIR", rotate_dir(f))
        .env("OLD_AT", OLD_CYCLE.to_string());
    c
}

/// Auto-switch readings in which X is spent and Y has room, written as the
/// prototype rotator writes them. The same shape #1801 and #2717 use: the
/// readings a capacity probe would accept. Kept here rather than shared with
/// `issue_1801_capacity_wait.rs` because the question this file asks of them
/// is the opposite one, that they end nothing.
fn readings_with_room(profiles: &Path, at: i64) {
    let dir = profiles.join(".auto-switch.d");
    fs::create_dir_all(&dir).unwrap();
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
    private_file(&dir.join("usage.json"), &body.to_string());
}

/// The recipe log a detached run would have: private, regular, empty.
fn recipe_log(f: &Fixture) -> PathBuf {
    let log = f.root.join("recipe-logs").join("run.log");
    private_file(&log, "");
    log
}

/// The wait lines in the recipe log, prefix removed.
fn log_lines(log: &Path) -> Vec<String> {
    fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.strip_prefix(WAIT_PREFIX))
        .map(str::to_string)
        .collect()
}

/// Runs `c` and runs `then` once the wait has written its second `progress`
/// line, then waits for the next whole second.
///
/// The wait takes its login baseline from the first two 30-second probes
/// that agree, so a change that lands before the second probe *becomes* the
/// baseline and is never seen as a change. Progress lines are written every
/// 60 seconds on the same clock, so the second one is after the second
/// probe: acting on it is the event that says the baseline has settled.
/// Issue #1973: this waits on a line the run writes, never on a delay.
fn run_during_wait(f: &Fixture, c: &mut Command, then: impl FnOnce() + Send + 'static) -> Output {
    use std::sync::mpsc;
    let (tx, rx) = mpsc::channel::<i64>();
    let writer = std::thread::spawn(move || {
        let at = rx.recv_timeout(CEILING).unwrap_or_else(|e| {
            panic!("the wait never settled on a baseline within {CEILING:?}: {e}")
        });
        wait_until("the second after the baseline settled", || real_now() > at);
        then();
    });
    let progress = format!("{WAIT_PREFIX}progress ");
    let mut seen = 0u32;
    let mut tx = Some(tx);
    let out = f.run_watching(c, |line| {
        if line.starts_with(&progress) {
            seen += 1;
            if seen >= 2
                && let Some(tx) = tx.take()
            {
                tx.send(real_now()).expect("the writer is waiting");
            }
        }
    });
    drop(tx);
    if let Err(panic) = writer.join() {
        std::panic::resume_unwind(panic);
    }
    out
}

/// Clears the stub's call counter, so several runs sharing one fixture each
/// start at call 1 and each meet the limit.
fn reset_calls(f: &Fixture) {
    let _ = fs::remove_file(f.records.join("count.0"));
}

// --- AC1: the login changed while the attempt was failing --------------------

#[test]
fn ac1_a_swap_during_the_failing_attempt_ends_the_wait_before_it_begins() {
    let f = with_profiles();
    x_is_live(&f);
    let mut c = home_step(&f);
    c.env("SWAP_ON_CALL", "1:Y").env("LIMIT_CALLS", "1");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        live_login(&f).as_deref(),
        Some("Y"),
        "the swap really happened: without it this test proves nothing"
    );
    assert_eq!(f.count("0"), 2, "the step retried once");
    assert_eq!(
        events(&out),
        [RESET_START, "end reason=login changed", "resume attempt=2"],
        "{}",
        describe(&out)
    );
    assert!(
        wait_lines(&out).iter().all(|l| !l.starts_with("progress ")),
        "no time passed, so there is nothing to report progress on"
    );
    assert_eq!(claude_stdout(&out), "attempt 2 output\n");
    assert_no_account_detail(&f, &out);
    f.assert_isolated("0");
}

/// The step's own deadline bounds the retry above, not just the wait it
/// replaces. `Waiter::new` keeps the step timeout so a wait cannot outlive
/// the step; a retry started in the wait's place is a whole fresh attempt
/// and would outlive it by more. Nothing downstream re-checks the deadline,
/// so if this return did not, the step would run on past its timeout.
///
/// Same swap as the test above, and the only difference is that the step is
/// already out of time: the closing line is the step timeout and no second
/// attempt is launched.
#[test]
fn a_swap_does_not_start_a_retry_once_the_step_is_out_of_time() {
    let f = with_profiles();
    x_is_live(&f);
    let mut c = home_step(&f);
    c.env("SWAP_ON_CALL", "1:Y")
        .env("LIMIT_CALLS", "1")
        // Less than the 60-second lead, so the deadline is already behind
        // the wait before it is planned.
        .env("AMPLIHACK_STEP_TIMEOUT", "1");
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(
        live_login(&f).as_deref(),
        Some("Y"),
        "the swap really happened: without it this test proves nothing"
    );
    assert_eq!(
        f.count("0"),
        1,
        "the step was out of time, so no retry ran: {}",
        describe(&out)
    );
    let ev = events(&out);
    assert!(ev[0].ends_with(" bound=step_timeout"), "{ev:?}");
    assert_eq!(
        ev[1..],
        [
            "end reason=step timeout",
            "failed reason=step_timeout: usage limit not lifted",
        ],
        "the closing line keeps the failure class in the evidence tail: {ev:?}"
    );
}

#[test]
fn a_wait_that_ended_before_it_began_records_no_time_in_the_wait_ledger() {
    // Auto-drive subtracts this file from its wall budget. A wait that never
    // slept must not credit the step with time it did not spend.
    let f = with_profiles();
    x_is_live(&f);
    let ledger = f.root.join("wait-ledger");
    let mut c = home_step(&f);
    c.env("SWAP_ON_CALL", "1:Y")
        .env("LIMIT_CALLS", "1")
        .env(WAIT_LEDGER_ENV, &ledger);
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        fs::read_to_string(&ledger).unwrap_or_default(),
        "",
        "no wait, no line"
    );
}

#[test]
fn an_unchanged_login_still_waits_as_it_always_did() {
    // The comparison fails closed: nothing changed, so the step waits for the
    // reset exactly as it did before #2731.
    let f = with_profiles();
    x_is_live(&f);
    let out = f.run(&mut home_step(&f));
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        events(&out),
        [RESET_START, "end reason=reset reached", "resume attempt=2"]
    );
}

#[test]
fn a_ledger_that_cannot_be_read_is_not_a_login_change() {
    // The rotator replaces the ledger by rename; a read that lands in between
    // must cost at most the wait the step was going to make anyway, never an
    // attempt burnt on a login that is still at its limit.
    let f = with_profiles();
    x_is_live(&f);
    let mut c = home_step(&f);
    // The swap script writes a name the rotator would never write, so the
    // ledger parses to no live login at all.
    c.env("SWAP_ON_CALL", "1:../escape").env("LIMIT_CALLS", "1");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        events(&out),
        [RESET_START, "end reason=reset reached", "resume attempt=2"],
        "an unreadable login name reads as no change: {}",
        describe(&out)
    );
}

// --- AC1: the login changed during the wait ----------------------------------

#[test]
fn ac1_a_swap_during_the_wait_still_ends_it() {
    let f = with_profiles();
    x_is_live(&f);
    let mut c = home_step(&f);
    // An epoch limit message, so the reset is on the same real clock the
    // swap below is stamped with.
    let reset = real_now() + 3000;
    c.env(
        "LIMIT_MESSAGE",
        format!("Claude AI usage limit reached|{reset}"),
    )
    .env("LIMIT_CALLS", "1");
    real_clock(&mut c);
    // The reset is 50 minutes off, which at this scale is half a minute of
    // real time: the login change is what ends this wait, not the deadline.
    c.env("AMPLIHACK_USAGE_LIMIT_WAIT_TEST_SCALE", "100");
    let profiles = f.profiles.clone();
    let out = run_during_wait(&f, &mut c, move || {
        let dir = profiles.join(".auto-switch.d").join("rotate");
        let body = format!(
            r#"{{"version":1,"live":"Y","alert":null,"last_cycle":{{"at":{OLD_CYCLE},"outcome":"stayed"}}}}"#
        );
        let tmp = dir.join("ledger.json.new");
        fs::write(&tmp, &body).unwrap();
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600)).unwrap();
        fs::rename(&tmp, dir.join("ledger.json")).unwrap();
    });
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2);
    let ev = events(&out);
    assert_eq!(
        ev[ev.len() - 2..],
        ["end reason=login changed", "resume attempt=2"],
        "{ev:?}"
    );
    assert_eq!(
        f.call("0", 2)["CLAUDE_CONFIG_DIR"],
        "<unset>",
        "the retry runs on the login now live in ~/.claude"
    );
    assert_no_account_detail(&f, &out);
}

// --- AC2: an organisation spend limit ----------------------------------------

#[test]
fn ac2_a_spend_limit_on_a_home_step_waits_for_the_login_to_change() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f);
    c.env("LIMIT_MESSAGE", SPEND_TEXT)
        .env("SWAP_ON_CALL", "1:Y")
        .env("LIMIT_CALLS", "1");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2, "it no longer ends the run at once");
    assert_eq!(
        events(&out),
        [
            "start reason=spend_limit wait=login_change harness=claude reset=none \
             until=2026-10-18 14:00:00 -07:00 bound=max_wait",
            "end reason=login changed",
            "resume attempt=2",
        ],
        "{}",
        describe(&out)
    );
    assert_no_account_detail(&f, &out);
}

#[test]
fn a_spend_limit_that_nothing_lifts_ends_the_step_naming_it() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f);
    c.env("LIMIT_MESSAGE", SPEND_TEXT).env(MAX_WAIT_ENV, "0");
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(f.count("0"), 1, "no retry");
    assert_eq!(
        events(&out),
        [
            "start reason=spend_limit wait=login_change harness=claude reset=none \
             until=2026-10-18 08:00:00 -07:00 bound=max_wait",
            "end reason=max reached",
            FAILED_SPEND,
        ]
    );
    assert_eq!(
        wait_lines(&out).last().map(String::as_str),
        Some(FAILED_SPEND),
        "the closing line keeps the failure class in the runner's evidence tail"
    );
}

#[test]
fn a_step_timeout_ends_a_login_change_wait_with_the_closing_line() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f);
    c.env("LIMIT_MESSAGE", SPEND_TEXT)
        .env("AMPLIHACK_STEP_TIMEOUT", "30");
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    let ev = events(&out);
    assert!(ev[0].contains(" wait=login_change "), "{ev:?}");
    assert!(ev[0].ends_with(" bound=step_timeout"), "{ev:?}");
    assert_eq!(
        ev[1..],
        [
            "end reason=step timeout",
            "failed reason=step_timeout: usage limit not lifted",
        ]
    );
}

/// Issue #2717's `issue_2694_a_spend_limit_still_skips_with_room_on_another_login`
/// covered one case under one name. Issue #2731 makes it three, because
/// whether a spend limit skips now turns on what could replace the login,
/// and only one of the three answers changed:
///
/// - a step with an **inherited profile** still skips, rotator or not: the
///   rotator swaps logins into `~/.claude` and nowhere else. That is
///   `issue_2731_an_inherited_profile_spend_limit_skips_though_a_rotator_runs`
///   in `issue_1801_capacity_wait.rs`.
/// - a `$HOME/.claude` step with **no rotator** still skips: nothing can
///   move another login in. That case keeps the old name, in the same file.
/// - a `$HOME/.claude` step **with a rotator running** no longer skips: the
///   login that arrives next may belong to another organisation, so the
///   same message starts a login-change wait. That is this test.
///
/// The readings in front of it show another login with room throughout, so
/// this also proves the capacity reading does not end the wait: the swap
/// does.
#[test]
fn issue_2731_a_shared_home_spend_limit_waits_for_the_swap_instead() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    readings_with_room(&f.profiles, TEST_NOW + 60);
    let mut c = home_step(&f);
    c.env(TOGGLE_ENV, "1")
        .env("LIMIT_MESSAGE", SPEND_TEXT)
        .env("SWAP_ON_CALL", "1:Y")
        .env("LIMIT_CALLS", "1");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        f.count("0"),
        2,
        "the wait ends on the swap and the step retries once: {}",
        describe(&out)
    );
    let ev = events(&out);
    assert!(
        ev[0].contains(" wait=login_change "),
        "a spend limit on a shared login waits for the swap: {ev:?}"
    );
    assert_eq!(
        ev[1..],
        ["end reason=login changed", "resume attempt=2"],
        "the swap ends it, and the reading that showed room does not: {ev:?}"
    );
    assert!(
        !events(&out).iter().any(|l| l.contains("login capacity")),
        "no capacity reading may end a login-change wait: {ev:?}"
    );
    assert_eq!(
        f.call("0", 2)["CLAUDE_CONFIG_DIR"],
        "<unset>",
        "the retry runs on the login the rotator put in ~/.claude, not on \
         the one a reading picked"
    );
    assert_no_account_detail(&f, &out);
}

/// The deputy's point 4. A rotator cycle is not a swap. A cycle that looks
/// at the live login, finds its five-hour window has room and stays put
/// records fresh capacity, and on a host whose organisation credit has run
/// out that is the ordinary outcome of every cycle: the session windows look
/// healthy because no work is being done. Ending the wait there sends the
/// step straight back onto the same spent login.
#[test]
fn a_fresh_rotator_cycle_without_a_swap_does_not_end_a_spend_limit_wait() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f);
    c.env("LIMIT_MESSAGE", SPEND_TEXT).env(MAX_WAIT_ENV, "300");
    real_clock(&mut c);
    // 300 s of wait at this scale is half a minute of real time, and the
    // cycle below lands a little after the wait's baseline settles.
    c.env("AMPLIHACK_USAGE_LIMIT_WAIT_TEST_SCALE", "10");
    let profiles = f.profiles.clone();
    let out = run_during_wait(&f, &mut c, move || {
        // A fresh cycle, measured now, naming the same login live: the
        // rotator looked and stayed.
        write_cycle(&profiles, "X", real_now());
    });
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(f.count("0"), 1, "no retry on the login that is still spent");
    let ev = events(&out);
    assert!(ev[0].contains(" wait=login_change "), "{ev:?}");
    assert_eq!(
        ev[1..],
        ["end reason=max reached", FAILED_SPEND],
        "the wait ran to its own deadline: a cycle that swapped nothing \
         lifted nothing: {ev:?}"
    );
    assert!(
        !ev.iter().any(|l| l.contains("rotator capacity")),
        "fresh capacity on the live login is not a login change: {ev:?}"
    );
    assert_eq!(
        live_login(&f).as_deref(),
        Some("X"),
        "the test's own premise: the live login never changed"
    );
}

#[test]
fn a_spend_limit_with_no_rotator_running_still_skips() {
    // Only the rotator moves a login into ~/.claude, so with none running
    // there is nothing this wait could be waiting for.
    let f = with_profiles();
    x_is_live(&f);
    let mut c = home_step(&f);
    c.env("LIMIT_MESSAGE", SPEND_TEXT);
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(f.count("0"), 1, "no retry, and no six-hour sleep");
    assert_eq!(events(&out), ["skipped reason=spend_limit"]);
}

#[test]
fn a_spend_limit_on_a_config_dir_step_still_skips() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    // `step()` keeps CLAUDE_CONFIG_DIR: the rotator never changes it.
    let mut c = f.step();
    c.env("LIMIT_MESSAGE", SPEND_TEXT);
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(events(&out), ["skipped reason=spend_limit"]);
}

#[test]
fn a_spend_limit_with_a_weekly_reset_is_unchanged() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f);
    c.env("LIMIT_MESSAGE", SPEND_WEEKLY_TEXT);
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(
        events(&out),
        ["skipped reason=reset_too_far"],
        "only a spend limit with no readable reset reaches a login-change wait"
    );
}

// --- AC3: the new login is limited too ---------------------------------------

#[test]
fn ac3_a_swap_to_a_limited_login_waits_again_keyed_to_it() {
    let f = with_profiles();
    x_is_live(&f);
    let mut c = home_step(&f);
    c.env("SWAP_ON_CALL", "1:Y").env("LIMIT_CALLS", "2");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 3, "X, then Y, then the retry after Y's wait");
    assert_eq!(
        events(&out),
        [
            RESET_START,
            "end reason=login changed",
            "resume attempt=2",
            RESET_START,
            "end reason=reset reached",
            "resume attempt=3",
        ],
        "{}",
        describe(&out)
    );
    assert!(
        !events(&out).iter().any(|l| l.contains("already_waited")),
        "the login the step moved to has a wait of its own"
    );
    assert_no_account_detail(&f, &out);
}

#[test]
fn a_login_already_waited_on_gets_no_second_wait() {
    // The rotator swaps X -> Y and back to X. The step waited on X already,
    // so the chain ends rather than running for ever.
    let f = with_profiles();
    x_is_live(&f);
    let mut c = home_step(&f);
    c.env("SWAP_ON_CALL", "1:Y 2:X 3:Y").env("LIMIT_CALLS", "5");
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    let ev = events(&out);
    assert_eq!(
        ev[ev.len() - 2..],
        ["skipped reason=already_waited", FAILED_AGAIN],
        "{ev:?}"
    );
    assert!(
        f.count("0") <= 4,
        "the marks bound the chain; it ran {} attempts",
        f.count("0")
    );
}

#[test]
fn the_second_wait_is_bounded_by_the_steps_own_maximum() {
    // `max_wait_secs` is a budget for the whole step. The first wait here
    // ended before it began and spent none of it, so the second gets it all
    // and ends at the maximum rather than at the reset.
    let f = with_profiles();
    x_is_live(&f);
    let mut c = home_step(&f);
    c.env("SWAP_ON_CALL", "1:Y")
        .env("LIMIT_CALLS", "2")
        .env(MAX_WAIT_ENV, "60");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    let ev = events(&out);
    assert_eq!(ev.len(), 6, "{ev:?}");
    assert!(
        ev[3].ends_with("until=2026-10-18 08:01:00 -07:00 bound=max_wait"),
        "{ev:?}"
    );
    assert_eq!(ev[4..], ["end reason=max reached", "resume attempt=3"]);
}

// --- AC5: the wait in the recipe log -----------------------------------------

#[test]
fn ac5_the_recipe_log_carries_one_start_and_one_end_line_for_each_wait() {
    let f = with_profiles();
    x_is_live(&f);
    let log = recipe_log(&f);
    let mut c = home_step(&f);
    c.env("SWAP_ON_CALL", "1:Y")
        .env("LIMIT_CALLS", "2")
        .env(RECIPE_LOG_ENV, &log);
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        log_lines(&log),
        [
            "start reason=usage_limit account=X reset=2026-10-18 08:30:00 -07:00 \
             until=2026-10-18 08:32:00 -07:00 bound=reset+margin",
            "end reason=login changed",
            "start reason=usage_limit account=Y reset=2026-10-18 08:30:00 -07:00 \
             until=2026-10-18 08:32:00 -07:00 bound=reset+margin",
            "end reason=reset reached",
        ],
        "one pair per wait, naming the login each wait was for"
    );
}

#[test]
fn no_other_event_reaches_the_recipe_log() {
    let f = with_profiles();
    x_is_live(&f);
    let log = recipe_log(&f);
    let mut c = home_step(&f);
    c.env("LIMIT_CALLS", "5").env(RECIPE_LOG_ENV, &log);
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    let lines = log_lines(&log);
    assert_eq!(lines.len(), 2, "{lines:?}");
    for unwanted in ["progress ", "resume ", "skipped ", "failed "] {
        assert!(
            !lines.iter().any(|l| l.starts_with(unwanted)),
            "{unwanted:?} in {lines:?}"
        );
    }
    assert!(
        events(&out).iter().any(|l| l.starts_with("skipped ")),
        "the step's own stderr still carries the full account"
    );
}

#[test]
fn a_login_change_wait_names_its_kind_and_its_account_in_the_log() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    let log = recipe_log(&f);
    let mut c = home_step(&f);
    c.env("LIMIT_MESSAGE", SPEND_TEXT)
        .env(MAX_WAIT_ENV, "0")
        .env(RECIPE_LOG_ENV, &log);
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(
        log_lines(&log),
        [
            "start reason=spend_limit wait=login_change account=X reset=none \
             until=2026-10-18 08:00:00 -07:00 bound=max_wait",
            "end reason=max reached",
        ],
        "the start line names the limit, so a reader grepping the recipe \
         log for a spend limit finds the wait as well as the failure"
    );
}

#[test]
fn the_recipe_log_start_line_is_the_stderr_one_with_the_account_in_place_of_the_harness() {
    let f = with_profiles();
    x_is_live(&f);
    let log = recipe_log(&f);
    let mut c = home_step(&f);
    c.env(RECIPE_LOG_ENV, &log);
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    let stderr_start = events(&out)
        .into_iter()
        .find(|l| l.starts_with("start "))
        .expect("a start line on stderr");
    assert_eq!(
        log_lines(&log)[0],
        stderr_start.replace("harness=claude", "account=X"),
        "one grep over both sinks must read as one stream"
    );
}

#[test]
fn a_step_with_no_rotator_files_says_the_account_is_unknown() {
    // A host with no profile store has neither the rotate ledger nor the
    // swap record, so there is no login name to give.
    let f = Fixture::new();
    let log = recipe_log(&f);
    let mut c = home_step(&f);
    c.env(RECIPE_LOG_ENV, &log);
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert!(
        log_lines(&log)[0].contains(" account=unknown "),
        "{:?}",
        log_lines(&log)
    );
}

#[test]
fn the_recipe_log_lines_carry_nothing_but_the_login_name() {
    let f = with_profiles();
    x_is_live(&f);
    let log = recipe_log(&f);
    let mut c = home_step(&f);
    c.env(RECIPE_LOG_ENV, &log);
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    let profiles = f.profiles.display().to_string();
    assert!(!log_lines(&log).is_empty(), "nothing was written to check");
    for l in log_lines(&log) {
        for word in ["@", "uuid-", "SENTINEL", profiles.as_str()] {
            assert!(!l.contains(word), "{word:?} in {l:?}");
        }
    }
}

// --- AC6: a run with no recipe log -------------------------------------------

#[test]
fn ac6_with_no_recipe_log_the_step_behaves_exactly_as_it_did() {
    let f = with_profiles();
    x_is_live(&f);
    let mut c = home_step(&f);
    c.env("SWAP_ON_CALL", "1:Y").env("LIMIT_CALLS", "2");
    let plain = f.run(&mut c);

    let g = with_profiles();
    x_is_live(&g);
    let log = recipe_log(&g);
    let mut c = home_step(&g);
    c.env("SWAP_ON_CALL", "1:Y")
        .env("LIMIT_CALLS", "2")
        .env(RECIPE_LOG_ENV, &log);
    let logged = g.run(&mut c);

    assert_eq!(plain.status.code(), logged.status.code());
    assert_eq!(
        events(&plain),
        events(&logged),
        "the same stderr either way"
    );
}

#[test]
fn an_unusable_recipe_log_path_changes_nothing_and_creates_nothing() {
    let f = with_profiles();
    x_is_live(&f);
    let target = f.root.join("not-the-log");
    private_file(&target, "");
    let link = f.root.join("linked.log");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let cases: [(&str, PathBuf); 3] = [
        ("relative", PathBuf::from("recipe-logs/run.log")),
        ("a symlink", link.clone()),
        ("a missing directory", f.root.join("gone").join("run.log")),
    ];
    for (why, path) in cases {
        reset_calls(&f);
        let mut c = home_step(&f);
        c.env(RECIPE_LOG_ENV, &path);
        let out = f.run(&mut c);
        assert!(out.status.success(), "{why}: {}", describe(&out));
        assert_eq!(
            events(&out),
            [RESET_START, "end reason=reset reached", "resume attempt=2"],
            "{why}"
        );
    }
    assert_eq!(fs::read_to_string(&target).unwrap(), "", "symlink followed");
    assert!(!f.root.join("gone").exists(), "a directory was created");
    assert!(
        !f.work.join("recipe-logs").exists(),
        "a relative path resolved against the working directory"
    );
}

// --- AC7: steps a `~/.claude` login change must not touch ---------------------

#[test]
fn ac7_an_explicit_config_dir_step_is_unaffected_by_a_home_login_change() {
    let f = with_profiles();
    x_is_live(&f);
    executable(&f.tools.join("claude"), SWAP_STUB);
    let swap = f.tools.join("swap-login");
    executable(&swap, SWAP_SCRIPT);
    // `step()` keeps CLAUDE_CONFIG_DIR, which the rotator never changes.
    let mut c = f.step();
    c.env("SWAP_SCRIPT", &swap)
        .env("LEDGER_DIR", rotate_dir(&f))
        .env("OLD_AT", OLD_CYCLE.to_string())
        .env("SWAP_ON_CALL", "1:Y 2:X")
        .env("LIMIT_CALLS", "2");
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(
        live_login(&f).as_deref(),
        Some("X"),
        "the swaps really happened, twice"
    );
    assert_eq!(f.count("0"), 2, "one wait, one retry, then the step ends");
    assert_eq!(
        events(&out),
        [
            RESET_START,
            "end reason=reset reached",
            "resume attempt=2",
            "skipped reason=already_waited",
            FAILED_AGAIN,
        ],
        "a login change in ~/.claude says nothing about this step"
    );
}

#[test]
fn ac7_an_inherited_profile_step_is_unaffected_by_a_home_login_change() {
    let f = with_profiles();
    x_is_live(&f);
    executable(&f.tools.join("claude"), SWAP_STUB);
    let swap = f.tools.join("swap-login");
    executable(&swap, SWAP_SCRIPT);
    let mut c = f.step();
    c.env("AMPLIHACK_CLAUDE_PROFILE", "X")
        .env("CLAUDE_CONFIG_DIR", f.profiles.join("X"))
        .env(TOGGLE_ENV, "0")
        .env("SWAP_SCRIPT", &swap)
        .env("LEDGER_DIR", rotate_dir(&f))
        .env("OLD_AT", OLD_CYCLE.to_string())
        .env("SWAP_ON_CALL", "1:Y 2:X")
        .env("LIMIT_CALLS", "2");
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(live_login(&f).as_deref(), Some("X"), "the swaps happened");
    assert_eq!(f.count("0"), 2);
    for n in [1, 2] {
        assert_eq!(f.call("0", n)["PROFILE"], "X", "call {n}");
    }
    assert_eq!(
        events(&out),
        [
            RESET_START,
            "end reason=reset reached",
            "resume attempt=2",
            "skipped reason=already_waited",
            FAILED_AGAIN,
        ]
    );
}

#[test]
fn a_config_dir_step_writes_an_unknown_account_to_the_recipe_log() {
    // It has no login key at all, which is what `account=unknown` says.
    let f = with_profiles();
    x_is_live(&f);
    let log = recipe_log(&f);
    let mut c = f.step();
    c.env(RECIPE_LOG_ENV, &log);
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert!(
        log_lines(&log)[0].contains(" account=unknown "),
        "the rotator's live profile is not this step's login: {:?}",
        log_lines(&log)
    );
}

// --- the stub itself ----------------------------------------------------------

/// Nothing above is worth anything if the stub cannot change the ledger, so
/// this checks the harness rather than the product.
#[test]
fn the_stub_swaps_the_live_login_when_it_is_told_to() {
    let f = with_profiles();
    x_is_live(&f);
    assert_eq!(live_login(&f).as_deref(), Some("X"));
    let mut c = home_step(&f);
    c.env("SWAP_ON_CALL", "1:Y").env("LIMIT_CALLS", "0");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(live_login(&f).as_deref(), Some("Y"));
    assert!(
        text(&out.stderr).lines().all(|l| !l.contains(WAIT_PREFIX)),
        "no limit, so no wait at all"
    );
    assert_eq!(TEST_NOW, 1_792_335_600, "the fixture's virtual clock");
}
