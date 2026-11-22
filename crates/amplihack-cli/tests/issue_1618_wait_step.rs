//! Issue #1618: a recipe agent step (`amplihack claude -p` with
//! `AMPLIHACK_NONINTERACTIVE=1`) that hits a usage limit waits for the reset
//! and runs once more with the same inputs, instead of ending the run.
//!
//! Covers AC1 (one retry, identical argv, cwd and stdin, only the retry's
//! stdout), AC7 (a second limit is waited out too, and never refused),
//! AC10 (exact log lines), the epoch form, and ST7 (a marker on exit 0 or a
//! failure that is not a usage limit never waits).
//!
//! Issue #2911 changed what the limits with no usable reset do. A spend
//! limit, a reset more than five hours away, a reset that does not parse and
//! a reset already behind the clock were each passed up at once, as
//! `skipped reason=spend_limit`, `reset_too_far`, `no_reset` or, for a reset
//! inside a ten-minute grace, a wait of no length. Each is now a wait for
//! capacity or for the login to change, announced by a
//! `waiting_for_capacity` line naming why, and the step retries when the
//! wait ends however it ended. Nothing counts a step's waits and no limit
//! refuses one: the only reading left here that ends a step is output with
//! no limit message in it at all. The stdin cases (a piped prompt, a
//! held-open pipe) are in `issue_1618_wait_stdin`. See
//! `issue_1618_fixture`.

#![cfg(unix)]

use crate::issue_1618_fixture::{
    Fixture, LIMIT_TEXT, SPEND_SESSION_TEXT, SPEND_TEXT, SPEND_WEEKLY_TEXT, WEEKLY_TEXT,
    assert_progress_shape, claude_stdout, describe, events, sha256_hex, text, wait_lines,
};
use std::fs;
use std::process::Stdio;

/// Runs a step whose stdin is `input`.
fn step_with_stdin(f: &Fixture, input: &[u8], extra: &[(&str, &str)]) -> std::process::Output {
    let path = f.root.join("stdin.bin");
    fs::write(&path, input).unwrap();
    let mut c = f.step();
    c.stdin(Stdio::from(fs::File::open(&path).unwrap()));
    for (k, v) in extra {
        c.env(k, v);
    }
    f.run(&mut c)
}

#[test]
fn ac1_a_step_limited_once_waits_then_retries_with_identical_inputs() {
    let f = Fixture::new();
    let input: Vec<u8> = b"the step's prompt\n".repeat(5000);
    let out = step_with_stdin(&f, &input, &[]);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2, "exactly one retry");
    let (first, second) = (f.call("0", 1), f.call("0", 2));
    for key in [
        "ARGV",
        "PWD",
        "HOME",
        "CLAUDE_CONFIG_DIR",
        "PROFILE",
        "STDIN",
    ] {
        assert_eq!(first[key], second[key], "{key} differs between attempts");
    }
    // Issue #2030 (D7): `-p <text>` is rewritten to a bare `-p` with the
    // text on the child's stdin. The caller's stdin follows it after a blank
    // line (crusty round 4), and the supervisor rewinds a regular-file stdin
    // for each attempt, so both attempts receive the same bytes.
    let mut expected = b"step\n\n".to_vec();
    expected.extend_from_slice(&input);
    assert_eq!(
        first["STDIN"],
        sha256_hex(&expected),
        "the rewritten prompt, then the caller's stdin, byte-exact"
    );
    assert_eq!(first["PROFILE"], "<unset>", "no profile override");
    assert_eq!(
        claude_stdout(&out),
        "attempt 2 output\n",
        "only the retry's stdout is printed"
    );
    assert_eq!(
        events(&out),
        vec![
            "start reason=usage_limit harness=claude reset=2026-10-18 08:30:00 -07:00 \
             until=2026-10-18 08:32:00 -07:00 bound=reset+margin",
            "end reason=reset reached",
            "resume attempt=2",
        ]
    );
    f.assert_isolated("0");
}

#[test]
fn ac10_lines_have_the_documented_fields_and_order() {
    let f = Fixture::new();
    let out = f.run(&mut f.step());
    assert!(out.status.success(), "{}", describe(&out));
    let lines = wait_lines(&out);
    let pos = |prefix: &str| {
        lines
            .iter()
            .position(|l| l.starts_with(prefix))
            .unwrap_or_else(|| panic!("no {prefix:?} line in {lines:?}"))
    };
    let (start, end, resume) = (pos("start "), pos("end "), pos("resume "));
    assert!(start < end && end < resume, "{lines:?}");
    for (i, l) in lines.iter().enumerate() {
        if i > start && i < end {
            assert!(
                l.starts_with("progress "),
                "only progress between start and end: {l}"
            );
        }
    }
    assert_progress_shape(&out);
    let stderr = text(&out.stderr);
    for l in stderr.lines().filter(|l| l.contains("usage_limit_wait")) {
        assert!(l.starts_with("amplihack.recipe.usage_limit_wait "), "{l:?}");
        assert!(l.bytes().all(|b| (0x20..0x7f).contains(&b)), "{l:?}");
    }
}

#[test]
fn the_epoch_form_gives_its_reset() {
    let f = Fixture::new();
    // 2026-10-18 08:10:00 -07:00, built so the literal stays in one place.
    let msg = format!("Claude AI usage limit {}|1792336200", "reached");
    let mut c = f.step();
    c.env("LIMIT_MESSAGE", &msg);
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        events(&out)[0],
        "start reason=usage_limit harness=claude reset=2026-10-18 08:10:00 -07:00 \
         until=2026-10-18 08:12:00 -07:00 bound=reset+margin"
    );
}

/// Issue #2911: a limit a wait cannot outlast is waited for all the same.
///
/// This is the core claim the issue reverses. Each of these messages used to
/// be passed up the moment it was read, as `skipped reason=spend_limit`,
/// `skipped reason=reset_too_far` or `skipped reason=no_reset`, on the
/// grounds that this single-login fixture has no other login that could take
/// over: refusing to wait because nothing has room *right now* is exactly
/// the failure #2911 removes, because the wait's own wake conditions notice a
/// login gaining room later. Every one of them now says
/// `waiting_for_capacity` with its reason, waits, and retries when the wait
/// ends. The one reading left that no wait can help, work prose with no
/// limit message in it, still ends the step, and is checked last.
///
/// Each wait is bounded by the 60-second floor and the fixture's virtual
/// clock, so the chain costs no real time.
#[test]
fn limits_a_wait_cannot_outlast_are_waited_for_not_passed_up() {
    // Every wait here is bounded by its configured maximum of 60 seconds,
    // the floor `plan::MIN_WAIT_SECS` sets.
    const UNTIL: &str = "2026-10-18 08:01:00 -07:00";
    for (message, why, start_reason, reset, end) in [
        // A spend limit with no reset waits for the login itself to change.
        (
            SPEND_TEXT,
            "spend_limit",
            "spend_limit wait=login_change",
            "none",
            "max reached",
        ),
        // A weekly reset is more than five hours away: next year's Oct 8.
        (
            SPEND_WEEKLY_TEXT,
            "reset_too_far",
            "usage_limit wait=capacity",
            "2027-10-08 00:00:00 -07:00",
            "capacity expired",
        ),
        (
            WEEKLY_TEXT,
            "reset_too_far",
            "usage_limit wait=capacity",
            "2027-10-08 00:00:00 -07:00",
            "capacity expired",
        ),
        // No reset that parses at all.
        (
            "You've hit your session limit · try again later",
            "no_reset",
            "usage_limit wait=capacity",
            "none",
            "capacity expired",
        ),
    ] {
        let f = Fixture::new();
        let mut c = f.step();
        c.env("LIMIT_MESSAGE", message)
            .env("AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS", "60");
        let out = f.run(&mut c);
        assert!(out.status.success(), "{why}: {}", describe(&out));
        assert_eq!(f.count("0"), 2, "{why}: the step waits, then retries once");
        assert_eq!(
            events(&out),
            [
                format!("waiting_for_capacity reason={why} harness=claude until={UNTIL}"),
                format!(
                    "start reason={start_reason} harness=claude reset={reset} until={UNTIL} \
                     bound=max_wait"
                ),
                format!("end reason={end}"),
                "resume attempt=2".to_string(),
            ],
            "{why}"
        );
    }
    // Neutral stderr: work markers such as "failed" outrank a limit, so the
    // output carries no limit message and no wait could help.
    let prose = "Done. Note: the usage limit resets 8:30am (America/Los_Angeles).";
    let f = Fixture::new();
    let mut c = f.step();
    c.env("LIMIT_MESSAGE", "step stopped")
        .env("STDOUT_TEXT", prose);
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(f.count("0"), 1, "no rerun for a step that met no limit");
    assert_eq!(events(&out), ["skipped reason=no_limit_message"]);
}

#[test]
fn a_spend_limit_that_states_a_session_reset_waits_for_it() {
    let f = Fixture::new();
    let mut c = f.step();
    c.env("LIMIT_MESSAGE", SPEND_SESSION_TEXT);
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2);
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

/// Issue #2911: the ten-minute grace is gone, and a reset already behind the
/// clock is a real wait for capacity.
///
/// A reset five minutes old used to fall inside a grace that planned a wait
/// of no length (`upgrade_passed_reset`, now deleted), so the step retried at
/// once. There are no zero-length waits any more: such a reset is always
/// `Capacity(PassedReset)`, because the message may be minutes stale and a
/// time that has been and gone is not something to sleep to, so the step
/// waits for a reading instead and retries when the wait ends.
#[test]
fn a_reset_already_behind_the_clock_waits_for_capacity() {
    let f = Fixture::new();
    let mut c = f.step();
    c.env(
        "LIMIT_MESSAGE",
        "You've hit your session limit · resets 7:55am (America/Los_Angeles)",
    )
    .env("AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS", "60");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        events(&out),
        vec![
            "waiting_for_capacity reason=passed_reset harness=claude \
             until=2026-10-18 08:01:00 -07:00",
            "start reason=usage_limit wait=capacity harness=claude \
             reset=2026-10-18 07:55:00 -07:00 until=2026-10-18 08:01:00 -07:00 bound=max_wait",
            "end reason=capacity expired",
            "resume attempt=2",
        ]
    );
    assert_eq!(f.count("0"), 2);
}

/// Issue #2911: a second limit is waited out too, and a limit is never
/// refused however many the step meets.
///
/// The step used to carry a per-step budget. The first wait ended
/// `reset reached`, the retry met the same message read two minutes past its
/// reset, that re-entry waited on capacity for whatever budget was left, and
/// the attempt after it was refused with `skipped reason=already_waited` and
/// `failed reason=already_waited`. That refusal is one of the exact failures
/// #2911 reports: a run ended by a limit that only meant *wait longer*. The
/// budget, the refusal and the marks that counted a login's waits are all
/// gone.
///
/// So the stub's five limits now produce five waits and six attempts, and the
/// step succeeds. The chain is paced by the waits costing time, not by a
/// count: every wait here ends strictly after it began, and the virtual clock
/// moves only when a wait sleeps, which is why the whole chain is exact. The
/// test is bounded by `LIMIT_CALLS`, that is, by the stub running out of
/// limits to report, rather than by anything ending the chain.
///
/// The second wait is the re-entry #2857 named, reported once on its own
/// line: the first wait said *try now* and the limit on its retry
/// contradicted that.
#[test]
fn ac7_a_second_limit_is_waited_out_and_never_refused() {
    let f = Fixture::new();
    let mut c = f.step();
    c.env("LIMIT_CALLS", "5");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 6, "five waits, and one attempt after each");
    let ev = events(&out);
    assert_eq!(
        ev,
        [
            // The limit's own reset, two minutes of margin added.
            "start reason=usage_limit harness=claude reset=2026-10-18 08:30:00 -07:00 \
             until=2026-10-18 08:32:00 -07:00 bound=reset+margin",
            "end reason=reset reached",
            "resume attempt=2",
            // The retry meets the same message, now read at 08:32: a reset
            // two minutes past, which contradicts the wait that said to try
            // now and waits for capacity rather than ending the step.
            "reentered met=usage_limit",
            "waiting_for_capacity reason=passed_reset harness=claude \
             until=2026-10-18 14:32:00 -07:00",
            "start reason=usage_limit wait=capacity harness=claude \
             reset=2026-10-18 08:30:00 -07:00 until=2026-10-18 14:32:00 -07:00 bound=max_wait",
            // Nothing lifted the limit, and the step retries all the same.
            "end reason=capacity expired",
            "resume attempt=3",
            // From here each 8:30am is tomorrow's, more than five hours off.
            "waiting_for_capacity reason=reset_too_far harness=claude \
             until=2026-10-18 20:32:00 -07:00",
            "start reason=usage_limit wait=capacity harness=claude \
             reset=2026-10-19 08:30:00 -07:00 until=2026-10-18 20:32:00 -07:00 bound=max_wait",
            "end reason=capacity expired",
            "resume attempt=4",
            "waiting_for_capacity reason=reset_too_far harness=claude \
             until=2026-10-19 02:32:00 -07:00",
            "start reason=usage_limit wait=capacity harness=claude \
             reset=2026-10-19 08:30:00 -07:00 until=2026-10-19 02:32:00 -07:00 bound=max_wait",
            "end reason=capacity expired",
            "resume attempt=5",
            // The fifth wait reaches that reset, which is now within the
            // wait's own length, so the step is retried on its arrival.
            "waiting_for_capacity reason=reset_too_far harness=claude \
             until=2026-10-19 08:32:00 -07:00",
            "start reason=usage_limit wait=capacity harness=claude \
             reset=2026-10-19 08:30:00 -07:00 until=2026-10-19 08:32:00 -07:00 bound=reset+margin",
            "end reason=reset reached",
            "resume attempt=6",
        ]
    );
    assert!(
        !ev.iter()
            .any(|l| l.starts_with("skipped ") || l.starts_with("failed ")),
        "no limit in the chain is refused and none ends the step: {ev:?}"
    );
    assert_eq!(claude_stdout(&out), "attempt 6 output\n");
    let stderr = text(&out.stderr);
    assert!(
        stderr.contains(LIMIT_TEXT),
        "each failed attempt's limit message still reaches the runner: {stderr}"
    );
}

#[test]
fn st7_a_limit_message_on_exit_zero_never_waits() {
    let f = Fixture::new();
    let mut c = f.step();
    c.env("LIMIT_EXIT", "0");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 1);
    assert!(wait_lines(&out).is_empty(), "{:?}", wait_lines(&out));
}

#[test]
fn a_failure_that_is_not_a_usage_limit_never_waits() {
    let f = Fixture::new();
    let mut c = f.step();
    c.env("LIMIT_MESSAGE", "fatal: something unrelated broke");
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(f.count("0"), 1);
    assert!(wait_lines(&out).is_empty(), "{:?}", wait_lines(&out));
}

#[test]
fn a_launch_that_is_not_a_print_step_is_left_alone() {
    let f = Fixture::new();
    let mut c = f.cmd(&["claude", "--subprocess-safe", "--", "step"]);
    c.stdin(Stdio::null());
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(f.count("0"), 1);
    assert!(wait_lines(&out).is_empty(), "{:?}", wait_lines(&out));
}

#[test]
fn a_launch_that_is_already_supervised_never_waits() {
    let f = Fixture::new();
    let mut c = f.step();
    c.env("AMPLIHACK_AUTO_SWITCH_SUPERVISED", "1");
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(
        f.count("0"),
        1,
        "a nested launch cannot start a second wait"
    );
    assert!(wait_lines(&out).is_empty(), "{:?}", wait_lines(&out));
}
