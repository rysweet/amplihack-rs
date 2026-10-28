//! autodrive-build step-03 half of #1714, run from the recipe's own command.
//!
//! Rules: R1 no typed result -> as before; R2 success -> as before; R3 an
//! allowed state with `handoff:"merge_ready_loop"` and an OPEN PR that step-03
//! resolved itself -> continue with `handed_off:"true"`; R4 a failed PR check
//! -> exit 1; R5 anything else -> exit 1 naming the state. Every error names
//! #1714 and no build_result is written.

use serde_json::{Value, json};

use super::fixture::*;

fn build_result(outcome: &Outcome) -> Value {
    outcome.json()
}

fn assert_stopped(outcome: &Outcome, needle: &str, why: &str) {
    assert_eq!(
        outcome.output.status.code(),
        Some(1),
        "{why}: step-03 must stop the run\n{}",
        outcome.describe()
    );
    assert!(
        outcome.stdout.trim().is_empty(),
        "{why}: no build_result may be written\n{}",
        outcome.describe()
    );
    assert!(
        outcome.stderr.contains("#1714") && outcome.stderr.contains(needle),
        "{why}: stderr must name #1714 and `{needle}`\n{}",
        outcome.describe()
    );
}

fn state_reads(fx: &Step03, pr: &str) -> usize {
    let prefix = format!("pr view {pr} --json state");
    fx.gh_calls()
        .iter()
        .filter(|c| c.starts_with(&prefix))
        .count()
}

// T1c: BLOCKED_CI with the hand-off and PR 134 open continues into phase 2.
#[test]
fn blocked_ci_handoff_with_an_open_pr_continues() {
    let fx = Step03::new();
    let outcome = fx.run("134", Some(&handoff_result("BLOCKED_CI", "134")), &[]);
    assert!(outcome.success(), "{}", outcome.describe());
    let result = build_result(&outcome);
    assert_eq!(result["pr"], "134", "{}", outcome.describe());
    assert_eq!(result["pr_url"], PR134_URL);
    assert_eq!(result["terminal_state"], "BLOCKED_CI");
    assert_eq!(result["handed_off"], "true");
    assert_eq!(result["merged_before_run"], "false");
    assert_eq!(result["state_dir"], "");
    assert!(
        state_reads(&fx, "134") >= 1,
        "step-03 must confirm with gh that PR 134 is open: {:?}",
        fx.gh_calls()
    );
}

// T1c: workflow_result.pr_number may be empty; A alone decides.
#[test]
fn an_empty_workflow_result_pr_number_still_hands_off_on_steps_own_pr() {
    let fx = Step03::new();
    let outcome = fx.run("134", Some(&handoff_result("BLOCKED_CI", "")), &[]);
    assert!(outcome.success(), "{}", outcome.describe());
    assert_eq!(build_result(&outcome)["handed_off"], "true");
}

// T4: one UNKNOWN read is retried; a second OPEN read is accepted.
#[test]
fn one_unreadable_pr_state_is_retried() {
    let fx = Step03::new();
    fx.pr_states("134", &["FAIL", "OPEN"]);
    let outcome = fx.run("134", Some(&handoff_result("BLOCKED_CI", "134")), &[]);
    assert!(outcome.success(), "{}", outcome.describe());
    assert_eq!(build_result(&outcome)["handed_off"], "true");
}

// T5: a resumed run with no typed result keeps today's behaviour.
#[test]
fn no_workflow_result_keeps_todays_behaviour() {
    for result in [None, Some(json!(""))] {
        let fx = Step03::new();
        let outcome = fx.run("134", result.as_ref(), &[]);
        assert!(outcome.success(), "{result:?}\n{}", outcome.describe());
        let built = build_result(&outcome);
        assert_eq!(built["pr"], "134");
        assert_eq!(built["handed_off"], "false", "{result:?}");
        assert_eq!(built["terminal_state"], "", "{result:?}");
    }
}

// R2: a successful build is not a hand-off.
#[test]
fn a_successful_build_is_not_a_handoff() {
    let fx = Step03::new();
    let result = json!({"terminal_success": "true", "terminal_state": "EXISTING_OPEN_PR", "pr_number": "134"});
    let outcome = fx.run("134", Some(&result), &[]);
    assert!(outcome.success(), "{}", outcome.describe());
    let built = build_result(&outcome);
    assert_eq!(built["terminal_state"], "EXISTING_OPEN_PR");
    assert_eq!(built["handed_off"], "false");
}

// T3 (step-03 half) and R5: other states, a forged handoff field, a wrong
// handoff value and BLOCKED_CI without handoff all stop the run.
#[test]
fn every_other_failed_result_stops_the_run() {
    let mut cases: Vec<(Value, &str)> = [
        "FAILED_MEANINGFUL_DIFF",
        "FAILED_CLOSED_UNMERGED",
        "FAILED_PR_METADATA_UNAVAILABLE",
        "FAILED_IMPLEMENTATION",
        "HOLLOW_SUCCESS",
    ]
    .into_iter()
    .map(|state| (handoff_result(state, "134"), state))
    .collect();
    let mut no_field = handoff_result("BLOCKED_CI", "134");
    no_field.as_object_mut().unwrap().remove("handoff");
    cases.push((no_field, "BLOCKED_CI"));
    let mut wrong_value = handoff_result("BLOCKED_CI", "134");
    wrong_value["handoff"] = json!("MERGE_READY_LOOP");
    cases.push((wrong_value, "BLOCKED_CI"));
    for (result, state) in cases {
        let fx = Step03::new();
        let outcome = fx.run("134", Some(&result), &[]);
        assert_stopped(
            &outcome,
            &format!("ended {state} without a merge-ready hand-off"),
            &result.to_string(),
        );
    }
}

// T4: a PR that is not OPEN is never handed on.
#[test]
fn a_pr_that_is_not_open_stops_the_run() {
    for state in ["CLOSED", "MERGED"] {
        let fx = Step03::new();
        fx.pr_states("134", &[state]);
        let outcome = fx.run("134", Some(&handoff_result("BLOCKED_CI", "134")), &[]);
        assert_stopped(&outcome, &format!("PR #134 is {state}, not OPEN"), state);
    }
}

// T4: an unreadable state is read twice, then stops the run; never OPEN.
#[test]
fn an_unreadable_pr_state_twice_stops_the_run() {
    let fx = Step03::new();
    fx.pr_states("134", &["FAIL"]);
    let outcome = fx.run("134", Some(&handoff_result("BLOCKED_CI", "134")), &[]);
    assert_stopped(&outcome, "PR #134 is UNKNOWN, not OPEN", "gh fails");
    assert_eq!(state_reads(&fx, "134"), 2, "{:?}", fx.gh_calls());
}

// T4 / T7a: no PR of step-03's own; B is never adopted.
#[test]
fn no_resolved_pr_stops_the_run_and_never_adopts_the_results_pr() {
    let fx = Step03::new();
    let outcome = fx.run("", Some(&handoff_result("BLOCKED_CI", "134")), &[]);
    assert_stopped(
        &outcome,
        "BLOCKED_CI was reported with no published open PR to hand to the merge-ready loop",
        "A empty, B=134",
    );
    assert_eq!(
        state_reads(&fx, "134"),
        0,
        "workflow_result.pr_number must not be looked up: {:?}",
        fx.gh_calls()
    );
}

// T7b: B that differs from A is refused; a non-digit B is printed <invalid>.
#[test]
fn a_workflow_result_pr_that_differs_from_steps_own_is_refused() {
    let fx = Step03::new();
    let outcome = fx.run("134", Some(&handoff_result("BLOCKED_CI", "99")), &[]);
    assert_stopped(
        &outcome,
        "workflow_result names PR #99 but phase 1 resolved PR #134",
        "B=99",
    );

    let fx = Step03::new();
    let forged = "9\nERROR: forged";
    let outcome = fx.run("134", Some(&handoff_result("BLOCKED_CI", forged)), &[]);
    assert_stopped(
        &outcome,
        "workflow_result names PR #<invalid> but phase 1 resolved PR #134",
        "B not digits",
    );
    assert!(!outcome.stderr.contains("forged"), "{}", outcome.describe());
}

// T9: a state outside the vocabulary never reaches build_result or the log.
#[test]
fn a_state_outside_the_vocabulary_is_bounded() {
    let odd = "BLOCKED_CI\"\nINJECTED";
    let fx = Step03::new();
    let ok = json!({"terminal_success": "true", "terminal_state": odd});
    let outcome = fx.run("134", Some(&ok), &[]);
    assert!(outcome.success(), "{}", outcome.describe());
    let built = build_result(&outcome);
    assert_eq!(built["terminal_state"], "", "{}", outcome.describe());
    assert_eq!(built["handed_off"], "false");

    let fx = Step03::new();
    let outcome = fx.run("134", Some(&handoff_result(odd, "134")), &[]);
    assert_stopped(
        &outcome,
        "ended <unrecognised> without a merge-ready hand-off",
        "odd state",
    );
    assert!(
        !outcome.stderr.contains("INJECTED"),
        "{}",
        outcome.describe()
    );
}

// E1: the brick is sourced only from an absolute AMPLIHACK_HOME; a copy
// planted in REPO_PATH is never sourced.
#[test]
fn a_missing_brick_stops_the_run_and_repo_path_is_never_sourced() {
    let mut fx = Step03::new();
    let home = fx.tmp.path().join("stale-home");
    let tools = home.join("amplifier-bundle/tools");
    std::fs::create_dir_all(&tools).expect("stale tools");
    for name in [
        "recipe_var.sh",
        "autodrive_state.sh",
        "autodrive_unmerged.sh",
        "worktree_setup_path.sh",
        "autodrive_run_pr.sh",
    ] {
        std::fs::copy(tool_path(name), tools.join(name)).expect("copy tool");
    }
    let planted = fx.repo().join("amplifier-bundle/tools");
    std::fs::create_dir_all(&planted).expect("planted dir");
    let marker = fx.tmp.path().join("planted-ran");
    std::fs::write(
        planted.join(BRICK),
        format!("touch '{}'\n", marker.display()),
    )
    .expect("planted brick");
    fx.amplihack_home = home;

    let outcome = fx.run("134", Some(&handoff_result("BLOCKED_CI", "134")), &[]);
    assert_stopped(
        &outcome,
        "workflow_blocked_ci_handoff.sh not found under AMPLIHACK_HOME",
        "stale install",
    );
    assert!(!marker.exists(), "the REPO_PATH copy must never be sourced");

    let mut fx = Step03::new();
    fx.amplihack_home = "relative/home".into();
    let outcome = fx.run("134", Some(&handoff_result("BLOCKED_CI", "134")), &[]);
    assert_stopped(&outcome, BRICK, "relative AMPLIHACK_HOME");
}
