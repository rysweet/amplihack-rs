//! Issue #1618: the usage-limit wait for a step that inherited a profile.
//!
//! - AC9: with auto-switch on and an eligible profile left, the #1569 rerun
//!   still runs at once, with no wait.
//! - A2: with auto-switch off, the step waits and retries under the same
//!   profile, and its scratch files go in `$HOME/.amplihack/rerun/`, not
//!   under the profile root.
//! - With auto-switch out of profiles, the wait is for the inherited
//!   profile's own reset, whether the candidate resets before or after it,
//!   because the retry runs under the inherited profile. A candidate's
//!   weekly or spend limit does not cancel that wait.
//! - D7: with the wait disabled, auto-switch's last failure is passed up
//!   with one `skipped reason=disabled` line.
//! - With no inherited profile but a `.default` pointer, the wait watches
//!   the pointed-to profile's login, the one the launch uses.
//! - A prompt piped on stdin is neither rerun by auto-switch nor waited for:
//!   the rerun would get no prompt.
//!
//! See `issue_1618_fixture`.

#![cfg(unix)]

use crate::issue_1618_fixture::{
    Fixture, SPEND_TEXT, WEEKLY_TEXT, describe, events, private_file, text, wait_lines,
};
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

const RESET_0830: &str = "You've hit your session limit · resets 8:30am (America/Los_Angeles)";
const RESET_0930: &str = "You've hit your session limit · resets 9:30am (America/Los_Angeles)";

/// X and Y both limited, X with `x_limit` and Y with `y_limit`: the step
/// waits once, for X's reset, then retries under X.
fn wait_after_both_profiles(x_limit: &str, y_limit: &str) -> Vec<String> {
    let f = with_profiles();
    switch_on(&f);
    let mut c = nested_x(&f);
    // The first two calls (X, then Y) are limited; the third succeeds.
    c.env(TOGGLE_ENV, "1")
        .env("LIMIT_PROFILE", "")
        .env("LIMIT_CALLS", "2")
        .env("LIMIT_MESSAGE_X", x_limit)
        .env("LIMIT_MESSAGE_Y", y_limit);
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
    events(&out)
}

fn waited_for(reset: &str, until: &str) -> Vec<String> {
    vec![
        format!(
            "start reason=usage_limit harness=claude reset=2026-10-18 {reset} -07:00 \
             until=2026-10-18 {until} -07:00 bound=reset+margin"
        ),
        "end reason=reset reached".to_string(),
        "resume attempt=3".to_string(),
    ]
}

/// The retry runs under X, so the wait is for X's reset in both orders:
/// not cut short when Y resets earlier, not stretched when Y resets later.
#[test]
fn after_the_last_auto_switch_candidate_the_step_waits_for_its_own_profiles_reset() {
    assert_eq!(
        wait_after_both_profiles(RESET_0930, RESET_0830),
        waited_for("09:30:00", "09:32:00"),
        "X resets after Y: the wait is for X's reset, not Y's"
    );
    assert_eq!(
        wait_after_both_profiles(RESET_0830, RESET_0930),
        waited_for("08:30:00", "08:32:00"),
        "Y resets after X: X has capacity from 8:30, so the wait ends then"
    );
}

/// A candidate's limit that no wait lifts (a weekly reset days away, an
/// organisation spend limit) says nothing about X: X's 8:30 reset is still
/// waited out and the retry under X runs.
#[test]
fn a_candidates_weekly_or_spend_limit_does_not_cancel_the_inherited_profiles_wait() {
    for y_limit in [WEEKLY_TEXT, SPEND_TEXT] {
        assert_eq!(
            wait_after_both_profiles(RESET_0830, y_limit),
            waited_for("08:30:00", "08:32:00"),
            "Y limited with {y_limit:?}"
        );
    }
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
    assert!(text(&out.stderr).contains("session limit"));
}

#[test]
fn with_a_default_pointer_the_wait_watches_that_profiles_login() {
    let f = with_profiles();
    let out = f.run(&mut f.cmd(&["profile", "switch", "X"]));
    assert!(out.status.success(), "profile switch X: {}", describe(&out));
    // X's login arrives during the wait (the stub creates the file in the
    // directory it runs with); nothing changes in CLAUDE_CONFIG_DIR.
    std::fs::remove_file(f.profiles.join("X").join(".credentials.json")).unwrap();
    let mut c = f.step();
    c.env("AMPLIHACK_USAGE_LIMIT_WAIT_TEST_SCALE", "100")
        .env("CHANGE_LOGIN_AFTER", "1");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    let x = f.profiles.join("X").to_string_lossy().to_string();
    assert_eq!(
        f.call("0", 1)["CLAUDE_CONFIG_DIR"],
        x,
        "the launch applied X"
    );
    let ev = events(&out);
    assert_eq!(
        ev[1..],
        ["end reason=login changed", "resume attempt=2"],
        "{ev:?}"
    );
}

#[test]
fn a_piped_prompt_is_not_rerun_by_auto_switch_either() {
    let f = with_profiles();
    switch_on(&f);
    // `-p` with no prompt text: the prompt is on the pipe.
    let mut c = f.cmd(&["claude", "--subprocess-safe", "--", "-p"]);
    c.env("AMPLIHACK_CLAUDE_PROFILE", "X")
        .env("CLAUDE_CONFIG_DIR", f.profiles.join("X"))
        .env(TOGGLE_ENV, "1")
        .env("LIMIT_CALLS", "5")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = c.spawn().expect("start amplihack");
    {
        use std::io::Write;
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(b"the piped prompt\n").unwrap();
    }
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(f.count("0"), 1, "no rerun under Y with an empty prompt");
    let stderr = text(&out.stderr);
    assert!(
        stderr.contains("auto-switch: no rerun under profile \"Y\""),
        "{stderr}"
    );
    assert_eq!(wait_lines(&out), vec!["skipped reason=piped_prompt"]);
}
