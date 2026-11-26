//! Issue #1918. Run the real PR reading and wait with conditional REST
//! responses. The sleep stand-in advances the scenario without elapsed time.

#![cfg(target_os = "linux")]

use super::support::{Harness, PR, describe};
use crate::support::{exec_stub::write_executable, fifo_release};
use serde_json::{Value, json};
use std::fs::{self, DirBuilder};
use std::os::unix::fs::DirBuilderExt;
use std::process::Command;

fn goal(h: &Harness, args: &[&str]) -> Command {
    let mut cmd = Command::new("bash");
    cmd.arg(
        h.bundle_root
            .join("amplifier-bundle/tools/autodrive_pr_goal.sh"),
    )
    .args(args)
    .arg(h.state.join("turns/001"))
    .current_dir(&h.repo)
    .env("PATH", format!("{}:/usr/bin:/bin", h.bin.display()))
    .env("HOME", &h.home)
    .env("TMPDIR", &h.base)
    .env("GH_STATE", &h.gh)
    .env("GH_ORIGIN", &h.origin)
    .env("GH_PR_BRANCH", super::support::PR_BRANCH)
    .env("AMPLIHACK_BIN", h.bin.join("amplihack"))
    .env("AMPLIHACK_STATE_DIR", &h.state_root)
    .env("AMPLIHACK_WORK_ROOT", &h.work)
    .env("AUTODRIVE_GOAL_CONTRACT", "1")
    .env("AUTODRIVE_STATE_DIR", &h.state)
    .env("RECIPE_VAR_pr_number", PR)
    .env_remove("RECIPE_VAR_checks_watch_interval")
    .env_remove("AMPLIHACK_RUN_GUIDANCE_DIRS");
    cmd
}

fn read_and_decide(h: &Harness, required: bool) -> Value {
    let turn = h.state.join("turns/001");
    DirBuilder::new()
        .mode(0o700)
        .recursive(true)
        .create(&turn)
        .unwrap();
    let out = goal(h, &["read"]).output().unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    let reading: Value =
        serde_json::from_slice(&fs::read(turn.join("goal.json")).unwrap()).unwrap();
    fs::write(
        turn.join("reading.json"),
        json!({"goal":reading}).to_string(),
    )
    .unwrap();
    fs::write(
        turn.join("decision.json"),
        json!({"action":"wait","args":{"required":required}}).to_string(),
    )
    .unwrap();
    reading
}

fn fake_sleep(h: &Harness, action: &str) {
    let body =
        format!("#!/bin/bash\nset -eu\nprintf '%s\\n' \"$1\" >> \"$GH_STATE/sleeps\"\n{action}\n");
    write_executable(&h.bin.join("sleep"), body, 0o755);
}

fn sleeps(h: &Harness) -> Vec<u64> {
    fs::read_to_string(h.gh.join("sleeps"))
        .unwrap_or_default()
        .lines()
        .map(|line| line.parse().unwrap())
        .collect()
}

fn wait(h: &Harness, interval: &str) -> Value {
    let out = goal(h, &["act", "wait"])
        .env("RECIPE_VAR_checks_watch_interval", interval)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    h.result("001")
}

#[test]
fn unchanged_empty_checks_remain_pending_for_ten_simulated_minutes_until_guidance() {
    let h = Harness::new();
    h.set_checks("");
    let reading = read_and_decide(&h, false);
    assert_eq!(reading["checks"]["state"], "pending");
    assert_eq!(reading["checks"]["counts"]["total"], 0);
    assert_eq!(reading["checks"]["counts"]["failing"], 0);
    fs::remove_file(h.gh.join("responses.log")).unwrap();
    fs::remove_file(h.gh.join("calls.log")).unwrap();
    fake_sleep(
        &h,
        r#"case "$(wc -l < "$GH_STATE/sleeps")" in
  6) read -r _ < "$GH_STATE/release.fifo"; exit 2 ;;
  [1-5]) ;;
  *) exit 2 ;;
esac"#,
    );
    let mut live = fifo_release::start(
        goal(&h, &["act", "wait"]),
        &h.gh.join("release.fifo"),
        "the ten-minute CI wait",
    );
    assert_eq!(sleeps(&h), [120; 6]);
    let responses = fs::read_to_string(h.gh.join("responses.log")).unwrap();
    assert_eq!(responses.lines().count(), 18, "{responses}");
    // The reading already populated both check caches, so only the first
    // PR head response is charged during the wait.
    assert_eq!(
        responses.lines().filter(|l| l.starts_with("200 ")).count(),
        1
    );
    assert_eq!(
        responses.lines().filter(|l| l.starts_with("304 ")).count(),
        17
    );
    let calls = h.calls();
    assert_eq!(calls.len(), 18, "{calls:?}");
    assert_eq!(
        calls
            .iter()
            .filter(|c| c.contains("If-None-Match:"))
            .count(),
        17
    );
    h.guide("Read the updated instructions.");
    let out = live.finish();
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(h.result("001")["outcome"], "guidance-arrived");
}

#[test]
fn the_wait_polled_at_the_configured_interval_ends_when_the_head_moves() {
    let h = Harness::new();
    h.set_checks("build\tpending\t0\n");
    read_and_decide(&h, false);
    fake_sleep(
        &h,
        r#"test "$(wc -l < "$GH_STATE/sleeps")" -eq 1
printf '%040d\n' 7 > "$GH_STATE/moved-head""#,
    );
    let result = wait(&h, "37");
    assert_eq!(result["outcome"], "head-changed", "{result}");
    assert_eq!(sleeps(&h), [37]);
    assert_eq!(
        h.calls()
            .iter()
            .filter(|c| c.contains("/check-runs?"))
            .count(),
        2,
        "only the reading and the first round read checks"
    );
}

#[test]
fn a_required_check_on_the_next_page_can_complete_the_wait_with_failure() {
    let h = Harness::new();
    h.classic_protection(&json!({"strict":false,"contexts":["integration"]}));
    let second_page = h.gh.join("checks-page-2.json");
    fs::write(&second_page, r#"{"check_runs":[]}"#).unwrap();
    let reading = read_and_decide(&h, true);
    assert_eq!(reading["checks"]["required_known"], true, "{reading}");
    assert_eq!(reading["checks"]["counts"]["pending"], 1);
    fake_sleep(
        &h,
        r#"test "$(wc -l < "$GH_STATE/sleeps")" -eq 1
printf '%s\n' '{"check_runs":[{"name":"integration","status":"completed","conclusion":"failure"}]}' > "$GH_STATE/checks-page-2.json""#,
    );
    let result = wait(&h, "037");
    assert_eq!(result["outcome"], "checks-complete", "{result}");
    assert_eq!(result["checks_state"], "failed");
    assert_eq!(sleeps(&h), [120]);
    assert_eq!(
        h.calls()
            .iter()
            .filter(|c| c.contains("If-None-Match:") && c.contains("check-runs?per_page=100&page=2"))
            .count(),
        2
    );
}

#[test]
fn legacy_commit_statuses_can_complete_the_wait_with_success() {
    let h = Harness::new();
    h.set_checks("");
    fs::write(
        h.gh.join("statuses.json"),
        r#"{"statuses":[{"context":"external","state":"success"}]}"#,
    )
    .unwrap();
    let reading = read_and_decide(&h, false);
    assert_eq!(reading["checks"]["counts"]["passed"], 1);
    let result = wait(&h, "");
    assert_eq!(result["outcome"], "checks-complete", "{result}");
    assert_eq!(result["checks_state"], "passed");
    assert!(sleeps(&h).is_empty());
}

#[test]
fn rate_limit_deadlines_resume_the_reading_and_wait_without_extra_agent_turns() {
    let h = Harness::new();
    fs::write(h.gh.join("decline-graphql"), "1\n").unwrap();
    fs::write(h.gh.join("decline-core"), "2\n").unwrap();
    fake_sleep(&h, "exit 0");
    h.policy(
        r#"if [ "${TURN##*/}" = 001 ]; then
  echo '{"action":"wait","args":{},"why":"Read the checks."}'
else
  echo '{"action":"handoff","args":{"owner":"operator","decision":"next work","text":"Checks are complete."},"why":"The wait finished."}'
fi"#,
    );
    let out = h.drive("auto-drive-to-merge", &["no_merge=true"]);
    assert_eq!(
        Harness::outcome(&out)["stop_reason"],
        "handoff",
        "{}",
        describe(&out)
    );
    assert_eq!(h.turns(), ["001", "002"]);
    assert_eq!(
        fs::read_to_string(h.base.join("agent-turns.log"))
            .unwrap()
            .lines()
            .count(),
        2
    );
    assert_eq!(h.reading("001")["goal"]["checks"]["state"], "passed");
    assert_eq!(h.result("001")["checks_state"], "passed");
    let calls = h.calls();
    let reading =
        |call: &str| call.starts_with("pr view") && call.contains("mergeStateStatus,isDraft");
    let head = |call: &str| call == "api --include repos/o/r/pulls/7";
    let keys: Vec<&str> = calls
        .iter()
        .filter_map(|call| {
            if reading(call) {
                Some("reading")
            } else if head(call) {
                Some("head")
            } else if call == "api rate_limit" {
                Some("rate_limit")
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        keys,
        [
            "reading",
            "rate_limit",
            "reading",
            "head",
            "rate_limit",
            "head",
            "rate_limit",
            "head",
            "reading"
        ]
    );
    let sleeps = sleeps(&h);
    assert_eq!(sleeps.len(), 3);
    assert!((26..=30).contains(&sleeps[0]), "{sleeps:?}");
    assert!(
        sleeps[1..].iter().all(|s| (21..=25).contains(s)),
        "{sleeps:?}"
    );
}
