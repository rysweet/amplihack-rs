//! tests/integration/workflow_publish_terminal_gate.rs
//!
//! TDD-red contracts for suppressing publish mutations after proven terminal
//! success states such as MERGED, CLOSED_OBSOLETE, and NO_DIFF_SUCCESS.

use std::fs;
use std::path::PathBuf;

use serde_yaml::Value;

fn workspace_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path
}

fn load_publish_recipe() -> Value {
    let path = workspace_root()
        .join("amplifier-bundle")
        .join("recipes")
        .join("workflow-publish.yaml");
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

fn steps(recipe: &Value) -> &[Value] {
    recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .expect("workflow-publish must contain top-level steps")
}

fn step_index(recipe: &Value, id: &str) -> usize {
    steps(recipe)
        .iter()
        .position(|step| step.get("id").and_then(Value::as_str) == Some(id))
        .unwrap_or_else(|| panic!("workflow-publish missing step `{id}`"))
}

fn find_terminal_probe_index(recipe: &Value) -> usize {
    steps(recipe)
        .iter()
        .position(|step| {
            step.get("recipe").and_then(Value::as_str) == Some("workflow-terminal-state")
        })
        .expect("workflow-publish must invoke workflow-terminal-state before mutation steps")
}

fn step_condition<'a>(recipe: &'a Value, id: &str) -> &'a str {
    steps(recipe)
        .iter()
        .find(|step| step.get("id").and_then(Value::as_str) == Some(id))
        .and_then(|step| step.get("condition").and_then(Value::as_str))
        .unwrap_or_else(|| panic!("workflow-publish step `{id}` must declare a condition"))
}

fn recipe_text(recipe: &Value) -> String {
    serde_yaml::to_string(recipe).expect("serialize workflow-publish")
}

#[test]
fn publish_runs_terminal_state_probe_before_any_mutation_or_publish_step() {
    let recipe = load_publish_recipe();
    let terminal_probe_index = find_terminal_probe_index(&recipe);

    for mutation_step in [
        "step-14-bump-version",
        "step-15-commit-push",
        "step-16-create-draft-pr",
        "step-16b-outside-in-fix-loop",
    ] {
        let mutation_index = step_index(&recipe, mutation_step);
        assert!(
            terminal_probe_index < mutation_index,
            "workflow-terminal-state must run before `{mutation_step}`"
        );
    }
}

#[test]
fn publish_mutation_steps_are_suppressed_when_terminal_success_is_true() {
    let recipe = load_publish_recipe();

    for mutation_step in [
        "step-14-bump-version",
        "step-15-commit-push",
        "step-16-create-draft-pr",
        "step-16b-outside-in-fix-loop",
    ] {
        let condition = step_condition(&recipe, mutation_step);
        assert!(
            condition.contains("terminal_success != 'true'")
                || condition.contains("terminal_state.terminal_success != 'true'")
                || condition.contains("should_publish == 'true'")
                || condition.contains("terminal_state.should_publish == 'true'"),
            "`{mutation_step}` must skip version/commit/push/PR work when terminal_success=true; condition was `{condition}`"
        );
    }
}

#[test]
fn publish_uses_required_terminal_and_followup_status_vocabulary() {
    let recipe = load_publish_recipe();
    let text = recipe_text(&recipe);

    for status in [
        "MERGED",
        "CLOSED_OBSOLETE",
        "NO_DIFF_SUCCESS",
        "FOLLOWUP_CREATED",
        "BLOCKED_CI",
    ] {
        assert!(
            text.contains(status),
            "workflow-publish must preserve required status `{status}`"
        );
    }
}

#[test]
fn publish_terminal_success_does_not_wait_for_ci_or_attempt_merge() {
    let recipe = load_publish_recipe();
    let text = recipe_text(&recipe);

    assert!(
        text.contains("should_run_ci_wait") && text.contains("should_merge"),
        "publish must carry terminal-state outputs that suppress CI wait and merge"
    );
    assert!(
        !text.contains("gh pr merge") || text.contains("should_merge == 'true'"),
        "publish must not attempt PR merge without an explicit should_merge=true gate"
    );
    assert!(
        !text.contains("ci-diagnostic-workflow") || text.contains("should_run_ci_wait == 'true'"),
        "publish must not wait on CI without an explicit should_run_ci_wait=true gate"
    );
}
