//! tests/integration/default_workflow_terminal_state.rs
//!
//! TDD-red contracts for the reusable default-workflow terminal-state probe.
//! These tests define the evidence and output vocabulary required before the
//! implementation exists.

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

fn recipe_text(name: &str) -> String {
    let path = recipe_path(name);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn load_recipe(name: &str) -> Value {
    let path = recipe_path(name);
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

fn step_commands(recipe: &Value) -> String {
    recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .expect("recipe must contain top-level steps")
        .iter()
        .filter_map(|step| step.get("command").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
}

fn assert_contains_all(haystack: &str, needles: &[&str], context: &str) {
    for needle in needles {
        assert!(
            haystack.contains(needle),
            "{context} must contain `{needle}`"
        );
    }
}

fn assert_ordered(haystack: &str, needles: &[&str], context: &str) {
    let mut previous = 0;
    for needle in needles {
        let position = haystack[previous..]
            .find(needle)
            .map(|offset| previous + offset)
            .unwrap_or_else(|| panic!("{context} must contain `{needle}` in order"));
        assert!(
            position >= previous,
            "{context} must place `{needle}` after earlier terminal checks"
        );
        previous = position;
    }
}

#[test]
fn terminal_state_recipe_exists_with_required_io_contract() {
    let recipe = load_recipe("workflow-terminal-state");
    let text = recipe_text("workflow-terminal-state");

    let input_names: Vec<_> = recipe
        .get("inputs")
        .and_then(Value::as_sequence)
        .expect("terminal-state recipe must declare inputs")
        .iter()
        .filter_map(|input| input.get("name").and_then(Value::as_str))
        .collect();

    for required_input in [
        "repo_path",
        "branch_name",
        "base_ref",
        "pr_number",
        "pr_url",
        "goal_already_met",
    ] {
        assert!(
            input_names.contains(&required_input),
            "workflow-terminal-state must declare input `{required_input}`"
        );
    }

    assert_contains_all(
        &text,
        &[
            "terminal_success",
            "terminal_state",
            "terminal_reason",
            "publish_status",
            "should_publish",
            "should_finalize",
            "should_run_ci_wait",
            "should_merge",
        ],
        "terminal-state recipe output contract",
    );
}

#[test]
fn terminal_state_probe_detects_outcomes_in_safe_fail_closed_order() {
    let recipe = load_recipe("workflow-terminal-state");
    let command = step_commands(&recipe);

    assert_ordered(
        &command,
        &[
            "git status --porcelain",
            "FAILED_DIRTY_WORKTREE",
            "mergedAt",
            "MERGED",
            "closed-unmerged",
            "CLOSED_OBSOLETE",
            "git diff --quiet",
            "NO_DIFF_SUCCESS",
            "FOLLOWUP_CREATED",
        ],
        "terminal-state detection order",
    );

    assert_contains_all(
        &command,
        &[
            "BLOCKED_CI",
            "FAILED_CLOSED_UNMERGED",
            "FAILED_MEANINGFUL_DIFF",
            "ambiguous",
            "exit 1",
        ],
        "terminal-state fail-closed semantics",
    );
}

#[test]
fn terminal_state_probe_validates_inputs_before_git_or_github_trust() {
    let recipe = load_recipe("workflow-terminal-state");
    let command = step_commands(&recipe);

    assert_contains_all(
        &command,
        &[
            "git rev-parse --is-inside-work-tree",
            "git check-ref-format --branch",
            "base_ref",
            "pr_number",
            "pr_url",
            "^[0-9][0-9]*$",
            "headRefName",
            "baseRefName",
            "headRefOid",
            "mergedAt",
        ],
        "terminal-state input validation",
    );

    let dirty_position = command
        .find("git status --porcelain")
        .expect("dirty worktree check must exist");
    for trusted_probe in ["gh_with_retry \"pr view\"", "git diff --quiet"] {
        let probe_position = command
            .find(trusted_probe)
            .unwrap_or_else(|| panic!("trusted probe `{trusted_probe}` must exist"));
        assert!(
            dirty_position < probe_position,
            "dirty worktree must be checked before `{trusted_probe}` can produce terminal success"
        );
    }
}

#[test]
fn terminal_state_probe_wraps_github_metadata_calls_with_bounded_retry() {
    let recipe = load_recipe("workflow-terminal-state");
    let command = step_commands(&recipe);

    assert_contains_all(
        &command,
        &[
            "sanitize_gh_stderr",
            "is_transient_gh_error",
            "gh_with_retry",
            "timeout 60 gh",
            "for attempt in 1 2 3",
            "retrying (${attempt}/3)",
            "gh_with_retry \"pr view\"",
            "gh_with_retry \"pr list\"",
            "unavailable PR metadata",
        ],
        "terminal-state GitHub adapter resilience",
    );

    assert!(
        !command.contains("gh pr view \"$pr_target\"")
            && !command.contains("gh pr list --head \"$BRANCH_INPUT\""),
        "terminal-state must not call gh metadata endpoints directly without retry/error handling"
    );
}

#[test]
fn default_workflow_propagates_terminal_state_context_between_phase_recipes() {
    let recipe = load_recipe("default-workflow");
    let context = recipe
        .get("context")
        .and_then(Value::as_mapping)
        .expect("default-workflow must declare a context mapping");

    for required_key in [
        "terminal_success",
        "terminal_state",
        "terminal_reason",
        "publish_status",
        "should_publish",
        "should_finalize",
        "should_run_ci_wait",
        "should_merge",
    ] {
        assert!(
            context.contains_key(Value::String(required_key.to_string())),
            "default-workflow context must propagate `{required_key}`"
        );
    }
}

#[test]
fn pr_review_phase_skips_stale_review_steps_after_terminal_success() {
    let recipe = load_recipe("workflow-pr-review");
    let steps = recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .expect("workflow-pr-review must contain top-level steps");

    for step in steps {
        let id = step
            .get("id")
            .and_then(Value::as_str)
            .expect("workflow-pr-review steps must have ids");
        let condition = step.get("condition").and_then(Value::as_str).unwrap_or("");
        assert!(
            condition.contains("terminal_success != 'true'")
                || condition.contains("terminal_state.terminal_success != 'true'")
                || condition.contains("should_finalize == 'true'"),
            "workflow-pr-review step `{id}` must be gated off for terminal_success=true"
        );
    }
}
