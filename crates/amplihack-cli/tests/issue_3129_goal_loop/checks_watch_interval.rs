//! The real PR goal's check wait with the stand-in gh and guidance command.

#![cfg(target_os = "linux")]

use super::support::{Harness, PR, describe};
use crate::support::fifo_release;
use serde_json::json;
use std::fs;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt};
use std::process::Output;

fn run_wait(value: Option<&str>, guidance: bool) -> (Harness, Output) {
    let h = Harness::new();
    let turn = h.state.join("turns/0001");
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&turn)
        .unwrap();
    fs::write(
        turn.join("decision.json"),
        json!({"action":"wait","args":{"required":true}}).to_string(),
    )
    .unwrap();
    fs::write(
        turn.join("reading.json"),
        json!({"goal":{"checks":{"required_known":true}}}).to_string(),
    )
    .unwrap();
    let mut command = h.tool_command("autodrive_pr_goal.sh");
    command
        .args(["act", "wait", turn.to_str().unwrap()])
        .env("AUTODRIVE_GOAL_CONTRACT", "1")
        .env("AUTODRIVE_STATE_DIR", &h.state)
        .env("AMPLIHACK_BIN", h.bin.join("amplihack"))
        .env("RECIPE_VAR_pr_number", PR)
        .env_remove("AMPLIHACK_RUN_GUIDANCE_DIRS")
        .env_remove("RECIPE_VAR_checks_watch_interval");
    if guidance {
        command.env("AMPLIHACK_RUN_GUIDANCE_DIRS", h.state.join("guidance"));
    }
    if let Some(value) = value {
        command.env("RECIPE_VAR_checks_watch_interval", value);
    }

    let mut live = fifo_release::start(command, &h.gh.join("release.fifo"), "the check wait");
    let wake = h.state.join("guidance-wake");
    let armed = wake.read_dir().is_ok_and(|mut entries| {
        entries.any(|entry| entry.unwrap().file_type().unwrap().is_fifo())
    });
    assert_eq!(armed, guidance);
    let out = live.finish();
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(h.result("0001")["outcome"], "checks-complete");
    (h, out)
}

#[test]
fn checks_watch_interval_unset_uses_the_gh_default() {
    let (h, _) = run_wait(None, true);
    assert_eq!(h.watch_calls(), ["pr checks 7 --watch --required"]);
}

#[test]
fn checks_watch_interval_is_passed_when_guidance_is_armed() {
    let (h, _) = run_wait(Some("120"), true);
    assert_eq!(
        h.watch_calls(),
        ["pr checks 7 --watch --required --interval 120"]
    );
}

#[test]
fn checks_watch_interval_is_passed_when_the_watch_runs_alone() {
    let (h, _) = run_wait(Some("120"), false);
    assert_eq!(
        h.watch_calls(),
        ["pr checks 7 --watch --required --interval 120"]
    );
}

#[test]
fn checks_watch_interval_accepts_the_largest_whole_second_duration() {
    let (h, _) = run_wait(Some("9223372036"), true);
    assert_eq!(
        h.watch_calls(),
        ["pr checks 7 --watch --required --interval 9223372036"]
    );
}

#[test]
fn checks_watch_interval_invalid_values_use_the_gh_default() {
    for value in ["abc", "0", "-5", "9223372037", "9999999999"] {
        let (h, out) = run_wait(Some(value), true);
        assert_eq!(
            h.watch_calls(),
            ["pr checks 7 --watch --required"],
            "{value}"
        );
        assert!(
            String::from_utf8_lossy(&out.stderr).contains(&format!(
                "WARNING: invalid checks_watch_interval ({value}). Using the gh default."
            )),
            "{value}: {}",
            describe(&out)
        );
    }
}
