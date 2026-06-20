//! TDD tests for generated workflow requirement sanitization (#801).
//!
//! Saved preference notices from launcher startup are process noise, not user
//! requirements. The workflow must filter only those exact notice lines while
//! preserving legitimate `NODE_OPTIONS` task requirements and strict guards.

use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
struct Recipe {
    #[serde(default)]
    steps: Vec<Step>,
}

#[derive(Debug, Deserialize)]
struct Step {
    id: String,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    prompt: Option<String>,
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn workflow_prep_path() -> PathBuf {
    workspace_root()
        .join("amplifier-bundle")
        .join("recipes")
        .join("workflow-prep.yaml")
}

fn workflow_prep() -> Recipe {
    let path = workflow_prep_path();
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

fn step(id: &str) -> Step {
    workflow_prep()
        .steps
        .into_iter()
        .find(|step| step.id == id)
        .unwrap_or_else(|| panic!("workflow-prep.yaml missing step {id}"))
}

#[test]
fn workflow_prep_defines_deterministic_saved_preference_notice_sanitizer() {
    let raw = std::fs::read_to_string(workflow_prep_path()).expect("read workflow-prep");
    assert!(
        raw.contains("sanitize") && raw.contains("saved preference"),
        "#801 regression: workflow-prep must define an explicit saved-preference notice sanitizer"
    );
    assert!(
        raw.contains("NODE_OPTIONS=--max-old-space-size="),
        "#801 regression: sanitizer must target the exact NODE_OPTIONS saved-preference notice shape"
    );
    assert!(
        raw.contains("(saved preference). To change:") || raw.contains("(saved preference)"),
        "#801 regression: sanitizer must key on the launcher notice suffix, not broad NODE_OPTIONS mentions"
    );
}

#[test]
fn issue_creation_uses_sanitized_requirements_not_raw_agent_output() {
    let create_issue = step("step-03-create-issue")
        .command
        .expect("create issue must be bash");
    assert!(
        create_issue.contains("SANITIZED") || create_issue.contains("sanitize"),
        "#801 regression: generated issue requirements must pass through the sanitizer"
    );
    assert!(
        !create_issue.contains(r#""$ISSUE_REQS""#) || create_issue.contains("SANITIZED_ISSUE_REQS"),
        "#801 regression: issue body must not persist raw requirements that can include launcher notices"
    );
}

#[test]
fn sanitizer_contract_preserves_legitimate_node_options_requirements() {
    let clarify_prompt = step("step-02-clarify-requirements")
        .prompt
        .expect("clarify step must have a prompt");
    assert!(
        clarify_prompt.contains("NODE_OPTIONS")
            || std::fs::read_to_string(workflow_prep_path())
                .unwrap()
                .contains("NODE_OPTIONS"),
        "test precondition: workflow-prep should document sanitizer precision around NODE_OPTIONS"
    );

    let raw = std::fs::read_to_string(workflow_prep_path()).expect("read workflow-prep");
    assert!(
        raw.contains("Ensure the workflow rejects unsafe NODE_OPTIONS")
            || raw.contains("legitimate") && raw.contains("NODE_OPTIONS"),
        "#801 regression: tests require sanitizer documentation/logic that preserves legitimate NODE_OPTIONS requirements"
    );
    assert!(
        !raw.contains("grep -v NODE_OPTIONS"),
        "#801 regression: broad NODE_OPTIONS line filtering would remove legitimate requirements"
    );
}

#[test]
fn sanitizer_does_not_weaken_artifact_guard_or_env_strictness() {
    let raw = std::fs::read_to_string(workflow_prep_path()).expect("read workflow-prep");
    for forbidden in [
        "SKIP_ARTIFACT_GUARD=true",
        "ALLOW_UNSAFE_NODE_OPTIONS",
        "NODE_OPTIONS=*",
        "set +e",
    ] {
        assert!(
            !raw.contains(forbidden),
            "#801 regression: sanitizer must not weaken workflow strictness via `{forbidden}`"
        );
    }
}
