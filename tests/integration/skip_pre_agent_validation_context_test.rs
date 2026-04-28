//! Integration tests for issue #453 — skip_pre_agent_validation context variable.
//!
//! These tests are written FIRST (TDD red). They MUST fail until the implementation
//! lands in the recipe YAML files:
//!
//!   * `smart-orchestrator.yaml` declares `skip_pre_agent_validation: "true"` in context
//!   * `default-workflow.yaml` declares `skip_pre_agent_validation: "true"` in context
//!   * `smart-execute-routing.yaml` forwards the variable in all context blocks that
//!     already forward `worktree_setup` / `allow_no_op`
//!   * `workflow-prep.yaml` step-01 contains a defensive guard that only runs
//!     pre-agent validation when `SKIP_PRE_AGENT_VALIDATION` is `"false"`
//!
//! Test strategy (mirrors `existing_branch_context_test.rs`):
//!   * Parse recipe YAML with `serde_yaml` to inspect `context:` maps and step bodies.
//!   * Assert presence of the variable, correct default value, proper forwarding,
//!     and guard strings in bash command bodies.
//!   * No subprocess execution needed — all tests are structural/contract tests.

use std::fs;
use std::path::{Path, PathBuf};

use serde_yaml::Value;

// ---------------------------------------------------------------------------
// Repo / recipe paths
// ---------------------------------------------------------------------------

fn workspace_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop(); // bins/amplihack -> bins
    p.pop(); // bins -> workspace root
    p
}

fn smart_orchestrator_yaml() -> PathBuf {
    workspace_root().join("amplifier-bundle/recipes/smart-orchestrator.yaml")
}

fn default_workflow_yaml() -> PathBuf {
    workspace_root().join("amplifier-bundle/recipes/default-workflow.yaml")
}

fn smart_execute_routing_yaml() -> PathBuf {
    workspace_root().join("amplifier-bundle/recipes/smart-execute-routing.yaml")
}

fn workflow_prep_yaml() -> PathBuf {
    workspace_root().join("amplifier-bundle/recipes/workflow-prep.yaml")
}

// ---------------------------------------------------------------------------
// Recipe parsing helpers
// ---------------------------------------------------------------------------

/// Load a recipe YAML and return the parsed serde_yaml::Value.
fn load_recipe(path: &Path) -> Value {
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {} as YAML: {e}", path.display()))
}

/// Return the recipe's `context:` map as a serde_yaml Mapping.
fn context_map(recipe: &Value) -> &serde_yaml::Mapping {
    recipe
        .get("context")
        .and_then(Value::as_mapping)
        .expect("recipe must have a top-level 'context' mapping")
}

/// Return the recipe's context keys as a Vec<String>.
fn context_keys(recipe: &Value) -> Vec<String> {
    context_map(recipe)
        .keys()
        .filter_map(|k| k.as_str().map(str::to_owned))
        .collect()
}

/// Return the string value of a context variable, or None if not present/not a string.
fn context_value(recipe: &Value, key: &str) -> Option<String> {
    context_map(recipe)
        .get(Value::String(key.to_owned()))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// Return all steps from a recipe as a Vec of serde_yaml::Value references.
fn recipe_steps(recipe: &Value) -> Vec<&Value> {
    recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .expect("recipe must have a top-level 'steps' sequence")
        .iter()
        .collect()
}

/// Find a step by id and return its `command:` body (or `prompt:` for agent steps).
fn extract_step_body(recipe: &Value, step_id: &str) -> String {
    let steps = recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .expect("recipe must have a top-level 'steps' sequence");

    for step in steps {
        let id = step.get("id").and_then(Value::as_str).unwrap_or("");
        if id == step_id {
            if let Some(cmd) = step.get("command").and_then(Value::as_str) {
                return cmd.to_owned();
            }
            if let Some(prompt) = step.get("prompt").and_then(Value::as_str) {
                return prompt.to_owned();
            }
            panic!("step '{step_id}' has neither 'command:' nor 'prompt:' body");
        }
    }
    panic!("step '{step_id}' not found in recipe");
}

/// Return the `context:` map from a specific step, if it has one.
fn step_context_map(step: &Value) -> Option<&serde_yaml::Mapping> {
    step.get("context").and_then(Value::as_mapping)
}

// ===========================================================================
// 1. Context declaration tests — variable must exist with correct default
// ===========================================================================

#[test]
fn smart_orchestrator_declares_skip_pre_agent_validation() {
    let recipe = load_recipe(&smart_orchestrator_yaml());
    let keys = context_keys(&recipe);
    assert!(
        keys.iter().any(|k| k == "skip_pre_agent_validation"),
        "smart-orchestrator.yaml must declare 'skip_pre_agent_validation' in its context block \
         so it propagates to sub-recipes. Found keys: {keys:?}"
    );
}

#[test]
fn smart_orchestrator_skip_pre_agent_validation_defaults_to_true() {
    let recipe = load_recipe(&smart_orchestrator_yaml());
    let val = context_value(&recipe, "skip_pre_agent_validation");
    assert_eq!(
        val.as_deref(),
        Some("true"),
        "smart-orchestrator.yaml skip_pre_agent_validation must default to string \"true\". \
         Got: {val:?}"
    );
}

#[test]
fn default_workflow_declares_skip_pre_agent_validation() {
    let recipe = load_recipe(&default_workflow_yaml());
    let keys = context_keys(&recipe);
    assert!(
        keys.iter().any(|k| k == "skip_pre_agent_validation"),
        "default-workflow.yaml must declare 'skip_pre_agent_validation' in its context block. \
         Found keys: {keys:?}"
    );
}

#[test]
fn default_workflow_skip_pre_agent_validation_defaults_to_true() {
    let recipe = load_recipe(&default_workflow_yaml());
    let val = context_value(&recipe, "skip_pre_agent_validation");
    assert_eq!(
        val.as_deref(),
        Some("true"),
        "default-workflow.yaml skip_pre_agent_validation must default to string \"true\". \
         Got: {val:?}"
    );
}

// ===========================================================================
// 2. Forwarding tests — variable must be explicitly forwarded in
//    smart-execute-routing.yaml wherever worktree_setup/allow_no_op are forwarded
// ===========================================================================

/// Collect all step IDs in smart-execute-routing.yaml that have a `context:`
/// block forwarding `worktree_setup`. Each of those steps MUST also forward
/// `skip_pre_agent_validation`.
#[test]
fn smart_execute_routing_forwards_skip_pre_agent_validation_alongside_worktree_setup() {
    let recipe = load_recipe(&smart_execute_routing_yaml());
    let steps = recipe_steps(&recipe);

    let mut steps_with_worktree_setup: Vec<String> = Vec::new();
    let mut steps_missing_skip_var: Vec<String> = Vec::new();

    for step in &steps {
        let id = step
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("<unknown>")
            .to_owned();
        if let Some(ctx) = step_context_map(step) {
            let has_worktree_setup = ctx.keys().any(|k| k.as_str() == Some("worktree_setup"));
            if has_worktree_setup {
                steps_with_worktree_setup.push(id.clone());
                let has_skip_var = ctx
                    .keys()
                    .any(|k| k.as_str() == Some("skip_pre_agent_validation"));
                if !has_skip_var {
                    steps_missing_skip_var.push(id);
                }
            }
        }
    }

    // Sanity: there must be at least one step forwarding worktree_setup
    assert!(
        !steps_with_worktree_setup.is_empty(),
        "Expected at least one step in smart-execute-routing.yaml with context forwarding \
         worktree_setup; found none. Steps may have been restructured."
    );

    assert!(
        steps_missing_skip_var.is_empty(),
        "The following steps in smart-execute-routing.yaml forward worktree_setup but NOT \
         skip_pre_agent_validation: {steps_missing_skip_var:?}. \
         All steps that forward worktree_setup must also forward skip_pre_agent_validation \
         to prevent silent propagation gaps."
    );
}

/// Verify the forwarding uses template syntax `{{skip_pre_agent_validation}}`
/// so the recipe runner substitutes the actual value.
#[test]
fn smart_execute_routing_uses_template_syntax_for_skip_var() {
    let recipe = load_recipe(&smart_execute_routing_yaml());
    let steps = recipe_steps(&recipe);

    for step in &steps {
        let id = step
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("<unknown>");
        if let Some(ctx) = step_context_map(step) {
            let skip_val = ctx
                .get(Value::String("skip_pre_agent_validation".to_owned()))
                .and_then(Value::as_str);
            if let Some(val) = skip_val {
                assert_eq!(
                    val, "{{skip_pre_agent_validation}}",
                    "Step '{id}' in smart-execute-routing.yaml must forward \
                     skip_pre_agent_validation using template syntax \
                     '{{{{skip_pre_agent_validation}}}}', got: '{val}'"
                );
            }
        }
    }
}

// ===========================================================================
// 3. Guard tests — workflow-prep.yaml step-01 must contain the defensive guard
// ===========================================================================

#[test]
fn workflow_prep_step01_contains_skip_pre_agent_validation_guard() {
    let recipe = load_recipe(&workflow_prep_yaml());
    let body = extract_step_body(&recipe, "step-01-prepare-workspace");

    assert!(
        body.contains("SKIP_PRE_AGENT_VALIDATION"),
        "workflow-prep.yaml step-01-prepare-workspace must reference \
         SKIP_PRE_AGENT_VALIDATION in its command body to guard against \
         inadvertent pre-agent validation commands. Command body:\n{body}"
    );
}

#[test]
fn workflow_prep_step01_guard_uses_opt_in_pattern() {
    // The guard must use an opt-IN pattern for validation: only run validation
    // when SKIP_PRE_AGENT_VALIDATION is explicitly "false". This is the safe
    // default-deny pattern — if the variable is unset or empty, validation
    // is skipped (not run).
    let recipe = load_recipe(&workflow_prep_yaml());
    let body = extract_step_body(&recipe, "step-01-prepare-workspace");

    // Check for the opt-in condition: validation runs only when var == "false"
    assert!(
        body.contains(r#""$SKIP_PRE_AGENT_VALIDATION" = "false""#)
            || body.contains(r#""${SKIP_PRE_AGENT_VALIDATION}" = "false""#),
        "workflow-prep.yaml step-01 guard must use opt-in pattern: \
         [ \"$SKIP_PRE_AGENT_VALIDATION\" = \"false\" ]. \
         This ensures validation only runs when explicitly enabled (default-deny). \
         Command body:\n{body}"
    );
}

#[test]
fn workflow_prep_step01_guard_appears_after_git_operations() {
    // The guard must NOT wrap the existing git operations (status, fetch, branch).
    // It must appear AFTER them, as a separate conditional block for future
    // validation commands.
    let recipe = load_recipe(&workflow_prep_yaml());
    let body = extract_step_body(&recipe, "step-01-prepare-workspace");

    // Find positions of key elements
    let git_fetch_pos = body.find("git fetch");
    let guard_pos = body.find("SKIP_PRE_AGENT_VALIDATION");

    assert!(
        git_fetch_pos.is_some(),
        "step-01 must still contain 'git fetch' (existing git operations preserved)"
    );
    assert!(
        guard_pos.is_some(),
        "step-01 must contain SKIP_PRE_AGENT_VALIDATION guard"
    );
    assert!(
        git_fetch_pos.unwrap() < guard_pos.unwrap(),
        "SKIP_PRE_AGENT_VALIDATION guard must appear AFTER git fetch, not before. \
         git fetch at byte {}, guard at byte {}. \
         The guard must not wrap existing git operations.",
        git_fetch_pos.unwrap(),
        guard_pos.unwrap()
    );
}

#[test]
fn workflow_prep_step01_preserves_existing_git_operations() {
    // Verify the implementation didn't accidentally remove or wrap
    // the existing git status/fetch/branch operations.
    let recipe = load_recipe(&workflow_prep_yaml());
    let body = extract_step_body(&recipe, "step-01-prepare-workspace");

    let required_commands = ["git status", "git fetch", "git branch --show-current"];
    for cmd in &required_commands {
        assert!(
            body.contains(cmd),
            "step-01-prepare-workspace must still contain '{cmd}' — \
             existing git operations must not be removed or wrapped in conditionals. \
             Command body:\n{body}"
        );
    }
}

// ===========================================================================
// 4. Brick-rule compliance — all modified files must be ≤500 lines
// ===========================================================================

#[test]
fn all_recipe_files_within_brick_limit() {
    let files = [
        smart_orchestrator_yaml(),
        default_workflow_yaml(),
        smart_execute_routing_yaml(),
        workflow_prep_yaml(),
    ];

    let max_lines = 500;
    let mut violations: Vec<String> = Vec::new();

    for path in &files {
        let text =
            fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let line_count = text.lines().count();
        if line_count > max_lines {
            violations.push(format!(
                "{}: {} lines (limit: {max_lines})",
                path.file_name().unwrap().to_string_lossy(),
                line_count
            ));
        }
    }

    assert!(
        violations.is_empty(),
        "Recipe files exceeding {max_lines}-line brick limit: {violations:?}"
    );
}

// ===========================================================================
// 5. Type safety — value must be string "true", not boolean true
// ===========================================================================

#[test]
fn smart_orchestrator_skip_var_is_string_not_boolean() {
    // The value must be YAML string "true" (quoted), not YAML boolean true,
    // because recipe-runner-rs exposes context vars as environment variables
    // which are always strings. Using native boolean causes type coercion
    // issues (same pattern as force_single_workstream: "false").
    let recipe = load_recipe(&smart_orchestrator_yaml());
    let ctx = context_map(&recipe);
    let val = ctx.get(Value::String("skip_pre_agent_validation".to_owned()));
    assert!(
        val.is_some(),
        "skip_pre_agent_validation must exist in smart-orchestrator.yaml context"
    );
    let val = val.unwrap();
    assert!(
        val.is_string(),
        "skip_pre_agent_validation must be a YAML string (quoted \"true\"), not a boolean. \
         Got type: {:?}, value: {val:?}. \
         Use \"true\" (quoted) to match the force_single_workstream pattern.",
        val
    );
}

#[test]
fn default_workflow_skip_var_is_string_not_boolean() {
    let recipe = load_recipe(&default_workflow_yaml());
    let ctx = context_map(&recipe);
    let val = ctx.get(Value::String("skip_pre_agent_validation".to_owned()));
    assert!(
        val.is_some(),
        "skip_pre_agent_validation must exist in default-workflow.yaml context"
    );
    let val = val.unwrap();
    assert!(
        val.is_string(),
        "skip_pre_agent_validation must be a YAML string (quoted \"true\"), not a boolean. \
         Got type: {:?}, value: {val:?}",
        val
    );
}

// ===========================================================================
// 6. Consistency — same default value across both declaring recipes
// ===========================================================================

#[test]
fn skip_var_default_consistent_across_recipes() {
    let so = load_recipe(&smart_orchestrator_yaml());
    let dw = load_recipe(&default_workflow_yaml());

    let so_val = context_value(&so, "skip_pre_agent_validation");
    let dw_val = context_value(&dw, "skip_pre_agent_validation");

    assert_eq!(
        so_val, dw_val,
        "skip_pre_agent_validation must have the same default value in both \
         smart-orchestrator.yaml ({so_val:?}) and default-workflow.yaml ({dw_val:?})"
    );
}
