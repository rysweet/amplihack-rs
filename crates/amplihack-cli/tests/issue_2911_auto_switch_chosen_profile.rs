//! Issue #2911, the owner's deputy's case of 21 November, end to end: a step
//! launched on a profile *auto-switch chose*, on a host with no rotator,
//! waits for a usage limit and carries on.
//!
//! On hypatia (no rotator, profiles chosen by auto-switch) the run
//! `amp-auto-drive-to-merge-mistt-infra-20261121-213610` ended at 21:45Z
//! because step `step-03-resolve-pr` printed
//!
//! ```text
//! You've hit your individual spend limit · run /usage-credits to ask your admin for a higher limit · your 5-hour limit resets 6:30pm (America/Los_Angeles)
//! ```
//!
//! while running under the profile auto-switch had substituted for the
//! exhausted inherited one. The run's log held no `usage_limit_wait` line at
//! all: no wait was started and the step failed. The deputy asked for two
//! things, and this file drives both at the `amplihack claude
//! --subprocess-safe -- -p step` boundary with that message text:
//!
//! 1. the per-member spend cap states its reset after the words `5-hour
//!    limit resets`, and that reset is what the step waits for;
//! 2. a step on a profile auto-switch chose, with no rotator on the host,
//!    waits rather than failing.
//!
//! The planner's own reading of the line is pinned in
//! `limit_wait/tests_spend_reset.rs`; what this file adds is the whole path,
//! including the substitution that put the step on another profile in the
//! first place.
//!
//! Both tests run on the fixture's virtual clock, so the wait's log times
//! are exact. The readings are stamped with real time, which is what
//! `substitute` compares them against, and which is far enough ahead of the
//! virtual clock that the wait's own 30-second probe treats them as stale
//! and never ends a wait on them (`takeover::is_fresh`): what ends these
//! waits is the reset.

#![cfg(unix)]

use crate::issue_1618_fixture::{Fixture, TEST_NOW, describe, events, private_file, text};
use crate::issue_1618_wait_profiles::{switch_on, with_profiles};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const TOGGLE_ENV: &str = "AMPLIHACK_CLAUDE_AUTO_SWITCH";
/// The line hypatia's step printed, byte for byte.
const SPEND_FIVE_HOUR: &str = "You've hit your individual spend limit · run /usage-credits to ask your admin for a higher limit · your 5-hour limit resets 6:30pm (America/Los_Angeles)";
/// 2026-10-18 15:00:00 -07:00, so the stated 6:30pm is 3.5 hours ahead and
/// inside the five-hour ceiling, as it was on the day (21:45Z, reset 18:30
/// Los Angeles).
const AFTERNOON: i64 = TEST_NOW + 7 * 3600;

fn real_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// The auto-switch readings, stamped with real time: each `(profile,
/// five-hour percentage)`. Written in one step so no reader sees half a
/// file.
fn readings(profiles: &Path, at: i64, of: &[(&str, f64)]) {
    let dir = profiles.join(".auto-switch.d");
    fs::create_dir_all(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let accounts: serde_json::Map<String, serde_json::Value> = of
        .iter()
        .map(|(name, pct)| {
            (
                (*name).to_string(),
                serde_json::json!({
                    "five_hour": {"used_percentage": pct, "resets_at": at + 3600},
                    "seven_day": {"used_percentage": 5.0, "resets_at": at + 86_400},
                    "observed_at": at,
                    "source": "statusline",
                }),
            )
        })
        .collect();
    let body = serde_json::json!({"version": 1, "accounts": accounts}).to_string();
    let path = dir.join("usage.json");
    let tmp = path.with_extension("tmp");
    private_file(&tmp, &body);
    fs::rename(&tmp, &path).unwrap();
}

/// A recipe agent step that inherited profile X, with auto-switch on, the
/// hypatia message on every profile for its first `limited` calls, and the
/// afternoon clock.
fn step_inheriting_x(f: &Fixture, limited: u32) -> Command {
    switch_on(f);
    let mut c = f.step();
    c.env("AMPLIHACK_CLAUDE_PROFILE", "X")
        .env("CLAUDE_CONFIG_DIR", f.profiles.join("X"))
        .env(TOGGLE_ENV, "1")
        .env("LIMIT_PROFILE", "")
        .env("LIMIT_CALLS", limited.to_string())
        .env("LIMIT_MESSAGE", SPEND_FIVE_HOUR)
        .env("AMPLIHACK_USAGE_LIMIT_WAIT_TEST_NOW", AFTERNOON.to_string());
    c
}

fn assert_auto_switch_chose_y(out: &std::process::Output) {
    assert!(
        text(&out.stderr).contains("auto-switch is using profile \"Y\" for this launch"),
        "the step ran on a profile auto-switch chose: {}",
        describe(out)
    );
}

fn assert_no_limit_ended_the_step(ev: &[String], out: &std::process::Output) {
    assert!(
        !ev.iter()
            .any(|l| l.starts_with("skipped ") || l.starts_with("failed ")),
        "a limit never ends a step: {ev:?}\n{}",
        describe(out)
    );
}

/// The deputy's own case: the inherited profile is over the auto-switch
/// threshold but has no window at 100%, so auto-switch runs the step on Y
/// and keeps X in reserve. Y meets the spend cap, no other profile is
/// eligible, the step goes back to X as an auto-switch rerun, X meets it
/// too, and only then is a wait planned, from the message's own `5-hour
/// limit resets 6:30pm`. The step then runs a third time and succeeds.
///
/// Before #2911 this run ended with the limit result and no wait line.
#[test]
fn a_spend_cap_on_an_auto_switch_chosen_profile_waits_for_its_stated_reset_and_continues() {
    let f = with_profiles();
    readings(&f.profiles, real_now(), &[("X", 96.0), ("Y", 10.0)]);
    // No rotator: no lock held and no rotate ledger written.
    assert!(
        !f.profiles.join(".auto-switch.d").join("rotate").exists(),
        "this host has no rotator"
    );
    let mut c = step_inheriting_x(&f, 2);
    let out = f.run(&mut c);

    assert!(out.status.success(), "{}", describe(&out));
    assert_auto_switch_chose_y(&out);
    assert_eq!(
        f.count("0"),
        3,
        "Y, then X, then the retry: {}",
        describe(&out)
    );
    assert_eq!(f.call("0", 1)["PROFILE"], "Y", "auto-switch chose it");
    assert_eq!(f.call("0", 2)["PROFILE"], "X", "the auto-switch rerun");
    assert_eq!(f.call("0", 3)["PROFILE"], "X", "the retry after the wait");
    assert_eq!(
        events(&out),
        vec![
            "start reason=usage_limit harness=claude reset=2026-10-18 18:30:00 -07:00 \
             until=2026-10-18 18:32:00 -07:00 bound=reset+margin",
            "end reason=reset reached",
            "resume attempt=3",
        ],
        "the wait is for the reset the message stated: {}",
        describe(&out)
    );
    assert_no_limit_ended_the_step(&events(&out), &out);
    f.assert_isolated("0");
}

/// The same path with the inherited profile at 100%, which is what hypatia's
/// stderr reported (`profile "ddrumpf" is at 100% of its five-hour limit`).
/// X is then exhausted rather than merely over the threshold, so there is no
/// rerun to go back to it: the step waits on the first limit it meets, timed
/// by the exhausted inherited window, and retries under X when that resets.
///
/// The reading's reset is moved onto the wait's clock as the distance it is,
/// so the times are asserted by shape rather than to the second.
#[test]
fn the_same_step_waits_when_the_inherited_profile_is_exhausted() {
    let f = with_profiles();
    readings(&f.profiles, real_now(), &[("X", 100.0), ("Y", 10.0)]);
    let mut c = step_inheriting_x(&f, 1);
    let out = f.run(&mut c);

    assert!(out.status.success(), "{}", describe(&out));
    assert_auto_switch_chose_y(&out);
    assert_eq!(f.count("0"), 2, "{}", describe(&out));
    assert_eq!(f.call("0", 1)["PROFILE"], "Y");
    assert_eq!(f.call("0", 2)["PROFILE"], "X", "the retry after the wait");
    let ev = events(&out);
    assert!(
        ev[0].starts_with("start reason=usage_limit harness=claude reset=2026-10-18 1")
            && ev[0].ends_with(" bound=reset+margin"),
        "a reset wait, timed by the exhausted inherited window: {ev:?}"
    );
    assert_eq!(ev[1], "end reason=reset reached", "{ev:?}");
    assert_eq!(ev[2], "resume attempt=2", "{ev:?}");
    assert_no_limit_ended_the_step(&ev, &out);
    f.assert_isolated("0");
}
