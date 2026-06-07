//! TDD-red contracts for workflow publish idempotency.
//!
//! These tests describe the publish behavior required before implementation:
//! no duplicate PRs, no empty PRs, and clean terminal success for already
//! handled branch states.

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

fn step<'a>(recipe: &'a Value, id: &str) -> &'a Value {
    recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .expect("recipe must contain top-level steps")
        .iter()
        .find(|step| step.get("id").and_then(Value::as_str) == Some(id))
        .unwrap_or_else(|| panic!("missing recipe step {id}"))
}

fn step_command(recipe: &Value, id: &str) -> String {
    step(recipe, id)
        .get("command")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("step {id} must be a bash command step"))
        .to_owned()
}

#[test]
fn publish_step_emits_structured_terminal_outcome_for_idempotent_states() {
    let recipe = load_publish_recipe();
    let publish_step = step(&recipe, "step-16-create-draft-pr");

    assert_eq!(
        publish_step.get("parse_json").and_then(Value::as_bool),
        Some(true),
        "step-16 must emit structured JSON so downstream steps can distinguish created, existing, merged, no-diff, and skipped states"
    );
    assert_eq!(
        publish_step.get("output").and_then(Value::as_str),
        Some("pr_publish_result"),
        "step-16 output must be a structured publish result, not only a raw PR URL"
    );
}

#[test]
fn publish_checks_branch_diff_and_all_pr_states_before_creating_pr() {
    let recipe = load_publish_recipe();
    let command = step_command(&recipe, "step-16-create-draft-pr");

    for required in [
        "--state all",
        "gh pr view",
        "headRefName",
        "headRefOid",
        "mergedAt",
        "git diff --quiet",
        "no-diff",
        "existing-open-pr",
        "already-merged",
        "closed-after-merge",
        "closed-unmerged-with-diff",
    ] {
        assert!(
            command.contains(required),
            "publish must classify branch diff and all PR terminal states before creating a PR; missing `{required}`"
        );
    }
}

#[test]
fn publish_never_invokes_create_for_no_diff_or_existing_pr_states() {
    let recipe = load_publish_recipe();
    let command = step_command(&recipe, "step-16-create-draft-pr");

    let create_position = command
        .rfind("gh_pr_create_with_retry")
        .or_else(|| command.rfind("gh pr create"))
        .expect("publish step must still create PRs for the create-new-pr state");
    let guard_position = command
        .find("PUBLISH_STATE")
        .expect("publish step must compute a PUBLISH_STATE before PR creation");

    assert!(
        guard_position < create_position,
        "PUBLISH_STATE classification must happen before the first gh pr create invocation"
    );

    for terminal in ["no-diff", "existing-open-pr", "already-merged"] {
        assert!(
            command.contains(&format!("PUBLISH_STATE=\"{terminal}\""))
                || command.contains(&format!("PUBLISH_STATE='{terminal}'")),
            "terminal state `{terminal}` must return before PR creation"
        );
    }
}

#[test]
fn publish_treats_non_github_hosts_as_structured_successful_skip() {
    let recipe = load_publish_recipe();
    let command = step_command(&recipe, "step-16-create-draft-pr");

    assert!(
        command.contains("non-github") && command.contains("terminal_status"),
        "non-GitHub hosts must emit a structured successful skip instead of an empty PR URL"
    );
    assert!(
        !command.contains("printf ''"),
        "non-GitHub skip must not be represented as an empty string output"
    );
}
