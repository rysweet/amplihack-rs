//! Issue #1811: turning the capacity governor on and off, end to end
//! through the real `amplihack` binary. The governor ships off
//! (`governor.enabled` defaults to `false`), because the owner retired every
//! admission control and capacity hold; `AMPLIHACK_GOVERNOR` and the user
//! config key turn it on. See `issue_1811_fixture` for the isolation rules.

#![cfg(unix)]

use crate::issue_1811_fixture::{Fixture, describe, text};

const DENIED: i32 = 80;

#[test]
fn kill_switch_runs_as_before_and_creates_no_state() {
    for off in ["off", "0", "false"] {
        let f = Fixture::new();
        f.readings(&[("alpha", 10.0, 100.0), ("beta", 10.0, 100.0)]);
        let out = f
            .cmd(&["claude", "--", "-p", "hi"])
            .env("AMPLIHACK_GOVERNOR", off)
            .output()
            .unwrap();
        assert!(out.status.success(), "{off}: {}", describe(&out));
        assert_eq!(f.claude_runs(), 1);
        assert!(
            !f.gov().exists(),
            "{off}: no .governor.d with the governor off"
        );
        assert!(
            !text(&out.stderr).contains("governor"),
            "{off}: {}",
            describe(&out)
        );
    }
}

#[test]
fn governor_is_off_by_default() {
    // The owner retired every admission control and capacity hold, so the
    // governor ships opt-in: with no `governor.enabled` and no
    // `AMPLIHACK_GOVERNOR`, readings that would deny a start change nothing.
    let f = Fixture::new();
    f.without_governor_config();
    f.readings(&[("alpha", 10.0, 100.0), ("beta", 10.0, 100.0)]);
    let out = f.run(&["claude", "--", "-p", "hi"]);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.claude_runs(), 1);
    assert!(!f.gov().exists(), "no .governor.d when the governor is off");
    assert!(
        !text(&out.stderr).contains("governor"),
        "{}",
        describe(&out)
    );
    let st = f.run(&["governor", "status", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&st.stdout).unwrap();
    assert_eq!(v["enabled"], false, "{v}");
}

#[test]
fn variable_or_config_key_turns_the_governor_on() {
    for on in ["1", "on", "true"] {
        let f = Fixture::new();
        f.without_governor_config();
        f.readings(&[("alpha", 10.0, 100.0), ("beta", 10.0, 100.0)]);
        let out = f
            .cmd(&["claude", "--", "-p", "hi"])
            .env("AMPLIHACK_GOVERNOR", on)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(DENIED), "{on}: {}", describe(&out));
        assert_eq!(f.claude_runs(), 0, "{on}");
    }
    let f = Fixture::new();
    f.without_governor_config();
    f.governor_config("enabled = true");
    f.readings(&[("alpha", 10.0, 100.0), ("beta", 10.0, 100.0)]);
    let out = f.run(&["claude", "--", "-p", "hi"]);
    assert_eq!(out.status.code(), Some(DENIED), "{}", describe(&out));
    assert_eq!(f.claude_runs(), 0);
}

#[test]
fn config_key_turns_the_governor_off() {
    let f = Fixture::new();
    f.readings(&[("alpha", 10.0, 100.0)]);
    f.governor_config("enabled = false");
    let out = f.run(&["claude", "--", "-p", "hi"]);
    assert!(out.status.success(), "{}", describe(&out));
    assert!(!f.gov().exists());
    let on = f
        .cmd(&["claude", "--", "-p", "hi"])
        .env("AMPLIHACK_GOVERNOR", "on")
        .output()
        .unwrap();
    assert_eq!(
        on.status.code(),
        Some(DENIED),
        "the variable outranks the key: {}",
        describe(&on)
    );
}
