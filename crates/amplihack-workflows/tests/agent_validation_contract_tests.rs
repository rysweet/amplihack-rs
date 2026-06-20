//! TDD contract tests for agentic finalizer validation.
//!
//! Judgment-heavy workflow steps stay agentic, but their outputs must validate
//! through deterministic contracts.

use amplihack_workflows::agent_contract::{
    AgentContractError, AgentFinalizerOutput, FinalizerConfidence, validate_finalizer_output,
};
use amplihack_workflows::workflow_contract::TerminalState;
use serde_json::json;

fn automated_github_evidence() -> serde_json::Value {
    json!({
        "provider": "GitHub",
        "capabilities": {
            "tracking_items": "Automated",
            "change_requests": "Automated",
            "stale_cleanup": "Automated"
        }
    })
}

#[test]
fn high_confidence_followup_output_with_evidence_is_valid_success() {
    let mut value = automated_github_evidence();
    let object = value.as_object_mut().unwrap();
    object.extend(
        json!({
        "schema_version": 1,
        "terminal_state": "FOLLOWUP_CREATED",
        "terminal_success": true,
        "confidence": "high",
        "reason": "Workflow published PR #812 with verification evidence.",
        "required_next_action": "Wait for review and required checks.",
        "hollow_success_detected": false,
        "evidence_used": [
            "change_request.id=812",
            "verification.completed=true"
        ]
        })
        .as_object()
        .unwrap()
        .clone(),
    );
    let output = validate_finalizer_output(value)
        .expect("valid finalizer output must pass deterministic validation");

    assert_eq!(output.terminal_state, TerminalState::FollowupCreated);
    assert!(output.terminal_success);
    assert_eq!(output.confidence, FinalizerConfidence::High);
}

#[test]
fn medium_confidence_success_claim_fails_closed() {
    let mut value = automated_github_evidence();
    let object = value.as_object_mut().unwrap();
    object.extend(
        json!({
        "schema_version": 1,
        "terminal_state": "FOLLOWUP_CREATED",
        "terminal_success": true,
        "confidence": "medium",
        "reason": "Probably published.",
        "required_next_action": "Maybe wait for review.",
        "hollow_success_detected": false,
        "evidence_used": ["change_request.id=812"]
        })
        .as_object()
        .unwrap()
        .clone(),
    );
    let error = validate_finalizer_output(value)
        .expect_err("only high confidence can prove terminal success");

    assert_eq!(error, AgentContractError::SuccessRequiresHighConfidence);
}

#[test]
fn non_json_or_missing_fields_become_failed_finalizer_output() {
    let error = AgentFinalizerOutput::from_agent_text("looks good to me")
        .expect_err("unstructured finalizer prose must fail closed");

    assert_eq!(error.terminal_state(), TerminalState::FailedFinalizerOutput);
}

#[test]
fn hollow_success_signal_cannot_be_reported_as_success() {
    let mut value = automated_github_evidence();
    let object = value.as_object_mut().unwrap();
    object.extend(
        json!({
        "schema_version": 1,
        "terminal_state": "HOLLOW_SUCCESS",
        "terminal_success": true,
        "confidence": "high",
        "reason": "Agent reported success but no implementation or publication evidence exists.",
        "required_next_action": "Retry with concrete implementation evidence.",
        "hollow_success_detected": true,
        "evidence_used": ["agent.output=success-without-artifacts"]
        })
        .as_object()
        .unwrap()
        .clone(),
    );
    let error =
        validate_finalizer_output(value).expect_err("HOLLOW_SUCCESS is never a success state");

    assert_eq!(error.terminal_state(), TerminalState::FailedInvalidEvidence);
}

#[test]
fn successful_finalizer_output_requires_evidence() {
    let mut value = automated_github_evidence();
    let object = value.as_object_mut().unwrap();
    object.extend(
        json!({
        "schema_version": 1,
        "terminal_state": "FOLLOWUP_CREATED",
        "terminal_success": true,
        "confidence": "high",
        "reason": "Workflow claims publication succeeded.",
        "required_next_action": "Monitor PR validation.",
        "hollow_success_detected": false,
        "evidence_used": []
        })
        .as_object()
        .unwrap()
        .clone(),
    );
    let error = validate_finalizer_output(value)
        .expect_err("terminal success without evidence is a hollow success claim");

    assert_eq!(error.terminal_state(), TerminalState::FailedInvalidEvidence);
}

#[test]
fn successful_finalizer_output_requires_provider_capability_evidence() {
    let error = validate_finalizer_output(json!({
        "schema_version": 1,
        "terminal_state": "FOLLOWUP_CREATED",
        "terminal_success": true,
        "confidence": "high",
        "reason": "Workflow published PR #812.",
        "required_next_action": "Monitor PR validation.",
        "hollow_success_detected": false,
        "evidence_used": ["change_request.id=812"]
    }))
    .expect_err("success without provider/capabilities evidence must fail closed");

    assert_eq!(error.terminal_state(), TerminalState::FailedFinalizerOutput);
}

#[test]
fn manual_provider_success_claim_fails_closed() {
    let error = validate_finalizer_output(json!({
        "schema_version": 1,
        "provider": "Manual",
        "capabilities": {
            "tracking_items": "ManualRequired",
            "change_requests": "ManualRequired",
            "stale_cleanup": "ManualRequired"
        },
        "terminal_state": "FOLLOWUP_CREATED",
        "terminal_success": true,
        "confidence": "high",
        "reason": "Manual provider claimed a follow-up was created.",
        "required_next_action": "Create the change request manually.",
        "hollow_success_detected": false,
        "evidence_used": ["provider=Manual", "change_requests=ManualRequired"]
    }))
    .expect_err("manual providers cannot claim automated terminal success");

    assert_eq!(error.terminal_state(), TerminalState::FailedInvalidEvidence);
}
