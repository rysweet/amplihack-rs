//! tests/integration/workflow_agentic_finalization.rs
//!
//! TDD-red contracts for issue #769 agentic workflow finalization.
//! These tests define the helper and recipe wiring expected after the current
//! deterministic terminal-state baseline grows an advisory agentic finalizer.

use std::fs;
use std::path::PathBuf;

use serde_yaml::Value;

fn workspace_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path
}

fn recipe_path(name: &str) -> PathBuf {
    workspace_root()
        .join("amplifier-bundle")
        .join("recipes")
        .join(format!("{name}.yaml"))
}

fn helper_path() -> PathBuf {
    workspace_root()
        .join("amplifier-bundle")
        .join("tools")
        .join("workflow_agentic_finalization.sh")
}

fn read(path: PathBuf) -> String {
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn helper_text() -> String {
    let path = helper_path();
    assert!(
        path.exists(),
        "issue #769 requires amplifier-bundle/tools/workflow_agentic_finalization.sh"
    );
    read(path)
}

fn load_recipe(name: &str) -> Value {
    let path = recipe_path(name);
    serde_yaml::from_str(&read(path)).unwrap_or_else(|e| panic!("parse {name}.yaml: {e}"))
}

fn steps(recipe: &Value) -> &[Value] {
    recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .expect("recipe must contain top-level steps")
}

fn step_index(recipe: &Value, id: &str) -> usize {
    steps(recipe)
        .iter()
        .position(|step| step.get("id").and_then(Value::as_str) == Some(id))
        .unwrap_or_else(|| panic!("recipe missing step `{id}`"))
}

fn command_step_index_containing(recipe: &Value, needle: &str) -> usize {
    steps(recipe)
        .iter()
        .position(|step| {
            step.get("command")
                .and_then(Value::as_str)
                .is_some_and(|command| command.contains(needle))
        })
        .unwrap_or_else(|| panic!("recipe must contain a bash step with `{needle}`"))
}

fn step_command<'a>(recipe: &'a Value, id: &str) -> &'a str {
    steps(recipe)
        .iter()
        .find(|step| step.get("id").and_then(Value::as_str) == Some(id))
        .and_then(|step| step.get("command").and_then(Value::as_str))
        .unwrap_or_else(|| panic!("step `{id}` must be a bash command step"))
}

#[test]
fn helper_exists_is_executable_and_uses_safe_shell_contract() {
    let path = helper_path();
    assert!(
        path.exists(),
        "agentic finalization helper must exist at {}",
        path.display()
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&path)
            .unwrap_or_else(|e| panic!("stat {}: {e}", path.display()))
            .permissions()
            .mode();
        assert!(
            mode & 0o111 != 0,
            "workflow_agentic_finalization.sh must preserve executable mode"
        );
    }

    let text = read(path);
    for required in [
        "set -euo pipefail",
        "command -v jq",
        "command -v git",
        "timeout",
        "AMPLIHACK_AGENT_BINARY",
    ] {
        assert!(
            text.contains(required),
            "agentic finalization helper must contain `{required}`"
        );
    }
    for forbidden in ["eval ", "sh -c", "${var@P}", "${!"] {
        assert!(
            !text.contains(forbidden),
            "agentic finalization helper must not use unsafe shell construct `{forbidden}`"
        );
    }
}

#[test]
fn helper_validates_structured_agentic_decision_schema_before_success() {
    let text = helper_text();

    for required in [
        "jq -e",
        "decision",
        "ready",
        "blocked",
        "needs_human",
        "confidence",
        "terminal_state",
        "terminal_success",
        "evidence_summary",
        "blocking_reasons",
        "malformed_agentic_finalization",
        "hollow_success",
    ] {
        assert!(
            text.contains(required),
            "agentic finalization helper schema contract must contain `{required}`"
        );
    }

    assert!(
        text.contains("exit 1"),
        "malformed, ambiguous, or hollow finalizer output must fail closed"
    );
}

#[test]
fn helper_rejects_generated_runtime_artifacts_as_commit_evidence() {
    let text = helper_text();

    for required in [
        ".claude/runtime",
        "git status --porcelain",
        "git diff --cached --name-only",
        "git ls-files --others --exclude-standard",
        "artifact_scope",
        "generated_runtime_artifacts",
    ] {
        assert!(
            text.contains(required),
            "agentic finalization helper must inspect artifact scope with `{required}`"
        );
    }

    assert!(
        !text.contains("git add -A .claude/runtime")
            && !text.contains("git add .claude/runtime")
            && !text.contains("cp -R .claude/runtime"),
        "generated runtime artifacts must never be staged or copied wholesale"
    );
}

#[test]
fn workflow_finalize_runs_agentic_assessment_after_deterministic_evidence_before_report() {
    let recipe = load_recipe("workflow-finalize");
    let deterministic_status = step_index(&recipe, "step-22b-final-status");
    let agentic = command_step_index_containing(&recipe, "workflow_agentic_finalization.sh");
    let complete = step_index(&recipe, "workflow-complete");

    assert!(
        deterministic_status < agentic && agentic < complete,
        "workflow-finalize must run deterministic status, then agentic finalization, then final report"
    );

    let agentic_step = &steps(&recipe)[agentic];
    assert_eq!(
        agentic_step.get("parse_json").and_then(Value::as_bool),
        Some(true),
        "agentic finalization step must parse structured JSON output"
    );
    assert_eq!(
        agentic_step.get("output").and_then(Value::as_str),
        Some("agentic_finalization"),
        "agentic finalization output must be available to workflow-complete"
    );
}

#[test]
fn workflow_complete_reports_agentic_decision_without_overriding_terminal_state() {
    let recipe = load_recipe("workflow-finalize");
    let command = step_command(&recipe, "workflow-complete");

    for required in [
        "agentic_finalization",
        "agentic_decision",
        "agentic_confidence",
        "agentic_blocking_reasons",
        "terminal_state",
        "terminal_success",
    ] {
        assert!(
            command.contains(required),
            "workflow-complete must expose `{required}` in final JSON"
        );
    }

    assert!(
        command.contains("terminal_success") && command.contains("agentic_decision"),
        "agentic readiness may inform final reporting but terminal_state remains the machine-checkable authority"
    );
}
