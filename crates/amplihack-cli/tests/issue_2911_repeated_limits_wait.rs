//! Issue #2911: a second and a third limit in one step each wait. No
//! `already_waited` refusal and no budget ends the chain.
//!
//! What ends each wait depends on the limit, and both halves are pinned at
//! the end of this file: a wait for a stated reset runs the step again at
//! its reset and not before, and a wait with no reset to wait for has no
//! deadline and runs it again only on an event
//! (docs/reference/usage-limit-waits.md, "A wait with no deadline").

#![cfg(unix)]

use crate::issue_1618_fixture::{
    Fixture, LIMIT_TEXT, WAIT_PREFIX, claude_stdout, describe, events, text, toggle_login,
    wait_lines,
};

/// A session limit that states no reset, so its wait is a capacity wait with
/// no deadline: only an event ends it.
const NO_RESET_TEXT: &str = "You've hit your session limit · try again later";

fn step(f: &Fixture) -> std::process::Command {
    let mut c = f.step();
    c.env("LIMIT_MESSAGE", LIMIT_TEXT);
    c
}

/// A step whose limit gives it no reset to wait for.
fn no_reset_step(f: &Fixture) -> std::process::Command {
    let mut c = f.step();
    c.env("LIMIT_MESSAGE", NO_RESET_TEXT);
    c
}

/// The first limit's reset is waited for. The second and third meet the
/// same message once its reset has passed, which gives a wait with no
/// deadline; a login change ends each of those.
#[test]
fn three_limits_in_one_step_each_wait_and_the_fourth_attempt_succeeds() {
    let f = Fixture::new();
    let mut c = step(&f);
    c.env("LIMIT_CALLS", "3");
    let out = f.run_with_a_login_change_per_open_wait(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 4, "{}", describe(&out));
    let ev = events(&out);
    assert_eq!(
        ev.iter().filter(|l| l.starts_with("start ")).count(),
        3,
        "{ev:?}"
    );
    assert_eq!(claude_stdout(&out), "attempt 4 output\n");
}

/// Three limits with no reset in one step: each is a wait with no deadline,
/// each is ended by a login change, and nothing counts them or ends the
/// chain.
#[test]
fn a_chain_of_limits_with_no_reset_each_waits_for_an_event() {
    let f = Fixture::new();
    let mut c = no_reset_step(&f);
    c.env("LIMIT_CALLS", "3");
    let out = f.run_with_a_login_change_per_open_wait(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 4, "{}", describe(&out));
    let ev = events(&out);
    let starts: Vec<&String> = ev.iter().filter(|l| l.starts_with("start ")).collect();
    assert_eq!(starts.len(), 3, "{ev:?}");
    for l in &starts {
        assert!(l.ends_with(" until=none bound=none"), "{l}");
    }
    assert_eq!(
        ev.iter()
            .filter(|l| l.starts_with("end "))
            .collect::<Vec<_>>(),
        ["end reason=login changed"; 3],
        "each wait ended on its event: {ev:?}"
    );
    assert!(
        !ev.iter().any(|l| l.starts_with("failed")),
        "no total ends the chain: {ev:?}"
    );
}

#[test]
fn no_refusal_or_budget_ending_is_ever_logged() {
    let f = Fixture::new();
    let mut c = step(&f);
    c.env("LIMIT_CALLS", "3");
    let out = f.run_with_a_login_change_per_open_wait(&mut c);
    for l in events(&out) {
        for gone in [
            "already_waited",
            "capacity_expired",
            "capacity expired",
            "max reached",
            "failed reason=spend_limit",
        ] {
            assert!(!l.contains(gone), "{gone}: {l}");
        }
    }
}

/// The variable that used to bound a wait with no reset is not read, and a
/// host that still exports it is told so. The wait it would have bounded has
/// no deadline.
#[test]
fn the_old_length_variable_is_not_read_and_a_wait_with_no_reset_has_no_deadline() {
    let f = Fixture::new();
    let mut c = no_reset_step(&f);
    c.env("LIMIT_CALLS", "1")
        .env("AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS", "0");
    let out = f.run_with_a_login_change_per_open_wait(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2, "{}", describe(&out));
    assert!(
        text(&out.stderr).contains(
            "warning: AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS is no longer read. A usage-limit \
             wait has no timed bound"
        ),
        "{}",
        describe(&out)
    );
    assert_eq!(
        events(&out),
        [
            "waiting_for_capacity reason=no_reset harness=claude until=none",
            "start reason=usage_limit wait=capacity harness=claude reset=none until=none \
             bound=none",
            "end reason=login changed",
            "resume attempt=2",
        ]
    );
}

/// A wait for a stated reset runs the step again at that reset and not
/// before: half an hour, with nothing to end it early, is one attempt before
/// the wait and one after its reset, and the progress lines show the wait
/// still running within two minutes of it.
#[test]
fn a_reset_wait_runs_the_step_again_only_at_its_reset() {
    let f = Fixture::new();
    let mut c = step(&f);
    c.env("LIMIT_CALLS", "1");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        f.count("0"),
        2,
        "one attempt before the wait and one after its reset: {}",
        describe(&out)
    );
    assert_eq!(
        events(&out),
        [
            "start reason=usage_limit harness=claude reset=2026-10-18 08:30:00 -07:00 \
             until=2026-10-18 08:32:00 -07:00 bound=reset+margin",
            "end reason=reset reached",
            "resume attempt=2",
        ],
        "the reset ended the wait"
    );
    let remaining: Vec<u64> = wait_lines(&out)
        .iter()
        .filter_map(|l| l.strip_prefix("progress remaining="))
        .filter_map(|r| r.strip_suffix('s'))
        .filter_map(|r| r.parse().ok())
        .collect();
    assert!(
        remaining.iter().any(|r| *r < 120),
        "the wait was still running within two minutes of its reset: {remaining:?}"
    );
    assert_eq!(claude_stdout(&out), "attempt 2 output\n");
}

/// The other half: a wait whose limit gives it no reset to wait for has no
/// deadline, so no clock runs the step again. Nothing records when such a
/// limit lifts, and on this host there is no rotator and no other login, so
/// the step waits for a person's event however long that takes.
///
/// Here the step is still on its first attempt after 400 virtual minutes,
/// past the six hours the removed outer bound used to give it, and the login
/// change made then is what runs it again.
#[test]
fn a_wait_with_no_reset_runs_the_step_again_only_on_an_event() {
    let f = Fixture::new();
    let mut c = no_reset_step(&f);
    c.env("LIMIT_CALLS", "1");
    let progress = format!("{WAIT_PREFIX}progress ");
    let mut seen = 0u32;
    let mut attempts_by_then = None;
    let out = f.run_watching(&mut c, |line| {
        if line.starts_with(&progress) {
            seen += 1;
            if seen == 400 {
                attempts_by_then = Some(f.count("0"));
                toggle_login(&f.config);
            }
        }
    });
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        attempts_by_then,
        Some(1),
        "no attempt ran in 400 minutes of waiting: {}",
        describe(&out)
    );
    assert_eq!(f.count("0"), 2, "{}", describe(&out));
    assert_eq!(
        events(&out),
        [
            "waiting_for_capacity reason=no_reset harness=claude until=none",
            "start reason=usage_limit wait=capacity harness=claude reset=none until=none \
             bound=none",
            "end reason=login changed",
            "resume attempt=2",
        ],
        "the login change, and no timer, ran the step again"
    );
    assert!(
        wait_lines(&out)
            .iter()
            .filter(|l| l.starts_with("progress "))
            .all(|l| l == "progress remaining=none"),
        "a wait with no deadline has nothing to count down to"
    );
}
