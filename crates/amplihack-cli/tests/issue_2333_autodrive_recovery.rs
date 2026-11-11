//! Issue #2333: execute the shared loop with controlled recipe responses.
//!
//! These integration regressions exercise invocation, delivered evidence and
//! decision consumption at the recipe boundary. They do not stand in for the
//! planned full receipt-validation-to-mocked-merge acceptance scenario.

use crate::support::auto_drive_light::show;
use crate::support::pr_evidence::{LoopFixture, round};
use serde_json::json;

#[path = "support/pr_evidence_agent.rs"]
mod agent;

#[test]
fn graphql_refusal_cannot_skip_the_second_health_evaluation() {
    for (name, blocked, ready) in [
        ("merge-ready", "NOT_MERGE_READY", "MERGE_READY"),
        ("crusty", "CONCERNS", "CLEAN"),
    ] {
        let fixture = LoopFixture::new();
        let out = fixture.run(
            name,
            &[
                round(blocked, "unreadable", "CONTINUE"),
                round(blocked, "unreadable", "CONTINUE"),
                round(blocked, "unreadable", "CONTINUE"),
                round(ready, "GREEN:0/2", "DONE"),
            ],
            &[],
        );
        assert_eq!(
            fixture.evaluations(),
            ["round-1", "round-2", "round-3", "round-4"],
            "{name}: every CONTINUE must be consumed, including unchanged unreadable CI:\n{}",
            show(&out)
        );
        assert!(out.status.success(), "{name}: {}", show(&out));
        assert_eq!(fixture.result(name)["loop_result"], "DONE");
    }
}

#[test]
fn pending_ci_can_persist_at_the_same_head_then_advance_when_green() {
    let fixture = LoopFixture::new();
    let out = fixture.run(
        "merge-ready",
        &[
            round("NOT_MERGE_READY", "PENDING:2/2", "CONTINUE"),
            round("NOT_MERGE_READY", "PENDING:2/2", "CONTINUE"),
            round("NOT_MERGE_READY", "PENDING:2/2", "CONTINUE"),
            round("NOT_MERGE_READY", "PENDING:2/2", "CONTINUE"),
            round("MERGE_READY", "GREEN:0/2", "DONE"),
        ],
        &[],
    );
    assert!(out.status.success(), "{}", show(&out));
    assert_eq!(fixture.evaluations().len(), 5);
    assert_eq!(fixture.result("merge-ready")["round_label"], "round-5");
}

#[test]
fn unchanged_findings_and_red_ci_remain_the_agents_convergence_decision() {
    let fixture = LoopFixture::new();
    let out = fixture.run(
        "merge-ready",
        &[
            round("NOT_MERGE_READY", "RED:1/2", "CONTINUE"),
            round("NOT_MERGE_READY", "RED:1/2", "CONTINUE"),
            round("NOT_MERGE_READY", "RED:1/2", "STUCK"),
        ],
        &[],
    );
    assert_eq!(
        fixture.evaluations(),
        ["round-1", "round-2", "round-3"],
        "{}",
        show(&out)
    );
    assert!(!out.status.success());
    assert!(
        fixture.fixture.join("explicit-stuck.json").exists(),
        "STUCK must come from the controlled evaluator"
    );
    assert_eq!(fixture.result("merge-ready")["loop_result"], "STUCK");
}

#[test]
fn repeated_done_on_nonclean_evidence_requests_reassessment_without_synthetic_stuck() {
    let fixture = LoopFixture::new();
    // PENDING prevents the old no-progress check from hiding the separate
    // repeated-DONE bug. Every round deliberately has the same findings/head.
    let out = fixture.run(
        "merge-ready",
        &[
            round("NOT_MERGE_READY", "PENDING:1/2", "DONE"),
            round("NOT_MERGE_READY", "PENDING:1/2", "DONE"),
            round("NOT_MERGE_READY", "PENDING:1/2", "CONTINUE"),
            round("MERGE_READY", "GREEN:0/2", "DONE"),
        ],
        &[],
    );
    assert!(
        out.status.success(),
        "inconsistent DONE must return to assessment:\n{}",
        show(&out)
    );
    assert_eq!(fixture.evaluations().len(), 4);
    let evidence = fixture.delivered(3);
    assert!(
        evidence.contains("DONE_WITH_NONCLEAN_ROUND"),
        "the next evaluator must receive the rejected DONE discrepancy: {evidence}"
    );
}

#[test]
fn evaluator_receives_collection_errors_actions_and_wait_history() {
    let fixture = LoopFixture::new();
    let out = fixture.run(
        "merge-ready",
        &[
            round("NOT_MERGE_READY", "unreadable", "CONTINUE"),
            round("MERGE_READY", "GREEN:0/2", "DONE"),
        ],
        &[],
    );
    assert!(out.status.success(), "{}", show(&out));
    let delivered = fixture.delivered(2);
    for required in [
        "RATE_LIMITED",
        "GRAPHQL",
        "403",
        "waiting_history",
        "elapsed_ms",
        "WAIT",
    ] {
        assert!(
            delivered.contains(required),
            "missing {required} in delivered evidence: {delivered}"
        );
    }
}

#[test]
fn clean_round_still_requires_evaluator_done() {
    let fixture = LoopFixture::new();
    let out = fixture.run(
        "merge-ready",
        &[
            round("MERGE_READY", "GREEN:0/2", "CONTINUE"),
            round("MERGE_READY", "GREEN:0/2", "DONE"),
        ],
        &[],
    );
    assert!(out.status.success(), "{}", show(&out));
    assert_eq!(fixture.evaluations(), ["round-1", "round-2"]);
}

#[test]
fn malformed_evaluator_output_is_an_evaluation_error_not_stuck() {
    let fixture = LoopFixture::new();
    let out = fixture.run(
        "merge-ready",
        &[round("NOT_MERGE_READY", "PENDING:1/2", "MALFORMED")],
        &[],
    );
    assert!(!out.status.success());
    assert_eq!(
        fixture.result("merge-ready")["loop_result"],
        "EVALUATOR_INVALID",
        "{}",
        show(&out)
    );
}

#[test]
fn evaluator_subprocess_failure_is_distinct_from_agent_stuck() {
    let fixture = LoopFixture::new();
    let out = fixture.run("crusty", &[round("CONCERNS", "", "FAILED")], &[]);
    assert!(!out.status.success());
    assert_eq!(
        fixture.result("crusty")["loop_result"],
        "EVALUATION_FAILED",
        "{}",
        show(&out)
    );
}

#[test]
fn operator_budget_still_stops_external_waiting_as_a_distinct_execution_control() {
    let fixture = LoopFixture::new();
    let out = fixture.run(
        "merge-ready",
        &[
            round("NOT_MERGE_READY", "PENDING:1/2", "CONTINUE"),
            round("NOT_MERGE_READY", "PENDING:1/2", "CONTINUE"),
            round("MERGE_READY", "GREEN:0/2", "DONE"),
        ],
        &["--context", "autodrive_budget_loop_rounds=2"],
    );
    assert!(!out.status.success());
    assert_eq!(
        fixture.result("merge-ready")["loop_result"],
        "BUDGET_EXHAUSTED",
        "{}",
        show(&out)
    );
    assert!(!fixture.evaluations().contains(&"round-3".to_owned()));
}

#[test]
fn policy_refusal_keeps_exit_79_and_never_becomes_agent_stuck() {
    let fixture = LoopFixture::new();
    let out = fixture.run(
        "merge-ready",
        &[round("NOT_MERGE_READY", "PENDING:1/2", "POLICY_REFUSED")],
        &[],
    );
    assert_eq!(out.status.code(), Some(79), "{}", show(&out));
    assert_eq!(
        fixture.result("merge-ready")["loop_result"],
        "TERMINAL_POLICY_REFUSAL"
    );
}

#[test]
fn readiness_event_observation_reaches_the_next_evaluation() {
    let fixture = LoopFixture::new();
    let mut event_round = round("MERGE_READY", "GREEN:0/2", "DONE");
    event_round["record"]["waiting_history"] = json!([{
        "wake_reason": "EVENT", "elapsed_ms": 1,
        "event_ref": {"artifact_id": "ci-completed-7", "sha256": "a".repeat(64)}
    }]);
    let out = fixture.run(
        "merge-ready",
        &[
            round("NOT_MERGE_READY", "PENDING:1/2", "CONTINUE"),
            event_round,
        ],
        &[],
    );
    assert!(out.status.success(), "{}", show(&out));
    assert!(
        fixture.delivered(2).contains("ci-completed-7"),
        "event wake and fresh CI observation must reach loop-health"
    );
}

#[test]
fn real_evaluator_preserves_the_agents_selected_wait_action() {
    crate::support::quiet_process::in_quiet_process(
        module_path!(),
        "real_evaluator_preserves_the_agents_selected_wait_action",
        || {
            let selected = json!({"kind": "WAIT", "reason": "CI is still running",
            "request_ref": {"artifact_id": "agent-selected-wait", "sha256": "a".repeat(64)}});
            let answer = json!({"loop_verdict": "CONTINUE", "not_converging": [],
            "moved": [], "recommended_action": "wait for CI", "next_action": selected});
            let (result, prompts) = agent::evaluate(&answer.to_string(), "0");
            assert_eq!(
                prompts.len(),
                1,
                "the real evaluator recipe must invoke its agent once: {result:?}"
            );
            assert!(prompts[0].contains("RATE_LIMITED"));
            assert!(prompts[0].contains("waiting_history"));
            assert!(result.success, "{result:?}");
            assert_eq!(result.context["loop_health"]["loop_verdict"], "CONTINUE");
            assert_eq!(
                result.context["loop_health"]["next_action"], selected,
                "the real resolution step must preserve the agent's action"
            );
        },
    );
}

#[test]
fn real_evaluator_does_not_normalise_a_malformed_agent_answer_to_stuck() {
    crate::support::quiet_process::in_quiet_process(
        module_path!(),
        "real_evaluator_does_not_normalise_a_malformed_agent_answer_to_stuck",
        || {
            let (result, prompts) = agent::evaluate("{truncated agent response", "0");
            assert_eq!(prompts.len(), 1, "{result:?}");
            let health = result
                .context
                .get("loop_health")
                .cloned()
                .unwrap_or_default();
            assert_ne!(
                health["loop_verdict"], "STUCK",
                "the actual recipe resolver manufactured an agent judgment: {result:?}"
            );
            assert!(
                serde_json::to_string(&result)
                    .unwrap()
                    .contains("EVALUATOR_INVALID"),
                "malformed evaluation must be reported distinctly: {result:?}"
            );
        },
    );
}

#[test]
fn real_evaluator_retains_an_explicit_stuck_judgment_and_its_evidence() {
    crate::support::quiet_process::in_quiet_process(
        module_path!(),
        "real_evaluator_retains_an_explicit_stuck_judgment_and_its_evidence",
        || {
            let reasons =
                json!(["required permission is unavailable; operator intervention is needed"]);
            let answer = json!({"loop_verdict": "STUCK", "not_converging": reasons,
                "moved": [], "recommended_action": "ask operator to restore permission",
                "next_action": {"kind": "STOP", "reason": "missing permission", "request_ref": null}});
            let (result, prompts) = agent::evaluate(&answer.to_string(), "0");
            assert_eq!(prompts.len(), 1, "{result:?}");
            assert!(!result.success, "STUCK must stop the caller");
            assert_eq!(result.context["loop_health"]["loop_verdict"], "STUCK");
            assert_eq!(result.context["loop_health"]["verdict_source"], "evaluator");
            assert_eq!(result.context["loop_health"]["not_converging"], reasons);
        },
    );
}
