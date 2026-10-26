//! Issue #1671: a git hook that rejects a checkpoint or publish commit, or a
//! publish push, goes to a fixing agent and the step retries, until the
//! loop-health evaluator ends the loop.
//!
//! Contract: `docs/reference/recipe-commit-hook-repair.md`.
//!
//! What is pinned here:
//!
//! - the fixing recipe, `hook-rejection-repair.yaml`, exists, is registered in
//!   `_recipe_manifest.json`, has exactly one agent step and no bash step, has
//!   no per-step timeout (#439), forbids hook bypasses in its prompt, and
//!   carries no routing marker (#1333);
//! - the driver and staging helper exist beside `git-identity.sh`;
//! - the three recipes the change touches stay inside the brick budget;
//! - the reference page is linked from the nav, the index and the executor
//!   environment page, and the shell test is wired into CI;
//! - the executable shell contract passes.
//!
//! The shell test drives the steps with a stub `amplihack` it writes itself,
//! so this file never starts the real binary and needs no `isolated(..)`.

use serde_yaml::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const BRICK_LINE_BUDGET: usize = 400;

/// The same list as `issue_1333_leaf_steps_do_not_orchestrate.rs`. A leaf
/// agent step told to orchestrate starts a nested run from inside a commit
/// step, which is the failure #1333 was about.
const ROUTING_MARKERS: &[&str] = &[
    "auto-intent-router",
    "auto-routed",
    "AMPLIHACK_CONTEXT_START",
    "recipe run smart-orchestrator",
    "Skill(skill=\"dev-orchestrator\")",
    "invoke the dev-orchestrator skill",
    "launching dev-orchestrator",
];

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|path| path.join("amplifier-bundle/recipes").is_dir())
        .map(Path::to_path_buf)
        .expect("workspace must contain amplifier-bundle/recipes")
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn recipe_path() -> PathBuf {
    workspace_root().join("amplifier-bundle/recipes/hook-rejection-repair.yaml")
}

fn recipe_text() -> String {
    read(&recipe_path())
}

fn recipe_yaml() -> Value {
    serde_yaml::from_str(&recipe_text())
        .unwrap_or_else(|e| panic!("parse {}: {e}", recipe_path().display()))
}

fn steps(recipe: &Value) -> Vec<Value> {
    recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .cloned()
        .expect("hook-rejection-repair must declare top-level steps")
}

fn only_step() -> Value {
    let steps = steps(&recipe_yaml());
    assert_eq!(
        steps.len(),
        1,
        "hook-rejection-repair must have exactly one step, the fixing agent"
    );
    steps.into_iter().next().unwrap()
}

fn str_field<'a>(step: &'a Value, name: &str) -> Option<&'a str> {
    step.get(name).and_then(Value::as_str)
}

fn prompt() -> String {
    str_field(&only_step(), "prompt")
        .expect("the fixing step must have a prompt")
        .to_string()
}

#[test]
fn fixing_recipe_exists_and_fits_the_brick_budget() {
    let lines = recipe_text().lines().count();
    assert!(
        lines <= BRICK_LINE_BUDGET,
        "hook-rejection-repair.yaml is {lines} lines; the brick budget is {BRICK_LINE_BUDGET}"
    );
    assert_eq!(
        recipe_yaml().get("name").and_then(Value::as_str),
        Some("hook-rejection-repair"),
        "`name:` must match the filename stem; `recipe run` resolves by stem"
    );
}

#[test]
fn fixing_recipe_has_one_agent_step_and_no_bash() {
    let step = only_step();
    assert_eq!(str_field(&step, "id"), Some("fix-hook-rejection"));
    assert_eq!(
        str_field(&step, "agent"),
        Some("amplihack:pre-commit-diagnostic")
    );
    assert_eq!(
        str_field(&step, "working_dir"),
        Some("{{hook_repair_worktree}}"),
        "the agent works in the task worktree"
    );
    assert_ne!(
        str_field(&step, "type"),
        Some("bash"),
        "SR-2: no bash step may interpolate hook_repair_* values"
    );
    assert!(
        step.get("command").is_none(),
        "SR-2: the fixing step must not carry a shell command"
    );
}

#[test]
fn fixing_recipe_has_no_timeout_or_iteration_cap() {
    let step = only_step();
    assert!(
        step.get("timeout").is_none() && step.get("timeout_seconds").is_none(),
        "#439: no per-step timeout on the fixing agent"
    );
    let text = recipe_text();
    for cap in [
        "max_attempts",
        "max_rounds",
        "max_iterations",
        "max_retries",
    ] {
        assert!(
            !text.contains(cap),
            "the loop is ended by the evaluator, not a count; found `{cap}`"
        );
    }
}

#[test]
fn fixing_recipe_receives_hook_output_only_through_the_evidence_file() {
    let text = recipe_text();
    for input in [
        "hook_repair_worktree",
        "hook_repair_evidence_file",
        "hook_repair_operation",
        "hook_repair_step",
        "hook_repair_hook_ids",
    ] {
        assert!(
            text.contains(input),
            "the fixing recipe must declare and use `{input}`"
        );
    }
    // SR-10: until single-pass template rendering is confirmed, hook output
    // containing `{{...}}` must never be rendered into the prompt.
    assert!(
        !text.contains("hook_repair_output_tail"),
        "the output tail must not reach the fixing recipe until SR-10 is confirmed"
    );
}

#[test]
fn fixing_prompt_forbids_bypasses_and_states_the_output_contract() {
    let prompt = prompt();
    for needle in [
        "--no-verify",
        "SKIP=",
        "core.hooksPath",
        "git -c",
        ".pre-commit-config.yaml",
        "allowlist",
        "not_fixable_in_code",
        "environment",
        "\"hook_repair\"",
        "\"status\"",
        "\"class\"",
        "\"summary\"",
        "hook_repair_evidence_file",
    ] {
        assert!(
            prompt.contains(needle),
            "the fixing prompt must mention `{needle}`"
        );
    }
    let lower = prompt.to_lowercase();
    for git_write in ["commit", "push", "stash", "reset", "update-ref"] {
        assert!(
            lower.contains(git_write),
            "the fixing prompt must forbid `git {git_write}`; the driver owns git writes"
        );
    }
}

#[test]
fn fixing_recipe_carries_no_routing_marker() {
    let text = recipe_text();
    for marker in ROUTING_MARKERS {
        assert!(
            !text.contains(marker),
            "#1333: the fixing recipe must not contain the routing marker `{marker}`"
        );
    }
    assert!(
        !text.contains("recipe run"),
        "the fixing agent must not start nested recipe runs"
    );
}

#[test]
fn fixing_recipe_is_registered_in_the_recipe_manifest() {
    let path = workspace_root().join("amplifier-bundle/recipes/_recipe_manifest.json");
    let manifest: serde_json::Value =
        serde_json::from_str(&read(&path)).expect("_recipe_manifest.json must be valid JSON");
    let hash = manifest
        .get("hook-rejection-repair")
        .and_then(serde_json::Value::as_str)
        .expect("hook-rejection-repair must be registered in _recipe_manifest.json");
    assert!(
        hash.len() == 16 && hash.chars().all(|c| c.is_ascii_hexdigit()),
        "the manifest entry must be a 16-hex hash like its neighbours, got `{hash}`"
    );
}

#[test]
fn helpers_live_beside_git_identity() {
    let tools = workspace_root().join("amplifier-bundle/tools");
    assert!(tools.join("git-identity.sh").is_file());
    for helper in ["workflow_hook_repair.sh", "workflow_stage_changes.sh"] {
        let path = tools.join(helper);
        assert!(path.is_file(), "missing {}", path.display());
        let text = read(&path);
        assert!(
            !text.contains("--no-verify"),
            "{helper} must never pass --no-verify"
        );
        assert!(
            !text.lines().any(|l| {
                let l = l.trim_start();
                !l.starts_with('#') && (l.contains("export SKIP") || l.contains("unset SKIP"))
            }),
            "SR-1: {helper} must neither export nor unset SKIP"
        );
        assert!(
            !text.lines().any(|l| {
                let l = l.trim_start();
                !l.starts_with('#') && l.contains("trap ")
            }),
            "{helper} must install no traps; step-15 owns its EXIT trap"
        );
        assert!(
            !text.lines().any(|l| {
                let l = l.trim_start();
                !l.starts_with('#') && (l.contains("eval ") || l.contains("sh -c"))
            }),
            "SR-9: {helper} must not run commands through eval or sh -c"
        );
    }
}

#[test]
fn touched_recipes_stay_inside_the_brick_budget() {
    let recipes = workspace_root().join("amplifier-bundle/recipes");
    for name in [
        "workflow-tdd.yaml",
        "workflow-refactor-review.yaml",
        "workflow-publish.yaml",
        "hook-rejection-repair.yaml",
    ] {
        let lines = read(&recipes.join(name)).lines().count();
        assert!(
            lines <= BRICK_LINE_BUDGET,
            "{name} is {lines} lines; the brick budget is {BRICK_LINE_BUDGET}"
        );
    }
}

#[test]
fn reference_page_is_linked_and_names_the_follow_up() {
    let root = workspace_root();
    let page = root.join("docs/reference/recipe-commit-hook-repair.md");
    let text = read(&page);
    for needle in [
        "not_fixable_in_code",
        "#1673",
        "Claude Code hooks",
        "launched `amplihack recipe run`",
    ] {
        assert!(
            text.contains(needle),
            "the reference page must cover `{needle}`"
        );
    }
    for (file, link) in [
        ("mkdocs.yml", "reference/recipe-commit-hook-repair.md"),
        ("docs/index.md", "recipe-commit-hook-repair.md"),
        (
            "docs/reference/recipe-executor-environment.md",
            "recipe-commit-hook-repair.md",
        ),
    ] {
        assert!(
            read(&root.join(file)).contains(link),
            "{file} must link the hook-repair reference page"
        );
    }
}

#[test]
fn shell_test_is_wired_into_ci() {
    let ci = read(&workspace_root().join(".github/workflows/ci.yml"));
    assert!(
        ci.contains("amplifier-bundle/recipes/tests/test-issue-1671-hook-rejection-repair.sh"),
        "ci.yml must run the #1671 shell contract like the other recipe shell tests"
    );
}

#[test]
fn hook_rejection_repair_shell_contract_passes() {
    let root = workspace_root();
    let script =
        root.join("amplifier-bundle/recipes/tests/test-issue-1671-hook-rejection-repair.sh");
    assert!(script.is_file(), "missing {}", script.display());

    let out = Command::new("bash")
        .arg(&script)
        .current_dir(&root)
        // The shell test points the steps at the repository's own helpers
        // and its own stub; an inherited override would test something else.
        .env_remove("WORKFLOW_HOOK_REPAIR_HELPER")
        .env_remove("WORKFLOW_STAGE_CHANGES_HELPER")
        .env_remove("HOOK_REPAIR_ROUND_RECIPE")
        .env_remove("HOOK_REPAIR_HEALTH_RECIPE")
        .env_remove("AMPLIHACK_BIN")
        .env_remove("SKIP")
        .output()
        .expect("run the #1671 shell contract");

    assert!(
        out.status.success(),
        "test-issue-1671-hook-rejection-repair.sh failed ({:?})\n--- stdout ---\n{}\n--- stderr ---\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}
