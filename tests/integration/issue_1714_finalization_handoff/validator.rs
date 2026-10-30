//! The validator half of #1714. Both BLOCKED_CI exits (the prior-state probe
//! and the open-PR check) hand off only when `RECIPE_VAR_finalization_handoff`
//! is exactly `true` and the PR number is valid; the result keeps
//! `terminal_success:"false"` and gains `handoff:"merge_ready_loop"`. Every
//! other state, and every run without the flag, exits 1 with no `handoff` key.

use serde_json::json;

use crate::isolated_amplihack::isolated;

use super::fixture::*;

const FLAG: (&str, &str) = ("RECIPE_VAR_finalization_handoff", "true");

/// A resumed run whose prior terminal probe reported `state` for PR `pr`.
fn prior_state_env(fx: &Fixture, state: &str, pr: &str) -> Vec<(&'static str, String)> {
    let mut envs = resumed_env(&fx.repo_path());
    envs.push(("TERMINAL_STATE_TERMINAL_STATE", state.to_owned()));
    if !pr.is_empty() {
        envs.push(("PR_NUMBER", pr.to_owned()));
    }
    envs
}

fn with_flag<'a>(envs: &'a [(&'static str, String)]) -> Vec<(&'a str, &'a str)> {
    let mut refs = as_refs(envs);
    refs.push(FLAG);
    refs
}

fn assert_handed_off(outcome: &Outcome, pr: &str, why: &str) {
    assert!(
        outcome.success(),
        "{why}: the hand-off exits 0\n{}",
        outcome.describe()
    );
    assert_eq!(outcome.field("terminal_state"), "BLOCKED_CI", "{why}");
    assert_eq!(outcome.field("terminal_success"), "false", "{why}");
    assert_eq!(outcome.field("terminal_failure"), "true", "{why}");
    assert_eq!(outcome.field("finalizer_output_valid"), "false", "{why}");
    assert_eq!(outcome.field("pr_number"), pr, "{why}");
    assert_eq!(outcome.field("handoff"), "merge_ready_loop", "{why}");
    assert!(
        outcome
            .stderr
            .contains(&format!("{HANDOFF_INFO} (PR #{pr})")),
        "{why}: stderr names the hand-off\n{}",
        outcome.describe()
    );
    assert!(
        !outcome.stderr.contains("failed closed"),
        "{why}: a hand-off is not announced as a fail-closed exit\n{}",
        outcome.describe()
    );
}

fn assert_failed_closed(outcome: &Outcome, state: &str, why: &str) {
    assert_eq!(
        outcome.output.status.code(),
        Some(1),
        "{why}: must fail closed\n{}",
        outcome.describe()
    );
    assert_eq!(outcome.field("terminal_state"), state, "{why}");
    assert_ne!(outcome.field("terminal_success"), "true", "{why}");
    assert_no_handoff_key(outcome, why);
}

// T1a: the prior-state probe, where the failing #1714 run stopped.
#[test]
fn prior_state_blocked_ci_with_the_flag_and_a_pr_is_handed_off() {
    let fx = Fixture::new();
    let envs = prior_state_env(&fx, "BLOCKED_CI", "134");
    let outcome = validate(&fx, &with_flag(&envs));
    assert_handed_off(&outcome, "134", "prior-state probe");
    fx.assert_gh_not_called("prior-state probe");
}

// T1a, production transport: the context arrives as RECIPE_VAR_* objects.
#[test]
fn canonical_transport_prior_state_blocked_ci_is_handed_off() {
    let fx = Fixture::new();
    let repo_path = fx.repo_path();
    let terminal_state =
        json!({"terminal_state": "BLOCKED_CI", "pr_number": "134", "pr_url": PR134_URL})
            .to_string();
    let outcome = validate(
        &fx,
        &[
            ("RECIPE_VAR_repo_path", repo_path.as_str()),
            ("RECIPE_VAR_branch_name", BRANCH),
            ("RECIPE_VAR_terminal_state", terminal_state.as_str()),
            ("RECIPE_VAR_finalizer_step_status", r#"{"status":"ok"}"#),
            FLAG,
        ],
    );
    assert_handed_off(&outcome, "134", "canonical transport");
    assert_eq!(outcome.field("pr_url"), PR134_URL);
}

// T1a: the completion envelope keeps terminal_success "false" everywhere and
// carries the handoff field inside workflow_result.
#[test]
fn completion_of_a_handed_off_result_never_reports_success() {
    let fx = Fixture::new();
    let envs = prior_state_env(&fx, "BLOCKED_CI", "134");
    let validated = validate(&fx, &with_flag(&envs));
    assert_handed_off(&validated, "134", "before completion");
    let result = validated.stdout.trim().to_owned();

    let outcome = dispatch(&fx, "complete", &[("RECIPE_VAR_workflow_result", &result)]);
    assert!(outcome.success(), "{}", outcome.describe());
    let completion = outcome.json();
    assert_eq!(completion["terminal_success"], "false");
    assert_eq!(completion["terminal_state"], "BLOCKED_CI");
    assert_eq!(completion["workflow_result"]["terminal_success"], "false");
    assert_eq!(completion["workflow_result"]["handoff"], "merge_ready_loop");
}

// T1b: the open-PR check with a failing check run.
#[test]
fn open_pr_with_a_failing_check_is_handed_off() {
    let fx = Fixture::new();
    fx.gh_view(&open_pr(json!([check_run("COMPLETED", json!("FAILURE"))])));
    let mut envs = resumed_env(&fx.repo_path());
    envs.push(("PR_URL", PR_URL.to_owned()));
    let outcome = validate(&fx, &with_flag(&envs));
    assert_handed_off(&outcome, "7", "open-PR check");
    assert_eq!(outcome.field("pr_url"), PR_URL);
    fx.assert_gh_reads_only();
}

// T2: anything but exactly "true" leaves a standalone run failing closed, and
// a bare FINALIZATION_HANDOFF is never read.
#[test]
fn without_exactly_true_blocked_ci_still_fails_closed() {
    let values: [Option<&str>; 8] = [
        None,
        Some(""),
        Some("1"),
        Some("yes"),
        Some("TRUE"),
        Some(" true"),
        Some("true "),
        Some("on"),
    ];
    for value in values {
        let fx = Fixture::new();
        let mut envs = prior_state_env(&fx, "BLOCKED_CI", "134");
        if let Some(value) = value {
            envs.push(("RECIPE_VAR_finalization_handoff", value.to_owned()));
        }
        envs.push(("FINALIZATION_HANDOFF", "true".to_owned()));
        let outcome = validate(&fx, &as_refs(&envs));
        assert_failed_closed(&outcome, "BLOCKED_CI", &format!("flag={value:?}"));
        assert!(!outcome.stderr.contains("#1714"), "flag={value:?}");
    }

    let fx = Fixture::new();
    fx.gh_view(&open_pr(json!([check_run("COMPLETED", json!("FAILURE"))])));
    let mut envs = resumed_env(&fx.repo_path());
    envs.push(("PR_URL", PR_URL.to_owned()));
    assert_failed_closed(
        &validate(&fx, &as_refs(&envs)),
        "BLOCKED_CI",
        "open-PR check, no flag",
    );
}

// T3: every other prior-state blocker keeps exit 1 with the flag on.
#[test]
fn other_prior_state_blockers_are_never_handed_off() {
    for state in [
        "FAILED_MEANINGFUL_DIFF",
        "FAILED_CLOSED_UNMERGED",
        "FAILED_PR_METADATA_UNAVAILABLE",
        "FAILED_INVALID_INPUT",
        "FAILED_WRONG_BRANCH",
    ] {
        let fx = Fixture::new();
        let envs = prior_state_env(&fx, state, "134");
        assert_failed_closed(&validate(&fx, &with_flag(&envs)), state, state);
        fx.assert_gh_not_called(state);
    }
}

// T3: the remaining non-CI failures keep exit 1 with the flag and a PR. The
// first is FAILED_IMPLEMENTATION with a PR number the open-PR check could not
// confirm (no live view), which is never handed off (#1714 case 2).
#[test]
fn non_ci_failures_with_the_flag_still_fail_closed() {
    let fx = Fixture::new();
    let mut envs = resumed_env(&fx.repo_path());
    envs.push(("PR_NUMBER", "134".to_owned()));
    assert_failed_closed(
        &validate(&fx, &with_flag(&envs)),
        "FAILED_IMPLEMENTATION",
        "no implementation evidence",
    );

    let fx = Fixture::new();
    let mut envs = prior_state_env(&fx, "BLOCKED_CI", "134");
    envs.push((
        "FINALIZATION_EVIDENCE_GIT_DIRTY_WORKTREE",
        "true".to_owned(),
    ));
    assert_failed_closed(
        &validate(&fx, &with_flag(&envs)),
        "FAILED_DIRTY_WORKTREE",
        "dirty worktree precedes the probe",
    );

    let fx = Fixture::new();
    let mut envs = prior_state_env(&fx, "HOLLOW_SUCCESS", "134");
    envs.push((
        "FINALIZATION_EVIDENCE_AGENT_OUTPUTS_HOLLOW_SUCCESS_SIGNALS",
        "true".to_owned(),
    ));
    assert_failed_closed(
        &validate(&fx, &with_flag(&envs)),
        "HOLLOW_SUCCESS",
        "hollow success",
    );
}

// T4, validator half: BLOCKED_CI with no valid PR number is not handed off.
#[test]
fn blocked_ci_without_a_valid_pr_number_fails_closed() {
    for pr in ["", "abc", "0", "#134", "134 ", "99999999999"] {
        let fx = Fixture::new();
        let envs = prior_state_env(&fx, "BLOCKED_CI", pr);
        let outcome = validate(&fx, &with_flag(&envs));
        assert_failed_closed(&outcome, "BLOCKED_CI", &format!("pr={pr:?}"));
        assert!(
            !outcome.stderr.contains(HANDOFF_INFO),
            "pr={pr:?}\n{}",
            outcome.describe()
        );
    }
}

// T11: the hand-off exits 0 only after a complete JSON object was written.
#[test]
fn a_failed_handoff_emit_exits_non_zero() {
    let fx = Fixture::new();
    break_handoff_jq(&fx);
    let envs = prior_state_env(&fx, "BLOCKED_CI", "134");
    let outcome = validate(&fx, &with_flag(&envs));
    assert!(
        !outcome.success(),
        "a jq failure while adding `handoff` must not exit 0\n{}",
        outcome.describe()
    );
    assert!(
        !outcome.stderr.contains(HANDOFF_INFO),
        "the INFO line is printed only after a complete result\n{}",
        outcome.describe()
    );
}

// The 19-key schema of a result that is not handed off is unchanged.
#[test]
fn a_success_result_has_no_handoff_key_even_with_the_flag() {
    let fx = Fixture::new();
    fx.gh_view(&open_pr(green_rollup()));
    let mut envs = resumed_env(&fx.repo_path());
    envs.push(("PR_URL", PR_URL.to_owned()));
    let outcome = validate(&fx, &with_flag(&envs));
    assert!(outcome.success(), "{}", outcome.describe());
    assert_eq!(outcome.field("terminal_state"), "EXISTING_OPEN_PR");
    assert_no_handoff_key(&outcome, "EXISTING_OPEN_PR");
    assert_eq!(outcome.json().as_object().map(|o| o.len()), Some(19));
}

/// Put a `jq` on the fixture's PATH that fails whenever its filter adds the
/// `handoff` field, and passes every other call to the real jq.
fn break_handoff_jq(fx: &Fixture) {
    let real = isolated("bash")
        .args(["-c", "command -v jq"])
        .output()
        .expect("locate jq");
    let real = String::from_utf8_lossy(&real.stdout).trim().to_owned();
    assert!(real.starts_with('/'), "jq must be on PATH for these tests");
    let body = format!(
        "#!/usr/bin/env bash\nfor a in \"$@\"; do case \"$a\" in *handoff*) printf '{{\"partial\":'; exit 5 ;; esac; done\nexec {real} \"$@\"\n"
    );
    write_exec(&fx.tmp.path().join("bin/jq"), &body);
}
