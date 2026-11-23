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
//!   to that login, and a login that comes round again is waited on again;
//! - AC5, AC6: the wait's `waiting_for_capacity`, `start` and `end` lines
//!   reach the run's recipe log, and a run without one behaves exactly as it
//!   did. The `start` line's `reason` names the limit that started the wait,
//!   so `spend_limit` for an organisation spend limit and `usage_limit` for
//!   the rest;
//! - a rotator cycle that swaps nothing ends no login-change wait: a cycle
//!   that finds room on the live login and stays put is the ordinary cycle
//!   on a host whose credit has run out, and it lifts nothing;
//! - AC7: an explicit `CLAUDE_CONFIG_DIR` step and an inherited-profile step
//!   are untouched by a `~/.claude` login change.
//!
//! Issue #2911 changed what a wait that lifts nothing costs. A spend limit
//! used to end the step, either at once (`skipped reason=spend_limit`, when
//! no rotator was running or the step's login was not one the rotator could
//! move) or when the wait ran out (`failed reason=spend_limit`). Neither
//! line exists now. Every spend limit with no readable reset starts a
//! login-change wait, whatever is or is not running, and a wait that
//! reaches its own deadline retries the step. No closing line is left in
//! this file: the last, `failed reason=step_timeout`, went with issue #3037,
//! because a step timeout no longer ends a limit wait. `max_wait_secs` is the length
//! of one wait rather than a budget for the whole step, and a configured
//! `0` reads as the 60-second floor, so the tests here that pin a short
//! wait get 60 seconds of the virtual clock.
//!
//! Every wait with no reset to end on also writes a `waiting_for_capacity`
//! line, naming the reason and the login, immediately before its `start`;
//! it is the first event of most of the sequences below.
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
/// The two lines a spend limit writes on the fixture's virtual clock when
/// the wait is one minute long: the `waiting_for_capacity` line #2911 added
/// and the `start` line after it. One minute is what `MAX_WAIT_ENV=0` and
/// `MAX_WAIT_ENV=60` both ask for, because the configured value is floored
/// at 60 seconds.
const SPEND_WAITING_A_MINUTE: &str =
    "waiting_for_capacity reason=spend_limit harness=claude until=2026-10-18 08:01:00 -07:00";
const SPEND_START_A_MINUTE: &str = "start reason=spend_limit wait=login_change harness=claude \
                                    reset=none until=2026-10-18 08:01:00 -07:00 bound=max_wait";

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
/// which login is live. It records `live_room: true` (issue #2857): a wait on
/// a reset or capacity ends on a login change only when the login now live
/// has room, and these tests are about the login-change path itself.
/// `issue_2857_wakes_only_on_room.rs` covers the same swap without room.
const SWAP_SCRIPT: &str = r#"#!/bin/sh
umask 077
d="$LEDGER_DIR"
printf '{"version":1,"live":"%s","alert":null,"last_cycle":{"at":%s,"outcome":"stayed","live_room":true}}' \
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

/// Issue #3037: the same swap with a step timeout
/// already behind the clock. This step used to stop at `failed
/// reason=step_timeout` instead of taking the retry the swap earned, on the
/// reading that the recipe runner kills a step at that time. It does not -
/// recipe-runner-rs enforces only a step's YAML `timeout:` - so the retry
/// runs, as it does with no timeout set.
#[test]
fn issue_3037_a_swap_starts_its_retry_whatever_the_step_timeout() {
    let f = with_profiles();
    x_is_live(&f);
    let mut c = home_step(&f);
    c.env("SWAP_ON_CALL", "1:Y")
        .env("LIMIT_CALLS", "1")
        .env("AMPLIHACK_STEP_TIMEOUT", "1");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        live_login(&f).as_deref(),
        Some("Y"),
        "the swap really happened: without it this test proves nothing"
    );
    assert_eq!(f.count("0"), 2, "the retry ran: {}", describe(&out));
    let ev = events(&out);
    assert!(!ev[0].contains("step_timeout"), "{ev:?}");
    assert!(
        ev.iter().any(|e| e == "end reason=login changed"),
        "the swap ended the wait before it began: {ev:?}"
    );
    assert!(
        ev.iter()
            .all(|e| !e.contains("step timeout") && !e.contains("failed")),
        "{ev:?}"
    );
    assert_eq!(claude_stdout(&out), "attempt 2 output\n");
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
            r#"{{"version":1,"live":"Y","alert":null,"last_cycle":{{"at":{OLD_CYCLE},"outcome":"stayed","live_room":true}}}}"#
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

/// The wait's own `waiting_for_capacity` line comes first (issue #2911):
/// such a wait has no reset to end on, so it says why and names the login
/// before the `start` line that follows it.
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
            "waiting_for_capacity reason=spend_limit harness=claude \
             until=2026-10-18 14:00:00 -07:00",
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

/// Issue #2911's own failure, inverted. This used to be
/// `a_spend_limit_that_nothing_lifts_ends_the_step_naming_it`, and it
/// asserted `failed reason=spend_limit: usage limit not lifted` and no
/// retry: a wait that reached its deadline ended the run. That line is
/// gone, and with it the whole of `FailReason`. The
/// wait now runs its length and the step tries again, which is the point of
/// the issue: nothing a limit says may end a run.
///
/// `MAX_WAIT_ENV=0` is what pins the floor. It used to mean "do not wait";
/// it now reads as the 60-second minimum, so the wait is real and one
/// minute long on the virtual clock. `recipe.usage_limit_wait = false` is
/// the only way left to turn waiting off.
#[test]
fn a_spend_limit_that_nothing_lifts_waits_its_length_and_retries() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f);
    c.env("LIMIT_MESSAGE", SPEND_TEXT).env(MAX_WAIT_ENV, "0");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        f.count("0"),
        2,
        "the wait ran out and the step tried again: {}",
        describe(&out)
    );
    assert_eq!(
        events(&out),
        [
            SPEND_WAITING_A_MINUTE,
            SPEND_START_A_MINUTE,
            "end reason=max reached",
            "resume attempt=2",
        ],
        "{}",
        describe(&out)
    );
    assert!(
        !wait_lines(&out).iter().any(|l| l.starts_with("failed ")),
        "a spend limit no longer closes a step: {:?}",
        wait_lines(&out)
    );
}

/// Issue #3037: a step timeout does not end a
/// login-change wait. It used to be the one thing that still closed such a
/// wait with `failed reason=step_timeout`; the wait now runs its own length
/// and the step retries.
#[test]
fn issue_3037_a_step_timeout_does_not_end_a_login_change_wait() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f);
    c.env("LIMIT_MESSAGE", SPEND_TEXT)
        .env("AMPLIHACK_STEP_TIMEOUT", "30");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2, "the step retried after its wait");
    let ev = events(&out);
    assert!(
        ev[0].starts_with("waiting_for_capacity reason=spend_limit harness=claude until="),
        "{ev:?}"
    );
    assert!(ev[1].contains(" wait=login_change "), "{ev:?}");
    assert!(ev[1].ends_with(" bound=max_wait"), "{ev:?}");
    assert!(ev.iter().any(|e| e == "resume attempt=2"), "{ev:?}");
    assert!(
        ev.iter()
            .all(|e| !e.contains("step timeout") && !e.contains("failed")),
        "{ev:?}"
    );
}

/// A spend limit on a shared `$HOME/.claude` login waits for the swap.
///
/// Issue #2731 made this one of three cases, the other two being a step
/// with an inherited profile and a `$HOME/.claude` step with no rotator
/// running, and only this one waited. Issue #2911 collapsed them again in
/// the other direction: every spend limit with no readable reset now waits,
/// whatever could or could not replace the login, because refusing to wait
/// because nothing has room *now* is the failure being fixed. The
/// no-rotator and config-directory tests below cover the two cases that
/// used to skip.
///
/// What is still particular to this case is what ends the wait. The
/// readings in front of it show another login with room throughout, and the
/// wait ignores them: a reading reports session and weekly windows, never
/// an organisation's credit, so only the swap ends it.
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
    assert_eq!(
        ev[0],
        "waiting_for_capacity reason=spend_limit harness=claude \
                until=2026-10-18 14:00:00 -07:00",
        "the wait says why it has no reset to end on: {ev:?}"
    );
    assert!(
        ev[1].contains(" wait=login_change "),
        "a spend limit on a shared login waits for the swap: {ev:?}"
    );
    assert_eq!(
        ev[2..],
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
///
/// The cycle lifting nothing used to show as the step ending on
/// `failed reason=spend_limit`. Issue #2911 removed that line: reaching the
/// wait's own deadline now retries, so what the cycle not being a swap
/// costs is the whole of the wait rather than the run, and the end to look
/// for is `max reached` rather than `rotator capacity` or `login changed`.
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
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        f.count("0"),
        2,
        "the wait ran its length and the step tried again: {}",
        describe(&out)
    );
    let ev = events(&out);
    assert!(
        ev[0].starts_with("waiting_for_capacity reason=spend_limit harness=claude until="),
        "{ev:?}"
    );
    assert!(ev[1].contains(" wait=login_change "), "{ev:?}");
    assert_eq!(
        ev[ev.len() - 2..],
        ["end reason=max reached", "resume attempt=2"],
        "the wait ran to its own deadline: a cycle that swapped nothing \
         lifted nothing: {ev:?}"
    );
    assert!(
        !ev.iter().any(|l| l.contains("rotator capacity")),
        "fresh capacity on the live login is not a login change: {ev:?}"
    );
    assert!(
        !ev.iter().any(|l| l.starts_with("failed ")),
        "the deadline retries the step rather than closing it: {ev:?}"
    );
    assert_eq!(
        live_login(&f).as_deref(),
        Some("X"),
        "the test's own premise: the live login never changed"
    );
}

/// With no rotator running the step still waits, and then retries.
///
/// This used to be `a_spend_limit_with_no_rotator_running_still_skips`, and
/// it asserted `skipped reason=spend_limit`: only the rotator moves a login
/// into `~/.claude`, so with none running the step was held to have nothing
/// to wait for. Issue #2911 deleted that reasoning along with
/// `plan::upgrade_spend`. A rotator can be started, or a login can gain
/// credit, while the step is waiting, and refusing to wait because nothing
/// can help *now* is exactly the failure the issue reports. So the wait is
/// made, nothing ends it early, and reaching its deadline retries the step.
#[test]
fn a_spend_limit_with_no_rotator_running_still_waits_then_retries() {
    let f = with_profiles();
    x_is_live(&f);
    let mut c = home_step(&f);
    // One minute of the virtual clock, so the wait is real but short.
    c.env("LIMIT_MESSAGE", SPEND_TEXT).env(MAX_WAIT_ENV, "60");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        f.count("0"),
        2,
        "the wait ran out and the step tried again: {}",
        describe(&out)
    );
    assert_eq!(
        events(&out),
        [
            SPEND_WAITING_A_MINUTE,
            SPEND_START_A_MINUTE,
            "end reason=max reached",
            "resume attempt=2",
        ],
        "{}",
        describe(&out)
    );
}

/// A step with an explicit `CLAUDE_CONFIG_DIR` waits as well, and the wait
/// is still shaped as a login-change wait.
///
/// This used to be `a_spend_limit_on_a_config_dir_step_still_skips`, and it
/// asserted `skipped reason=spend_limit`: the rotator swaps logins into
/// `~/.claude` and nowhere else, so this step's login is not one it can
/// move. Issue #2911 makes the shape of the wait a reading of the *message*
/// alone (`plan::wait_for_message`), so a spend limit stating no reset is a
/// login-change wait whatever directory the step runs in. Nothing about
/// this step's login can end it early, which is what the sequence shows:
/// the wait runs its whole length and then the step retries, exactly as the
/// no-rotator case above does.
#[test]
fn a_spend_limit_on_a_config_dir_step_waits_too_and_retries() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    // `step()` keeps CLAUDE_CONFIG_DIR: the rotator never changes it.
    let mut c = f.step();
    c.env("LIMIT_MESSAGE", SPEND_TEXT).env(MAX_WAIT_ENV, "60");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2, "{}", describe(&out));
    assert_eq!(
        events(&out),
        [
            SPEND_WAITING_A_MINUTE,
            SPEND_START_A_MINUTE,
            "end reason=max reached",
            "resume attempt=2",
        ],
        "{}",
        describe(&out)
    );
}

/// Issue #2882 supersedes the old row: a spend limit that states a reset is
/// a capacity wait when another login could take over, not `reset_too_far`.
/// Only a spend limit with no readable reset reaches a login-change wait.
///
/// The reset here is a year out, so the wait is a capacity one and says so
/// twice: `waiting_for_capacity reason=reset_too_far` and then `wait=capacity`
/// on the `start` line (issue #2911). Reaching a capacity wait's deadline
/// used to write `failed reason=capacity_expired` and end the step; it now
/// retries, and `capacity expired` is only the reason the wait ended.
#[test]
fn a_spend_limit_with_a_weekly_reset_is_a_capacity_wait_not_a_login_change_wait() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f);
    c.env("LIMIT_MESSAGE", SPEND_WEEKLY_TEXT)
        .env(MAX_WAIT_ENV, "0");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    let ev = events(&out);
    // Next year's Oct 8, held to a spend limit's 62-day span, is the wait's
    // deadline; the configured maximum does not cut it short.
    assert_eq!(
        ev[0],
        "waiting_for_capacity reason=reset_too_far harness=claude \
         until=2026-12-19 07:02:00 -08:00",
        "{ev:?}: {}",
        describe(&out)
    );
    assert!(
        ev[1].starts_with("start reason=usage_limit wait=capacity harness=claude reset=2")
            && ev[1].ends_with(" bound=reset+margin"),
        "{ev:?}: {}",
        describe(&out)
    );
    assert!(
        !ev.iter().any(|l| l.contains("reason=spend_limit")),
        "a stated reset is not a login-change wait: {ev:?}"
    );
    assert_eq!(
        ev[2..],
        ["end reason=reset reached", "resume attempt=2"],
        "{ev:?}"
    );
}

// --- AC3: the new login is limited too ---------------------------------------

/// The login the step moved to gets a wait of its own: two `start` lines,
/// two ends, two retries.
///
/// This used to add that no `already_waited` line was written. Issue #2911
/// removed `Skip::AlreadyWaited` and `FailReason::AlreadyWaited` outright,
/// so there is no longer such a line to find anywhere, and the exact event
/// sequence is the whole of the proof.
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
    assert_no_account_detail(&f, &out);
}

/// A login that comes round again is waited on again, and the chain of
/// waits runs for as long as the limits do.
///
/// Three issues have rewritten this test. It first said that a login the
/// step had already waited on gets no second wait, so the marks ended an
/// oscillating rotator's chain. Issue #2857 lifted that refusal, because
/// every wait here ends on a login change, which says *try now*, so a limit
/// on the retry is the signal being wrong: X and then Y are waited on
/// again, each time with one `reentered` line. It then said the step's
/// *budget* ended the chain. Issue #2911 removed the budget, so nothing
/// ends a chain of limits at all: `max_wait_secs` is the outer bound of one
/// capacity or login-change wait and a step waits as many times as it meets a
/// limit. What bounds this
/// test is therefore the stub, not the product: after `LIMIT_CALLS` the
/// stub stops reporting a limit.
///
/// The rotator stops swapping after call 3, the fourth attempt waits out
/// its reset, and the fifth meets the same message read at 08:32, a reset
/// two minutes past. That is a re-entry as well, and a capacity wait
/// (`passed_reset`); its deadline retries rather than ending the step,
/// which is where `failed reason=capacity_expired` used to be written. The
/// sixth attempt, which the stub no longer limits, succeeds.
#[test]
fn a_login_that_comes_round_again_is_waited_on_again_and_the_chain_continues() {
    let f = with_profiles();
    x_is_live(&f);
    let mut c = home_step(&f);
    c.env("SWAP_ON_CALL", "1:Y 2:X 3:Y").env("LIMIT_CALLS", "5");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    let ev = events(&out);
    assert_eq!(
        ev[ev.len() - 2..],
        ["end reason=capacity expired", "resume attempt=6"],
        "the last wait's deadline retries the step: {ev:?}"
    );
    assert_eq!(
        f.count("0"),
        6,
        "five limits, each waited out, then the attempt the stub allows"
    );
    assert_eq!(
        ev.iter()
            .filter(|l| *l == "reentered met=usage_limit")
            .count(),
        3,
        "X came round again at call 3, Y at call 4, and the fifth limit after the reset wait: {ev:?}"
    );
    assert!(
        !ev.iter().any(|l| l.starts_with("failed ")),
        "no limit in the chain closed the step: {ev:?}"
    );
}

/// The second wait is a reset wait, and
/// a reset wait is not cut short by `max_wait_secs`. It used to be: this
/// test pinned the second wait ending at the one-minute maximum with `max
/// reached` and the agent run again half an hour before the reset, a
/// re-entry on a timer. It now runs to the reset, the event it waits for.
/// The first wait here ended before it began (`login changed`), so the
/// second starts from the same instant and would have had its whole
/// minute; the configured length is still read, and still ignored by a
/// reset wait.
#[test]
fn the_second_wait_runs_to_its_reset_however_short_the_maximum() {
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
        ev[3].ends_with("until=2026-10-18 08:32:00 -07:00 bound=reset+margin"),
        "{ev:?}"
    );
    assert_eq!(ev[4..], ["end reason=reset reached", "resume attempt=3"]);
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

/// Exactly four kinds of line reach the recipe log: `start`,
/// `waiting_for_capacity`, `end` and `reentered`. Everything else the wait
/// writes stays on the step's own stderr.
///
/// Issue #2911 added `waiting_for_capacity` to that set, because a step
/// waiting for capacity may wait a long time and an operator reading the
/// run's log has to be able to tell it from a hung one. It also took
/// `skipped` and `failed` out of this run altogether: with five limits and
/// nothing lifting them the step used to end, and it now waits five times
/// and takes the attempt the stub allows.
#[test]
fn no_other_event_reaches_the_recipe_log() {
    let f = with_profiles();
    x_is_live(&f);
    let log = recipe_log(&f);
    let mut c = home_step(&f);
    c.env("LIMIT_CALLS", "5").env(RECIPE_LOG_ENV, &log);
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    let lines = log_lines(&log);
    let allowed = ["start ", "waiting_for_capacity ", "end ", "reentered "];
    for l in &lines {
        assert!(
            allowed.iter().any(|p| l.starts_with(p)),
            "{l:?} in {lines:?}"
        );
    }
    // The allow-list says nothing unless each kind really occurs: the first
    // wait is a reset wait, the re-entry after it writes `reentered` and,
    // its reset having passed, a capacity wait that names itself.
    for wanted in allowed {
        assert!(
            lines.iter().any(|l| l.starts_with(wanted)),
            "{wanted:?} missing from {lines:?}"
        );
    }
    for unwanted in ["progress ", "resume ", "skipped ", "failed "] {
        assert!(
            !lines.iter().any(|l| l.starts_with(unwanted)),
            "{unwanted:?} in {lines:?}"
        );
    }
    // `wait_lines` rather than `events`, which drops the progress lines.
    let stderr = wait_lines(&out);
    for kept in ["progress ", "resume "] {
        assert!(
            stderr.iter().any(|l| l.starts_with(kept)),
            "the step's own stderr still carries the full account: {kept:?}"
        );
    }
}

/// The login-change wait names its kind and its account in the recipe log,
/// on both of the lines that open it.
///
/// Issue #2911 added the `waiting_for_capacity` line, which carries the
/// same `account=` as the `start` line, and removed the step's closing
/// `failed reason=spend_limit`: the wait's deadline retries instead. So a
/// reader grepping the recipe log for `spend_limit` finds the wait twice
/// and finds no failure, which is the point.
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
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        log_lines(&log),
        [
            "waiting_for_capacity reason=spend_limit account=X \
             until=2026-10-18 08:01:00 -07:00",
            "start reason=spend_limit wait=login_change account=X reset=none \
             until=2026-10-18 08:01:00 -07:00 bound=max_wait",
            "end reason=max reached",
        ],
        "both opening lines name the limit and the login"
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
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        live_login(&f).as_deref(),
        Some("X"),
        "the swaps really happened, twice"
    );
    assert_eq!(
        f.count("0"),
        3,
        "one wait, one retry, one capacity wait for the re-entry, one retry"
    );
    // Issue #2857: the retry meets a reset two minutes past. That limit is a
    // re-entry and waits on capacity. The login change in ~/.claude says
    // nothing to this step, so nothing ends that wait before its deadline,
    // and issue #2911 makes reaching the deadline a retry.
    let ev = events(&out);
    assert_eq!(
        ev[..3],
        [RESET_START, "end reason=reset reached", "resume attempt=2"],
        "{ev:?}"
    );
    assert_eq!(ev[3], "reentered met=usage_limit", "{ev:?}");
    assert_eq!(
        ev[4],
        "waiting_for_capacity reason=passed_reset harness=claude \
         until=2026-10-18 14:32:00 -07:00",
        "{ev:?}"
    );
    assert!(
        ev[5].starts_with("start reason=usage_limit wait=capacity ")
            && ev[5].ends_with("bound=max_wait"),
        "{ev:?}"
    );
    assert_eq!(
        ev[6..],
        ["end reason=capacity expired", "resume attempt=3"],
        "{ev:?}"
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
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(live_login(&f).as_deref(), Some("X"), "the swaps happened");
    assert_eq!(f.count("0"), 3);
    for n in [1, 2, 3] {
        assert_eq!(f.call("0", n)["PROFILE"], "X", "call {n}");
    }
    // Issue #2857, as for the config-directory step above: the retry's reset
    // is two minutes past, so the re-entry waits on capacity, says why
    // (issue #2911), and retries when that wait reaches its deadline.
    let ev = events(&out);
    assert_eq!(
        ev[..3],
        [RESET_START, "end reason=reset reached", "resume attempt=2"],
        "{ev:?}"
    );
    assert_eq!(ev[3], "reentered met=usage_limit", "{ev:?}");
    assert_eq!(
        ev[4],
        "waiting_for_capacity reason=passed_reset harness=claude \
         until=2026-10-18 14:32:00 -07:00",
        "{ev:?}"
    );
    assert!(
        ev[5].starts_with("start reason=usage_limit wait=capacity ")
            && ev[5].ends_with("bound=max_wait"),
        "{ev:?}"
    );
    assert_eq!(
        ev[6..],
        ["end reason=capacity expired", "resume attempt=3"],
        "{ev:?}"
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
