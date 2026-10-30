//! Issue #1761: while a login rotator runs for `$HOME/.claude`, a step that
//! hits a usage limit again waits again and reruns, up to
//! `max_waits_per_step` waits.
//!
//! - AC1: two limits in one run both wait and rerun.
//! - AC2: a rotator whose last cycle left no capacity still counts as
//!   running; each wait runs to the reset.
//! - AC5: each wait and rerun names the step, the attempt, the seconds
//!   waited and the login resumed on; the record holds the same seconds.
//! - SR-1: `max_wait_secs = 0` still counts every retry as a wait, so the
//!   cap ends the step.
//! - SR-9: no line names the run id, a path or the account.
//!
//! See `issue_1618_fixture` and `issue_1761_fixture`.

#![cfg(unix)]

use crate::issue_1618_fixture::{Fixture, LIMIT_TEXT, claude_stdout, describe, raw_events, text};
use crate::issue_1761_fixture::{
    FakeRotator, LIVE, RUN_ID_VAR, WAITS_VAR, assert_lines, budget_record, budget_warnings,
    cap_note, field, home_claude, line_after, matches, record_lines, run_id, run_with_pid, waited,
    write_ledger,
};
use std::process::Command;

const START_1: &str = "start reason=usage_limit harness=claude reset=2026-10-18 08:30:00 -07:00 \
                       until=2026-10-18 08:32:00 -07:00 bound=reset+margin step=\\d+ attempt=1 rotator=running";
const START_2: &str = "start reason=usage_limit harness=claude reset=2026-10-18 08:30:00 -07:00 \
                       until=2026-10-18 08:32:00 -07:00 bound=reset+margin step=\\d+ attempt=2 rotator=running";

/// A step in `$HOME/.claude`, part of recipe run `id`, with a running
/// rotator whose last cycle was `outcome` for profile `team`.
fn rotating(f: &Fixture, outcome: &str, id: &str) -> (FakeRotator, Command) {
    let r = FakeRotator::start(f);
    write_ledger(f, LIVE, outcome);
    let mut c = f.step();
    home_claude(f, &mut c);
    c.env(RUN_ID_VAR, id);
    (r, c)
}

#[test]
fn ac1_two_limits_in_one_run_both_wait_and_rerun() {
    let f = Fixture::new();
    let (_r, mut c) = rotating(&f, "swapped", &run_id());
    c.env("LIMIT_CALLS", "2");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 3, "two waits, two reruns");
    assert_lines(
        &raw_events(&out),
        &[
            START_1,
            "end reason=rotator capacity waited=\\d+s",
            "resume attempt=2 step=\\d+ waited=\\d+s login=team",
            START_2,
            "end reason=rotator capacity waited=\\d+s",
            "resume attempt=3 step=\\d+ waited=\\d+s login=team",
        ],
    );
    assert_eq!(claude_stdout(&out), "attempt 3 output\n");
    assert!(
        budget_warnings(&out).is_empty(),
        "{:?}",
        budget_warnings(&out)
    );
    f.assert_isolated("0");
}

#[test]
fn ac2_a_rotator_with_no_capacity_still_counts_and_each_wait_runs_to_the_reset() {
    let f = Fixture::new();
    let (_r, mut c) = rotating(&f, "held", &run_id());
    c.env("LIMIT_CALLS", "2");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 3);
    // The second limit comes at 08:32, after the stated 08:30 reset, so
    // its wait is short; it still counts.
    assert_lines(
        &raw_events(&out),
        &[
            START_1,
            "end reason=reset reached waited=\\d+s",
            "resume attempt=2 step=\\d+ waited=\\d+s login=team",
            "start reason=usage_limit harness=claude reset=2026-10-18 08:30:00 -07:00 \
             until=2026-10-18 08:32:00 -07:00 bound=reset+margin step=\\d+ attempt=2 rotator=running",
            "end reason=reset reached waited=\\d+s",
            "resume attempt=3 step=\\d+ waited=\\d+s login=team",
        ],
    );
    f.assert_isolated("0");
}

#[test]
fn ac5_each_wait_and_rerun_names_the_step_attempt_wait_and_login() {
    let f = Fixture::new();
    let id = run_id();
    let (_r, mut c) = rotating(&f, "swapped", &id);
    c.env("LIMIT_CALLS", "2");
    let (pid, out) = run_with_pid(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    let ev = raw_events(&out);
    for l in ev.iter().filter(|l| !l.starts_with("end ")) {
        assert_eq!(field(l, "step"), Some(pid.to_string().as_str()), "{l}");
    }
    for l in ev.iter().filter(|l| l.starts_with("end ")) {
        assert_eq!(field(l, "step"), None, "end has no step: {l}");
    }
    let ends = waited(&ev, "end ");
    assert_eq!(ends.len(), 2, "{ev:?}");
    assert_eq!(waited(&ev, "resume "), ends, "resume repeats the wait");
    assert_eq!(
        record_lines(&budget_record(&f, &id)),
        ends,
        "each wait's seconds go to the run's budget record"
    );
    let attempts: Vec<_> = ev.iter().filter_map(|l| field(l, "attempt")).collect();
    assert_eq!(attempts, ["1", "2", "2", "3"], "{ev:?}");
}

#[test]
fn with_a_rotator_a_step_waits_five_times_by_default_then_stops() {
    let f = Fixture::new();
    let (_r, mut c) = rotating(&f, "swapped", &run_id());
    c.env("LIMIT_CALLS", "10");
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(f.count("0"), 6, "five waits, five reruns, then give up");
    let ev = raw_events(&out);
    assert_eq!(ev.iter().filter(|l| l.starts_with("start ")).count(), 5);
    assert!(
        matches(
            ev.last().unwrap(),
            "skipped reason=step_waits_spent step=\\d+"
        ),
        "{ev:?}"
    );
    assert_eq!(
        line_after(&out, "skipped reason=step_waits_spent"),
        Some(cap_note(5)),
        "the plain line follows its log line"
    );
    assert_eq!(claude_stdout(&out), "attempt 6 output\n");
    assert!(
        text(&out.stderr).contains(LIMIT_TEXT),
        "the runner must still see Claude's own message"
    );
}

#[test]
fn sr1_a_zero_maximum_counts_every_retry_toward_the_cap() {
    let f = Fixture::new();
    let (_r, mut c) = rotating(&f, "held", &run_id());
    c.env("AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS", "0")
        .env(WAITS_VAR, "3")
        .env("LIMIT_CALLS", "10");
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(f.count("0"), 4, "exactly three 0 s waits");
    let ev = raw_events(&out);
    let starts: Vec<_> = ev.iter().filter(|l| l.starts_with("start ")).collect();
    assert_eq!(starts.len(), 3, "{ev:?}");
    for (n, l) in starts.iter().enumerate() {
        assert!(l.contains(" bound=max_wait "), "{l}");
        assert!(
            l.ends_with(&format!(" attempt={} rotator=running", n + 1)),
            "{l}"
        );
    }
    assert_eq!(waited(&ev, "end "), vec![0, 0, 0]);
    assert!(
        matches(
            ev.last().unwrap(),
            "skipped reason=step_waits_spent step=\\d+"
        ),
        "{ev:?}"
    );
    assert_eq!(
        line_after(&out, "skipped reason=step_waits_spent"),
        Some(cap_note(3))
    );
}

#[test]
fn sr9_no_line_names_the_run_id_a_path_or_the_account() {
    let f = Fixture::new();
    let id = run_id();
    let (_r, mut c) = rotating(&f, "swapped", &id);
    crate::issue_1618_fixture::private_file(
        &f.home.join(".claude").join(".claude.json"),
        r#"{"oauthAccount":{"accountUuid":"uuid-SECRET-1761","emailAddress":"secret-person@example.invalid"}}"#,
    );
    c.env("LIMIT_CALLS", "2");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    let stderr = text(&out.stderr);
    for secret in [
        id.as_str(),
        "SECRET-1761",
        "secret-person",
        "example.invalid",
    ] {
        assert!(!stderr.contains(secret), "{secret:?} leaked: {stderr}");
    }
    let root = f.root.to_string_lossy().into_owned();
    for l in stderr
        .lines()
        .filter(|l| l.contains("usage_limit_wait") || l.contains("usage-limit"))
    {
        assert!(!l.contains(&root), "a path leaked into {l:?}");
        assert!(l.bytes().all(|b| (0x20..0x7f).contains(&b)), "{l:?}");
    }
}

#[test]
fn a_live_name_that_is_not_a_profile_name_resumes_as_unknown() {
    let f = Fixture::new();
    let (_r, mut c) = rotating(&f, "swapped", &run_id());
    write_ledger(&f, "a@b.com", "swapped");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    let ev = raw_events(&out);
    assert!(ev[0].ends_with(" rotator=running"), "{ev:?}");
    assert!(
        matches(
            ev.last().unwrap(),
            "resume attempt=2 step=\\d+ waited=\\d+s login=unknown"
        ),
        "{ev:?}"
    );
    assert!(!text(&out.stderr).contains("a@b.com"));
}
