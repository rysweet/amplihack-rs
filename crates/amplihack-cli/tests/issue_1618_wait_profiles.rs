//! Issue #1618: the usage-limit wait for a step that inherited a profile.
//!
//! - AC9: with auto-switch on and an eligible profile left, the #1569 rerun
//!   still runs at once, with no wait.
//! - A2: with auto-switch off, the step waits and retries under the same
//!   profile, and its scratch files go in `$HOME/.amplihack/rerun/`, not
//!   under the profile root.
//! - D7: with the wait disabled, auto-switch's last failure is passed up
//!   with one `skipped reason=disabled` line.
//!
//! See `issue_1618_fixture`.

#![cfg(unix)]

use crate::issue_1618_fixture::{Fixture, describe, events, private_file, text, wait_lines};
use std::process::Command;

const TOGGLE_ENV: &str = "AMPLIHACK_CLAUDE_AUTO_SWITCH";

/// The fixture with profiles `X` and `Y`, each logged in with a distinct
/// account.
fn with_profiles() -> Fixture {
    let f = Fixture::new();
    for name in ["X", "Y"] {
        let out = f.run(&mut f.cmd(&["profile", "add", name]));
        assert!(
            out.status.success(),
            "profile add {name}: {}",
            describe(&out)
        );
        let dir = f.profiles.join(name);
        private_file(
            &dir.join(".credentials.json"),
            &format!(r#"{{"claudeAiOauth":{{"accessToken":"SENTINEL-{name}"}}}}"#),
        );
        private_file(
            &dir.join(".claude.json"),
            &format!(
                r#"{{"oauthAccount":{{"emailAddress":"{}@example.invalid","accountUuid":"uuid-{name}"}}}}"#,
                name.to_lowercase()
            ),
        );
    }
    f
}

/// Turns auto-switch on through its command, as the #1569 tests do.
fn switch_on(f: &Fixture) {
    let out = f.run(&mut f.cmd(&["profile", "auto-switch", "on"]));
    assert!(out.status.success(), "auto-switch on: {}", describe(&out));
}

/// A step that inherited profile `X`; only `X` is limited.
fn nested_x(f: &Fixture) -> Command {
    let mut c = f.step();
    c.env("AMPLIHACK_CLAUDE_PROFILE", "X")
        .env("CLAUDE_CONFIG_DIR", f.profiles.join("X"))
        .env("LIMIT_PROFILE", "X")
        .env("LIMIT_CALLS", "5");
    c
}

#[test]
fn ac9_an_eligible_profile_reruns_at_once_with_no_wait() {
    let f = with_profiles();
    switch_on(&f);
    let mut c = nested_x(&f);
    c.env(TOGGLE_ENV, "1");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2);
    assert_eq!(f.call("0", 1)["PROFILE"], "X");
    assert_eq!(f.call("0", 2)["PROFILE"], "Y");
    assert!(
        !wait_lines(&out).iter().any(|l| l.starts_with("start ")),
        "no wait while a profile is left: {:?}",
        wait_lines(&out)
    );
    f.assert_isolated("0");
}

#[test]
fn a2_with_auto_switch_off_the_step_waits_and_retries_under_its_own_profile() {
    let f = with_profiles();
    let mut c = nested_x(&f);
    c.env(TOGGLE_ENV, "0").env("LIMIT_CALLS", "1");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2);
    for n in [1, 2] {
        let call = f.call("0", n);
        assert_eq!(call["PROFILE"], "X", "call {n}");
        assert_eq!(
            call["CLAUDE_CONFIG_DIR"],
            f.profiles.join("X").display().to_string()
        );
    }
    assert_eq!(
        events(&out),
        vec![
            "start reason=usage_limit harness=claude reset=2026-10-18 08:30:00 -07:00 \
             until=2026-10-18 08:32:00 -07:00 bound=reset+margin",
            "end reason=reset reached",
            "resume attempt=2",
        ]
    );
    assert!(
        !f.profiles.join(".auto-switch.d").exists(),
        "auto-switch off creates nothing under the profile root"
    );
    assert!(
        f.rerun_dir().is_dir(),
        "scratch goes in $HOME/.amplihack/rerun"
    );
    f.assert_isolated("0");
}

#[test]
fn after_the_last_auto_switch_candidate_the_step_waits_then_retries_its_own_profile() {
    let f = with_profiles();
    switch_on(&f);
    let mut c = nested_x(&f);
    // The first two calls (X, then Y) are limited; the third succeeds.
    c.env(TOGGLE_ENV, "1")
        .env("LIMIT_PROFILE", "")
        .env("LIMIT_CALLS", "2");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 3, "X, then Y, then the retry after the wait");
    assert_eq!(f.call("0", 1)["PROFILE"], "X");
    assert_eq!(f.call("0", 2)["PROFILE"], "Y");
    assert_eq!(
        f.call("0", 3)["PROFILE"],
        "X",
        "the retry uses the inherited profile, not the last candidate"
    );
    let ev = events(&out);
    assert_eq!(
        ev.last().map(String::as_str),
        Some("resume attempt=3"),
        "{ev:?}"
    );
}

#[test]
fn d7_disabled_with_a_profile_writes_one_skipped_line_and_passes_the_failure_up() {
    let f = with_profiles();
    switch_on(&f);
    let mut c = nested_x(&f);
    c.env(TOGGLE_ENV, "1")
        .env("LIMIT_PROFILE", "")
        .env("AMPLIHACK_USAGE_LIMIT_WAIT", "0");
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(f.count("0"), 2, "X then Y, no wait");
    assert_eq!(wait_lines(&out), vec!["skipped reason=disabled"]);
    assert!(text(&out.stderr).contains("usage limit"));
}
