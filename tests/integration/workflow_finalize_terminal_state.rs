//! tests/integration/workflow_finalize_terminal_state.rs
//!
//! TDD-red contracts for finalization as the loud terminal-state arbiter.
//! Terminal success may complete the workflow, but dirty work, CI failures,
//! closed-unmerged PRs, and meaningful unmerged diffs must remain blockers.

use std::fs;
use std::path::PathBuf;

use serde_yaml::Value;

fn workspace_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path
}

fn load_finalize_recipe() -> Value {
    let path = workspace_root()
        .join("amplifier-bundle")
        .join("recipes")
        .join("workflow-finalize.yaml");
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

fn steps(recipe: &Value) -> &[Value] {
    recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .expect("workflow-finalize must contain top-level steps")
}

fn step_index(recipe: &Value, id: &str) -> usize {
    steps(recipe)
        .iter()
        .position(|step| step.get("id").and_then(Value::as_str) == Some(id))
        .unwrap_or_else(|| panic!("workflow-finalize missing step `{id}`"))
}

fn find_terminal_probe_index(recipe: &Value) -> usize {
    steps(recipe)
        .iter()
        .position(|step| step.get("recipe").and_then(Value::as_str) == Some("workflow-terminal-state"))
        .expect("workflow-finalize must re-run workflow-terminal-state before mutation, CI, or merge steps")
}

fn step_condition<'a>(recipe: &'a Value, id: &str) -> &'a str {
    steps(recipe)
        .iter()
        .find(|step| step.get("id").and_then(Value::as_str) == Some(id))
        .and_then(|step| step.get("condition").and_then(Value::as_str))
        .unwrap_or_else(|| panic!("workflow-finalize step `{id}` must declare a condition"))
}

fn step_command(recipe: &Value, id: &str) -> String {
    steps(recipe)
        .iter()
        .find(|step| step.get("id").and_then(Value::as_str) == Some(id))
        .and_then(|step| step.get("command").and_then(Value::as_str))
        .unwrap_or_else(|| panic!("workflow-finalize step `{id}` must be a bash command step"))
        .to_owned()
}

fn step_by_id<'a>(recipe: &'a Value, id: &str) -> &'a Value {
    steps(recipe)
        .iter()
        .find(|step| step.get("id").and_then(Value::as_str) == Some(id))
        .unwrap_or_else(|| panic!("workflow-finalize missing step `{id}`"))
}

fn step_type<'a>(recipe: &'a Value, id: &str) -> &'a str {
    step_by_id(recipe, id)
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("")
}

fn step_output<'a>(recipe: &'a Value, id: &str) -> &'a str {
    step_by_id(recipe, id)
        .get("output")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("workflow-finalize step `{id}` must declare output"))
}

fn step_prompt(recipe: &Value, id: &str) -> String {
    step_by_id(recipe, id)
        .get("prompt")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("workflow-finalize step `{id}` must declare a prompt"))
        .to_owned()
}

fn recipe_text(recipe: &Value) -> String {
    serde_yaml::to_string(recipe).expect("serialize workflow-finalize")
}

#[test]
fn finalize_rechecks_terminal_state_before_mutation_ci_or_merge_steps() {
    let recipe = load_finalize_recipe();
    let terminal_probe_index = find_terminal_probe_index(&recipe);

    for guarded_step in [
        "step-20b-push-cleanup",
        "step-21-pr-ready",
        "step-22-ensure-mergeable",
    ] {
        let guarded_index = step_index(&recipe, guarded_step);
        assert!(
            terminal_probe_index < guarded_index,
            "workflow-finalize must re-check terminal state before `{guarded_step}`"
        );
    }
}

#[test]
fn finalize_mutation_and_ci_steps_are_suppressed_for_terminal_success() {
    let recipe = load_finalize_recipe();

    for guarded_step in [
        "step-20b-push-cleanup",
        "step-21-pr-ready",
        "step-22-ensure-mergeable",
    ] {
        let condition = step_condition(&recipe, guarded_step);
        assert!(
            condition.contains("terminal_success != 'true'")
                || condition.contains("terminal_state.terminal_success != 'true'")
                || condition.contains("should_finalize == 'true'")
                || condition.contains("should_run_ci_wait == 'true'"),
            "`{guarded_step}` must be skipped when terminal_success=true; condition was `{condition}`"
        );
    }
}

#[test]
fn finalize_preserves_loud_blockers_instead_of_converting_them_to_success() {
    let recipe = load_finalize_recipe();
    let text = recipe_text(&recipe);

    for blocker in [
        "FAILED_CLOSED_UNMERGED",
        "FAILED_MEANINGFUL_DIFF",
        "BLOCKED_CI",
        "exit 1",
    ] {
        assert!(
            text.contains(blocker),
            "workflow-finalize must preserve loud blocker `{blocker}`"
        );
    }
}

#[test]
fn final_status_reports_terminal_success_and_blocked_semantics_structurally() {
    let recipe = load_finalize_recipe();
    let command = step_command(&recipe, "workflow-complete");

    for required in [
        "terminal_state",
        "terminal_success",
        "MERGED",
        "CLOSED_OBSOLETE",
        "NO_DIFF_SUCCESS",
        "FOLLOWUP_CREATED",
        "BLOCKED_CI",
    ] {
        assert!(
            command.contains(required),
            "workflow-complete JSON must expose `{required}`"
        );
    }

    assert!(
        !command.contains("status: \"complete\"") || command.contains("terminal_success"),
        "workflow-complete must not unconditionally report complete without terminal_success evidence"
    );
}

#[test]
fn finalize_pipeline_boxes_judgment_in_agentic_finalizer_and_keeps_validation_deterministic() {
    let recipe = load_finalize_recipe();

    assert_eq!(
        step_type(&recipe, "collect-finalization-evidence"),
        "bash",
        "workflow-finalize must collect structured evidence deterministically before agent judgment"
    );
    assert_eq!(
        step_output(&recipe, "collect-finalization-evidence"),
        "finalization_evidence",
        "evidence collection must persist a structured finalization_evidence object"
    );
    assert!(
        step_by_id(&recipe, "agentic-finalizer")
            .get("agent")
            .and_then(Value::as_str)
            .is_some(),
        "workflow-finalize must use an agentic finalizer for the human-readable narrative"
    );
    assert_eq!(
        step_output(&recipe, "agentic-finalizer"),
        "agentic_finalizer_narrative",
        "the agentic finalizer output is a human-readable narrative artifact, never machine-control state"
    );
    assert_eq!(
        step_type(&recipe, "finalizer-step-status"),
        "bash",
        "a deterministic step must record the reporting step's typed status"
    );
    assert_eq!(
        step_output(&recipe, "finalizer-step-status"),
        "finalizer_step_status",
        "finalizer-step-status must persist typed {{status, reporting_failure}} state"
    );
    assert_eq!(
        step_by_id(&recipe, "finalizer-step-status")
            .get("parse_json")
            .and_then(Value::as_bool),
        Some(true),
        "finalizer-step-status must emit typed JSON parsed into recipe state"
    );
    assert_eq!(
        step_type(&recipe, "validate-agentic-finalization"),
        "bash",
        "terminal-state classification must remain deterministic"
    );
    assert_eq!(
        step_output(&recipe, "validate-agentic-finalization"),
        "workflow_result",
        "deterministic validation must be the only writer of canonical workflow_result"
    );

    assert!(
        step_index(&recipe, "collect-finalization-evidence")
            < step_index(&recipe, "agentic-finalizer")
            && step_index(&recipe, "agentic-finalizer")
                < step_index(&recipe, "finalizer-step-status")
            && step_index(&recipe, "finalizer-step-status")
                < step_index(&recipe, "validate-agentic-finalization")
            && step_index(&recipe, "validate-agentic-finalization")
                < step_index(&recipe, "workflow-complete"),
        "finalize pipeline order must be evidence collection -> agentic narrative -> typed step status -> deterministic validation -> workflow-complete"
    );
}

#[test]
fn agentic_finalizer_prompt_requests_human_narrative_not_machine_json() {
    let recipe = load_finalize_recipe();
    let prompt = step_prompt(&recipe, "agentic-finalizer");

    // Issue #969: the finalizer emits a free-form narrative for humans. The
    // brittle "one JSON object / no prose" machine contract must be gone.
    for forbidden in [
        "single JSON object",
        "no prose",
        "Return only the single JSON object",
        "Output schema",
    ] {
        assert!(
            !prompt.contains(forbidden),
            "agentic finalizer prompt must not demand a machine-parseable JSON blob; found `{forbidden}`"
        );
    }

    for required in ["narrative", "structured evidence"] {
        assert!(
            prompt.contains(required),
            "agentic finalizer prompt must request a human-readable narrative from structured evidence; missing `{required}`"
        );
    }

    // Observed failure-mode context is still valuable operator background.
    for failure_mode in [
        "dirty worktree",
        "closed-unmerged",
        "missing tooling",
        "failed CI",
        "hollow success",
    ] {
        assert!(
            prompt.contains(failure_mode),
            "agentic finalizer prompt should preserve observed failure-mode context `{failure_mode}`"
        );
    }
}

#[test]
fn validation_step_classifies_from_typed_evidence_not_agent_prose() {
    let recipe = load_finalize_recipe();
    let command = step_command(&recipe, "validate-agentic-finalization");

    // The classifier must never touch agent narrative or the retired brittle state.
    for forbidden in [
        "AGENTIC_FINALIZER_OUTPUT",
        "agentic_finalizer_output",
        "FAILED_FINALIZER_OUTPUT",
        "single JSON object",
    ] {
        assert!(
            !command.contains(forbidden),
            "validation step must not consume agent prose via `{forbidden}`"
        );
    }

    // It must classify from typed deterministic evidence and typed markers.
    for required in [
        "finalization_evidence",
        "finalizer_step_status",
        "FAILED_INVALID_EVIDENCE",
        "FAILED_REPORTING",
        "FAILED_IMPLEMENTATION",
        "exit 1",
    ] {
        assert!(
            command.contains(required),
            "validation step must classify from typed evidence; missing `{required}`"
        );
    }

    // Preserved fail-closed invariants.
    for invariant in [
        "FAILED_DIRTY_WORKTREE",
        "FAILED_PR_METADATA_UNAVAILABLE",
        "BLOCKED_CI",
        "FAILED_MEANINGFUL_DIFF",
    ] {
        assert!(
            command.contains(invariant),
            "validation step must enforce terminal invariant `{invariant}`"
        );
    }

    assert!(
        !command.contains("|| true") && !command.contains("status: \"complete\""),
        "validation step must not mask a non-success terminal state as successful completion"
    );
}

#[test]
fn workflow_complete_reports_canonical_agentic_workflow_result_fields_and_failure_vocabulary() {
    let recipe = load_finalize_recipe();
    let command = step_command(&recipe, "workflow-complete");

    for required in [
        "workflow_result",
        "required_next_action",
        "hollow_success_detected",
        "evidence_used",
        "reporting_failure",
        "finalizer_schema_version",
        "finalizer_confidence",
        "finalizer_output_valid",
        "terminal_failure",
        "FAILED_INVALID_EVIDENCE",
        "FAILED_REPORTING",
        "FAILED_IMPLEMENTATION",
        "FAILED_MISSING_TOOLING",
        "FAILED_PR_METADATA_UNAVAILABLE",
        "FAILED_DIRTY_WORKTREE",
        "FAILED_MEANINGFUL_DIFF",
        "HOLLOW_SUCCESS",
        "INCOMPLETE",
        "SUPERSEDED",
    ] {
        assert!(
            command.contains(required),
            "workflow-complete must report canonical agentic workflow_result field/state `{required}`"
        );
    }

    assert!(
        !command.contains("FAILED_FINALIZER_OUTPUT"),
        "workflow-complete must retire the brittle FAILED_FINALIZER_OUTPUT state"
    );
    assert!(
        !command.contains("status: \"complete\"") && !command.contains("steps_completed: 23"),
        "workflow-complete must not report legacy unconditional completion once finalizer validation owns terminal status"
    );
}
