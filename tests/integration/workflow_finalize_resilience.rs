//! TDD-red contracts for workflow finalization idempotency.
//!
//! Finalization must treat already terminal successful states as success while
//! still surfacing active CI failures and closed-unmerged PRs as actionable
//! failures.

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

fn step_command(recipe: &Value, id: &str) -> String {
    recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .expect("recipe must contain top-level steps")
        .iter()
        .find(|step| step.get("id").and_then(Value::as_str) == Some(id))
        .and_then(|step| step.get("command").and_then(Value::as_str))
        .unwrap_or_else(|| panic!("step {id} must be a bash command step"))
        .to_owned()
}

#[test]
fn ready_step_distinguishes_merged_closed_after_merge_and_closed_unmerged() {
    let recipe = load_finalize_recipe();
    let command = step_command(&recipe, "step-21-pr-ready");

    for required in [
        "mergedAt",
        "closed-after-merge",
        "closed-unmerged",
        "terminal_status",
        "exit 1",
    ] {
        assert!(
            command.contains(required),
            "step-21 must classify merged/closed PR states explicitly; missing `{required}`"
        );
    }

    assert!(
        !command.contains("PR is closed — no ready-for-review action possible")
            && !command.contains("PR is closed - no ready-for-review action possible"),
        "closed-unmerged PRs must not be silently skipped as successful finalization"
    );
}

#[test]
fn final_status_accepts_no_diff_and_already_merged_as_successful_terminal_outcomes() {
    let recipe = load_finalize_recipe();
    let command = step_command(&recipe, "step-22b-final-status");

    for required in [
        "git diff --quiet",
        "no-diff",
        "already-merged",
        "closed-after-merge",
        "terminal_status",
    ] {
        assert!(
            command.contains(required),
            "final status must report idempotent successful terminal outcomes; missing `{required}`"
        );
    }
}

#[test]
fn workflow_complete_json_reports_terminal_outcome_instead_of_unconditional_merge_ready() {
    let recipe = load_finalize_recipe();
    let command = step_command(&recipe, "workflow-complete");

    assert!(
        command.contains("terminal_outcome"),
        "workflow-complete JSON must include the terminal outcome that made finalization succeed"
    );
    assert!(
        !command.contains("ready_to_merge: true"),
        "workflow-complete must not unconditionally claim merge readiness for no-diff or already-merged terminal states"
    );
}
