//! Tests for issue #449: step-02b-analyze-codebase needs a 1800s timeout.
//!
//! These tests are intentionally written before the fix lands (TDD).
//! They assert that `amplifier-bundle/recipes/workflow-prep.yaml` declares
//! `timeout_seconds: 1800` on the `step-02b-analyze-codebase` step, placed
//! directly under `agent:`, and that the recipe still parses cleanly.

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
fn step_02b_has_timeout_seconds_1800() {
    let recipe = load_workflow_prep();
    let step = find_step(&recipe, "step-02b-analyze-codebase");

    let timeout = step
        .get("timeout_seconds")
        .unwrap_or_else(|| panic!("step-02b-analyze-codebase is missing `timeout_seconds`"));

    let n = timeout
        .as_u64()
        .unwrap_or_else(|| panic!("timeout_seconds must be an integer, got {timeout:?}"));

    assert_eq!(
        n, 1800,
        "issue #449: step-02b-analyze-codebase timeout_seconds must be 1800 (30 min), got {n}"
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
fn timeout_seconds_appears_directly_under_agent_field() {
    // Convention check: in the raw YAML text, `timeout_seconds:` for this step
    // must appear on the line immediately following the `agent:` line, matching
    // sibling recipes (smart-classify-route.yaml, smart-reflect-loop.yaml).
    let text = std::fs::read_to_string(workflow_prep_path()).expect("read workflow-prep.yaml");

    let lines: Vec<&str> = text.lines().collect();
    let id_idx = lines
        .iter()
        .position(|l| l.contains(r#"id: "step-02b-analyze-codebase""#))
        .expect("step-02b id line");

    // Find the `agent:` line within the next few lines after the id.
    let agent_idx = (id_idx + 1..(id_idx + 6).min(lines.len()))
        .find(|&i| lines[i].trim_start().starts_with("agent:"))
        .expect("agent: line within step-02b");

    let next = lines
        .get(agent_idx + 1)
        .expect("line after agent: must exist")
        .trim_start();

    assert!(
        next.starts_with("timeout_seconds:"),
        "expected `timeout_seconds:` directly under `agent:`, found: {next:?}"
    );
}

#[test]
fn no_other_step_timeouts_changed_in_workflow_prep() {
    // Guardrail: only step-02b should carry timeout_seconds in workflow-prep.yaml
    // as part of issue #449. If future work intentionally adds more, update this test.
    let recipe = load_workflow_prep();
    let steps = recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .expect("steps sequence");

    let with_timeout: Vec<String> = steps
        .iter()
        .filter(|s| s.get("timeout_seconds").is_some())
        .map(|s| {
            s.get("id")
                .and_then(Value::as_str)
                .unwrap_or("<unknown>")
                .to_string()
        })
        .collect();

    assert_eq!(
        with_timeout,
        vec!["step-02b-analyze-codebase".to_string()],
        "issue #449 scope: only step-02b-analyze-codebase should declare timeout_seconds in workflow-prep.yaml"
    );
}
