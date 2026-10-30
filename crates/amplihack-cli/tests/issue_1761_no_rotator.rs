//! Issue #1761: with no rotator seen, a step keeps the #1618 behaviour: it
//! waits once for the stated reset, and a second limit ends it with
//! `skipped reason=already_waited step=<id> rotator=none`. Each `start`
//! says that no rotator was seen.
//!
//! - AC4: the fixture's own `CLAUDE_CONFIG_DIR`, no rotator anywhere.
//! - `$HOME/.claude` with no pid file, or one naming no rotator: no
//!   rotator, though a capacity cycle in the ledger still ends the wait
//!   early and names the login.
//! - A rotator swaps logins only into `$HOME/.claude`, so it does not count
//!   for another config directory or (AC6) for an inherited profile.
//!
//! See `issue_1618_fixture` and `issue_1761_fixture`.

#![cfg(unix)]

use crate::issue_1618_fixture::{Fixture, describe, private_file, raw_events, text};
use crate::issue_1618_wait_profiles::with_profiles;
use crate::issue_1761_fixture::{
    FakeRotator, LIVE, RUN_ID_VAR, assert_lines, home_claude, run_id, write_ledger,
};
use std::process::Output;

const ALREADY_WAITED: &str = "skipped reason=already_waited step=\\d+ rotator=none";

/// The step gave up at its second limit after one wait, with no plain line.
fn assert_one_wait(f: &Fixture, out: &Output) {
    assert_eq!(out.status.code(), Some(1), "{}", describe(out));
    assert_eq!(f.count("0"), 2, "one wait, one retry, then give up");
    assert!(
        !text(&out.stderr).contains("not waiting again"),
        "already_waited has no plain line"
    );
}

#[test]
fn ac4_without_a_rotator_a_second_limit_ends_the_step_after_one_wait() {
    let f = Fixture::new();
    let mut c = f.step();
    c.env(RUN_ID_VAR, run_id()).env("LIMIT_CALLS", "5");
    let out = f.run(&mut c);
    assert_one_wait(&f, &out);
    assert_lines(
        &raw_events(&out),
        &[
            "start reason=usage_limit harness=claude reset=2026-10-18 08:30:00 -07:00 \
             until=2026-10-18 08:32:00 -07:00 bound=reset+margin step=\\d+ attempt=1 rotator=none",
            "end reason=reset reached waited=\\d+s",
            "resume attempt=2 step=\\d+ waited=\\d+s login=unknown",
            ALREADY_WAITED,
        ],
    );
    f.assert_isolated("0");
}

#[test]
fn home_claude_without_a_running_rotator_waits_once() {
    for stale in [None, Some("1\n"), Some("not a pid\n")] {
        let f = Fixture::new();
        write_ledger(&f, LIVE, "swapped");
        if let Some(body) = stale {
            private_file(
                &f.profiles
                    .join(".auto-switch.d")
                    .join("rotate")
                    .join("rotate.pid"),
                body,
            );
        }
        let mut c = f.step();
        home_claude(&f, &mut c);
        c.env(RUN_ID_VAR, run_id()).env("LIMIT_CALLS", "5");
        let out = f.run(&mut c);
        assert_one_wait(&f, &out);
        // The 30 s capacity poll still reads the ledger, and `login=` names
        // its live profile whether or not a rotator was seen.
        assert_lines(
            &raw_events(&out),
            &[
                "start reason=usage_limit harness=claude reset=2026-10-18 08:30:00 -07:00 \
                 until=2026-10-18 08:32:00 -07:00 bound=reset+margin step=\\d+ attempt=1 rotator=none",
                "end reason=rotator capacity waited=\\d+s",
                "resume attempt=2 step=\\d+ waited=\\d+s login=team",
                ALREADY_WAITED,
            ],
        );
        f.assert_isolated("0");
    }
}

#[test]
fn a_rotator_does_not_count_for_another_config_dir() {
    let f = Fixture::new();
    let _r = FakeRotator::start(&f);
    write_ledger(&f, LIVE, "swapped");
    let mut c = f.step();
    c.env(RUN_ID_VAR, run_id()).env("LIMIT_CALLS", "5");
    let out = f.run(&mut c);
    assert_one_wait(&f, &out);
    let ev = raw_events(&out);
    assert!(ev[0].ends_with(" attempt=1 rotator=none"), "{ev:?}");
    assert!(ev[2].ends_with(" login=unknown"), "{ev:?}");
    assert!(
        crate::issue_1761_fixture::matches(ev.last().unwrap(), ALREADY_WAITED),
        "{ev:?}"
    );
}

#[test]
fn ac6_a_step_under_an_inherited_profile_ignores_the_rotator() {
    let f = with_profiles();
    let _r = FakeRotator::start(&f);
    write_ledger(&f, "X", "swapped");
    let mut c = f.step();
    c.env("AMPLIHACK_CLAUDE_PROFILE", "X")
        .env("CLAUDE_CONFIG_DIR", f.profiles.join("X"))
        .env("AMPLIHACK_CLAUDE_AUTO_SWITCH", "0")
        .env(RUN_ID_VAR, run_id())
        .env("LIMIT_PROFILE", "X")
        .env("LIMIT_CALLS", "5");
    let out = f.run(&mut c);
    assert_one_wait(&f, &out);
    assert_lines(
        &raw_events(&out),
        &[
            "start reason=usage_limit harness=claude reset=2026-10-18 08:30:00 -07:00 \
             until=2026-10-18 08:32:00 -07:00 bound=reset+margin step=\\d+ attempt=1 rotator=none",
            "end reason=reset reached waited=\\d+s",
            "resume attempt=2 step=\\d+ waited=\\d+s login=X",
            ALREADY_WAITED,
        ],
    );
    for n in [1, 2] {
        assert_eq!(f.call("0", n)["PROFILE"], "X", "call {n}");
    }
    f.assert_isolated("0");
}
