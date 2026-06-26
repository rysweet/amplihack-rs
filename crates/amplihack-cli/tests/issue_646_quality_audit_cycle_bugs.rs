//! Tests for issue #646: Fix 3 bugs in quality-audit-cycle.yaml recipe.
//!
//! Bug 1: verify-fixes step trusts fix-agent JSON output without checking
//!        git diff — must add a `git diff --quiet` cross-check so that if the
//!        fix-agent claims `Fixed > 0` but no files are actually modified,
//!        the step reports `VERIFY: FAIL`.
//!
//! Bug 2: run-recursive-cycle uses `type: recipe` which does not propagate
//!        results back — must be converted to `type: bash` invoking
//!        `amplihack recipe run quality-audit-cycle` as a subprocess.
//!
//! Bug 3: run-recursive-cycle has no timeout guard — must wrap the subprocess
//!        call in shell `timeout 900` (NOT a YAML-level `timeout:` field,
//!        which would violate issue #439's CI tests).
//!
//! These tests are written TDD-style: they assert the expected YAML structure
//! BEFORE the implementation changes land, so they fail initially and pass
//! once the fixes are applied.

use serde_yaml::Value;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("repo root")
        .to_path_buf()
}

fn recipes_dir() -> PathBuf {
    repo_root().join("amplifier-bundle/recipes")
}

fn load_recipe() -> Value {
    let path = recipes_dir().join("quality-audit-cycle.yaml");
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

fn recipe_text() -> String {
    let path = recipes_dir().join("quality-audit-cycle.yaml");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn find_step<'a>(recipe: &'a Value, step_id: &str) -> &'a Value {
    let steps = recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .expect("recipe must have steps");
    steps
        .iter()
        .find(|s| s.get("id").and_then(Value::as_str) == Some(step_id))
        .unwrap_or_else(|| panic!("step `{step_id}` not found in quality-audit-cycle.yaml"))
}

// =========================================================================
// Step inventory — the full list of steps must be preserved after changes.
// If a step is added, removed, or renamed, update this list.
// =========================================================================

const EXPECTED_STEP_INVENTORY: &[&str] = &[
    "seek",
    "validate-agent-1",
    "validate-agent-2",
    "validate-agent-3",
    "merge-validations",
    "fix",
    "verify-fixes",
    "accumulate-history",
    "recurse-decision",
    "compute-next-cycle",
    "run-recursive-cycle",
    "summary",
    "self-improvement",
    "final-report",
];

#[test]
fn step_inventory_is_preserved() {
    let recipe = load_recipe();
    let steps = recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .expect("recipe must have steps");

    let actual: Vec<&str> = steps
        .iter()
        .filter_map(|s| s.get("id").and_then(Value::as_str))
        .collect();

    assert_eq!(
        actual.len(),
        EXPECTED_STEP_INVENTORY.len(),
        "step count changed: got {}, expected {}.\nActual: {actual:?}",
        actual.len(),
        EXPECTED_STEP_INVENTORY.len()
    );
    for (i, (a, e)) in actual.iter().zip(EXPECTED_STEP_INVENTORY).enumerate() {
        assert_eq!(a, e, "step #{i}: got '{a}', expected '{e}'");
    }
}

// =========================================================================
// Bug 1: verify-fixes must cross-check git diff
// =========================================================================

#[test]
fn verify_fixes_command_contains_git_diff_quiet() {
    let recipe = load_recipe();
    let step = find_step(&recipe, "verify-fixes");
    let cmd = step
        .get("command")
        .and_then(Value::as_str)
        .expect("verify-fixes must have a command field");

    // Performance: `git diff --quiet` short-circuits on first difference
    // instead of computing full --stat output across all changed files.
    assert!(
        cmd.contains("git diff --quiet"),
        "Bug 1: verify-fixes must run `git diff --quiet` to cross-check \
         fix-agent claims against actual file modifications.\n\
         Current command does NOT contain 'git diff --quiet'."
    );
}

#[test]
fn verify_fixes_reports_fail_when_no_files_modified() {
    let recipe = load_recipe();
    let step = find_step(&recipe, "verify-fixes");
    let cmd = step
        .get("command")
        .and_then(Value::as_str)
        .expect("verify-fixes must have a command field");

    // The script must check whether git diff shows modifications and,
    // when no files changed but the fix-agent claims Fixed > 0,
    // output a VERIFY: FAIL message.
    assert!(
        cmd.contains("VERIFY: FAIL"),
        "Bug 1: verify-fixes must emit 'VERIFY: FAIL' when fix-agent claims \
         fixes but git diff shows no actual modifications.\n\
         Current command does NOT contain a 'VERIFY: FAIL' for the git diff check."
    );

    // The fail message should mention that git diff shows no modifications
    assert!(
        cmd.contains("no file modifications")
            || cmd.contains("no files modified")
            || cmd.contains("no modified files")
            || cmd.contains("no actual file"),
        "Bug 1: VERIFY: FAIL message must explain that git diff shows no \
         file modifications, so the user understands why verification failed."
    );
}

#[test]
fn verify_fixes_git_diff_check_occurs_after_jq_parsing() {
    let recipe = load_recipe();
    let step = find_step(&recipe, "verify-fixes");
    let cmd = step
        .get("command")
        .and_then(Value::as_str)
        .expect("verify-fixes must have a command field");

    let jq_pos = cmd
        .find("jq ")
        .or_else(|| cmd.find("jq\n"))
        .expect("verify-fixes must contain a jq invocation");
    let diff_pos = cmd
        .find("git diff --quiet")
        .expect("verify-fixes must contain 'git diff --quiet'");

    assert!(
        diff_pos > jq_pos,
        "Bug 1: git diff check must come AFTER jq parsing so we know the \
         fix-agent's claimed fix count before comparing against actual changes.\n\
         Found jq at position {jq_pos}, git diff at position {diff_pos}."
    );
}

// =========================================================================
// Bug 2: run-recursive-cycle must be type:bash, not type:recipe
// =========================================================================

#[test]
fn run_recursive_cycle_is_bash_type() {
    let recipe = load_recipe();
    let step = find_step(&recipe, "run-recursive-cycle");
    let step_type = step
        .get("type")
        .and_then(Value::as_str)
        .expect("run-recursive-cycle must have a type field");

    assert_eq!(
        step_type, "bash",
        "Bug 2: run-recursive-cycle must be type=bash (subprocess invocation), \
         not type=recipe (which fails to propagate results back).\n\
         Current type: '{step_type}'."
    );
}

#[test]
fn run_recursive_cycle_has_no_sub_context() {
    let recipe = load_recipe();
    let step = find_step(&recipe, "run-recursive-cycle");

    assert!(
        step.get("sub_context").is_none(),
        "Bug 2: run-recursive-cycle must not have sub_context (that's the \
         type:recipe pattern). Context vars should be passed via -c flags \
         in the bash command."
    );
}

#[test]
fn run_recursive_cycle_has_no_recipe_field() {
    let recipe = load_recipe();
    let step = find_step(&recipe, "run-recursive-cycle");

    assert!(
        step.get("recipe").is_none(),
        "Bug 2: run-recursive-cycle must not have a recipe: field (that's \
         the type:recipe pattern). The recipe name should appear in the \
         bash command as 'amplihack recipe run quality-audit-cycle'."
    );
}

#[test]
fn run_recursive_cycle_invokes_amplihack_recipe_run() {
    let recipe = load_recipe();
    let step = find_step(&recipe, "run-recursive-cycle");
    let cmd = step
        .get("command")
        .and_then(Value::as_str)
        .expect("run-recursive-cycle (type:bash) must have a command field");

    assert!(
        cmd.contains("amplihack recipe run quality-audit-cycle"),
        "Bug 2: run-recursive-cycle must invoke 'amplihack recipe run \
         quality-audit-cycle' as a subprocess.\n\
         Current command: {cmd}"
    );
}

#[test]
fn run_recursive_cycle_passes_all_context_vars() {
    let recipe = load_recipe();
    let step = find_step(&recipe, "run-recursive-cycle");
    let cmd = step
        .get("command")
        .and_then(Value::as_str)
        .expect("run-recursive-cycle must have a command field");

    // All 13 context variables that were in the old sub_context must be
    // passed via -c flags to the subprocess invocation.
    let required_vars = [
        "task_description",
        "repo_path",
        "target_path",
        "min_cycles",
        "max_cycles",
        "validation_threshold",
        "severity_threshold",
        "module_loc_limit",
        "fix_all_per_cycle",
        "categories",
        "output_dir",
        "cycle_number",
        "cycle_history",
    ];

    for var in &required_vars {
        // Each should appear as -c var= or -c "var=" pattern
        let pattern = format!("-c {var}=");
        let pattern_quoted = format!("-c \"{var}=");
        assert!(
            cmd.contains(&pattern) || cmd.contains(&pattern_quoted),
            "Bug 2: run-recursive-cycle must pass context var '{var}' via \
             '-c {var}=...' to the subprocess.\n\
             Missing from command."
        );
    }
}

#[test]
fn run_recursive_cycle_uses_tmpfile_for_cycle_history() {
    let recipe = load_recipe();
    let step = find_step(&recipe, "run-recursive-cycle");
    let cmd = step
        .get("command")
        .and_then(Value::as_str)
        .expect("run-recursive-cycle must have a command field");

    // cycle_history is multi-line JSON — must use heredoc-to-tmpfile pattern
    // (not direct interpolation) to avoid arg-length limits and shell injection.
    assert!(
        cmd.contains("mktemp"),
        "Bug 2: run-recursive-cycle must use mktemp for cycle_history \
         (multi-line JSON needs heredoc-to-tmpfile pattern, not inline interpolation)."
    );

    assert!(
        cmd.contains("HEREDOC") || cmd.contains("heredoc") || cmd.contains("<<'"),
        "Bug 2: run-recursive-cycle must use a heredoc to write cycle_history \
         to the temp file (single-quoted delimiter prevents shell expansion)."
    );
}

#[test]
fn run_recursive_cycle_captures_output() {
    let recipe = load_recipe();
    let step = find_step(&recipe, "run-recursive-cycle");

    let output = step
        .get("output")
        .and_then(Value::as_str)
        .expect("run-recursive-cycle must have an output field");

    assert_eq!(
        output, "final_report",
        "run-recursive-cycle must output to 'final_report' (same as before)."
    );
}

// =========================================================================
// Bug 3: run-recursive-cycle must have a 900s timeout guard
// =========================================================================

#[test]
fn run_recursive_cycle_has_shell_timeout_900() {
    let recipe = load_recipe();
    let step = find_step(&recipe, "run-recursive-cycle");
    let cmd = step
        .get("command")
        .and_then(Value::as_str)
        .expect("run-recursive-cycle must have a command field");

    // The timeout must be a shell `timeout` command wrapping the subprocess,
    // NOT a YAML-level `timeout:` field (which would violate issue #439).
    assert!(
        cmd.contains("timeout 900") || cmd.contains("timeout 900s"),
        "Bug 3: run-recursive-cycle must use shell 'timeout 900' to guard \
         the subprocess call against unbounded execution.\n\
         Current command does NOT contain 'timeout 900'."
    );
}

#[test]
fn run_recursive_cycle_has_no_yaml_timeout_field() {
    let recipe = load_recipe();
    let step = find_step(&recipe, "run-recursive-cycle");

    // Issue #439 compliance: no YAML-level timeout: on non-network bash steps
    assert!(
        step.get("timeout").is_none(),
        "Bug 3 / Issue #439: run-recursive-cycle must NOT have a YAML-level \
         timeout: field. Use shell 'timeout 900' inside the command instead. \
         The issue_439_no_per_step_timeout test enforces: bash step YAML \
         timeouts only for network commands and >= 1800s."
    );
    assert!(
        step.get("timeout_seconds").is_none(),
        "Bug 3 / Issue #439: run-recursive-cycle must NOT have a YAML-level \
         timeout_seconds: field."
    );
}

// =========================================================================
// Safety: recipe still parses as valid YAML after changes
// =========================================================================

#[test]
fn recipe_parses_as_valid_yaml() {
    let _ = load_recipe();
}

// =========================================================================
// Brick limit: recipe file must stay within reasonable bounds
// =========================================================================

#[test]
fn recipe_under_brick_line_limit() {
    let text = recipe_text();
    let lines = text.lines().count();
    // #646 bug fixes took the recipe to ~815 lines. #820 added a validator-output
    // normalization layer to merge-validations (~+35 lines) → ~850. Ceiling of 880
    // leaves modest headroom while still guarding against unbounded growth.
    assert!(
        lines <= 880,
        "quality-audit-cycle.yaml is {lines} lines; expected <= 880 \
         (#646 fixes + #820 merge-validations normalization)."
    );
}

// =========================================================================
// Condition preservation: run-recursive-cycle condition must not change
// =========================================================================

#[test]
fn run_recursive_cycle_condition_preserved() {
    let recipe = load_recipe();
    let step = find_step(&recipe, "run-recursive-cycle");
    let condition = step
        .get("condition")
        .and_then(Value::as_str)
        .expect("run-recursive-cycle must have a condition field");

    assert!(
        condition.contains("CONTINUE:") && condition.contains("recurse_decision"),
        "run-recursive-cycle condition must still gate on 'CONTINUE:' in \
         recurse_decision.\nCurrent condition: '{condition}'."
    );
}

// =========================================================================
// Security: heredoc patterns use single-quoted delimiters
// =========================================================================

#[test]
fn verify_fixes_heredocs_use_single_quoted_delimiters() {
    let recipe = load_recipe();
    let step = find_step(&recipe, "verify-fixes");
    let cmd = step
        .get("command")
        .and_then(Value::as_str)
        .expect("verify-fixes must have a command field");

    // All heredoc delimiters in the recipe use single-quoted format
    // (<<'DELIMITER') to prevent shell expansion of adversarial content.
    for line in cmd.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("cat >") && trimmed.contains("<<") {
            assert!(
                trimmed.contains("<<'"),
                "Security: heredoc delimiters must be single-quoted to prevent \
                 shell expansion. Found unquoted heredoc: {trimmed}"
            );
        }
    }
}

#[test]
fn run_recursive_cycle_heredocs_use_single_quoted_delimiters() {
    let recipe = load_recipe();
    let step = find_step(&recipe, "run-recursive-cycle");
    let cmd = step
        .get("command")
        .and_then(Value::as_str)
        .expect("run-recursive-cycle must have a command field");

    for line in cmd.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("cat >") && trimmed.contains("<<") {
            assert!(
                trimmed.contains("<<'"),
                "Security: heredoc delimiters must be single-quoted to prevent \
                 shell expansion. Found unquoted heredoc: {trimmed}"
            );
        }
    }
}

// =========================================================================
// Temp file cleanup: both modified steps must trap EXIT for cleanup
// =========================================================================

#[test]
fn run_recursive_cycle_has_trap_cleanup() {
    let recipe = load_recipe();
    let step = find_step(&recipe, "run-recursive-cycle");
    let cmd = step
        .get("command")
        .and_then(Value::as_str)
        .expect("run-recursive-cycle must have a command field");

    assert!(
        cmd.contains("trap") && cmd.contains("EXIT"),
        "run-recursive-cycle must have 'trap ... EXIT' for temp file cleanup."
    );
}
