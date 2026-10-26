//! Issue #1618: a step moved off its inherited profile before the first
//! attempt only because that profile is over the auto-switch threshold (no
//! window at 100%) retries under it at once when the candidate is limited.
//! The candidate's limit belongs to another account: it neither times the
//! wait nor, when too far away, cancels the retry.
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

/// The step inherits X, is moved to Y, and Y is limited with `y_limit`;
/// returns the wait events after asserting the retry ran under X.
fn y_limited_with(y_limit: &str) -> Vec<String> {
    let f = with_profiles();
    switch_on(&f);
    x_over_threshold(&f);
    let mut c = f.step();
    c.env("AMPLIHACK_CLAUDE_PROFILE", "X")
        .env("CLAUDE_CONFIG_DIR", f.profiles.join("X"))
        .env("AMPLIHACK_CLAUDE_AUTO_SWITCH", "1")
        .env("LIMIT_PROFILE", "Y")
        .env("LIMIT_CALLS", "5")
        .env("LIMIT_MESSAGE_Y", y_limit);
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2, "Y, then the retry under X");
    assert_eq!(f.call("0", 1)["PROFILE"], "Y", "{}", describe(&out));
    assert_eq!(
        f.call("0", 2)["PROFILE"],
        "X",
        "the retry uses the inherited profile"
    );
    f.assert_isolated("0");
    events(&out)
}

/// The wait starts and ends at `TEST_NOW`: no time is spent on Y's reset.
fn retried_at_once() -> Vec<String> {
    vec![
        "start reason=usage_limit harness=claude reset=2026-10-18 08:00:00 -07:00 \
         until=2026-10-18 08:00:00 -07:00 bound=reset+margin"
            .to_string(),
        "end reason=reset reached".to_string(),
        "resume attempt=2".to_string(),
    ]
}

#[test]
fn a_profile_only_over_the_threshold_is_retried_at_once_not_after_the_candidates_reset() {
    // Y resets at noon, four hours after TEST_NOW.
    let y = "You've hit your session limit · resets 12pm (America/Los_Angeles)";
    assert_eq!(y_limited_with(y), retried_at_once());
}

#[test]
fn a_candidates_weekly_limit_does_not_stop_the_retry_under_a_profile_with_capacity() {
    assert_eq!(y_limited_with(WEEKLY_TEXT), retried_at_once());
}
