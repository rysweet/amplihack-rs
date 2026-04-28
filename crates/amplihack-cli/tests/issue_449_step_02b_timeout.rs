//! Tests for issue #449 → #439: step-02b-analyze-codebase timeout_seconds REMOVED.
//!
//! Originally (issue #449) these tests asserted that `timeout_seconds: 1800`
//! was present on step-02b. Issue #439 removes all hard agent-step timeouts
//! from default-workflow recipes. These tests are now inverted to assert
//! that `timeout_seconds` is **absent** from step-02b.

use std::path::PathBuf;

use serde_yaml::Value;

fn workflow_prep_path() -> PathBuf {
    // CARGO_MANIFEST_DIR = crates/amplihack-cli; walk up to repo root.
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .expect("repo root")
        .join("amplifier-bundle/recipes/workflow-prep.yaml")
}

fn load_workflow_prep() -> Value {
    let path = workflow_prep_path();
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

fn find_step<'a>(recipe: &'a Value, step_id: &str) -> &'a Value {
    let steps = recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .expect("workflow-prep.yaml must have a top-level `steps:` sequence");
    steps
        .iter()
        .find(|s| s.get("id").and_then(Value::as_str) == Some(step_id))
        .unwrap_or_else(|| panic!("step `{step_id}` not found in workflow-prep.yaml"))
}

#[test]
fn workflow_prep_yaml_parses() {
    let _ = load_workflow_prep();
}

#[test]
fn step_02b_has_no_timeout_seconds() {
    let recipe = load_workflow_prep();
    let step = find_step(&recipe, "step-02b-analyze-codebase");

    assert!(
        step.get("timeout_seconds").is_none(),
        "issue #439: step-02b-analyze-codebase must NOT have `timeout_seconds` \
         (hard agent-step timeouts removed)"
    );
}

#[test]
fn step_02b_preserves_required_fields() {
    let recipe = load_workflow_prep();
    let step = find_step(&recipe, "step-02b-analyze-codebase");

    assert_eq!(
        step.get("agent").and_then(Value::as_str),
        Some("amplihack:architect"),
        "agent field must remain `amplihack:architect`"
    );
    assert!(
        step.get("prompt").and_then(Value::as_str).is_some(),
        "prompt field must be preserved"
    );
}

#[test]
fn no_step_timeouts_in_workflow_prep() {
    // Issue #439: NO agent steps in workflow-prep.yaml should carry timeout_seconds.
    let recipe = load_workflow_prep();
    let steps = recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .expect("steps sequence");

    let agent_steps_with_timeout: Vec<String> = steps
        .iter()
        .filter(|s| {
            // Only check agent steps (type: "agent" or has "agent:" field)
            s.get("agent").is_some()
        })
        .filter(|s| s.get("timeout_seconds").is_some())
        .map(|s| {
            s.get("id")
                .and_then(Value::as_str)
                .unwrap_or("<unknown>")
                .to_string()
        })
        .collect();

    assert!(
        agent_steps_with_timeout.is_empty(),
        "issue #439: no agent steps in workflow-prep.yaml should have timeout_seconds, \
         but found it on: {agent_steps_with_timeout:?}"
    );
}
