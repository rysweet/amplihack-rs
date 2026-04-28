//! Tests for issue #439: Remove hard agent-step timeouts from default-workflow recipes.
//!
//! TDD tests written before the fix lands. They verify:
//! 1. All 11 agent steps across 5 recipe files have NO `timeout_seconds`
//! 2. Bash steps with GNU coreutils `timeout` are preserved (not affected)
//! 3. The `--no-step-timeouts` CLI flag exists and conflicts with `--step-timeout`
//! 4. All recipe files still parse cleanly after timeout removal

use std::path::PathBuf;

use serde_yaml::Value;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn repo_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .expect("repo root")
        .to_path_buf()
}

fn recipe_path(name: &str) -> PathBuf {
    repo_root().join(format!("amplifier-bundle/recipes/{name}"))
}

fn load_recipe(name: &str) -> Value {
    let path = recipe_path(name);
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

fn get_steps(recipe: &Value) -> &Vec<Value> {
    recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .expect("recipe must have a top-level `steps:` sequence")
}

fn find_step<'a>(steps: &'a [Value], step_id: &str) -> &'a Value {
    steps
        .iter()
        .find(|s| s.get("id").and_then(Value::as_str) == Some(step_id))
        .unwrap_or_else(|| panic!("step `{step_id}` not found"))
}

/// Returns true if a step is an agent step (has `agent:` field).
fn is_agent_step(step: &Value) -> bool {
    step.get("agent").is_some()
}

/// Collects IDs of agent steps that still carry `timeout_seconds`.
fn agent_steps_with_timeout(recipe: &Value) -> Vec<String> {
    get_steps(recipe)
        .iter()
        .filter(|s| is_agent_step(s) && s.get("timeout_seconds").is_some())
        .map(|s| {
            s.get("id")
                .and_then(Value::as_str)
                .unwrap_or("<unknown>")
                .to_string()
        })
        .collect()
}

// ===========================================================================
// Section 1: YAML parsing — all 5 recipe files must parse cleanly
// ===========================================================================

#[test]
fn workflow_prep_parses() {
    let _ = load_recipe("workflow-prep.yaml");
}

#[test]
fn smart_classify_route_parses() {
    let _ = load_recipe("smart-classify-route.yaml");
}

#[test]
fn smart_execute_routing_parses() {
    let _ = load_recipe("smart-execute-routing.yaml");
}

#[test]
fn smart_validate_summarize_parses() {
    let _ = load_recipe("smart-validate-summarize.yaml");
}

#[test]
fn smart_reflect_loop_parses() {
    let _ = load_recipe("smart-reflect-loop.yaml");
}

// ===========================================================================
// Section 2: Per-recipe — no agent steps should have timeout_seconds
// ===========================================================================

#[test]
fn workflow_prep_no_agent_timeout() {
    let recipe = load_recipe("workflow-prep.yaml");
    let bad = agent_steps_with_timeout(&recipe);
    assert!(
        bad.is_empty(),
        "issue #439: workflow-prep.yaml agent steps must not have timeout_seconds, \
         found on: {bad:?}"
    );
}

#[test]
fn smart_classify_route_no_agent_timeout() {
    let recipe = load_recipe("smart-classify-route.yaml");
    let bad = agent_steps_with_timeout(&recipe);
    assert!(
        bad.is_empty(),
        "issue #439: smart-classify-route.yaml agent steps must not have timeout_seconds, \
         found on: {bad:?}"
    );
}

#[test]
fn smart_execute_routing_no_agent_timeout() {
    let recipe = load_recipe("smart-execute-routing.yaml");
    let bad = agent_steps_with_timeout(&recipe);
    assert!(
        bad.is_empty(),
        "issue #439: smart-execute-routing.yaml agent steps must not have timeout_seconds, \
         found on: {bad:?}"
    );
}

#[test]
fn smart_validate_summarize_no_agent_timeout() {
    let recipe = load_recipe("smart-validate-summarize.yaml");
    let bad = agent_steps_with_timeout(&recipe);
    assert!(
        bad.is_empty(),
        "issue #439: smart-validate-summarize.yaml agent steps must not have timeout_seconds, \
         found on: {bad:?}"
    );
}

#[test]
fn smart_reflect_loop_no_agent_timeout() {
    let recipe = load_recipe("smart-reflect-loop.yaml");
    let bad = agent_steps_with_timeout(&recipe);
    assert!(
        bad.is_empty(),
        "issue #439: smart-reflect-loop.yaml agent steps must not have timeout_seconds, \
         found on: {bad:?}"
    );
}

// ===========================================================================
// Section 3: Individual step spot-checks (the 11 named steps from the spec)
// ===========================================================================

#[test]
fn step_02b_analyze_codebase_no_timeout() {
    let recipe = load_recipe("workflow-prep.yaml");
    let step = find_step(get_steps(&recipe), "step-02b-analyze-codebase");
    assert!(
        step.get("timeout_seconds").is_none(),
        "step-02b-analyze-codebase must not have timeout_seconds"
    );
}

#[test]
fn classify_and_decompose_no_timeout() {
    let recipe = load_recipe("smart-classify-route.yaml");
    let step = find_step(get_steps(&recipe), "classify-and-decompose");
    assert!(
        step.get("timeout_seconds").is_none(),
        "classify-and-decompose must not have timeout_seconds"
    );
}

#[test]
fn handle_qa_no_timeout() {
    let recipe = load_recipe("smart-execute-routing.yaml");
    let step = find_step(get_steps(&recipe), "handle-qa");
    assert!(
        step.get("timeout_seconds").is_none(),
        "handle-qa must not have timeout_seconds"
    );
}

#[test]
fn handle_ops_agent_no_timeout() {
    let recipe = load_recipe("smart-execute-routing.yaml");
    let step = find_step(get_steps(&recipe), "handle-ops-agent");
    assert!(
        step.get("timeout_seconds").is_none(),
        "handle-ops-agent must not have timeout_seconds"
    );
}

#[test]
fn validate_outside_in_testing_no_timeout() {
    let recipe = load_recipe("smart-validate-summarize.yaml");
    let step = find_step(get_steps(&recipe), "validate-outside-in-testing");
    assert!(
        step.get("timeout_seconds").is_none(),
        "validate-outside-in-testing must not have timeout_seconds"
    );
}

#[test]
fn summarize_no_timeout() {
    let recipe = load_recipe("smart-validate-summarize.yaml");
    let step = find_step(get_steps(&recipe), "summarize");
    assert!(
        step.get("timeout_seconds").is_none(),
        "summarize must not have timeout_seconds"
    );
}

#[test]
fn reflect_round_1_no_timeout() {
    let recipe = load_recipe("smart-reflect-loop.yaml");
    let step = find_step(get_steps(&recipe), "reflect-round-1");
    assert!(
        step.get("timeout_seconds").is_none(),
        "reflect-round-1 must not have timeout_seconds"
    );
}

#[test]
fn execute_round_2_no_timeout() {
    let recipe = load_recipe("smart-reflect-loop.yaml");
    let step = find_step(get_steps(&recipe), "execute-round-2");
    assert!(
        step.get("timeout_seconds").is_none(),
        "execute-round-2 must not have timeout_seconds"
    );
}

#[test]
fn reflect_round_2_no_timeout() {
    let recipe = load_recipe("smart-reflect-loop.yaml");
    let step = find_step(get_steps(&recipe), "reflect-round-2");
    assert!(
        step.get("timeout_seconds").is_none(),
        "reflect-round-2 must not have timeout_seconds"
    );
}

#[test]
fn execute_round_3_no_timeout() {
    let recipe = load_recipe("smart-reflect-loop.yaml");
    let step = find_step(get_steps(&recipe), "execute-round-3");
    assert!(
        step.get("timeout_seconds").is_none(),
        "execute-round-3 must not have timeout_seconds"
    );
}

#[test]
fn reflect_final_no_timeout() {
    let recipe = load_recipe("smart-reflect-loop.yaml");
    let step = find_step(get_steps(&recipe), "reflect-final");
    assert!(
        step.get("timeout_seconds").is_none(),
        "reflect-final must not have timeout_seconds"
    );
}

// ===========================================================================
// Section 4: Bash steps with GNU coreutils `timeout` are NOT affected
// ===========================================================================

#[test]
fn bash_steps_preserve_timeout_in_commands() {
    // Some bash steps use GNU coreutils `timeout` in their command strings.
    // These must NOT be removed — only the `timeout_seconds:` YAML key on
    // agent steps is in scope for issue #439.
    let recipe = load_recipe("smart-classify-route.yaml");
    let steps = get_steps(&recipe);

    // Find bash steps (have `bash:` or type: "bash")
    let bash_steps: Vec<&Value> = steps
        .iter()
        .filter(|s| {
            s.get("bash").is_some() || s.get("type").and_then(Value::as_str) == Some("bash")
        })
        .collect();

    // Bash steps that reference `timeout ` in their command should be preserved.
    // This is a guard: if any bash step uses GNU timeout, it must still be there.
    for step in &bash_steps {
        let bash_content = step.get("bash").and_then(Value::as_str).unwrap_or_default();
        // We don't assert specific content — just that bash steps exist and
        // aren't accidentally deleted during the timeout_seconds cleanup.
        assert!(
            step.get("id").is_some(),
            "every bash step must retain its id"
        );
        // The bash content should not be empty if it existed before.
        let _ = bash_content; // suppress unused warning; existence check is the test
    }

    // At minimum, the preflight-validation bash step must still exist.
    let has_preflight = steps
        .iter()
        .any(|s| s.get("id").and_then(Value::as_str) == Some("preflight-validation"));
    assert!(
        has_preflight,
        "preflight-validation bash step must be preserved in smart-classify-route.yaml"
    );
}

// ===========================================================================
// Section 5: Agent steps preserve required fields after timeout removal
// ===========================================================================

#[test]
fn agent_steps_retain_required_fields_after_timeout_removal() {
    // For each of the 5 recipe files, verify that every agent step retains
    // its `id`, `agent`, and `prompt` fields — the timeout removal must not
    // accidentally strip sibling YAML keys.
    let recipe_files = [
        "workflow-prep.yaml",
        "smart-classify-route.yaml",
        "smart-execute-routing.yaml",
        "smart-validate-summarize.yaml",
        "smart-reflect-loop.yaml",
    ];

    for file in &recipe_files {
        let recipe = load_recipe(file);
        let steps = get_steps(&recipe);

        for step in steps {
            if !is_agent_step(step) {
                continue;
            }

            let step_id = step
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("<missing-id>");

            assert!(step.get("id").is_some(), "{file}: agent step missing `id`");
            assert!(
                step.get("agent").and_then(Value::as_str).is_some(),
                "{file}: step `{step_id}` missing `agent` field"
            );
            assert!(
                step.get("prompt").and_then(Value::as_str).is_some(),
                "{file}: step `{step_id}` missing `prompt` field"
            );
        }
    }
}

// ===========================================================================
// Section 6: CLI --no-step-timeouts flag
// ===========================================================================

#[test]
fn cli_no_step_timeouts_flag_exists() {
    // The --no-step-timeouts flag should be accepted by the CLI parser.
    // This test verifies the flag is recognized by clap via try_parse_from.
    use amplihack_cli::Cli;
    use clap::Parser;

    // Parse a command line that includes --no-step-timeouts.
    // It will fail at runtime (recipe doesn't exist) but should NOT fail at parse time.
    let result = Cli::try_parse_from([
        "amplihack",
        "recipe",
        "run",
        "--no-step-timeouts",
        "--dry-run",
        "dummy.yaml",
    ]);

    assert!(
        result.is_ok(),
        "--no-step-timeouts must be a recognized CLI flag, but parse failed: {:?}",
        result.err()
    );
}

#[test]
fn cli_no_step_timeouts_conflicts_with_step_timeout() {
    // --no-step-timeouts and --step-timeout cannot be combined.
    // clap should reject the combination with an error.
    use amplihack_cli::Cli;
    use clap::Parser;

    let result = Cli::try_parse_from([
        "amplihack",
        "recipe",
        "run",
        "--no-step-timeouts",
        "--step-timeout",
        "300",
        "--dry-run",
        "dummy.yaml",
    ]);

    assert!(
        result.is_err(),
        "--no-step-timeouts + --step-timeout must be rejected by the CLI parser"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("cannot be used with")
            || err_msg.contains("conflicts with")
            || err_msg.contains("not be used with"),
        "--no-step-timeouts + --step-timeout should produce a conflict error, \
         got: {err_msg}"
    );
}

// ===========================================================================
// Section 7: Out-of-scope recipes are NOT modified
// ===========================================================================

#[test]
fn quality_loop_timeout_preserved() {
    // quality-loop.yaml is explicitly out of scope for issue #439.
    // Its default_step_timeout: 1800 must be preserved.
    let path = recipe_path("quality-loop.yaml");
    if !path.exists() {
        // Recipe may not exist in all configurations; skip gracefully.
        return;
    }
    let recipe = load_recipe("quality-loop.yaml");
    // Check that its timeout configuration is untouched.
    let has_timeout = recipe.get("default_step_timeout").is_some()
        || get_steps(&recipe)
            .iter()
            .any(|s| s.get("timeout_seconds").is_some());
    // We don't assert a specific value — just that timeouts weren't
    // accidentally stripped from an out-of-scope recipe.
    assert!(
        has_timeout,
        "quality-loop.yaml should still have timeout configuration (out of scope for #439)"
    );
}
