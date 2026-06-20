//! TDD contract tests for agentic finalizer validation.
//!
//! Judgment-heavy workflow steps stay agentic, but their outputs must validate
//! through deterministic contracts.

use amplihack_workflows::agent_contract::{
    AgentContractError, AgentFinalizerOutput, FinalizerConfidence, validate_finalizer_output,
};
use amplihack_workflows::workflow_contract::TerminalState;
use serde_json::json;

#[test]
fn high_confidence_followup_output_with_evidence_is_valid_success() {
    let output = validate_finalizer_output(json!({
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
    }))
    .expect("valid finalizer output must pass deterministic validation");

    assert_eq!(output.terminal_state, TerminalState::FollowupCreated);
    assert!(output.terminal_success);
    assert_eq!(output.confidence, FinalizerConfidence::High);
}

#[test]
fn medium_confidence_success_claim_fails_closed() {
    let error = validate_finalizer_output(json!({
        "schema_version": 1,
        "terminal_state": "FOLLOWUP_CREATED",
        "terminal_success": true,
        "confidence": "medium",
        "reason": "Probably published.",
        "required_next_action": "Maybe wait for review.",
        "hollow_success_detected": false,
        "evidence_used": ["change_request.id=812"]
    }))
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
    let error = validate_finalizer_output(json!({
        "schema_version": 1,
        "terminal_state": "HOLLOW_SUCCESS",
        "terminal_success": true,
        "confidence": "high",
        "reason": "Agent reported success but no implementation or publication evidence exists.",
        "required_next_action": "Retry with concrete implementation evidence.",
        "hollow_success_detected": true,
        "evidence_used": []
    }))
    .expect_err("HOLLOW_SUCCESS is never a success state");

    assert_eq!(error.terminal_state(), TerminalState::FailedInvalidEvidence);
}

#[test]
fn successful_finalizer_output_requires_evidence() {
    let error = validate_finalizer_output(json!({
        "schema_version": 1,
        "terminal_state": "FOLLOWUP_CREATED",
        "terminal_success": true,
        "confidence": "high",
        "reason": "Workflow claims publication succeeded.",
        "required_next_action": "Monitor PR validation.",
        "hollow_success_detected": false,
        "evidence_used": []
    }))
    .expect_err("terminal success without evidence is a hollow success claim");

    assert_eq!(error.terminal_state(), TerminalState::FailedInvalidEvidence);
}
