//! The CI wait uses the interval from #3378 with and without a guidance wake.

#![cfg(target_os = "linux")]

use super::ci_wait::{fake_sleep, goal, read_and_decide, sleeps};
use super::support::{Harness, describe};
use serde_json::Value;
use std::fs;
use std::os::unix::fs::DirBuilderExt;

fn run_wait(value: Option<&str>, guidance: bool) -> (Vec<u64>, usize) {
    let h = Harness::new();
    h.set_checks("build\tpending\t0\n");
    read_and_decide(&h, true);
    let turn = h.state.join("turns/001");
    let path = turn.join("reading.json");
    let mut reading: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    reading["goal"]["guidance_target"] = Value::Null;
    fs::write(path, reading.to_string()).unwrap();
    fake_sleep(
        &h,
        r#"test "$(wc -l < "$GH_STATE/sleeps")" -eq 1
printf 'build\tpass\t0\n' > "$GH_STATE/checks""#,
    );
    let mut command = goal(&h, &["act", "wait"]);
    if guidance {
        let directory = h.state.join("guidance");
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .unwrap();
        command.env("AMPLIHACK_RUN_GUIDANCE_DIRS", directory);
    }
    if let Some(value) = value {
        command.env("RECIPE_VAR_checks_watch_interval", value);
    }
    let out = command.output().unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(h.result("001")["checks_state"], "passed");
    assert_eq!(h.state.join("guidance-wake").is_dir(), guidance);
    let diagnostics = fs::read_to_string(turn.join("wait.err")).unwrap();
    (sleeps(&h), diagnostics.lines().count())
}

#[test]
fn an_empty_interval_uses_the_default_with_no_diagnostic() {
    let baseline = run_wait(Some("120"), true);
    assert_eq!(baseline.0, [120]);
    for value in [None, Some("")] {
        assert_eq!(run_wait(value, true), baseline);
    }
}

#[test]
fn the_configured_interval_is_used_when_guidance_is_available() {
    for value in [37, 9_223_372_036_u64] {
        assert_eq!(run_wait(Some(&value.to_string()), true).0, [value]);
    }
}

#[test]
fn the_configured_interval_is_used_when_the_guidance_wake_is_unavailable() {
    assert_eq!(run_wait(Some("37"), false).0, [37]);
}

#[test]
fn an_invalid_interval_uses_the_default_and_reports_it_once() {
    // Count diagnostics relative to the same command with a valid setting.
    let (_, baseline) = run_wait(Some("120"), true);
    for value in ["abc", "0", "-5", "037", "9223372037", "9999999999"] {
        assert_eq!(
            run_wait(Some(value), true),
            (vec![120], baseline + 1),
            "{value}"
        );
    }
}
