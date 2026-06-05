//! Workflow-generated PR metadata should be concise and evidence-rich, not a
//! copy of the full task description/design prompt.

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
}

fn workflow_publish_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("amplifier-bundle")
        .join("recipes")
        .join("workflow-publish.yaml")
}

fn step_16_command() -> String {
    let path = workflow_publish_path();
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let recipe: Recipe =
        serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));
    recipe
        .steps
        .into_iter()
        .find(|step| step.id == "step-16-create-draft-pr")
        .expect("workflow-publish must include step-16-create-draft-pr")
        .command
        .expect("step-16-create-draft-pr must be a bash step")
}

#[test]
fn pr_title_is_synthesized_from_evidence_not_raw_task_description() {
    let command = step_16_command();
    assert!(
        !command.contains("PR_TITLE=\"${TASK_DESC")
            && !command.contains("PR_TITLE=\"$(printf '%s' \"$TASK_DESC\""),
        "PR title must not be a direct copy/truncation of the full task description"
    );
    for needle in ["git diff", "git log", "ISSUE_NUM"] {
        assert!(
            command.contains(needle),
            "PR title/body synthesis should use concrete repository evidence; missing `{needle}`"
        );
    }
}

#[test]
fn pr_body_contains_changed_files_validation_and_behavior_context() {
    let command = step_16_command();
    for needle in [
        "Changed files",
        "Validation",
        "Behavior",
        "Risk",
        "git diff --name-only",
        "step-13",
    ] {
        assert!(
            command.contains(needle),
            "PR body must include concise evidence-rich reviewer context; missing `{needle}`"
        );
    }
    assert!(
        !command.contains("PR_BODY=$(printf '## Summary\\n%s")
            && !command.contains("\"$TASK_DESC\" \"$ISSUE_LINK\" \"$PR_DESIGN\""),
        "PR body must not copy the full task description/design spec as its primary content"
    );
}
