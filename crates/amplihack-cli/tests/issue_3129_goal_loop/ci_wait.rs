//! Issue #1918. Run the real PR reading and wait with conditional REST
//! responses. The sleep stand-in advances the scenario without elapsed time.

#![cfg(target_os = "linux")]

use super::support::{Harness, PR, PR_BRANCH, describe, git};
use crate::support::{exec_stub::write_executable, fifo_release};
use serde_json::{Value, json};
use std::fs::{self, DirBuilder};
use std::os::unix::fs::DirBuilderExt;
use std::process::Command;

pub(super) fn goal(h: &Harness, args: &[&str]) -> Command {
    goal_tool(h, "autodrive_pr_goal.sh", args)
}

fn goal_tool(h: &Harness, script: &str, args: &[&str]) -> Command {
    let mut cmd = crate::isolated_amplihack::isolated("bash");
    cmd.arg(h.bundle_root.join("amplifier-bundle/tools").join(script))
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
        .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
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

pub(super) fn wait(h: &Harness, interval: &str) -> Value {
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
    assert_ten_minute_pending_wait(&h, false, 18, 1);
}

#[test]
fn required_wait_budget_includes_the_uncached_admin_404_each_scan() {
    let h = Harness::new();
    fs::write(h.gh.join("classic404"), "").unwrap();
    fs::write(
        h.gh.join("branch.json"),
        r#"{"protected":true,"protection":{"required_status_checks":{"checks":[{"context":"build","app_id":23}]}}}"#,
    )
    .unwrap();
    // Six scans each read the PR, two check endpoints, rules, classic
    // protection and the branch fallback. The cold PR and six 404s cost quota.
    assert_ten_minute_pending_wait(&h, true, 36, 7);
    let responses = fs::read_to_string(h.gh.join("responses.log")).unwrap();
    assert_eq!(
        responses
            .lines()
            .filter(|line| *line == "404 repos/o/r/branches/main/protection/required_status_checks")
            .count(),
        6,
        "{responses}"
    );
}

fn assert_ten_minute_pending_wait(
    h: &Harness,
    required: bool,
    requests: usize,
    non_304_responses: usize,
) {
    h.set_checks("");
    let reading = read_and_decide(h, required);
    assert_eq!(reading["checks"]["state"], "pending");
    assert_eq!(reading["checks"]["required_known"], true);
    assert_eq!(reading["checks"]["counts"]["total"], u64::from(required));
    assert_eq!(reading["checks"]["counts"]["failing"], 0);
    fs::remove_file(h.gh.join("responses.log")).unwrap();
    fs::remove_file(h.gh.join("calls.log")).unwrap();
    fake_sleep(
        h,
        r#"case "$(wc -l < "$GH_STATE/sleeps")" in
  6) read -r _ < "$GH_STATE/release.fifo"; exit 2 ;;
  [1-5]) ;;
  *) exit 2 ;;
esac"#,
    );
    let guidance_with_startup_output = h.bin.join("guidance-with-startup-output");
    write_executable(
        &guidance_with_startup_output,
        "#!/bin/bash\necho 'Checking framework assets'\nexec \"${0%/*}/amplihack\" \"$@\"\n",
        0o755,
    );
    let mut command = goal(h, &["act", "wait"]);
    command.env("AMPLIHACK_BIN", guidance_with_startup_output);
    let mut live = fifo_release::start(
        command,
        &h.gh.join("release.fifo"),
        "the ten-minute CI wait",
    );
    assert_eq!(sleeps(h), [120; 6]);
    let calls = h.calls();
    assert_eq!(calls.len(), requests, "{calls:?}");
    let responses = fs::read_to_string(h.gh.join("responses.log")).unwrap();
    assert_eq!(responses.lines().count(), calls.len(), "{responses}");
    assert_eq!(
        responses.lines().filter(|l| !l.starts_with("304 ")).count(),
        non_304_responses,
        "{responses}"
    );
    assert_eq!(
        calls
            .iter()
            .filter(|c| c.contains("If-None-Match:"))
            .count(),
        requests - non_304_responses
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
fn the_wait_uses_the_new_required_checks_if_the_target_or_its_policy_changes() {
    for change in [
        "printf 'release\\n' > \"$GH_STATE/moved-base\"",
        "printf '%040d\\n' 7 > \"$GH_STATE/moved-base-sha\"",
        "",
    ] {
        let h = Harness::new();
        h.set_checks("build\tpending\t0\n");
        h.classic_protection(&json!({"strict":false,"contexts":["build"]}));
        read_and_decide(&h, true);
        fake_sleep(
            &h,
            &format!(
                r#"case "$(wc -l < "$GH_STATE/sleeps")" in
  1)
    {change}
    printf '%s\n' '{{"strict":false,"contexts":["integration"]}}' > "$GH_STATE/protection.json"
    printf 'build\tpass\t0\n' > "$GH_STATE/checks" ;;
  2) printf 'build\tpass\t0\nintegration\tfail\t0\n' > "$GH_STATE/checks" ;;
  *) exit 1 ;;
esac"#
            ),
        );
        let result = wait(&h, "37");
        assert_eq!(result["outcome"], "checks-complete", "{result}");
        assert_eq!(result["checks_state"], "failed", "{result}");
        assert_eq!(sleeps(&h), [37, 37]);
    }
}

#[test]
fn removing_an_absent_requirement_unblocks_the_wait_at_the_same_head_and_base() {
    assert_removing_requirement_unblocks_wait(false, false);
}

#[test]
fn removing_a_queued_requirement_unblocks_the_wait_at_the_same_head_and_base() {
    assert_removing_requirement_unblocks_wait(true, false);
}

#[test]
fn a_removed_requirement_that_fails_while_waiting_does_not_fail_the_wait() {
    assert_removing_requirement_unblocks_wait(true, true);
}

fn assert_removing_requirement_unblocks_wait(reported: bool, fails: bool) {
    let h = Harness::new();
    if reported {
        fs::write(
            h.gh.join("check-runs.json"),
            r#"{"check_runs":[{"name":"build","status":"queued","conclusion":null},{"name":"integration","status":"completed","conclusion":"success"}]}"#,
        )
        .unwrap();
    } else {
        h.set_checks("integration\tpass\t0\n");
    }
    h.classic_protection(&json!({"strict":false,"contexts":["build", "integration"]}));
    let reading = read_and_decide(&h, true);
    assert_eq!(reading["checks"]["state"], "pending", "{reading}");
    assert_eq!(reading["checks"]["counts"]["pending"], 1);
    assert_eq!(reading["checks"]["counts"]["passed"], 1);
    fs::remove_file(h.gh.join("responses.log")).unwrap();
    if fails {
        fs::write(
            h.gh.join("failed-check-runs.json"),
            r#"{"check_runs":[{"name":"build","status":"completed","conclusion":"failure"},{"name":"integration","status":"completed","conclusion":"success"}]}"#,
        )
        .unwrap();
    }
    fake_sleep(
        &h,
        r#"case "$(wc -l < "$GH_STATE/sleeps")" in
  1) ;;
  2) printf '%s\n' '{"strict":false,"contexts":["integration"]}' > "$GH_STATE/protection.json"
     if [ -f "$GH_STATE/failed-check-runs.json" ]; then
       mv "$GH_STATE/failed-check-runs.json" "$GH_STATE/check-runs.json"
     fi ;;
  *) exit 2 ;;
esac"#,
    );

    let result = wait(&h, "37");
    assert_eq!(result["outcome"], "checks-complete", "{result}");
    assert_eq!(result["checks_state"], "passed", "{result}");
    assert_eq!(sleeps(&h), [37, 37]);
    let responses = fs::read_to_string(h.gh.join("responses.log")).unwrap();
    let policy: Vec<_> = responses
        .lines()
        .filter(|line| line.contains("/rules/branches/") || line.contains("/protection/"))
        .collect();
    // One conditional read per policy endpoint per pending scan, followed by
    // the usual pass verification. Only the policy edit changes a response.
    assert_eq!(policy.len(), 8, "{policy:?}");
    assert_eq!(
        policy
            .iter()
            .filter(|line| line.starts_with("304 "))
            .count(),
        7,
        "{policy:?}"
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
    assert_eq!(result["checks_state"], "unverified", "{result}");
    assert_eq!(
        result["unverified_checks"][0]["unverified_reason"],
        "status_app_unverified"
    );
    assert_eq!(sleeps(&h), [120]);
}

#[test]
fn required_only_keeps_a_failed_status_alongside_a_passing_app_check() {
    let h = Harness::new();
    h.classic_protection(&json!({"strict":false,"checks":[{"context":"build","app_id":42}]}));
    h.set_checks("build\tpass\t0\t\t42\n");
    fs::write(
        h.gh.join("statuses.json"),
        r#"{"statuses":[{"context":"build","state":"failure"}]}"#,
    )
    .unwrap();
    let reading = read_and_decide(&h, true);
    assert_eq!(reading["checks"]["state"], "failed", "{reading}");
    assert_eq!(reading["checks"]["counts"]["passed"], 1);
    assert_eq!(reading["checks"]["counts"]["failing"], 1);
    assert_eq!(
        reading["checks"]["unverified_checks"][0]["expected_app_ids"],
        json!([42])
    );
    let result = wait(&h, "");
    assert_eq!(result["outcome"], "checks-complete", "{result}");
    assert_eq!(result["checks_state"], "failed", "{result}");
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
                .is_some_and(|s| !s.is_empty())
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
fn an_optional_pass_cannot_verify_a_required_workflow_rule() {
    let h = Harness::new();
    fs::write(h.gh.join("rules.json"), "[]").unwrap();
    fs::write(
        h.gh.join("rules-page-2.json"),
        r#"[{"type":"workflows","parameters":{"workflows":[{"repository_id":42,"path":".github/workflows/required.yml","ref":"refs/heads/main"}]}}]"#,
    )
    .unwrap();
    h.set_checks("optional\tpass\t0\n");

    for required in [false, true] {
        let reading = read_and_decide(&h, required);
        let checks = &reading["checks"];
        assert_eq!(checks["state"], "unverified", "{checks}");
        assert_eq!(checks["required_known"], false);
        assert!(
            checks["required_error"]
                .as_str()
                .is_some_and(|s| !s.is_empty())
        );
        assert_eq!(checks["counts"]["passed"], 1);

        let result = wait(&h, "");
        assert_eq!(result["outcome"], "checks-complete", "{result}");
        assert_eq!(result["checks_state"], "unverified", "{result}");
        assert_eq!(result["required_error"], checks["required_error"]);
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
fn the_reading_discards_completed_checks_if_the_head_moves_during_the_status_retry() {
    let h = Harness::new();
    h.set_checks("build\tpass\t0\n");
    fs::write(h.gh.join("decline-core-status"), "1\n").unwrap();
    fake_sleep(&h, r#"printf '%040d\n' 7 > "$GH_STATE/moved-head""#);

    let reading = read_and_decide(&h, false);
    assert!(reading["pr"]["head"].is_null(), "{reading}");
    assert!(
        reading["checks"]["error"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
    );
    assert!(reading["checks"]["counts"].is_null(), "{reading}");
    assert_eq!(sleeps(&h).len(), 1);
}

#[test]
fn the_reading_discards_completed_checks_if_the_pr_is_retargeted_during_a_retry() {
    let h = Harness::new();
    h.set_checks("build\tpass\t0\n");
    fs::write(h.gh.join("decline-core-status"), "1\n").unwrap();
    fake_sleep(&h, r#"printf 'release\n' > "$GH_STATE/moved-base""#);

    let reading = read_and_decide(&h, true);
    assert_eq!(reading["pr"]["head"], h.origin_head(PR_BRANCH), "{reading}");
    assert!(reading["pr"]["base"].is_null(), "{reading}");
    assert!(
        reading["checks"]["error"]
            .as_str()
            .is_some_and(|error| !error.is_empty()),
        "{reading}"
    );
    assert!(reading["checks"]["counts"].is_null(), "{reading}");
    assert_eq!(sleeps(&h).len(), 1);
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
fn a_rerun_cannot_use_cached_successes_from_an_older_attempt() {
    assert_rerun_rejects_cached_checks("success", "passed");
}

#[test]
fn a_rerun_cannot_use_cached_failures_from_an_older_attempt() {
    assert_rerun_rejects_cached_checks("failure", "failed");
}

fn assert_rerun_rejects_cached_checks(conclusion: &str, state: &str) {
    let h = Harness::new();
    let check = |id| {
        json!({"check_runs":[{"name":"build","status":"completed","conclusion":conclusion,
            "app":{"slug":"github-actions","id":15368},"id":id,"check_suite":{"id":42}}]})
        .to_string()
    };
    let run = |attempt, status| {
        json!({"workflow_runs":[{"id":17,"head_sha":h.initial_head,"check_suite_id":42,
            "run_attempt":attempt,"status":status}]})
        .to_string()
    };
    let runs = h.gh.join("workflow-runs.json");
    let checks = h.gh.join("check-runs.json");
    fs::write(&checks, check(100)).unwrap();
    fs::write(&runs, run(1, "completed")).unwrap();
    let reading = read_and_decide(&h, true);
    assert_eq!(reading["checks"]["state"], state, "{reading}");

    fs::write(&runs, run(2, "queued")).unwrap();
    let reading = read_and_decide(&h, true);
    assert_eq!(reading["checks"]["state"], "pending", "{reading}");
    let terminal_count = if state == "failed" {
        "failing"
    } else {
        "passed"
    };
    assert_eq!(reading["checks"]["counts"][terminal_count], 1, "{reading}");
    assert_eq!(reading["checks"]["counts"]["pending"], 1, "{reading}");
    fs::remove_file(h.gh.join("responses.log")).unwrap();
    fs::write(h.gh.join("completed-rerun.json"), run(2, "completed")).unwrap();
    fs::write(
        h.gh.join("attempt-jobs.json"),
        json!({"jobs":[{"status":"completed","conclusion":conclusion,
            "check_run_url":"https://api.github.com/repos/o/r/check-runs/101"}]})
        .to_string(),
    )
    .unwrap();
    fake_sleep(
        &h,
        r#"test "$(wc -l < "$GH_STATE/sleeps")" -eq 1
mv "$GH_STATE/completed-rerun.json" "$GH_STATE/workflow-runs.json""#,
    );
    let stale = wait(&h, "37");
    assert_eq!(sleeps(&h), [37]);
    assert_eq!(stale["checks_state"], "unverified", "{stale}");
    assert!(
        fs::read_to_string(h.gh.join("responses.log"))
            .unwrap()
            .lines()
            .any(|line| line.starts_with("304 ") && line.contains("/check-runs?"))
    );
    fs::write(&checks, check(101)).unwrap();
    let current = wait(&h, "37");
    assert_eq!(current["checks_state"], state, "{current}");
    assert_eq!(sleeps(&h), [37]);
}

#[test]
fn a_rerun_queued_during_the_pinned_jobs_read_keeps_the_wait_pending() {
    let h = Harness::new();
    let runs = |attempt, status| {
        json!({"workflow_runs":[{"id":17,"head_sha":h.initial_head,"check_suite_id":42,
            "run_attempt":attempt,"status":status}]})
        .to_string()
    };
    fs::write(h.gh.join("workflow-runs.json"), runs(2, "completed")).unwrap();
    fs::write(
        h.gh.join("check-runs.json"),
        r#"{"check_runs":[{"name":"build","status":"completed","conclusion":"success","app":{"slug":"github-actions","id":15368},"id":101,"check_suite":{"id":42}}]}"#,
    )
    .unwrap();
    fs::write(
        h.gh.join("attempt-jobs.json"),
        r#"{"jobs":[{"status":"completed","conclusion":"success","check_run_url":"https://api.github.com/repos/o/r/check-runs/101"}]}"#,
    )
    .unwrap();
    let reading = read_and_decide(&h, true);
    assert_eq!(reading["checks"]["state"], "passed", "{reading}");
    fs::remove_file(h.gh.join("responses.log")).unwrap();
    fs::write(h.gh.join("rerun-during-jobs.json"), runs(3, "queued")).unwrap();
    fs::write(h.gh.join("completed-rerun.json"), runs(3, "completed")).unwrap();
    fake_sleep(
        &h,
        r#"test "$(wc -l < "$GH_STATE/sleeps")" -eq 1
rm "$GH_STATE/check-runs.json"
mv "$GH_STATE/completed-rerun.json" "$GH_STATE/workflow-runs.json"
printf 'build\tfail\t0\n' > "$GH_STATE/checks""#,
    );

    let result = wait(&h, "37");
    assert_eq!(result["outcome"], "checks-complete", "{result}");
    assert_eq!(result["checks_state"], "failed", "{result}");
    assert_eq!(sleeps(&h), [37]);
    let responses = fs::read_to_string(h.gh.join("responses.log")).unwrap();
    let actions: Vec<_> = responses
        .lines()
        .filter(|line| line.contains("/actions/runs"))
        .collect();
    assert_eq!(actions.len(), 3, "{actions:?}");
    assert!(actions[0].starts_with("304 ") && actions[0].contains("/actions/runs?"));
    assert!(actions[1].starts_with("304 ") && actions[1].contains("/attempts/2/jobs?"));
    assert!(actions[2].starts_with("200 ") && actions[2].contains("/actions/runs?"));
}

#[test]
fn the_reading_discards_a_pass_when_a_rerun_queues_during_final_policy_verification() {
    let h = Harness::new();
    passing_actions_check(&h);
    queue_rerun_during_policy(&h);

    let reading = read_and_decide(&h, true);
    assert_eq!(reading["pr"]["head"], h.initial_head, "{reading}");
    assert_eq!(reading["checks"]["required_known"], true, "{reading}");
    assert_eq!(reading["checks"]["state"], "pending", "{reading}");
    assert_eq!(reading["checks"]["counts"]["pending"], 1, "{reading}");
    assert!(!h.gh.join("rerun-during-policy.json").exists());
    assert!(sleeps(&h).is_empty());
}

#[test]
fn a_rerun_queued_during_final_policy_verification_keeps_the_wait_pending() {
    let h = Harness::new();
    passing_actions_check(&h);
    let reading = read_and_decide(&h, true);
    assert_eq!(reading["checks"]["state"], "passed", "{reading}");
    fs::remove_file(h.gh.join("calls.log")).unwrap();
    queue_rerun_during_policy(&h);
    fake_sleep(
        &h,
        r#"test "$(wc -l < "$GH_STATE/sleeps")" -eq 1
test ! -e "$GH_STATE/rerun-during-policy.json"
sed 's/"queued"/"completed"/' "$GH_STATE/workflow-runs.json" > "$GH_STATE/completed-rerun.json"
mv "$GH_STATE/completed-rerun.json" "$GH_STATE/workflow-runs.json"
printf '%s\n' '{"check_runs":[{"name":"build","status":"completed","conclusion":"failure","app":{"slug":"github-actions","id":15368},"id":101,"check_suite":{"id":42}}]}' > "$GH_STATE/check-runs.json"
printf '%s\n' '{"jobs":[{"status":"completed","conclusion":"failure","check_run_url":"https://api.github.com/repos/o/r/check-runs/101"}]}' > "$GH_STATE/attempt-jobs.json""#,
    );

    let result = wait(&h, "37");
    assert_eq!(result["outcome"], "checks-complete", "{result}");
    assert_eq!(result["checks_state"], "failed", "{result}");
    assert_eq!(sleeps(&h), [37]);
}

#[test]
fn a_push_during_final_actions_verification_ends_the_wait() {
    let h = Harness::new();
    passing_actions_check(&h);
    let reading = read_and_decide(&h, true);
    assert_eq!(reading["checks"]["state"], "passed", "{reading}");
    fs::remove_file(h.gh.join("calls.log")).unwrap();
    fs::write(h.gh.join("moved-head-during-actions"), "7".repeat(40)).unwrap();

    let result = wait(&h, "37");
    assert_eq!(result["outcome"], "head-changed", "{result}");
    assert!(result["checks_state"].is_null(), "{result}");
    assert!(sleeps(&h).is_empty());
}

#[test]
fn a_base_update_during_final_actions_verification_rescans_in_the_wait() {
    let h = Harness::new();
    passing_actions_check(&h);
    let reading = read_and_decide(&h, true);
    let base_sha = "7".repeat(40);
    assert_ne!(reading["pr"]["base_sha"], base_sha);
    fs::remove_file(h.gh.join("calls.log")).unwrap();
    fs::write(h.gh.join("moved-base-sha-during-actions"), &base_sha).unwrap();

    let result = wait(&h, "37");
    assert_eq!(result["checks_state"], "passed", "{result}");
    assert_eq!(result["head"], h.initial_head, "{result}");
    assert_eq!(result["base"], "main", "{result}");
    assert_eq!(result["base_sha"], base_sha, "{result}");
    assert_eq!(
        h.calls()
            .iter()
            .filter(|call| call.contains("/check-runs?"))
            .count(),
        2
    );
    assert!(sleeps(&h).is_empty());
}

#[test]
fn the_reading_discards_checks_if_the_pr_changes_during_final_actions_verification() {
    for (conclusion, change, field) in [
        ("success", "moved-head", "head"),
        ("success", "moved-base-sha", "base_sha"),
        ("failure", "moved-head", "head"),
    ] {
        let h = Harness::new();
        completed_actions_check(&h, conclusion);
        fs::write(
            h.gh.join(format!("{change}-during-actions")),
            "7".repeat(40),
        )
        .unwrap();

        let reading = read_and_decide(&h, true);
        assert!(reading["pr"][field].is_null(), "{reading}");
        assert!(
            reading["repository"]["strict_status_checks"].is_null(),
            "{reading}"
        );
        assert!(reading["checks"]["counts"].is_null(), "{reading}");
        assert!(
            reading["checks"]["error"]
                .as_str()
                .is_some_and(|error| !error.is_empty()),
            "{reading}"
        );
    }
}

#[test]
fn the_reading_discards_a_failed_legacy_status_if_the_pr_changes_during_the_scan() {
    for (file, value, field) in [
        ("moved-head-during-status", "7".repeat(40), "head"),
        ("moved-base-during-status", "release".to_owned(), "base"),
    ] {
        let h = Harness::new();
        h.set_checks("");
        h.classic_protection(&json!({"strict":false,"contexts":["external"]}));
        fs::write(
            h.gh.join("statuses.json"),
            r#"{"statuses":[{"context":"external","state":"failure"}]}"#,
        )
        .unwrap();
        fs::write(h.gh.join(file), value).unwrap();

        let reading = read_and_decide(&h, true);
        assert!(reading["pr"][field].is_null(), "{reading}");
        assert!(
            reading["repository"]["strict_status_checks"].is_null(),
            "{reading}"
        );
        assert!(reading["checks"]["counts"].is_null(), "{reading}");
        assert!(reading["checks"]["state"].is_null(), "{reading}");
        assert!(
            reading["checks"]["error"]
                .as_str()
                .is_some_and(|error| !error.is_empty()),
            "{reading}"
        );
        assert!(sleeps(&h).is_empty());
    }
}

#[test]
fn unverified_actions_checks_report_the_sanitized_api_error_or_unreadable_response() {
    for (file, contents, expected) in [
        ("actions403", "gho_1234567890123456", "403"),
        ("workflow-runs.json", "{}", "repos/o/r/actions/runs"),
    ] {
        let h = Harness::new();
        passing_actions_check(&h);
        fs::write(h.gh.join(file), contents).unwrap();
        let reading = read_and_decide(&h, true);
        let checks = &reading["checks"];
        let detail = checks["execution_detail"].as_str().unwrap();
        assert_eq!(checks["state"], "unverified", "{checks}");
        assert!(detail.contains(expected), "{detail}");
        assert!(!detail.contains("gho_"), "{detail}");

        let result = wait(&h, "");
        assert_eq!(result["checks_state"], "unverified", "{result}");
        assert_eq!(result["execution_detail"], detail, "{result}");
    }
}

#[test]
fn the_build_summary_surfaces_unverified_actions_results() {
    let h = Harness::new();
    passing_actions_check(&h);
    fs::write(h.gh.join("workflow-runs.json"), r#"{"workflow_runs":[]}"#).unwrap();
    let origin = "https://github.com/o/r.git";
    let local_origin = format!("url.{}.insteadOf", h.origin.display());
    git(&h.repo, &["config", &local_origin, origin]);
    git(&h.repo, &["remote", "set-url", "origin", origin]);
    let turn = h.state.join("turns/001");
    DirBuilder::new()
        .mode(0o700)
        .recursive(true)
        .create(&turn)
        .unwrap();

    let out = goal_tool(&h, "autodrive_build_goal.sh", &["read"])
        .env("RECIPE_VAR_repo_path", &h.repo)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    let reading: Value =
        serde_json::from_slice(&fs::read(turn.join("goal.json")).unwrap()).unwrap();
    let checks = &reading["checks"];
    assert_eq!(checks["state"], "unverified", "{reading}");
    assert_eq!(checks["counts"]["passed"], 1, "{reading}");
    let detail = checks["execution_detail"].as_str().unwrap();
    assert!(!detail.is_empty(), "{reading}");
    let summary = reading["summary"].as_str().unwrap();
    assert!(
        summary.contains(checks["state"].as_str().unwrap()),
        "{summary}"
    );
    assert!(summary.contains(detail), "{summary}");
}

#[test]
fn actions_revalidation_reports_the_unmatched_suite() {
    let h = Harness::new();
    passing_actions_check(&h);
    let reading = read_and_decide(&h, true);
    assert_eq!(reading["checks"]["state"], "passed", "{reading}");
    fs::remove_file(h.gh.join("calls.log")).unwrap();
    fs::write(
        h.gh.join("workflow-runs.json-during-actions"),
        r#"{"workflow_runs":[]}"#,
    )
    .unwrap();

    let result = wait(&h, "");
    assert_eq!(result["checks_state"], "unverified", "{result}");
    let detail = result["execution_detail"].as_str().unwrap();
    assert!(
        detail.split_whitespace().any(|part| part == "42"),
        "{detail}"
    );
}

fn passing_actions_check(h: &Harness) {
    completed_actions_check(h, "success");
}

fn completed_actions_check(h: &Harness, conclusion: &str) {
    h.classic_protection(&json!({"strict":false,"contexts":["build"]}));
    fs::write(
        h.gh.join("check-runs.json"),
        json!({"check_runs":[{"name":"build","status":"completed","conclusion":conclusion,
            "app":{"slug":"github-actions","id":15368},"id":100,"check_suite":{"id":42}}]})
        .to_string(),
    )
    .unwrap();
    set_actions_attempt(h, "workflow-runs.json", 1, "completed");
}

fn queue_rerun_during_policy(h: &Harness) {
    set_actions_attempt(h, "rerun-during-policy.json", 2, "queued");
}

fn set_actions_attempt(h: &Harness, file: &str, attempt: i32, status: &str) {
    fs::write(
        h.gh.join(file),
        json!({"workflow_runs":[{"id":17,"head_sha":h.initial_head,"check_suite_id":42,
            "run_attempt":attempt,"status":status}]})
        .to_string(),
    )
    .unwrap();
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
fn a_declined_read_without_a_deadline_delivers_guidance_to_the_same_turn() {
    let h = Harness::new();
    fs::write(h.gh.join("no-retry-after"), "").unwrap();
    fs::write(h.gh.join("hold-rate-limit"), "").unwrap();
    fs::write(h.gh.join("decline-core-check-runs"), "2\n").unwrap();
    fake_sleep(&h, "exit 0");
    h.policy(
        r#"echo '{"action":"handoff","args":{"owner":"operator","decision":"recover the GitHub login","text":"The CI read is unresolved."},"why":"The check read is unresolved."}'"#,
    );
    let mut live = fifo_release::start(
        h.drive_command("auto-drive-to-merge", &["no_merge=true"]),
        &h.gh.join("release.fifo"),
        "the rate-limited turn reading",
    );
    h.guide("Check the GitHub login before reading the checks again.");
    let out = live.finish();
    assert_eq!(
        Harness::outcome(&out)["stop_reason"],
        "handoff",
        "{}",
        describe(&out)
    );
    assert_eq!(h.turns(), ["001"]);
    let reading = h.reading("001");
    assert_eq!(reading["guidance"][0]["new"], true, "{reading}");
    assert!(
        reading["goal"]["checks"]["error"]
            .as_str()
            .is_some_and(|error| !error.is_empty()),
        "{reading}"
    );
    assert!(h.turn_prompt("001").contains("Check the GitHub login"));
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
    fake_sleep(&h, "exit 0");
    h.policy(
        r#"if [ "${TURN##*/}" = 001 ]; then
  printf '2\n' > "$GH_STATE/decline-core"
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
            "head",
            "rate_limit",
            "head",
            "rate_limit",
            "head",
            "head",
            "reading",
            "head"
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
