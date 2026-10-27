//! Issue #1618: a step moved off its inherited profile before the first
//! attempt only because that profile is over the auto-switch threshold (no
//! window at 100%) retries under it at once when the candidate is limited.
//! That retry is an auto-switch rerun, not a wait: the step's single wait
//! stays free for the inherited profile's own limit. The candidate's limit
//! belongs to another account: it neither times the wait nor, when too far
//! away, cancels the retry.
//!
//! See `issue_1618_fixture`.

#![cfg(unix)]

use crate::issue_1618_fixture::{Fixture, WEEKLY_TEXT, describe, events, private_file};
use crate::issue_1618_wait_profiles::{switch_on, with_profiles};
use std::os::unix::fs::PermissionsExt;

/// Seeds the auto-switch readings: X at 95% of its five-hour window (over
/// the default 90% threshold, not used up), Y at 10%.
fn x_over_threshold(f: &Fixture) {
    let state = f.profiles.join(".auto-switch.d");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700)).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let reading = |pct: f64| {
        serde_json::json!({
            "five_hour": {"used_percentage": pct, "resets_at": now + 3600},
            "seven_day": {"used_percentage": 5.0, "resets_at": now + 86400},
            "observed_at": now,
            "source": "statusline",
        })
    };
    let body = serde_json::json!({
        "version": 1,
        "accounts": {"X": reading(95.0), "Y": reading(10.0)},
    });
    private_file(&state.join("usage.json"), &body.to_string());
}

/// The step inherits X and is moved to Y. The first `limit_calls` calls are
/// limited, Y with `y_limit` and X with `x_limit`. Returns the run's wait
/// events after asserting the attempts ran under Y, then X, and succeeded.
fn run_from_y(limit_calls: u32, y_limit: &str, x_limit: &str) -> Vec<String> {
    let f = with_profiles();
    switch_on(&f);
    x_over_threshold(&f);
    let mut c = f.step();
    c.env("AMPLIHACK_CLAUDE_PROFILE", "X")
        .env("CLAUDE_CONFIG_DIR", f.profiles.join("X"))
        .env("AMPLIHACK_CLAUDE_AUTO_SWITCH", "1")
        .env("LIMIT_PROFILE", "")
        .env("LIMIT_CALLS", limit_calls.to_string())
        .env("LIMIT_MESSAGE_Y", y_limit)
        .env("LIMIT_MESSAGE_X", x_limit);
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    let attempts = limit_calls + 1;
    assert_eq!(f.count("0"), attempts, "{}", describe(&out));
    assert_eq!(f.call("0", 1)["PROFILE"], "Y", "{}", describe(&out));
    for n in 2..=attempts {
        assert_eq!(
            f.call("0", n)["PROFILE"],
            "X",
            "attempt {n} uses the inherited profile"
        );
    }
    f.assert_isolated("0");
    events(&out)
}

/// Y resets at noon, four hours after `TEST_NOW`.
const Y_NOON: &str = "You've hit your session limit · resets 12pm (America/Los_Angeles)";
const X_0830: &str = "You've hit your session limit · resets 8:30am (America/Los_Angeles)";

#[test]
fn a_profile_only_over_the_threshold_is_retried_at_once_not_after_the_candidates_reset() {
    assert_eq!(
        run_from_y(1, Y_NOON, ""),
        Vec::<String>::new(),
        "the retry under X is an auto-switch rerun: no wait starts or ends"
    );
}

#[test]
fn a_candidates_weekly_limit_does_not_stop_the_retry_under_a_profile_with_capacity() {
    assert_eq!(run_from_y(1, WEEKLY_TEXT, ""), Vec::<String>::new());
}

/// X has only 5% of its window left, so the retry under it is the attempt
/// likeliest to be limited. The rerun back to X did not use the step's
/// wait, so X's limit is waited out (for X's 8:30 reset) and the step
/// succeeds, instead of ending with `skipped reason=already_waited`.
#[test]
fn a_limit_on_the_retry_under_the_inherited_profile_still_gets_the_wait() {
    assert_eq!(
        run_from_y(2, Y_NOON, X_0830),
        vec![
            "start reason=usage_limit harness=claude reset=2026-10-18 08:30:00 -07:00 \
             until=2026-10-18 08:32:00 -07:00 bound=reset+margin"
                .to_string(),
            "end reason=reset reached".to_string(),
            "resume attempt=3".to_string(),
        ]
    );
}
