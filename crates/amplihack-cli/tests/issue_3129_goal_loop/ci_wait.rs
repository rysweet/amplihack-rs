//! Issue #1918. Run the real PR reading and wait with conditional REST
//! responses. The sleep stand-in advances the scenario without elapsed time.

#![cfg(target_os = "linux")]

use super::support::{Harness, PR, PR_BRANCH, describe};
use crate::support::{exec_stub::write_executable, fifo_release};
use serde_json::{Value, json};
use std::fs::{self, DirBuilder};
use std::os::unix::fs::DirBuilderExt;
use std::process::Command;

pub(super) fn goal(h: &Harness, args: &[&str]) -> Command {
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

pub(super) fn read_and_decide(h: &Harness, required: bool) -> Value {
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

pub(crate) fn fake_sleep(h: &Harness, action: &str) {
    let body =
        format!("#!/bin/bash\nset -eu\nprintf '%s\\n' \"$1\" >> \"$GH_STATE/sleeps\"\n{action}\n");
    write_executable(&h.bin.join("sleep"), body, 0o755);
}

pub(crate) fn sleeps(h: &Harness) -> Vec<u64> {
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
fn required_checks_wait_for_the_configured_apps_in_classic_protection_and_rules() {
    let h = Harness::new();
    h.classic_protection(&json!({"strict":false,"contexts":["build"],
        "checks":[{"context":"build","app_id":23}]}));
    fs::write(h.gh.join("rules.json"), "[]").unwrap();
    fs::write(
        h.gh.join("rules-page-2.json"),
        r#"[{"type":"required_status_checks","parameters":{"strict_required_status_checks_policy":false,"required_status_checks":[{"context":"integration","integration_id":42}]}}]"#,
    )
    .unwrap();
    h.set_checks("build\tpass\t0\t\t99\nintegration\tpass\t0\t\t99\n");
    fs::write(
        h.gh.join("statuses.json"),
        r#"{"statuses":[{"context":"build","state":"success"}]}"#,
    )
    .unwrap();
    let reading = read_and_decide(&h, true);
    assert_eq!(reading["checks"]["required_known"], true, "{reading}");
    assert_eq!(
        reading["checks"]["required_checks"],
        json!([{"name":"build","app_id":23}, {"name":"integration","app_id":42}])
    );
    assert_eq!(reading["checks"]["counts"]["pending"], 1);
    assert_eq!(reading["checks"]["counts"]["unverified"], 1);
    assert_eq!(reading["checks"]["counts"]["passed"], 0);
    fake_sleep(
        &h,
        r#"test "$(wc -l < "$GH_STATE/sleeps")" -eq 1
printf 'build\tpass\t0\t\t23\nbuild\tfail\t0\t\t99\nintegration\tpass\t0\t\t42\n' > "$GH_STATE/checks""#,
    );
    let result = wait(&h, "");
    assert_eq!(result["outcome"], "checks-complete", "{result}");
    assert_eq!(result["checks_state"], "passed", "{result}");
    assert_eq!(sleeps(&h), [120]);
}

#[test]
fn skipped_or_neutral_required_jobs_are_visible_as_unverified() {
    for conclusion in ["skipping", "neutral"] {
        let h = Harness::new();
        h.classic_protection(&json!({"strict":false,"contexts":["build"]}));
        h.set_checks(&format!("build\t{conclusion}\t0\noptional\tpass\t0\n"));
        let reading = read_and_decide(&h, true);
        assert_eq!(reading["checks"]["state"], "unverified", "{reading}");
        assert_eq!(reading["checks"]["counts"]["passed"], 0);
        assert_eq!(reading["checks"]["counts"]["skipped"], 1);
        let result = wait(&h, "");
        assert_eq!(result["outcome"], "checks-complete", "{result}");
        assert_eq!(result["checks_state"], "unverified", "{result}");
        assert_eq!(result["unverified_checks"][0]["name"], "build");
        assert_eq!(
            result["unverified_checks"][0]["unverified_reason"],
            "skipped_or_neutral"
        );

        // The all-checks wait must also account for the known required job.
        read_and_decide(&h, false);
        assert_eq!(wait(&h, "")["checks_state"], "unverified");
        assert!(sleeps(&h).is_empty());
    }
}

#[test]
fn a_readable_branch_policy_allows_required_waits_after_an_admin_404() {
    let h = Harness::new();
    fs::write(h.gh.join("classic404"), "").unwrap();
    let branch = h.gh.join("branch.json");
    fs::write(
        &branch,
        r#"{"protected":true,"protection":{"required_status_checks":{"contexts":["build"],"checks":[{"context":"build","app_id":23}]}}}"#,
    )
    .unwrap();
    h.set_checks("build\tpass\t0\t\t23\noptional\tfail\t0\n");
    let reading = read_and_decide(&h, true);
    assert_eq!(reading["checks"]["required_known"], true, "{reading}");
    assert_eq!(
        reading["checks"]["required_checks"],
        json!([{"name":"build","app_id":23}])
    );
    assert!(reading["repository"]["strict_status_checks"].is_null());
    let result = wait(&h, "");
    assert_eq!(result["checks_state"], "passed", "{result}");

    // GitHub's context-only branch response still gives the required name.
    fs::write(
        branch,
        r#"{"protected":true,"protection":{"required_status_checks":{"enforcement_level":"non_admins","contexts":["build"]}}}"#,
    )
    .unwrap();
    let reading = read_and_decide(&h, true);
    assert_eq!(reading["checks"]["required_known"], true, "{reading}");
    assert_eq!(
        reading["checks"]["required_checks"][0]["source_known"],
        false
    );
    let result = wait(&h, "");
    assert_eq!(result["outcome"], "checks-complete", "{result}");
    assert_eq!(result["checks_state"], "unverified", "{result}");
    assert_eq!(
        result["unverified_checks"][0]["unverified_reason"],
        "required_app_unreadable"
    );
    assert!(sleeps(&h).is_empty());
}

#[test]
fn a_completed_app_bound_commit_status_without_app_evidence_is_unverified() {
    let h = Harness::new();
    fs::write(
        h.gh.join("rules.json"),
        r#"[{"type":"required_status_checks","parameters":{"strict_required_status_checks_policy":false,"required_status_checks":[{"context":"external","integration_id":42}]}}]"#,
    )
    .unwrap();
    fs::write(
        h.gh.join("statuses.json"),
        r#"{"statuses":[{"context":"external","state":"success","target_url":"https://ci.example.com/result"}]}"#,
    )
    .unwrap();
    h.set_checks("external\tpass\t0\t\t99\n");
    let reading = read_and_decide(&h, true);
    assert_eq!(reading["checks"]["state"], "unverified", "{reading}");
    assert_eq!(reading["checks"]["counts"]["pending"], 0);
    assert_eq!(reading["checks"]["counts"]["passed"], 0);
    let result = wait(&h, "");
    assert_eq!(result["outcome"], "checks-complete", "{result}");
    assert_eq!(result["checks_state"], "unverified", "{result}");
    let job = &result["unverified_checks"][0];
    assert_eq!(job["name"], "external");
    assert_eq!(job["unverified_reason"], "status_app_unverified");
    assert_eq!(job["expected_app_ids"], json!([42]));
    assert_eq!(job["observed_state"], "pass");
    assert_eq!(job["link"], "https://ci.example.com/result");
    assert!(sleeps(&h).is_empty());
}

#[test]
fn rules_alone_cannot_establish_required_check_completion() {
    for classic_error in ["classic403", "classic404"] {
        let h = Harness::new();
        fs::write(h.gh.join(classic_error), "").unwrap();
        fs::write(
            h.gh.join("rules.json"),
            r#"[{"type":"required_status_checks","parameters":{"strict_required_status_checks_policy":false,"required_status_checks":[{"context":"build"}]}}]"#,
        )
        .unwrap();
        h.set_checks("build\tpass\t0\nclassic\tpending\t0\n");
        let reading = read_and_decide(&h, true);
        assert_eq!(reading["checks"]["required"], json!(["build"]));
        assert_eq!(reading["checks"]["required_known"], false, "{reading}");
        assert!(reading["checks"]["required_error"].is_string());
        assert_eq!(reading["checks"]["counts"]["pending"], 1);
        fake_sleep(
            &h,
            r#"test "$(wc -l < "$GH_STATE/sleeps")" -eq 1
printf 'build\tpass\t0\nclassic\tpass\t0\n' > "$GH_STATE/checks""#,
        );
        let result = wait(&h, "");
        assert_eq!(result["outcome"], "checks-complete", "{result}");
        assert_eq!(result["checks_state"], "unverified", "{result}");
        assert_eq!(
            result["required_error"],
            reading["checks"]["required_error"]
        );
        assert_eq!(result["counts"]["passed"], 2);
        assert_eq!(sleeps(&h), [120]);
    }
}

#[test]
fn incomplete_policy_keeps_the_skipped_required_job_and_optional_success_visible() {
    let h = Harness::new();
    fs::write(h.gh.join("classic403"), "").unwrap();
    fs::write(
        h.gh.join("rules.json"),
        r#"[{"type":"required_status_checks","parameters":{"strict_required_status_checks_policy":false,"required_status_checks":[{"context":"build"}]}}]"#,
    )
    .unwrap();
    h.set_checks("build\tskipping\t0\noptional\tpass\t0\n");

    for required in [false, true] {
        let reading = read_and_decide(&h, required);
        let checks = &reading["checks"];
        assert_eq!(checks["state"], "unverified", "{checks}");
        assert_eq!(checks["required"], json!(["build"]));
        assert_eq!(checks["required_known"], false);
        assert!(
            checks["required_error"]
                .as_str()
                .unwrap()
                .contains("classic protection")
        );
        assert_eq!(checks["counts"]["passed"], 1);
        assert_eq!(checks["counts"]["skipped"], 1);
        assert_eq!(checks["unverified_checks"][0]["name"], "build");
        assert_eq!(
            checks["unverified_checks"][0]["unverified_reason"],
            "skipped_or_neutral"
        );

        let result = wait(&h, "");
        assert_eq!(result["outcome"], "checks-complete", "{result}");
        assert_eq!(result["checks_state"], "unverified", "{result}");
        for field in ["counts", "required_error", "unverified_checks"] {
            assert_eq!(result[field], checks[field], "{field}");
        }
    }
    assert!(sleeps(&h).is_empty());
}

#[test]
fn the_reading_discards_a_pass_if_a_rerun_starts_during_the_status_retry() {
    let h = Harness::new();
    h.set_checks("build\tpass\t0\n");
    fs::write(h.gh.join("decline-core-status"), "1\n").unwrap();
    fake_sleep(&h, r#"printf 'build\tpending\t0\n' > "$GH_STATE/checks""#);

    let reading = read_and_decide(&h, false);
    assert_eq!(reading["pr"]["head"], h.origin_head(PR_BRANCH));
    assert_eq!(reading["checks"]["state"], "pending", "{reading}");
    assert_eq!(reading["checks"]["counts"]["passed"], 0);
    assert_eq!(reading["checks"]["checks"][0]["state"], "pending");
    let calls = h.calls();
    assert_eq!(
        calls.iter().filter(|c| c.contains("/check-runs?")).count(),
        2
    );
    assert_eq!(calls.iter().filter(|c| c.contains("/status?")).count(), 3);
    let sleeps = sleeps(&h);
    assert_eq!(sleeps.len(), 1);
    assert!((21..=25).contains(&sleeps[0]), "{sleeps:?}");
}

#[test]
fn completed_checks_are_discarded_if_the_head_moves_during_a_rate_limit_sleep() {
    let h = Harness::new();
    read_and_decide(&h, false);
    fs::write(h.gh.join("decline-core-check-runs"), "1\n").unwrap();
    fake_sleep(
        &h,
        r#"test "$(wc -l < "$GH_STATE/sleeps")" -eq 1
printf '%040d\n' 7 > "$GH_STATE/moved-head""#,
    );
    let result = wait(&h, "");
    assert_eq!(result["outcome"], "head-changed", "{result}");
    let sleeps = sleeps(&h);
    assert_eq!(sleeps.len(), 1);
    assert!((21..=25).contains(&sleeps[0]), "{sleeps:?}");
}

#[test]
fn a_rerun_at_the_same_head_discards_a_pass_during_the_final_rate_limit_sleep() {
    let h = Harness::new();
    let reading = read_and_decide(&h, false);
    assert_eq!(reading["checks"]["state"], "passed", "{reading}");
    fs::write(h.gh.join("decline-core-after-checks"), "1\n").unwrap();
    fake_sleep(
        &h,
        r#"case "$(wc -l < "$GH_STATE/sleeps")" in
  1) printf 'build\tpending\t0\n' > "$GH_STATE/checks" ;;
  2) printf 'build\tfail\t0\n' > "$GH_STATE/checks" ;;
  *) exit 2 ;;
esac"#,
    );
    let result = wait(&h, "37");
    assert_eq!(result["outcome"], "checks-complete", "{result}");
    assert_eq!(result["checks_state"], "failed", "{result}");
    let sleeps = sleeps(&h);
    assert_eq!(sleeps.len(), 2);
    assert!((21..=25).contains(&sleeps[0]), "{sleeps:?}");
    assert_eq!(sleeps[1], 37);
}

#[test]
fn a_secondary_decline_without_a_deadline_waits_for_guidance_without_retrying() {
    let h = Harness::new();
    read_and_decide(&h, false);
    fs::remove_file(h.gh.join("calls.log")).unwrap();
    fs::write(h.gh.join("no-retry-after"), "").unwrap();
    fs::write(h.gh.join("decline-core-check-runs"), "1\n").unwrap();
    write_executable(
        &h.bin.join("mkfifo"),
        "#!/bin/bash\nset -eu\n/usr/bin/mkfifo \"$@\"\nread -r _ < \"$GH_STATE/release.fifo\"\n",
        0o755,
    );
    fake_sleep(&h, "exit 2");
    let mut live = fifo_release::start(
        goal(&h, &["act", "wait"]),
        &h.gh.join("release.fifo"),
        "the wait for guidance after a declined CI read",
    );
    h.guide("Review the GitHub limit before continuing.");
    let out = live.finish();
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(h.result("001")["outcome"], "guidance-arrived");
    let calls = h.calls();
    assert_eq!(
        calls.iter().filter(|c| c.contains("/check-runs?")).count(),
        1
    );
    assert_eq!(calls.iter().filter(|c| *c == "api rate_limit").count(), 1);
    assert!(sleeps(&h).is_empty());
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
fn graphql_secondary_decline_uses_the_response_deadline_with_quota_remaining() {
    let h = Harness::new();
    fs::write(h.gh.join("decline-graphql"), "1\n").unwrap();
    fs::write(h.gh.join("graphql-secondary"), "").unwrap();
    // If the deadline is missed, let the guidance fallback fail promptly.
    write_executable(&h.bin.join("mkfifo"), "#!/bin/bash\nexit 1\n", 0o755);
    fake_sleep(&h, "exit 0");

    let reading = read_and_decide(&h, false);
    assert_eq!(reading["pr"]["head"], h.origin_head(PR_BRANCH), "{reading}");
    assert_eq!(reading["checks"]["state"], "passed");
    let sleeps = sleeps(&h);
    assert_eq!(sleeps.len(), 1);
    assert!((13..=17).contains(&sleeps[0]), "{sleeps:?}");
    let calls = h.calls();
    assert_eq!(calls.iter().filter(|c| c.starts_with("pr view")).count(), 2);
    assert_eq!(calls.iter().filter(|c| *c == "api rate_limit").count(), 1);
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
    let head =
        |call: &str| call.starts_with("api --include ") && call.ends_with("repos/o/r/pulls/7");
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
