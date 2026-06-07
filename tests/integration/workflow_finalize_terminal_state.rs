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
        "FAILED_DIRTY_WORKTREE",
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
