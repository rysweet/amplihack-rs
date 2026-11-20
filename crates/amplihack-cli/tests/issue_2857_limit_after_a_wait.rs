//! Issue #2857, part 1: a limit or 429 on the first attempt after a limit wait
//! has ended re-enters the wait, once reported, instead of ending the run.
//!
//! The case this comes from: a wait ends because something said *try now* (the
//! rotator recorded capacity, the login changed). The retry it earns meets a
//! 429 straight away, because the login the wait woke on was not usable. Before
//! the change the step ended there, as an indeterminate failure, on the first
//! attempt after a wait it had just finished.
//!
//! What these tests pin:
//!
//! - a 429 on the attempt a wait's own resume launched, after a wait that ended
//!   on a positive signal, starts a capacity wait with `reset=none`, writes one
//!   `reentered met=rate_limit` line before that wait's `start`, and the run
//!   succeeds when the second wait's signal comes;
//! - a usage-limit message on that attempt is planned by the ordinary planner
//!   (`met=usage_limit`), so a message that states a near reset is a reset wait;
//! - a wait that ended at its own ceiling earns nothing: nothing said *try*, so
//!   the `already_waited` refusal stands, and a 429 after it is not re-entered;
//! - the position is the whole rule. A 429 on the first attempt (no wait yet), a
//!   429 phrase in the model's prose, or a 429 on a step where no capacity wait
//!   could start, re-enters nothing;
//! - a usage-limit message on that attempt whose stated reset the clock has
//!   already passed is waited for as a capacity wait where another login could
//!   take over, reported once, and bounded by the budget the step has left. A
//!   reset wait for it would end at once and spend nothing. Where no capacity
//!   wait could start, for a spend limit, and for a login the step has never
//!   waited on, the earlier conduct stands;
//! - the step's `max_wait_secs` budget still bounds the chain.
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
const FAILED_AGAIN: &str = "failed reason=already_waited: usage limit not lifted";
const FAILED_EXPIRED: &str = "failed reason=capacity_expired: usage limit not lifted";
const REENTERED_RATE: &str = "reentered met=rate_limit";
const REENTERED_USAGE: &str = "reentered met=usage_limit";

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

/// The first wait of the home cases below: a weekly limit, a capacity wait,
/// woken by the rotator's fresh cycle with room that call 1 wrote.
fn assert_first_wait_woke_on_the_rotator(ev: &[String]) {
    assert!(
        ev[0].starts_with("start reason=usage_limit wait=capacity harness=claude reset="),
        "{ev:?}"
    );
    assert_eq!(ev[1], "end reason=rotator capacity", "{ev:?}");
    assert_eq!(ev[2], "resume attempt=2", "{ev:?}");
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
    assert_eq!(ev.len(), 7, "{ev:?}");
    assert_first_wait_woke_on_the_rotator(&ev);
    assert_eq!(
        ev[3], REENTERED_RATE,
        "before the second wait's start: {ev:?}"
    );
    assert!(
        ev[4].starts_with("start reason=usage_limit wait=capacity harness=claude reset=none "),
        "a 429 states no reset, so the second wait is a capacity wait with none: {ev:?}"
    );
    assert_eq!(ev[5], "end reason=rotator capacity", "{ev:?}");
    assert_eq!(ev[6], "resume attempt=3", "{ev:?}");
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
    assert_eq!(ev[3], REENTERED_RATE, "{ev:?}");
}

#[test]
fn a_usage_limit_message_on_the_retry_is_planned_as_the_first_wait_was() {
    // The retry's message states a reset an hour away, so the second wait is a
    // reset wait, not a capacity wait: the same planner, the same checks. Only
    // the same-login refusal is lifted.
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
    assert_eq!(ev.len(), 7, "{ev:?}");
    assert_first_wait_woke_on_the_rotator(&ev);
    assert_eq!(ev[3], REENTERED_USAGE, "{ev:?}");
    assert_eq!(
        ev[4],
        "start reason=usage_limit harness=claude reset=2026-10-18 09:00:00 -07:00 \
         until=2026-10-18 09:02:00 -07:00 bound=reset+margin",
        "{ev:?}"
    );
    assert_eq!(ev[5], "end reason=reset reached", "{ev:?}");
    assert_eq!(ev[6], "resume attempt=3", "{ev:?}");
}

// --- a reset that has already passed ------------------------------------------------
//
// The retry of the second wait meets the message it met before, read at 09:02:
// "resets 9:00am" is two minutes past, inside the ten minutes a clock time is
// still read as today. A *reset* wait for it would end the moment it began, spend
// none of the budget and run the same attempt again, so the ordinary planner
// has nothing to wait on. That is no reason to end the step: a reset in the
// past shows the stated reset cannot be relied on, not that no wait is owed.
// Where another login could take over the second wait is planned from capacity,
// which keeps it non-zero and so keeps the step's budget a bound on the chain.
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
fn a_reset_that_has_already_passed_is_waited_for_as_capacity_where_another_login_could_take_over() {
    // The budget is 7200 s: 30 s spent by the first wait and 3690 s by the
    // second leave 3480 s, so the third wait ends at 10:00:00, the budget's end.
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
        "a reset the clock has passed must not end the step while a login could take over: {}",
        describe(&out)
    );
    assert_eq!(
        f.count("0"),
        4,
        "limit, re-entered limit, passed reset, success"
    );
    let ev = events(&out);
    assert_eq!(ev.len(), 11, "{ev:?}");
    assert_first_wait_woke_on_the_rotator(&ev);
    assert_eq!(ev[3], REENTERED_USAGE, "{ev:?}");
    assert_eq!(ev[4], NINE_AM_RESET_WAIT, "{ev:?}");
    assert_eq!(ev[5], "end reason=reset reached", "{ev:?}");
    assert_eq!(ev[6], "resume attempt=3", "{ev:?}");
    assert_eq!(
        ev[7], REENTERED_USAGE,
        "one line for this re-entry, before its start: {ev:?}"
    );
    assert_eq!(
        ev[8],
        passed_reset_capacity_start("2026-10-18 10:00:00 -07:00", "max_wait"),
        "a capacity wait that prints the reset the message stated and is bounded \
         by the budget the step has left, not a fresh one: {ev:?}"
    );
    assert_eq!(ev[9], "end reason=rotator capacity", "{ev:?}");
    assert_eq!(ev[10], "resume attempt=4", "{ev:?}");
    assert_eq!(count(&ev, "reentered "), 2, "{ev:?}");
    assert_eq!(claude_stdout(&out), "attempt 4 output\n");
    f.assert_isolated("0");
}

#[test]
fn a_reset_that_has_already_passed_where_no_capacity_wait_could_start_is_refused_as_before() {
    // No rotator is running, so nothing could end a capacity wait before its
    // deadline and no capacity wait is started. The first wait is an ordinary
    // reset wait, which needs no rotator; the retry meets the same "resets
    // 8:30am", read at 08:32, and is passed up as already waited on.
    let f = with_profiles();
    x_is_live(&f);
    let mut c = home_step(&f);
    c.env("LIMIT_MESSAGE", LIMIT_TEXT).env("LIMIT_CALLS", "2");
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(f.count("0"), 2);
    let ev = events(&out);
    assert_eq!(
        ev,
        [
            "start reason=usage_limit harness=claude reset=2026-10-18 08:30:00 -07:00 \
             until=2026-10-18 08:32:00 -07:00 bound=reset+margin",
            "end reason=reset reached",
            "resume attempt=2",
            "skipped reason=already_waited",
            FAILED_AGAIN,
        ],
        "no `reentered` line and no second `start`: {ev:?}"
    );
    assert!(
        text(&out.stderr).contains(LIMIT_TEXT),
        "the runner's classifier must still see the limit"
    );
}

#[test]
fn an_organisation_spend_limit_that_states_a_passed_session_reset_is_never_a_capacity_wait() {
    // Another login of the same organisation does not lift a spend limit, so
    // the rule that turns a passed reset into a capacity wait must not reach it,
    // even with a rotator running and another login stored. The message states
    // its session reset, so it is waited out like any other, and the retry that
    // meets it again at 08:32 is passed up as already waited on.
    let f = with_profiles();
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f);
    c.env("LIMIT_MESSAGE", SPEND_SESSION_TEXT)
        .env("LIMIT_CALLS", "2");
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(f.count("0"), 2);
    let ev = events(&out);
    assert_eq!(
        ev,
        [
            "start reason=usage_limit harness=claude reset=2026-10-18 08:30:00 -07:00 \
             until=2026-10-18 08:32:00 -07:00 bound=reset+margin",
            "end reason=reset reached",
            "resume attempt=2",
            "skipped reason=already_waited",
            FAILED_AGAIN,
        ],
        "never a `wait=capacity` start for a spend limit: {ev:?}"
    );
}

#[test]
fn a_retry_on_a_login_the_step_has_never_waited_on_keeps_its_first_wait_grace() {
    // Call 1 fails on X while the rotator swaps to Y, so the login moved
    // before the wait began and the step retries at once, on Y. That retry
    // meets a reset five minutes behind the clock. It is a re-entry (the wait
    // that ended said *try now*), but Y is a login this step has never waited
    // on, so the ordinary rule refuses nothing: this is Y's first wait, a reset
    // wait that ends at once and retries, as it always did. No `reentered`
    // line, and no capacity wait, although a rotator is running and another
    // login could take over.
    let f = with_profiles();
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f);
    c.env("LIMIT_CALLS", "2")
        .env(
            "LIMIT_MESSAGE_2",
            "You've hit your session limit · resets 7:55am (America/Los_Angeles)",
        )
        .env("ROOM_ON_CALL", room_on_login(1, 5, true, "Y"));
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 3, "limit on X, limit on Y, success");
    let ev = events(&out);
    assert_eq!(ev.len(), 6, "{ev:?}");
    assert!(
        ev[0].starts_with("start reason=usage_limit wait=capacity harness=claude reset="),
        "{ev:?}"
    );
    assert_eq!(ev[1], "end reason=login changed", "{ev:?}");
    assert_eq!(ev[2], "resume attempt=2", "{ev:?}");
    assert_eq!(
        ev[3],
        "start reason=usage_limit harness=claude reset=2026-10-18 07:55:00 -07:00 \
         until=2026-10-18 08:00:00 -07:00 bound=reset+margin",
        "Y's own first wait, which ends at once: {ev:?}"
    );
    assert_eq!(ev[4], "end reason=reset reached", "{ev:?}");
    assert_eq!(ev[5], "resume attempt=3", "{ev:?}");
    assert_eq!(count(&ev, "reentered "), 0, "{ev:?}");
    assert_eq!(
        count(&ev, "start reason=usage_limit wait=capacity"),
        1,
        "{ev:?}"
    );
}

/// The chain of re-entries for a passed reset is bounded by the step's budget.
///
/// The stub says the limit ten times and writes a rotator cycle with room that
/// wakes every capacity wait at its first poll, so nothing but the budget can
/// stop the chain. A wait for a passed reset that planned no time (a reset wait
/// ending at once, or a capacity wait with no deadline of its own) would run
/// all ten limits and end in success on the eleventh attempt. The budget is
/// 3810 s: the first wait spends 30 s and the second 3690 s, which leaves 90 s
/// at 09:02:00. Each capacity wait is planned with what is left and is woken
/// after one 30 s poll, so the third has 90 s, the fourth 60 s and the fifth
/// 30 s, which ends at its own deadline before a poll could wake it. The step
/// ends on the fifth attempt, at 09:03:30, which is the budget's end.
///
/// What the test reads is the clock, not the count: every capacity wait of the
/// chain ends at `until=09:03:30`, and the last of them is *reached*
/// (`capacity expired`), which the wait can only report once its clock has got
/// there. A wake that cost nothing could not.
#[test]
fn the_budget_still_bounds_a_chain_of_waits_for_a_reset_that_keeps_passing() {
    let f = with_profiles();
    let _rotator = rotator_running(&f);
    let mut c = passed_reset_step(&f, 10);
    let mut rooms = vec![room_on(1, 5, true)];
    // Stamped far after any wait starts, so each wake is at its first poll.
    rooms.extend((3..=10).map(|call| room_on(call, 50_000, true)));
    c.env(MAX_WAIT_ENV, "3810")
        .env("ROOM_ON_CALL", rooms.join(" "));
    let out = f.run(&mut c);
    assert_eq!(
        out.status.code(),
        Some(1),
        "the chain must end, not run to the stub's last limit: {}",
        describe(&out)
    );
    assert_eq!(
        f.count("0"),
        5,
        "the budget ends the chain on the fifth attempt, not the eleventh: {}",
        describe(&out)
    );
    let ev = events(&out);
    assert_eq!(
        ev[ev.len() - 2..],
        ["end reason=capacity expired", FAILED_EXPIRED],
        "{ev:?}"
    );
    let capacity: Vec<&String> = ev
        .iter()
        .filter(|l| l.starts_with("start reason=usage_limit wait=capacity "))
        .collect();
    assert_eq!(
        capacity.len(),
        4,
        "the weekly wait and three for the passed reset: {ev:?}"
    );
    for l in &capacity[1..] {
        assert_eq!(
            **l,
            passed_reset_capacity_start("2026-10-18 09:03:30 -07:00", "max_wait"),
            "every wait of the chain ends at the budget's end: {ev:?}"
        );
    }
    assert_eq!(
        count(&ev, "reentered "),
        4,
        "one per re-entry, calls 2 to 5: {ev:?}"
    );
    assert!(
        text(&out.stderr).contains("resets 9:00am"),
        "the runner's classifier must still see the limit"
    );
}

#[test]
fn the_step_timeout_still_ends_a_wait_for_a_passed_reset() {
    // The step may run for 3900 s, so a wait must end 60 s before that: at
    // 09:04:00, which is earlier than the budget (unset: 21600 s) would allow.
    let f = with_profiles();
    let _rotator = rotator_running(&f);
    let mut c = passed_reset_step(&f, 3);
    c.env("AMPLIHACK_STEP_TIMEOUT", "3900")
        .env("ROOM_ON_CALL", room_on(1, 5, true));
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(
        f.count("0"),
        3,
        "no attempt after the timeout: {}",
        describe(&out)
    );
    let ev = events(&out);
    assert_eq!(ev[7], REENTERED_USAGE, "{ev:?}");
    assert_eq!(
        ev[8],
        passed_reset_capacity_start("2026-10-18 09:04:00 -07:00", "step_timeout"),
        "{ev:?}"
    );
    assert_eq!(
        ev[ev.len() - 2..],
        [
            "end reason=step timeout",
            "failed reason=step_timeout: usage limit not lifted"
        ],
        "{ev:?}"
    );
}

#[test]
fn the_budget_of_a_step_still_bounds_the_chain_of_waits() {
    // `max_wait_secs` is the step's whole budget. The first wait spends its 30
    // seconds to the poll that sees the rotator; the second is planned with
    // what is left, 15 seconds, so it ends at its own deadline before the
    // next poll could wake it. No retry count and no timer bounds the chain:
    // the budget does, as it always has.
    let f = with_profiles();
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f);
    c.env("LIMIT_CALLS", "5")
        .env("LIMIT_MESSAGE_2", RATE_429)
        .env(MAX_WAIT_ENV, "45")
        .env(
            "ROOM_ON_CALL",
            format!("{} {}", room_on(1, 5, true), room_on(2, 100, true)),
        );
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(f.count("0"), 2, "no third attempt: {}", describe(&out));
    let ev = events(&out);
    assert_eq!(ev[3], REENTERED_RATE, "{ev:?}");
    assert_eq!(
        ev[ev.len() - 2..],
        ["end reason=capacity expired", FAILED_EXPIRED],
        "{ev:?}"
    );
    assert_eq!(count(&ev, "start "), 2, "{ev:?}");
}

// --- a wait that ran out earns nothing --------------------------------------

/// A step with no rotator and no profile root of its own: the reset wait the
/// stock message asks for, planned with `max_wait_secs = 0`, ends at its
/// ceiling (`max reached`) and still gives the step its one retry.
fn ceiling_step(f: &Fixture) -> Command {
    executable(&f.tools.join("claude"), STUB);
    let mut c = f.step();
    c.env(MAX_WAIT_ENV, "0");
    c
}

#[test]
fn a_usage_limit_after_a_wait_that_ran_out_is_refused_as_it_always_was() {
    let f = Fixture::new();
    let mut c = ceiling_step(&f);
    c.env("LIMIT_CALLS", "2");
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(f.count("0"), 2, "one wait, one retry, then the step ends");
    let ev = events(&out);
    assert_eq!(count(&ev, "start "), 1, "{ev:?}");
    assert_eq!(ev[1], "end reason=max reached", "{ev:?}");
    assert_eq!(
        ev[ev.len() - 2..],
        ["skipped reason=already_waited", FAILED_AGAIN],
        "nothing said try, so a limit after the wait is no contradiction: {ev:?}"
    );
    assert_eq!(count(&ev, "reentered "), 0, "{ev:?}");
    assert!(
        text(&out.stderr).contains(LIMIT_TEXT),
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
        ev[1..],
        ["end reason=max reached", "resume attempt=2"],
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
    assert_eq!(ev.len(), 3, "only the first wait: {ev:?}");
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
    assert_eq!(ev[3], REENTERED_USAGE, "{ev:?}");
    assert!(
        ev[4].contains(" harness=claude reset=2026-10-18 09:00:00 -07:00 "),
        "a reset wait for the stated reset: {ev:?}"
    );
    assert_eq!(count(&ev, "reentered "), 1, "{ev:?}");
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
