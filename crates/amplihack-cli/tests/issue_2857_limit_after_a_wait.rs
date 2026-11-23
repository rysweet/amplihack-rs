//! Issue #2857, part 1, as issue #2911 leaves it: a limit or 429 on the first
//! attempt after a limit wait has ended goes back into a wait, once reported,
//! instead of ending the run.
//!
//! The case this comes from: a wait ends because something said *try now* (the
//! rotator recorded capacity, the login changed). The retry it earns meets a
//! 429 straight away, because the login the wait woke on was not usable. Before
//! #2857 the step ended there, as an indeterminate failure, on the first
//! attempt after a wait it had just finished.
//!
//! #2857 bounded the wait that followed with the two refusals it inherited: a
//! login the step had already waited on, and a per-step `max_wait_secs`
//! budget. Both ended runs that a limit should only have delayed, so #2911
//! removed them, and this file no longer describes either. In particular
//! `skipped reason=already_waited`, `failed reason=already_waited` and
//! `failed reason=capacity_expired` no longer exist; `max_wait_secs` is the
//! outer bound of one capacity or login-change wait rather than a total for
//! the step, and since round 6 of the review of PR #2992 it does not shorten a
//! reset wait; and a reset already behind the clock is a capacity wait for
//! every limit, not only for a re-entry the old rule would have refused. What
//! paces a chain of limits now is that every wait costs time. Nothing bounds
//! the chain: the step timeout plays no part (issue #3037), and an outer bound
//! that is reached runs the step again rather than ending it.
//!
//! What these tests pin:
//!
//! - a 429 on the attempt a wait's own resume launched, after a wait that ended
//!   on a positive signal, starts a capacity wait with `reset=none`, writes one
//!   `reentered met=rate_limit` line before that wait's `waiting_for_capacity`
//!   and `start`, and the run succeeds when the second wait's signal comes;
//! - a usage-limit message on that attempt is planned by the ordinary planner
//!   (`met=usage_limit`), so a message that states a near reset is a reset
//!   wait, and a reset wait writes no `waiting_for_capacity` line;
//! - a wait that ended at its own ceiling earns no `reentered` line, because
//!   nothing said *try*, and a 429 after it is still not re-entered; the usage
//!   limit after it is nonetheless waited for again (#2911);
//! - the position is the whole rule for a 429. A 429 on the first attempt (no
//!   wait yet), a 429 phrase in the model's prose, or a 429 on a step where no
//!   login could take over, re-enters nothing;
//! - a usage-limit message whose stated reset the clock has already passed is
//!   waited for as a capacity wait: with another login able to take over, with
//!   none, for an organisation spend limit that states such a reset, and on a
//!   login the step has never waited on. The `reentered` line is the one thing
//!   that still turns on that last question;
//! - a chain of limits is paced by its waits rather than counted: each wait's
//!   deadline lies strictly in the future, no `failed` and no `skipped` line is
//!   written, and `max_wait_secs` is spent again by every wait;
//! - no bound is left: since issue #3037 the step's own timeout does not end
//!   a wait either, and no line says `failed reason=step_timeout`.
//!
//! Every wait with no usable future reset writes a `waiting_for_capacity
//! reason=<why>` line immediately before its `start` (#2911), which is why the
//! sequences below carry one for each capacity and login-change wait and none
//! for a reset wait.
//!
//! Everything runs on the fixture's virtual clock, so no test sleeps, and
//! nothing is asked of a delay. The rotator is a stub ledger: the stub `claude`
//! writes the rotate ledger's cycle during the attempt it plays, stamped on the
//! virtual clock, which is what the wait's capacity reading sees. Nothing reads
//! or writes the real `~/.claude`, `~/.amplihack` or a real rotator.

#![cfg(unix)]

use crate::issue_1618_fixture::{
    Fixture, LIMIT_TEXT, SPEND_SESSION_TEXT, TEST_NOW, WEEKLY_TEXT, claude_stdout, describe,
    events, executable, text,
};
use crate::issue_1618_wait_profiles::with_profiles;
use crate::issue_1801_capacity_wait::{rotator_running, x_is_live};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

const MAX_WAIT_ENV: &str = "AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS";

/// A 429 as the harness prints it: the envelope the recipe runner's own
/// transient markers describe (`api error: 429`, `rate_limit_error`).
const RATE_429: &str = r#"API Error: 429 {"type":"error","error":{"type":"rate_limit_error","message":"This request would exceed your account's rate limit. Please try again later."}}"#;
const REENTERED_RATE: &str = "reentered met=rate_limit";
const REENTERED_USAGE: &str = "reentered met=usage_limit";

/// The `waiting_for_capacity` line a wait with no usable future reset writes
/// immediately before its own `start` (issue #2911), on the step's stderr,
/// where it names the harness rather than the account.
fn waiting_for(why: &str, until: &str) -> String {
    format!("waiting_for_capacity reason={why} harness=claude until={until}")
}

/// The stub `claude`. It is the #1618 stub with two additions, both keyed on
/// the call number so a test can say what happens on which attempt:
///
/// - `LIMIT_MESSAGE_<n>` (else `LIMIT_MESSAGE`) is printed on call `n` while
///   `n <= $LIMIT_CALLS`, on stderr, or on stdout when `LIMIT_ON_STDOUT_<n>`
///   is set, then `LIMIT_TRAILER_<n>` on stdout if set (so a 429 can be an
///   earlier line of the output rather than its last), and the call exits
///   `$LIMIT_EXIT` (default 1);
/// - `ROOM_ON_CALL` is a space-separated list of
///   `<call>:<measured_at>:<room>[:<live>]`; on that call the stub replaces
///   the rotate ledger, in one rename and private, with a `swapped` cycle
///   whose `measured_at` and `at` are `<measured_at>`, whose `live_room` is
///   `<room>` and whose live login is `<live>` (default `X`).
const STUB: &str = r#"#!/bin/sh
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
for spec in $ROOM_ON_CALL; do
  call="${spec%%:*}"; rest="${spec#*:}"; at="${rest%%:*}"; room="${rest#*:}"
  live=X
  case "$room" in *:*) live="${room#*:}"; room="${room%%:*}" ;; esac
  if [ "$call" = "$n" ]; then
    umask 077
    printf '{"version":1,"live":"%s","alert":null,"last_cycle":{"at":%s,"measured_at":%s,"outcome":"swapped","live_room":%s}}' \
      "$live" "$at" "$at" "$room" > "$LEDGER_DIR/ledger.json.new"
    chmod 600 "$LEDGER_DIR/ledger.json.new"
    mv "$LEDGER_DIR/ledger.json.new" "$LEDGER_DIR/ledger.json"
  fi
done
limit="${LIMIT_CALLS:-0}"
if [ "$n" -le "$limit" ]; then
  eval "msg=\${LIMIT_MESSAGE_$n-\$LIMIT_MESSAGE}"
  eval "on_stdout=\${LIMIT_ON_STDOUT_$n-}"
  if [ -n "$on_stdout" ]; then printf '%s\n' "$msg"; else printf '%s\n' "$msg" >&2; fi
  eval "trailer=\${LIMIT_TRAILER_$n-}"
  if [ -n "$trailer" ]; then printf '%s\n' "$trailer"; fi
  exit "${LIMIT_EXIT:-1}"
fi
exit 0
"#;

fn rotate_dir(f: &Fixture) -> PathBuf {
    f.profiles.join(".auto-switch.d").join("rotate")
}

/// A step on `$HOME/.claude` with X live, the stub installed, and a ledger
/// directory for the stub to write into. The rotator itself is not started:
/// `rotator_running` is held by the caller when the case needs one.
fn home_step(f: &Fixture) -> Command {
    fs::create_dir_all(f.home.join(".claude")).unwrap();
    executable(&f.tools.join("claude"), STUB);
    x_is_live(f);
    let mut c = f.step();
    c.env_remove("CLAUDE_CONFIG_DIR")
        .env("LEDGER_DIR", rotate_dir(f))
        .env("LIMIT_MESSAGE", WEEKLY_TEXT);
    c
}

/// `<call>:<measured_at>:<room>` offsets from the virtual clock's start.
fn room_on(call: u32, after_start: i64, room: bool) -> String {
    format!("{call}:{}:{room}", TEST_NOW + after_start)
}

/// `room_on`, with the cycle naming `login` as the one now live: the rotator
/// swapped the step's login while the attempt was failing.
fn room_on_login(call: u32, after_start: i64, room: bool, login: &str) -> String {
    format!("{}:{login}", room_on(call, after_start, room))
}

fn count(ev: &[String], prefix: &str) -> usize {
    ev.iter().filter(|l| l.starts_with(prefix)).count()
}

/// The first wait of the home cases below: a weekly limit, whose reset is
/// further off than five hours, so a capacity wait that says `reset_too_far`
/// on its own line first (issue #2911), woken by the rotator's fresh cycle
/// with room that call 1 wrote. Four lines, so the re-entry that follows
/// begins at `ev[4]`.
fn assert_first_wait_woke_on_the_rotator(ev: &[String]) {
    assert!(
        ev[0].starts_with("waiting_for_capacity reason=reset_too_far harness=claude until="),
        "{ev:?}"
    );
    assert!(
        ev[1].starts_with("start reason=usage_limit wait=capacity harness=claude reset="),
        "{ev:?}"
    );
    assert_eq!(ev[2], "end reason=rotator capacity", "{ev:?}");
    assert_eq!(ev[3], "resume attempt=2", "{ev:?}");
}

// --- the 429 the issue is about ----------------------------------------------

#[test]
fn a_429_on_the_retry_a_rotator_wake_earned_waits_again_and_says_so_once() {
    let f = with_profiles();
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f);
    c.env("LIMIT_CALLS", "2")
        .env("LIMIT_MESSAGE_2", RATE_429)
        // Call 1 records capacity with room as the limit hits; call 2 records
        // a newer one as the 429 hits, which is what the second wait wakes on.
        .env(
            "ROOM_ON_CALL",
            format!("{} {}", room_on(1, 5, true), room_on(2, 100, true)),
        );
    let out = f.run(&mut c);
    assert!(
        out.status.success(),
        "the run must not end indeterminate on a 429 after a wait: {}",
        describe(&out)
    );
    assert_eq!(f.count("0"), 3, "limit, 429, then the attempt that works");
    let ev = events(&out);
    assert_eq!(ev.len(), 9, "{ev:?}");
    assert_first_wait_woke_on_the_rotator(&ev);
    assert_eq!(
        ev[4], REENTERED_RATE,
        "before the second wait's own lines: {ev:?}"
    );
    assert_eq!(
        ev[5],
        waiting_for("no_reset", "2026-10-18 14:00:30 -07:00"),
        "a 429 states no reset, and the wait says so before it starts: {ev:?}"
    );
    assert!(
        ev[6].starts_with("start reason=usage_limit wait=capacity harness=claude reset=none "),
        "a 429 states no reset, so the second wait is a capacity wait with none: {ev:?}"
    );
    assert_eq!(ev[7], "end reason=rotator capacity", "{ev:?}");
    assert_eq!(ev[8], "resume attempt=3", "{ev:?}");
    assert_eq!(
        count(&ev, "reentered "),
        1,
        "reported once for the one re-entry: {ev:?}"
    );
    assert_eq!(claude_stdout(&out), "attempt 3 output\n");
    f.assert_isolated("0");
}

#[test]
fn a_429_as_the_last_line_of_stdout_counts_like_one_on_stderr() {
    let f = with_profiles();
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f);
    c.env("LIMIT_CALLS", "2")
        .env("LIMIT_MESSAGE_2", RATE_429)
        .env("LIMIT_ON_STDOUT_2", "1")
        .env(
            "ROOM_ON_CALL",
            format!("{} {}", room_on(1, 5, true), room_on(2, 100, true)),
        );
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    let ev = events(&out);
    assert_eq!(count(&ev, "reentered "), 1, "{ev:?}");
    assert_eq!(ev[4], REENTERED_RATE, "{ev:?}");
}

#[test]
fn a_usage_limit_message_on_the_retry_is_planned_as_the_first_wait_was() {
    // The retry's message states a reset an hour away, so the second wait is a
    // reset wait, not a capacity wait: the same planner, the same rows. A reset
    // wait has a reset to end on, so it writes no `waiting_for_capacity` line,
    // which is what tells the two kinds apart in the sequence (issue #2911).
    let f = with_profiles();
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f);
    c.env("LIMIT_CALLS", "2")
        .env(
            "LIMIT_MESSAGE_2",
            "You've hit your session limit · resets 9:00am (America/Los_Angeles)",
        )
        .env("ROOM_ON_CALL", room_on(1, 5, true));
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 3);
    let ev = events(&out);
    assert_eq!(ev.len(), 8, "{ev:?}");
    assert_first_wait_woke_on_the_rotator(&ev);
    assert_eq!(ev[4], REENTERED_USAGE, "{ev:?}");
    assert_eq!(
        ev[5],
        "start reason=usage_limit harness=claude reset=2026-10-18 09:00:00 -07:00 \
         until=2026-10-18 09:02:00 -07:00 bound=reset+margin",
        "{ev:?}"
    );
    assert_eq!(ev[6], "end reason=reset reached", "{ev:?}");
    assert_eq!(ev[7], "resume attempt=3", "{ev:?}");
    assert_eq!(
        count(&ev, "waiting_for_capacity "),
        1,
        "only the first wait had no reset to end on: {ev:?}"
    );
}

// --- a reset that has already passed ------------------------------------------------
//
// The retry of the second wait meets the message it met before, read at 09:02:
// "resets 9:00am" is two minutes past, inside the ten minutes a clock time is
// still read as today. A *reset* wait for it would end the moment it began and
// run the same attempt again, so the ordinary planner has nothing to wait on.
// That is no reason to end the step: a reset in the past shows the stated reset
// cannot be relied on, not that no wait is owed. Such a limit is planned from
// capacity instead, which is what keeps the wait non-zero and so keeps the
// chain paced (issue #2911). Since #2911 that holds for every limit whose
// stated reset has passed, not only for a re-entry the old one-wait-per-login
// rule would have refused.
//
// The timeline of the virtual clock, from 08:00:00 (`TEST_NOW`), which the
// tests below rely on and which a poll of 30 s fixes:
//
//   call 1, 08:00:00  a weekly limit: a capacity wait, woken at the first poll
//                     by the rotator's cycle call 1 wrote (30 s spent);
//   call 2, 08:00:30  "resets 9:00am", an hour ahead: a reset wait to 09:02:00
//                     (3690 s spent, 3720 s in all);
//   call 3, 09:02:00  "resets 9:00am" again: the reset is two minutes behind.

const NINE_AM: &str = "You've hit your session limit · resets 9:00am (America/Los_Angeles)";
/// A session limit whose stated reset is five minutes behind `TEST_NOW`, and
/// stays inside the ten minutes a bare clock time is still read as today's
/// for the first few minutes of a run.
const SEVEN_FIFTY_FIVE: &str =
    "You've hit your session limit · resets 7:55am (America/Los_Angeles)";
/// The reset wait of call 2, and the reset every later `start` line prints.
const NINE_AM_RESET_WAIT: &str = "start reason=usage_limit harness=claude \
                                  reset=2026-10-18 09:00:00 -07:00 \
                                  until=2026-10-18 09:02:00 -07:00 bound=reset+margin";

/// The second `reentered` line and the line after it: a capacity wait that
/// carries the passed reset and ends at `until`.
fn passed_reset_capacity_start(until: &str, bound: &str) -> String {
    format!(
        "start reason=usage_limit wait=capacity harness=claude \
         reset=2026-10-18 09:00:00 -07:00 until={until} bound={bound}"
    )
}

/// Calls 1 to 3 as the timeline above: `ROOM_ON_CALL` names the rotator's cycle
/// call 1 wrote, and the retry of call 2 and call 3 each state `NINE_AM`.
fn passed_reset_step(f: &Fixture, calls_limited: u32) -> Command {
    let mut c = home_step(f);
    c.env("LIMIT_CALLS", calls_limited.to_string());
    for call in 2..=calls_limited {
        c.env(format!("LIMIT_MESSAGE_{call}"), NINE_AM);
    }
    c
}

#[test]
fn a_reset_that_has_already_passed_is_waited_for_as_capacity_and_the_wake_retries() {
    // Renamed from `..._where_another_login_could_take_over`: whether a login
    // could take over is no longer asked when a wait is started (issue #2911),
    // so the rotator here only supplies the readings that wake the waits.
    //
    // `max_wait_secs` is 7200, the length of each wait rather than a total for
    // the step. The first wait is woken after 30 s by call 1's cycle; the
    // second is a reset wait to 09:02:00; the third is a capacity wait for the
    // reset that is now two minutes behind the clock, planned for its own full
    // 7200 s, which is 11:02:00, and woken after 30 s by call 3's cycle.
    let f = with_profiles();
    let _rotator = rotator_running(&f);
    let mut c = passed_reset_step(&f, 3);
    c.env(MAX_WAIT_ENV, "7200").env(
        "ROOM_ON_CALL",
        // Call 3's cycle is stamped after its wait starts (09:02:00 is
        // `TEST_NOW + 3720`), so the wait wakes at its first poll.
        format!("{} {}", room_on(1, 5, true), room_on(3, 4000, true)),
    );
    let out = f.run(&mut c);
    assert!(
        out.status.success(),
        "a reset the clock has passed must not end the step: {}",
        describe(&out)
    );
    assert_eq!(
        f.count("0"),
        4,
        "limit, re-entered limit, passed reset, success"
    );
    let ev = events(&out);
    assert_eq!(ev.len(), 13, "{ev:?}");
    assert_first_wait_woke_on_the_rotator(&ev);
    assert_eq!(ev[4], REENTERED_USAGE, "{ev:?}");
    assert_eq!(ev[5], NINE_AM_RESET_WAIT, "{ev:?}");
    assert_eq!(ev[6], "end reason=reset reached", "{ev:?}");
    assert_eq!(ev[7], "resume attempt=3", "{ev:?}");
    assert_eq!(
        ev[8], REENTERED_USAGE,
        "one line for this re-entry, before its own lines: {ev:?}"
    );
    assert_eq!(
        ev[9],
        waiting_for("passed_reset", "2026-10-18 11:02:00 -07:00"),
        "the wait names the reason it has no reset to end on: {ev:?}"
    );
    assert_eq!(
        ev[10],
        passed_reset_capacity_start("2026-10-18 11:02:00 -07:00", "max_wait"),
        "a capacity wait that prints the reset the message stated and gets a \
         whole `max_wait_secs` of its own, not what an earlier wait left: {ev:?}"
    );
    assert_eq!(ev[11], "end reason=rotator capacity", "{ev:?}");
    assert_eq!(ev[12], "resume attempt=4", "{ev:?}");
    assert_eq!(count(&ev, "reentered "), 2, "{ev:?}");
    assert_eq!(claude_stdout(&out), "attempt 4 output\n");
    f.assert_isolated("0");
}

#[test]
fn a_reset_that_has_already_passed_with_no_login_to_take_over_still_waits_and_then_retries() {
    // Renamed from `..._waits_out_the_budget`: there is no budget left to wait
    // out (issue #2911). No rotator is running and there is no second login,
    // so nothing can end the capacity wait before its own deadline, and
    // nothing need be able to: a wait is started whether or not a login could
    // take over now, because a login can gain room while the wait runs.
    //
    // The first wait is an ordinary reset wait, from 08:00 to 08:32. The retry
    // meets the same "resets 8:30am", read at 08:32, and is a re-entry: it is
    // reported once, and the step waits again on capacity, carrying the passed
    // reset, for its own 7200 s, which ends at 10:32:00. Reaching that
    // deadline retries; it does not end the step. Before #2911 the step ended
    // here with `skipped reason=already_waited`, which is the run issue #2911
    // reports.
    let f = with_profiles();
    x_is_live(&f);
    let mut c = home_step(&f);
    c.env("LIMIT_MESSAGE", LIMIT_TEXT)
        .env("LIMIT_CALLS", "2")
        .env(MAX_WAIT_ENV, "7200");
    let out = f.run(&mut c);
    assert!(
        out.status.success(),
        "a limit after a wait must not end the step: {}",
        describe(&out)
    );
    assert_eq!(f.count("0"), 3, "limit, re-entered limit, success");
    let ev = events(&out);
    let waiting = waiting_for("passed_reset", "2026-10-18 10:32:00 -07:00");
    assert_eq!(
        ev,
        [
            "start reason=usage_limit harness=claude reset=2026-10-18 08:30:00 -07:00 \
             until=2026-10-18 08:32:00 -07:00 bound=reset+margin",
            "end reason=reset reached",
            "resume attempt=2",
            REENTERED_USAGE,
            waiting.as_str(),
            "start reason=usage_limit wait=capacity harness=claude \
             reset=2026-10-18 08:30:00 -07:00 until=2026-10-18 10:32:00 -07:00 \
             bound=max_wait",
            "end reason=capacity expired",
            "resume attempt=3",
        ],
        "one `reentered` line, then the capacity wait's two opening lines: {ev:?}"
    );
    assert_eq!(claude_stdout(&out), "attempt 3 output\n");
    assert_eq!(count(&ev, "skipped "), 0, "{ev:?}");
    assert_eq!(count(&ev, "failed "), 0, "{ev:?}");
    f.assert_isolated("0");
}

#[test]
fn an_organisation_spend_limit_that_states_a_passed_session_reset_waits_for_capacity_too() {
    // Renamed and inverted. It used to assert `skipped reason=already_waited`
    // and `failed reason=already_waited`: a spend limit was held back from the
    // passed-reset rule, because another login of the same organisation does
    // not lift one, and the step ended on the second reading of the message.
    //
    // Issue #2911 took both endings away. A spend limit that states a session
    // reset is waited out on that window (the window is what stops work
    // first), and when the clock has passed the stated reset the retry takes
    // the same `passed_reset` row every other limit takes, so it is a capacity
    // wait and the step goes on. `wait_for_message` has no spend case for a
    // reset it can read, which is what
    // `tests_spend_reset::issue_2911_a_spend_reset_already_behind_the_clock_waits_for_capacity`
    // pins in the planner. A reading may end such a wait and send the retry
    // back at the same spent credit; under #2911 that costs an attempt and
    // another wait rather than the run.
    let f = with_profiles();
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f);
    c.env("LIMIT_MESSAGE", SPEND_SESSION_TEXT)
        .env("LIMIT_CALLS", "2")
        .env(MAX_WAIT_ENV, "7200");
    let out = f.run(&mut c);
    assert!(
        out.status.success(),
        "a spend limit must no longer end the step: {}",
        describe(&out)
    );
    assert_eq!(f.count("0"), 3, "limit, re-entered limit, success");
    let ev = events(&out);
    let waiting = waiting_for("passed_reset", "2026-10-18 10:32:00 -07:00");
    assert_eq!(
        ev,
        [
            "start reason=usage_limit harness=claude reset=2026-10-18 08:30:00 -07:00 \
             until=2026-10-18 08:32:00 -07:00 bound=reset+margin",
            "end reason=reset reached",
            "resume attempt=2",
            REENTERED_USAGE,
            waiting.as_str(),
            "start reason=usage_limit wait=capacity harness=claude \
             reset=2026-10-18 08:30:00 -07:00 until=2026-10-18 10:32:00 -07:00 \
             bound=max_wait",
            "end reason=capacity expired",
            "resume attempt=3",
        ],
        "a spend limit with a passed session reset is a capacity wait: {ev:?}"
    );
    assert_eq!(count(&ev, "skipped "), 0, "{ev:?}");
    assert_eq!(count(&ev, "failed "), 0, "{ev:?}");
}

#[test]
fn a_retry_on_a_login_the_step_has_never_waited_on_still_waits_for_a_passed_reset() {
    // Renamed from `..._keeps_its_first_wait_grace`, because that grace is
    // gone (issue #2911). `plan::upgrade_passed_reset` used to lift a passed
    // reset into a capacity wait only for a re-entry the one-wait-per-login
    // rule would have refused, so a *first* limit on a login whose stated
    // reset had passed was planned as a reset wait with a deadline of `now`:
    // it ended the moment it began and retried at once. That zero-length wait
    // is exactly the unpaced chain #2911 had to remove, so a reset behind the
    // clock is now a capacity wait for every limit, first or not.
    //
    // Call 1 fails on X while the rotator swaps to Y, so the login moved
    // before the wait began and the step retries at once, on Y. That retry
    // meets a reset five minutes behind the clock. Y is a login this step has
    // never waited on, so no `reentered` line is written: that record's one
    // remaining job is to tell a second wait on a login from a first. The wait
    // itself happens all the same, as a capacity wait, and the rotator's fresh
    // cycle wakes it at its first poll.
    let f = with_profiles();
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f);
    c.env("LIMIT_CALLS", "2")
        .env("LIMIT_MESSAGE_2", SEVEN_FIFTY_FIVE)
        .env("ROOM_ON_CALL", room_on_login(1, 5, true, "Y"));
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 3, "limit on X, limit on Y, success");
    let ev = events(&out);
    assert_eq!(ev.len(), 8, "{ev:?}");
    assert!(
        ev[0].starts_with("waiting_for_capacity reason=reset_too_far harness=claude until="),
        "{ev:?}"
    );
    assert!(
        ev[1].starts_with("start reason=usage_limit wait=capacity harness=claude reset="),
        "{ev:?}"
    );
    assert_eq!(ev[2], "end reason=login changed", "{ev:?}");
    assert_eq!(ev[3], "resume attempt=2", "{ev:?}");
    assert_eq!(
        ev[4],
        waiting_for("passed_reset", "2026-10-18 14:00:00 -07:00"),
        "Y's own first wait has no reset it can use either: {ev:?}"
    );
    assert_eq!(
        ev[5],
        "start reason=usage_limit wait=capacity harness=claude \
         reset=2026-10-18 07:55:00 -07:00 until=2026-10-18 14:00:00 -07:00 \
         bound=max_wait",
        "a capacity wait with a real deadline, not a reset wait ending at once: {ev:?}"
    );
    assert_eq!(ev[6], "end reason=rotator capacity", "{ev:?}");
    assert_eq!(ev[7], "resume attempt=3", "{ev:?}");
    assert_eq!(
        count(&ev, "reentered "),
        0,
        "Y is a login this step had not waited on: {ev:?}"
    );
}

/// A chain of limits is paced by its waits, and nothing counts them
/// (issue #2911).
///
/// This replaces the two tests that proved the opposite,
/// `the_budget_of_a_step_still_bounds_the_chain_of_waits` and
/// `the_budget_still_bounds_a_chain_of_waits_for_a_reset_that_keeps_passing`.
/// Both read `max_wait_secs` as a budget for the whole step: each wait was
/// planned with what the last had left, the chain ran out of time, and the
/// step ended with `failed reason=already_waited` or
/// `failed reason=capacity_expired`. Those endings are gone, and so is the
/// budget. `max_wait_secs` is the length of ONE wait, and what keeps a chain
/// of limits from spinning is that every wait of it costs real time.
///
/// The stub states a reset five minutes behind the clock on each of its first
/// three calls, so every one of them takes the `passed_reset` row and is a
/// capacity wait; no rotator cycle is fresh and no login has room, so nothing
/// ends any of them early. With `max_wait_secs` of 60, the floor, each wait
/// runs its own full minute: the deadlines are 08:01:00, 08:02:00 and
/// 08:03:00, each strictly after the wait's own start, and each ends
/// `capacity expired`, which a wait can only report once its clock has got
/// there. Three waits of
/// 60 s out of a configured 60 is the whole of the change: a budget would have
/// ended the step inside the first minute.
///
/// What ends this chain is the stub running out of limits, which is why
/// `LIMIT_CALLS` bounds it. Nothing else ends a chain of waits: since issue
/// #3037 not even the step's own timeout does, and
/// `issue_3037_a_step_timeout_does_not_end_a_wait_for_a_passed_reset` is the
/// guard for that.
#[test]
fn a_chain_of_limits_is_paced_by_each_wait_costing_time_and_nothing_counts_them() {
    let f = with_profiles();
    let mut c = home_step(&f);
    c.env("LIMIT_MESSAGE", SEVEN_FIFTY_FIVE)
        .env("LIMIT_CALLS", "3")
        .env(MAX_WAIT_ENV, "60");
    let out = f.run(&mut c);
    assert!(
        out.status.success(),
        "three limits in a row must each wait, not end the step: {}",
        describe(&out)
    );
    assert_eq!(f.count("0"), 4, "three limits, then the attempt that works");
    let ev = events(&out);
    let mut want = Vec::new();
    for (n, until) in [
        (2, "2026-10-18 08:01:00 -07:00"),
        (3, "2026-10-18 08:02:00 -07:00"),
        (4, "2026-10-18 08:03:00 -07:00"),
    ] {
        want.push(waiting_for("passed_reset", until));
        want.push(format!(
            "start reason=usage_limit wait=capacity harness=claude \
             reset=2026-10-18 07:55:00 -07:00 until={until} bound=max_wait"
        ));
        want.push("end reason=capacity expired".to_string());
        want.push(format!("resume attempt={n}"));
    }
    assert_eq!(
        ev, want,
        "each wait gets its own whole minute, and each deadline is later than \
         the last: {ev:?}"
    );
    assert_eq!(count(&ev, "failed "), 0, "nothing ended the step: {ev:?}");
    assert_eq!(count(&ev, "skipped "), 0, "nothing was refused: {ev:?}");
    assert_eq!(claude_stdout(&out), "attempt 4 output\n");
    assert!(
        text(&out.stderr).contains("resets 7:55am"),
        "the runner's classifier must still see the limit"
    );
}

/// Issue #3037, review of PR #2992: the step timeout no longer ends a chain
/// of waits. This test used to be the guard that it still did: with a
/// 3900-second timeout the third wait ended at 09:04:00, a minute before it,
/// with `failed reason=step_timeout` while the limit still stood. Nothing
/// enforces that variable on a step - recipe-runner-rs enforces only a
/// step's YAML `timeout:` - so the third wait now runs its own length and
/// the step retries.
#[test]
fn issue_3037_a_step_timeout_does_not_end_a_wait_for_a_passed_reset() {
    let f = with_profiles();
    let _rotator = rotator_running(&f);
    let mut c = passed_reset_step(&f, 3);
    c.env("AMPLIHACK_STEP_TIMEOUT", "3900")
        .env("ROOM_ON_CALL", room_on(1, 5, true));
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        f.count("0"),
        4,
        "the attempt after the third wait ran: {}",
        describe(&out)
    );
    let ev = events(&out);
    assert_eq!(ev[8], REENTERED_USAGE, "{ev:?}");
    assert!(
        ev[9].starts_with("waiting_for_capacity reason=passed_reset harness=claude until="),
        "{ev:?}"
    );
    assert_ne!(
        ev[9],
        waiting_for("passed_reset", "2026-10-18 09:04:00 -07:00"),
        "the deadline is no longer pulled back to the step timeout: {ev:?}"
    );
    assert!(
        ev[10].ends_with(" bound=max_wait"),
        "the wait's own length bounds it: {ev:?}"
    );
    assert!(ev.iter().any(|e| e == "resume attempt=4"), "{ev:?}");
    assert_eq!(count(&ev, "failed "), 0, "nothing ended the step: {ev:?}");
    assert!(ev.iter().all(|e| !e.contains("step timeout")), "{ev:?}");
}

// --- a wait that ran out earns no `reentered` line --------------------------

/// A session limit that states no reset: its wait is a capacity wait with
/// no reset row, so only its outer bound can end it here.
const NO_RESET_TEXT: &str = "You've hit your session limit · try again later";

/// A step with no rotator and no profile root of its own, whose limit states
/// no reset: a capacity wait planned with a configured `max_wait_secs` of 0,
/// which reads as the 60-second floor (issue #2911), ends at that outer
/// bound (`capacity expired`) and the step retries.
///
/// It used to be the stock message's reset wait, cut short by the
/// configured length (`max reached`). Since round 6 of the review of PR
/// #2992 a reset wait has no outer bound and runs to its reset, which does
/// say to try, so the wait that runs out is now a capacity wait.
fn ceiling_step(f: &Fixture) -> Command {
    executable(&f.tools.join("claude"), STUB);
    let mut c = f.step();
    c.env(MAX_WAIT_ENV, "0").env("LIMIT_MESSAGE", NO_RESET_TEXT);
    c
}

/// A limit after a wait that ran out is waited for again, and never refused
/// (issue #2911).
///
/// Renamed from `a_usage_limit_after_a_wait_that_ran_out_is_refused_as_it_always_was`,
/// which asserted `skipped reason=already_waited` and `failed
/// reason=already_waited`. That pair is the exact failure issue #2911
/// reports: a usage limit ended the step, on a login whose window had not
/// even been waited out, because the step had already waited once. Both lines
/// are gone, and the step waits again.
///
/// The configured `max_wait_secs` here is 0, which no longer means "do not
/// wait": it reads as the `MIN_WAIT_SECS` floor of 60 seconds, so each wait
/// has a real deadline a minute ahead of its own start. To turn waiting off a
/// config must set `recipe.usage_limit_wait = false`. Nothing said *try* when
/// either wait ran out, so neither re-entry writes a `reentered` line; the
/// waiting happens all the same.
#[test]
fn a_usage_limit_after_a_wait_that_ran_out_waits_again_and_never_refuses() {
    let f = Fixture::new();
    let mut c = ceiling_step(&f);
    c.env("LIMIT_CALLS", "2");
    let out = f.run(&mut c);
    assert!(
        out.status.success(),
        "the second limit must be waited for, not refused: {}",
        describe(&out)
    );
    assert_eq!(f.count("0"), 3, "two waits, then the attempt that works");
    let ev = events(&out);
    assert_eq!(
        ev,
        [
            "waiting_for_capacity reason=no_reset harness=claude \
             until=2026-10-18 08:01:00 -07:00",
            "start reason=usage_limit wait=capacity harness=claude reset=none \
             until=2026-10-18 08:01:00 -07:00 bound=max_wait",
            "end reason=capacity expired",
            "resume attempt=2",
            "waiting_for_capacity reason=no_reset harness=claude \
             until=2026-10-18 08:02:00 -07:00",
            "start reason=usage_limit wait=capacity harness=claude reset=none \
             until=2026-10-18 08:02:00 -07:00 bound=max_wait",
            "end reason=capacity expired",
            "resume attempt=3",
        ],
        "a configured 0 is the 60-second floor, twice over: {ev:?}"
    );
    assert_eq!(count(&ev, "reentered "), 0, "nothing said try: {ev:?}");
    assert_eq!(count(&ev, "skipped "), 0, "{ev:?}");
    assert_eq!(count(&ev, "failed "), 0, "{ev:?}");
    assert!(
        text(&out.stderr).contains(NO_RESET_TEXT),
        "the runner's classifier must still see the limit"
    );
}

#[test]
fn a_429_after_a_wait_that_ran_out_is_not_re_entered() {
    let f = Fixture::new();
    let mut c = ceiling_step(&f);
    c.env("LIMIT_CALLS", "2").env("LIMIT_MESSAGE_2", RATE_429);
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(f.count("0"), 2);
    let ev = events(&out);
    assert_eq!(
        ev[2..],
        ["end reason=capacity expired", "resume attempt=2"],
        "the 429 is passed up as before; nothing waits for it: {ev:?}"
    );
}

// --- the position is the whole rule ------------------------------------------

#[test]
fn a_429_on_the_first_attempt_starts_no_wait() {
    let f = with_profiles();
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f);
    c.env("LIMIT_CALLS", "1").env("LIMIT_MESSAGE_1", RATE_429);
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(f.count("0"), 1);
    assert_eq!(events(&out), Vec::<String>::new(), "no wait, no line");
}

#[test]
fn a_429_phrase_in_the_models_prose_after_a_wait_is_not_a_429() {
    // The retry fails for another reason. Its stdout is the model's answer and
    // mentions a 429, once mid-line and once as an earlier line; neither is
    // the last non-empty line starting the way a harness 429 does.
    let f = with_profiles();
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f);
    c.env("LIMIT_CALLS", "2")
        .env("ROOM_ON_CALL", room_on(1, 5, true))
        .env(
            "LIMIT_MESSAGE_2",
            "The build failed: API Error: 429 was in the log, retrying",
        )
        .env("LIMIT_ON_STDOUT_2", "1");
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(f.count("0"), 2);
    let ev = events(&out);
    assert_eq!(
        ev.len(),
        4,
        "only the first wait, with its `waiting_for_capacity` line: {ev:?}"
    );
    assert_eq!(count(&ev, "reentered "), 0, "{ev:?}");

    // The same 429 as an earlier line of stdout, followed by the model's own
    // closing line, is prose too: only the last non-empty line is read.
    let h = with_profiles();
    let _rotator = rotator_running(&h);
    let mut c = home_step(&h);
    c.env("LIMIT_CALLS", "2")
        .env("ROOM_ON_CALL", room_on(1, 5, true))
        .env("LIMIT_MESSAGE_2", RATE_429)
        .env("LIMIT_ON_STDOUT_2", "1")
        .env("LIMIT_TRAILER_2", "Sorry, I could not finish that.");
    let out = h.run(&mut c);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(count(&events(&out), "reentered "), 0, "{}", describe(&out));

    // The positive control for the same fixture: the same message as the last
    // line, in the shape of a harness 429, is re-entered.
    let g = with_profiles();
    let _rotator = rotator_running(&g);
    let mut c = home_step(&g);
    c.env("LIMIT_CALLS", "2")
        .env(
            "ROOM_ON_CALL",
            format!("{} {}", room_on(1, 5, true), room_on(2, 100, true)),
        )
        .env("LIMIT_MESSAGE_2", RATE_429);
    let out = g.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(count(&events(&out), "reentered "), 1);
}

#[test]
fn a_429_where_no_capacity_wait_could_start_is_passed_up_not_waited_for() {
    // No rotator is running, so nothing could end a capacity wait before its
    // deadline. The first wait is an ordinary reset wait (no rotator needed)
    // that reaches its reset; the 429 after it goes back to the recipe runner
    // as it always did, with no `reentered` line and no second wait.
    let f = with_profiles();
    x_is_live(&f);
    let mut c = home_step(&f);
    c.env("LIMIT_MESSAGE", LIMIT_TEXT)
        .env("LIMIT_CALLS", "2")
        .env("LIMIT_MESSAGE_2", RATE_429);
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(f.count("0"), 2);
    let ev = events(&out);
    assert_eq!(count(&ev, "start "), 1, "{ev:?}");
    assert_eq!(count(&ev, "reentered "), 0, "{ev:?}");
    assert_eq!(ev[1], "end reason=reset reached", "{ev:?}");
}

// --- nothing else in the line, and nothing in the control path reads it back ----

#[test]
fn the_reentered_line_carries_no_account_and_no_output_text() {
    let f = with_profiles();
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f);
    c.env("LIMIT_CALLS", "2")
        .env("LIMIT_MESSAGE_2", RATE_429)
        .env(
            "ROOM_ON_CALL",
            format!("{} {}", room_on(1, 5, true), room_on(2, 100, true)),
        );
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    let line = events(&out)
        .into_iter()
        .find(|l| l.starts_with("reentered "))
        .expect("one reentered line");
    assert_eq!(line, REENTERED_RATE, "closed token and nothing after it");
    let stderr = text(&out.stderr);
    for leaked in [
        "rate_limit_error",
        "account's rate limit",
        "SENTINEL",
        "uuid-",
    ] {
        let wait_lines: Vec<&str> = stderr
            .lines()
            .filter(|l| l.starts_with("amplihack.recipe.usage_limit_wait "))
            .collect();
        assert!(
            wait_lines.iter().all(|l| !l.contains(leaked)),
            "{leaked:?} in a wait line: {wait_lines:?}"
        );
    }
}

// --- both on one attempt: the usage limit decides --------------------------------

#[test]
fn a_usage_limit_beside_a_429_decides_the_wait() {
    // The retry prints both. A usage-limit message states what is wrong and
    // when it clears, so it is planned as one (`met=usage_limit`, a reset wait);
    // the 429 beside it adds nothing. No capacity wait with `reset=none`.
    let f = with_profiles();
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f);
    c.env("LIMIT_CALLS", "2")
        .env(
            "LIMIT_MESSAGE_2",
            format!(
                "{RATE_429}\nYou've hit your session limit · resets 9:00am (America/Los_Angeles)"
            ),
        )
        .env("ROOM_ON_CALL", room_on(1, 5, true));
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    let ev = events(&out);
    assert_eq!(ev[4], REENTERED_USAGE, "{ev:?}");
    assert!(
        ev[5].starts_with(
            "start reason=usage_limit harness=claude reset=2026-10-18 09:00:00 -07:00 "
        ),
        "a reset wait for the stated reset, so no `waiting_for_capacity` line: {ev:?}"
    );
    assert_eq!(count(&ev, "reentered "), 1, "{ev:?}");
    assert_eq!(
        count(&ev, "waiting_for_capacity "),
        1,
        "only the first wait's: {ev:?}"
    );
}

// --- the rotator is not told a 429 was a limit -------------------------------------

/// A 429 on the retry records no profile as limited and moves nothing: the
/// re-entry is a wait on the login the step is on, and the step's own login
/// is unchanged after it. The rotate ledger's live login is the one the
/// retry runs on, both times.
#[test]
fn a_429_re_entry_runs_the_retry_on_the_login_the_step_was_on() {
    let f = with_profiles();
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f);
    c.env("LIMIT_CALLS", "2")
        .env("LIMIT_MESSAGE_2", RATE_429)
        .env(
            "ROOM_ON_CALL",
            format!("{} {}", room_on(1, 5, true), room_on(2, 100, true)),
        );
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 3);
    for n in 1..=3 {
        assert_eq!(
            f.call("0", n)["CLAUDE_CONFIG_DIR"],
            "<unset>",
            "call {n} ran on $HOME/.claude, as the step did"
        );
    }
    // Nothing was recorded against the profile store: no login was marked
    // limited and no usage reading was written by the 429.
    assert!(
        !f.profiles
            .join(".auto-switch.d")
            .join("usage.json")
            .exists(),
        "a 429 is not a usage reading and records no profile as limited"
    );
}
