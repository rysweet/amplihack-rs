//! Default-workflow decomposition parity tests.
//!
//! `default-workflow.yaml` was decomposed (v3.0.0) from a 3098-line monolith
//! into a thin composer that calls 9 phase sub-recipes:
//!
//!   workflow-prep, workflow-worktree, workflow-design, workflow-tdd,
//!   workflow-refactor-review, workflow-precommit-test, workflow-publish,
//!   workflow-pr-review, workflow-finalize
//!
//! Every original step prompt is preserved verbatim — the user's stated
//! design constraint is that the LLM converges on correct behaviour through
//! many recursive review layers, so no layer may be silently dropped.
//!
//! These tests lock the contract: if a future edit drops a step or reorders
//! the inventory or breaks the brick budget (≤400 LOC per sub-recipe), CI
//! fails before the regression ships.
//!
//! The tests deliberately do NOT exercise the recipe runner end-to-end (that
//! is `recipe_e2e_test.rs`'s job). They are pure structural assertions on
//! YAML content, fast, and have no external dependencies.

use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, ExitStatus};
use std::sync::{
    LazyLock,
    atomic::{AtomicUsize, Ordering},
};

const BRICK_LIMIT: usize = 400;

const EXTRA_RECIPE_NAMES: &[&str] = &["default-workflow", "smart-validate-summarize"];

static RECIPE_TEXTS: LazyLock<HashMap<&'static str, String>> = LazyLock::new(|| {
    PHASE_RECIPES
        .iter()
        .chain(EXTRA_RECIPE_NAMES)
        .map(|&name| {
            let path = recipe_path(name);
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
            (name, text)
        })
        .collect()
});

static RECIPES: LazyLock<HashMap<&'static str, Recipe>> = LazyLock::new(|| {
    RECIPE_TEXTS
        .iter()
        .filter(|&(name, _)| *name != "smart-validate-summarize")
        .map(|(&name, text)| {
            let path = recipe_path(name);
            let recipe = serde_yaml::from_str(text)
                .unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));
            (name, recipe)
        })
        .collect()
});

static WORKFLOW_PR_REVIEW_YAML: LazyLock<serde_yaml::Value> = LazyLock::new(|| {
    let name = "workflow-pr-review";
    let path = recipe_path(name);
    serde_yaml::from_str(recipe_text(name))
        .unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
});

static STEP_COMMAND_RUN_COUNTER: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Deserialize)]
struct Recipe {
    name: String,
    #[serde(default)]
    steps: Vec<Step>,
}

#[derive(Debug, Deserialize)]
struct Step {
    id: String,
    #[serde(rename = "type", default)]
    step_type: Option<String>,
    #[serde(default)]
    recipe: Option<String>,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    prompt: Option<String>,
}

fn recipes_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("amplifier-bundle")
        .join("recipes")
}

fn recipe_path(name: &str) -> PathBuf {
    recipes_dir().join(format!("{name}.yaml"))
}

fn recipe_text(name: &str) -> &'static str {
    RECIPE_TEXTS
        .get(name)
        .unwrap_or_else(|| panic!("uncached test recipe: {name}"))
}

fn load(name: &str) -> &'static Recipe {
    RECIPES
        .get(name)
        .unwrap_or_else(|| panic!("uncached test recipe: {name}"))
}

fn load_yaml(name: &str) -> &'static serde_yaml::Value {
    match name {
        "workflow-pr-review" => &WORKFLOW_PR_REVIEW_YAML,
        _ => panic!("uncached test YAML recipe: {name}"),
    }
}

fn step_command(recipe: &str, step_id: &str) -> &'static str {
    load(recipe)
        .steps
        .iter()
        .find(|step| step.id == step_id)
        .unwrap_or_else(|| panic!("{recipe}.yaml missing step {step_id}"))
        .command
        .as_deref()
        .unwrap_or_else(|| panic!("{recipe}.yaml step {step_id} must be a bash step"))
}

struct StepRun {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

fn run_step_03b_extract_issue_number(issue_creation: &str, task_description: &str) -> StepRun {
    let stub_dir = std::env::temp_dir().join(format!(
        "amplihack-step-03b-test-{}-{}",
        std::process::id(),
        STEP_COMMAND_RUN_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&stub_dir).unwrap_or_else(|e| panic!("create {}: {e}", stub_dir.display()));

    let gh_stub = stub_dir.join("gh");
    fs::write(&gh_stub, "#!/usr/bin/env bash\nexit 0\n")
        .unwrap_or_else(|e| panic!("write {}: {e}", gh_stub.display()));
    let mut permissions = fs::metadata(&gh_stub)
        .unwrap_or_else(|e| panic!("stat {}: {e}", gh_stub.display()))
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&gh_stub, permissions)
        .unwrap_or_else(|e| panic!("chmod {}: {e}", gh_stub.display()));

    let path = match std::env::var("PATH") {
        Ok(existing) if !existing.is_empty() => format!("{}:{existing}", stub_dir.display()),
        _ => stub_dir.display().to_string(),
    };

    let output = Command::new("bash")
        .arg("-c")
        .arg(step_command(
            "workflow-prep",
            "step-03b-extract-issue-number",
        ))
        .env("ISSUE_CREATION", issue_creation)
        .env("TASK_DESCRIPTION", task_description)
        .env("PATH", path)
        .output()
        .expect("run step-03b-extract-issue-number command");

    let _ = fs::remove_dir_all(&stub_dir);

    StepRun {
        status: output.status,
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

fn step_prompt(recipe: &str, step_id: &str) -> &'static str {
    load(recipe)
        .steps
        .iter()
        .find(|step| step.id == step_id)
        .unwrap_or_else(|| panic!("{recipe}.yaml missing step {step_id}"))
        .prompt
        .as_deref()
        .unwrap_or_else(|| panic!("{recipe}.yaml step {step_id} must have a prompt"))
}

/// The expected step inventory of `default-workflow`, in execution order, as
/// of the v2 -> v3 decomposition. This is the contract every PR must preserve.
///
/// If you intentionally add or remove a step, update this list AND adjust the
/// owning sub-recipe; the test then verifies you didn't accidentally lose any
/// other step in the same change.
const EXPECTED_STEP_INVENTORY: &[&str] = &[
    // Phase 1a: workflow-prep (steps 00-03b)
    "step-00-workflow-preparation",
    "step-01-prepare-workspace",
    "step-02-clarify-requirements",
    "step-02b-analyze-codebase",
    "step-02c-resolve-ambiguity",
    "step-02d-detect-host-type",
    "step-03-create-issue",
    "step-03b-extract-issue-number",
    // Phase 1b: workflow-worktree (step 04)
    "step-04-setup-worktree",
    // Phase 2: workflow-design (steps 05-06d)
    "step-05-architecture",
    "step-05b-api-design",
    "step-05c-database-design",
    "step-05d-security-review",
    "step-05e-design-consolidation",
    "step-06-documentation",
    "step-06b-documentation-review",
    "step-06c-documentation-refinement",
    "step-06d-goal-already-met-probe",
    // Phase 3: workflow-tdd (steps 07-08c)
    "step-07-write-tests",
    "step-08-implement",
    "step-08c-work-verifier",
    "step-08c-enforce-verdict",
    "step-08b-integration",
    "checkpoint-after-implementation",
    "implementation-terminal-evidence",
    // Phase 4: workflow-refactor-review (steps 09-11b)
    "step-09-refactor",
    "step-09b-optimize",
    "step-10-pre-commit-review",
    "step-10b-security-review",
    "step-10c-philosophy-check",
    "step-11-incorporate-feedback",
    "step-11b-implement-feedback",
    "checkpoint-after-review-feedback",
    // Phase 5: workflow-precommit-test (steps 12-13)
    "step-12-run-precommit",
    "step-13-local-testing",
    "verification-terminal-evidence",
    // Phase 6: workflow-publish (terminal gate + steps 14-16b)
    "publish-terminal-state",
    "step-14-bump-version",
    "step-14g-artifact-guard",
    "step-15-commit-push",
    "step-16-create-draft-pr",
    "step-16b-outside-in-fix-loop",
    // Phase 7: workflow-pr-review (steps 17a-19d)
    "step-17a-compliance-verification",
    "step-17b-reviewer-agent",
    "step-17c-security-review",
    "step-17d-philosophy-guardian",
    "step-17e-address-blocking-issues",
    "step-17f-verification-gate",
    "step-18a-analyze-feedback",
    "step-18b-implement-feedback",
    "step-18c-push-feedback-changes",
    "step-18d-respond-to-comments",
    "step-18e-verification-gate",
    "step-19a-philosophy-check",
    "step-19b-patterns-check",
    "step-19c-zero-bs-verification",
    "step-19d-verification-gate",
    // Phase 8: workflow-finalize (terminal gate + steps 20-22b + complete)
    "finalize-terminal-state",
    "step-20-final-cleanup",
    "step-20a-artifact-guard",
    "step-20b-push-cleanup",
    "step-20c-quality-audit",
    "step-21-pr-ready",
    "step-22-ensure-mergeable",
    "step-22b-final-status",
    "collect-finalization-evidence",
    "agentic-finalizer",
    "validate-agentic-finalization",
    "workflow-complete",
];

const PHASE_RECIPES: &[&str] = &[
    "workflow-prep",
    "workflow-worktree",
    "workflow-design",
    "workflow-tdd",
    "workflow-refactor-review",
    "workflow-precommit-test",
    "workflow-publish",
    "workflow-pr-review",
    "workflow-finalize",
];

const PR_REVIEW_STEP_INVENTORY: &[&str] = &[
    "step-17a-compliance-verification",
    "step-17b-reviewer-agent",
    "step-17c-security-review",
    "step-17d-philosophy-guardian",
    "step-17e-address-blocking-issues",
    "step-17f-verification-gate",
    "step-18a-analyze-feedback",
    "step-18b-implement-feedback",
    "step-18c-push-feedback-changes",
    "step-18d-respond-to-comments",
    "step-18e-verification-gate",
    "step-19a-philosophy-check",
    "step-19b-patterns-check",
    "step-19c-zero-bs-verification",
    "step-19d-verification-gate",
];

/// The composer must call exactly the 9 phase sub-recipes, in order, and
/// declare nothing else.
#[test]
fn composer_calls_nine_phase_subrecipes_in_order() {
    let composer = load("default-workflow");
    assert_eq!(composer.name, "default-workflow");
    assert_eq!(
        composer.steps.len(),
        PHASE_RECIPES.len(),
        "composer must have exactly {} sub-recipe calls; found {}",
        PHASE_RECIPES.len(),
        composer.steps.len()
    );
    for (i, (step, expected)) in composer.steps.iter().zip(PHASE_RECIPES).enumerate() {
        assert_eq!(
            step.step_type.as_deref(),
            Some("recipe"),
            "composer step {i} must be type=recipe"
        );
        assert_eq!(
            step.recipe.as_deref(),
            Some(*expected),
            "composer step {i} must call recipe '{expected}'; got {:?}",
            step.recipe
        );
        assert_eq!(
            step.id, *expected,
            "composer step id should match recipe name for traceability"
        );
    }
}

/// Every phase sub-recipe must parse and have a non-empty step list.
#[test]
fn every_phase_subrecipe_loads_and_has_steps() {
    for name in PHASE_RECIPES {
        let r = load(name);
        assert_eq!(&r.name, name, "recipe.name must match filename");
        assert!(!r.steps.is_empty(), "{name} must declare at least one step");
    }
}

/// Concatenating the steps of all 9 phase sub-recipes (in composer order) must
/// reproduce the original 58-step inventory in the original order.
#[test]
fn composed_step_inventory_matches_expected_in_order() {
    let mut composed: Vec<String> = Vec::new();
    for name in PHASE_RECIPES {
        let r = load(name);
        composed.extend(r.steps.iter().map(|s| s.id.clone()));
    }
    let expected: Vec<&str> = EXPECTED_STEP_INVENTORY.to_vec();
    assert_eq!(
        composed.len(),
        expected.len(),
        "composed inventory has {} steps, expected {}",
        composed.len(),
        expected.len()
    );
    for (i, (c, e)) in composed.iter().zip(&expected).enumerate() {
        assert_eq!(c, e, "step #{i}: composed has '{c}', expected '{e}'");
    }
}

/// No step ID may appear twice anywhere across the decomposition. Duplicates
/// would silently shadow context outputs.
#[test]
fn no_duplicate_step_ids_across_subrecipes() {
    let mut seen: HashSet<String> = HashSet::new();
    let mut dups: Vec<String> = Vec::new();
    for name in PHASE_RECIPES {
        for step in &load(name).steps {
            if !seen.insert(step.id.clone()) {
                dups.push(step.id.clone());
            }
        }
    }
    assert!(dups.is_empty(), "duplicate step IDs found: {dups:?}");
}

#[test]
fn step_13_does_not_mandate_uvx_or_remote_git_install_flow() {
    let prompt = step_prompt("workflow-precommit-test", "step-13-local-testing");
    let forbidden_phrases = [
        "Get the remote URL (for uvx testing)",
        "git remote get-url origin",
        "Pass the PR branch name and repository URL so tests run against the actual branch",
        "uvx --from git+<remote-url>@<branch-name>",
        "uvx --from git+...@<branch>",
        "via `uvx --from git+...`",
    ];

    for phrase in forbidden_phrases {
        assert!(
            !prompt.contains(phrase),
            "Step 13 must not present `{phrase}` as the universal outside-in validation path"
        );
    }
}

#[test]
fn step_13_requires_agentic_toolchain_detection_before_validation_selection() {
    let prompt = step_prompt("workflow-precommit-test", "step-13-local-testing");
    let lower = prompt.to_ascii_lowercase();

    for required in ["outside-in", "detect", "toolchain", "validation strategy"] {
        assert!(
            lower.contains(required),
            "Step 13 must preserve outside-in validation while requiring agents to detect project languages/toolchains and select a validation strategy; missing `{required}`"
        );
    }

    assert!(
        lower.contains("choose") || lower.contains("select"),
        "Step 13 must delegate validation pattern selection to the acting agent"
    );
    assert!(
        lower.contains("record") && lower.contains("chosen"),
        "Step 13 must require evidence of the chosen validation strategy, not just raw command output"
    );
}

#[test]
fn step_13_covers_major_toolchain_examples_without_making_uvx_global() {
    let prompt = step_prompt("workflow-precommit-test", "step-13-local-testing");
    let lower = prompt.to_ascii_lowercase();
    let toolchain_examples = [
        ("Rust/Cargo", lower.contains("cargo")),
        ("Node/npm", lower.contains("npm")),
        (
            "Python/uv/uvx",
            lower.contains("python") && lower.contains("uv") && lower.contains("uvx"),
        ),
        (
            "Go",
            lower.contains("go test")
                || lower.contains("go run")
                || lower.contains("golang")
                || prompt.contains("Go/"),
        ),
        (".NET", lower.contains("dotnet") || prompt.contains(".NET")),
    ];

    for (toolchain, present) in toolchain_examples {
        assert!(
            present,
            "Step 13 must include a generalized outside-in validation example for {toolchain}"
        );
    }

    assert!(
        lower.contains("python") && lower.find("python") < lower.rfind("uvx"),
        "Step 13 may mention uvx only as a Python/uv-specific validation option"
    );
}

#[test]
fn outside_in_validation_gates_require_strategy_evidence_without_uvx_specific_commands() {
    for recipe in [
        "smart-validate-summarize",
        "workflow-pr-review",
        "workflow-publish",
    ] {
        let text = recipe_text(recipe);
        let lower = text.to_ascii_lowercase();

        assert!(
            !lower.contains("uvx --from git+"),
            "{recipe}.yaml must not validate Step 13 by requiring uvx remote Git execution"
        );
        assert!(
            lower.contains("chosen") && lower.contains("strategy"),
            "{recipe}.yaml must ask for the chosen outside-in validation strategy so reviewers can verify toolchain-aware selection"
        );
    }
}

/// Brick rule: every phase sub-recipe must be ≤ 400 lines. The composer is
/// already tiny by construction; checking it too costs nothing.
#[test]
fn every_phase_subrecipe_under_400_lines() {
    let mut violations: Vec<(String, usize)> = Vec::new();
    for name in PHASE_RECIPES
        .iter()
        .chain(std::iter::once(&"default-workflow"))
    {
        let lines = recipe_text(name).lines().count();
        if lines >= BRICK_LIMIT {
            violations.push((name.to_string(), lines));
        }
    }
    assert!(
        violations.is_empty(),
        "brick rule violation (must be <{BRICK_LIMIT} physical lines): {violations:?}"
    );
}

#[test]
fn workflow_pr_review_phase_contract_is_strict_and_ordered() {
    let line_count = recipe_text("workflow-pr-review").lines().count();
    assert!(
        line_count < BRICK_LIMIT,
        "workflow-pr-review.yaml is {line_count} lines; PR review phase bricks must be <{BRICK_LIMIT} physical lines"
    );

    let step_ids: Vec<String> = load("workflow-pr-review")
        .steps
        .iter()
        .map(|step| step.id.clone())
        .collect();
    assert_eq!(
        step_ids, PR_REVIEW_STEP_INVENTORY,
        "workflow-pr-review must preserve every PR review step from 17a through 19d in order"
    );
}

#[test]
fn workflow_pr_review_has_no_recipe_level_timeouts_or_foreground_long_sleep() {
    let raw = recipe_text("workflow-pr-review");
    let parsed = load_yaml("workflow-pr-review");
    let top_level = parsed
        .as_mapping()
        .expect("workflow-pr-review.yaml must parse as a YAML mapping");

    for forbidden in ["timeout", "timeout_seconds", "default_step_timeout"] {
        assert!(
            !top_level.contains_key(serde_yaml::Value::String(forbidden.to_string())),
            "workflow-pr-review.yaml must not define recipe-level `{forbidden}`"
        );
    }

    let foreground_sleeps: Vec<&str> = raw
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            trimmed.starts_with("sleep ")
                || trimmed.contains("&& sleep ")
                || trimmed.contains("; sleep ")
                || trimmed.contains(" sleep $")
        })
        .collect();
    assert!(
        foreground_sleeps.is_empty(),
        "workflow-pr-review.yaml must not use foreground sleeps: {foreground_sleeps:?}"
    );
}

#[test]
fn workflow_pr_review_fails_loud_for_required_worktree_context() {
    let push_command = step_command("workflow-pr-review", "step-18c-push-feedback-changes");
    let zero_bs_command = step_command("workflow-pr-review", "step-19c-zero-bs-verification");

    // step-18c: early-stage — MUST keep hard-fail on missing worktree
    assert!(
        push_command.contains("set -euo pipefail"),
        "step-18c must fail loudly on shell errors"
    );
    assert!(
        push_command.contains("WORKTREE_SETUP_WORKTREE_PATH:?"),
        "step-18c must require worktree_setup.worktree_path instead of inventing a default"
    );

    // step-19c: late-stage — MUST be resilient (issue #647)
    // It can function from repo_path or cwd; hard-fail is wrong here.
    assert!(
        zero_bs_command.contains("set -euo pipefail"),
        "step-19c must still fail loudly on shell errors (only cd target selection is resilient)"
    );
    assert!(
        zero_bs_command.contains("WARNING"),
        "step-19c must emit WARNING on stderr when falling back from missing worktree"
    );
    assert!(
        zero_bs_command.contains("REPO_PATH"),
        "step-19c must fall back to REPO_PATH when worktree is unavailable"
    );
    assert!(
        !zero_bs_command.contains("cd \"${WORKTREE_SETUP_WORKTREE_PATH:?"),
        "step-19c must NOT hard-fail on cd into missing worktree (issue #647)"
    );

    // step-18c specific assertions preserved from original
    let hidden_git_failures: Vec<&str> = push_command
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            trimmed.starts_with("git ") && trimmed.contains("|| true")
        })
        .collect();
    assert!(
        hidden_git_failures.is_empty(),
        "step-18c must not hide git failures with `|| true`: {hidden_git_failures:?}"
    );
    assert!(
        push_command.contains("git commit failed") && push_command.contains("exit \"$commit_rc\""),
        "step-18c must surface git commit failures and exit with the original status"
    );
    assert!(
        push_command.contains("git push failed") && push_command.contains("exit \"$push_rc\""),
        "step-18c must surface git push failures and exit with the original status"
    );
}

#[test]
fn workflow_finalize_late_steps_are_resilient_to_missing_worktree() {
    // Issue #647: steps 20b and 21 are late-stage — the worktree may have
    // been cleaned up by the agent or a prior step. They must fall back to
    // REPO_PATH or cwd instead of aborting the recipe.
    let push_cleanup = step_command("workflow-finalize", "step-20b-push-cleanup");
    let pr_ready = step_command("workflow-finalize", "step-21-pr-ready");

    for (step, command) in [
        ("step-20b-push-cleanup", push_cleanup),
        ("step-21-pr-ready", pr_ready),
    ] {
        assert!(
            command.contains("set -euo pipefail"),
            "{step} must still fail loudly on shell errors (only cd target is resilient)"
        );
        assert!(
            command.contains("WARNING"),
            "{step} must emit WARNING on stderr when falling back from missing worktree"
        );
        assert!(
            command.contains("REPO_PATH"),
            "{step} must fall back to REPO_PATH when worktree is unavailable"
        );
        assert!(
            !command.contains("cd \"${WORKTREE_SETUP_WORKTREE_PATH:?"),
            "{step} must NOT hard-fail on cd into missing worktree (issue #647)"
        );
    }
}

#[test]
fn workflow_pr_review_input_description_reflects_resilient_step_19c() {
    // Issue #647: step-19c is now resilient, so the input description should
    // only list step-18c as requiring the worktree path.
    let text = recipe_text("workflow-pr-review");
    assert!(
        text.contains("Required by") && text.contains("step-18c"),
        "worktree_setup.worktree_path input description must mention step-18c"
    );
    assert!(
        !text.contains("step-19c-zero-bs-verification that cd into the worktree"),
        "worktree_setup.worktree_path input description must not claim step-19c requires cd into worktree (issue #647)"
    );
}

#[test]
fn workflow_pr_review_retries_remote_push_operations_without_silencing_failures() {
    let command = step_command("workflow-pr-review", "step-18c-push-feedback-changes");
    assert!(
        command.contains("git_remote_with_retry()") && command.contains("for attempt in 1 2 3"),
        "step-18c must retry transient git remote operations"
    );
    assert!(
        command.contains("git_remote_with_retry git pull --rebase")
            && command.contains("git_remote_with_retry git push"),
        "step-18c must wrap pull/rebase and push with retry handling"
    );
    assert!(
        command.contains("retrying ($attempt/3)") && command.contains("return \"$rc\""),
        "step-18c retry handling must be bounded and preserve the failing status"
    );
}

#[test]
fn workflow_pr_review_scopes_pre_commit_allow_no_config_to_stale_hook_case() {
    let command = step_command("workflow-pr-review", "step-18c-push-feedback-changes");
    assert_eq!(
        command.matches("PRE_COMMIT_ALLOW_NO_CONFIG").count(),
        1,
        "PRE_COMMIT_ALLOW_NO_CONFIG must only appear at the scoped git commit callsite"
    );
    assert!(
        command.contains("pre_commit_hook=\"$(git rev-parse --git-path hooks/pre-commit)\""),
        "step-18c must inspect the repository-local pre-commit hook path"
    );
    assert!(
        command.contains("[ -f \"$pre_commit_hook\" ]"),
        "PRE_COMMIT_ALLOW_NO_CONFIG is allowed only for a stale pre-commit hook file"
    );
    assert!(
        command.contains("[ ! -f .pre-commit-config.yaml ]"),
        "PRE_COMMIT_ALLOW_NO_CONFIG is allowed only when .pre-commit-config.yaml is missing"
    );
    assert!(
        command.contains("PRE_COMMIT_ALLOW_NO_CONFIG=1 git commit \"$@\"")
            && command.contains("elif git commit \"$@\"")
            && command.contains("commit_with_pre_commit_guard -m \"address review feedback\""),
        "PRE_COMMIT_ALLOW_NO_CONFIG must be scoped to the single git commit invocation"
    );
    assert!(
        command.contains("address review feedback")
            && command.contains("Implemented reviewer suggestions")
            && command.contains("fixed identified issues")
            && command.contains("updated per security review")
            && command.contains("addressed philosophy compliance items"),
        "step-18c must preserve the review-feedback commit subject and body"
    );
}

#[test]
fn workflow_pr_review_zero_bs_scan_covers_tracked_and_untracked_without_recursive_walks() {
    let command = step_command("workflow-pr-review", "step-19c-zero-bs-verification");
    assert!(
        command.contains("git grep -n -E") && command.contains("ERROR: git grep failed"),
        "step-19c must use fail-loud git grep scans for repository verification"
    );
    assert!(
        command.contains("git ls-files --others --exclude-standard -z")
            && command.contains("mapfile -d '' -t files")
            && command.contains("grep -n -E -H -- \"$pattern\" \"${files[@]}\"")
            && command.contains("ERROR: grep failed during untracked"),
        "step-19c must restore fail-loud untracked-file scanning"
    );
    assert!(
        command.contains("'*.rs'")
            && command.contains("'*.md'")
            && command.contains("'*.yaml'")
            && command.contains("'*.yml'"),
        "step-19c TODO/FIXME scanning must include source, config, and docs globs"
    );
    assert!(
        !command.contains("grep -r"),
        "step-19c must avoid repeated recursive filesystem grep scans"
    );
}

#[test]
fn base_branch_detection_remains_explicit_and_fail_loud() {
    for (recipe, step, resolver) in [
        (
            "workflow-worktree",
            "step-04-setup-worktree",
            "resolve_base_ref",
        ),
        (
            "workflow-publish",
            "step-16-create-draft-pr",
            "resolve_pr_base_ref",
        ),
    ] {
        let command = step_command(recipe, step);
        assert!(
            command.contains(resolver),
            "{recipe}/{step} must use explicit base-ref detection"
        );
        assert!(
            command.contains("refs/remotes/origin/HEAD")
                && command.contains("git remote set-head origin -a"),
            "{recipe}/{step} must detect the remote default branch instead of assuming main"
        );
        assert!(
            command.contains("origin/master origin/develop"),
            "{recipe}/{step} must preserve the tested non-main fallback order"
        );
        assert!(
            command.contains("ERROR: no supported remote base ref found"),
            "{recipe}/{step} must fail loudly when base-branch detection fails"
        );
        for forbidden in [
            "origin/main..HEAD",
            "reset --hard origin/main",
            "-b \"${BRANCH_NAME}\" origin/main",
        ] {
            assert!(
                !command.contains(forbidden),
                "{recipe}/{step} must not reintroduce hard-coded main fallback `{forbidden}`"
            );
        }
    }
}

/// MANDATORY steps from the v2 contract (0, 14, 17, 18) must remain present.
/// Listed explicitly so a regression that drops a mandatory gate is loud.
#[test]
fn mandatory_steps_still_present() {
    let composed: HashSet<String> = PHASE_RECIPES
        .iter()
        .flat_map(|name| load(name).steps.iter().map(|s| s.id.clone()))
        .collect();
    for required in [
        "step-00-workflow-preparation",
        "step-14-bump-version",
        "step-17a-compliance-verification",
        "step-18a-analyze-feedback",
    ] {
        assert!(
            composed.contains(required),
            "MANDATORY step '{required}' missing from decomposed workflow"
        );
    }
}

// ==========================================================================
// Issue #684: step-03 must detect remote host type and branch accordingly.
// TDD — these tests define the required contract BEFORE implementation.
// ==========================================================================

#[test]
fn workflow_prep_step_02d_detects_remote_host_type() {
    let command = step_command("workflow-prep", "step-02d-detect-host-type");

    // Must check for GitHub
    assert!(
        command.contains("github.com"),
        "step-02d must detect github.com in the remote URL"
    );

    // Must check for Azure DevOps (both modern and legacy domains)
    assert!(
        command.contains("dev.azure.com"),
        "step-02d must detect dev.azure.com in the remote URL"
    );
    assert!(
        command.contains("visualstudio.com"),
        "step-02d must detect visualstudio.com in the remote URL"
    );
}

#[test]
fn workflow_prep_step_03_consumes_remote_host_type() {
    let command = step_command("workflow-prep", "step-03-create-issue");

    // Step-03 must consume REMOTE_HOST_TYPE from step-02d (not re-detect)
    assert!(
        command.contains("REMOTE_HOST_TYPE"),
        "step-03 must consume REMOTE_HOST_TYPE from step-02d"
    );
}

#[test]
fn workflow_prep_step_03_accepts_azure_devops_host_alias() {
    let command = step_command("workflow-prep", "step-03-create-issue");

    assert!(
        command.contains("azdo"),
        "step-03 must continue to support the existing 'azdo' Azure DevOps host value"
    );
    assert!(
        command.contains("azure-devops"),
        "step-03 must also support the explicit 'azure-devops' host value from recipe context"
    );

    let alias_dispatch = command.contains("azdo|azure-devops")
        || command.contains("azure-devops|azdo")
        || (command.contains("\"azdo\"") && command.contains("\"azure-devops\""))
        || (command.contains("'azdo'") && command.contains("'azure-devops'"));
    assert!(
        alias_dispatch,
        "step-03 must dispatch 'azdo' and 'azure-devops' through the same Azure DevOps branch"
    );
}

#[test]
fn workflow_prep_step_03_reuses_existing_issue_number_before_provider_commands() {
    let command = step_command("workflow-prep", "step-03-create-issue");

    let issue_number_pos = command
        .find("ISSUE_NUMBER")
        .expect("step-03 must read existing ISSUE_NUMBER context before creating/searching issues");
    let first_gh_issue_pos = command
        .find("gh issue")
        .expect("step-03 must retain GitHub issue logic for GitHub hosts");
    let first_az_boards_pos = command
        .find("az boards")
        .expect("step-03 must retain Azure Boards create/lookup logic for Azure DevOps hosts");

    assert!(
        issue_number_pos < first_gh_issue_pos,
        "step-03 must check existing ISSUE_NUMBER before entering GitHub issue logic"
    );
    assert!(
        issue_number_pos < first_az_boards_pos,
        "step-03 must check existing ISSUE_NUMBER before Azure CLI work-item logic"
    );
    assert!(
        command.contains("AB#"),
        "step-03 must emit an Azure Boards reference such as AB#N when reusing existing work items"
    );
}

#[test]
fn workflow_prep_step_03_has_github_path_inside_host_conditional() {
    let command = step_command("workflow-prep", "step-03-create-issue");

    // gh issue commands must still exist (for GitHub remotes)
    assert!(
        command.contains("gh issue"),
        "step-03 must retain 'gh issue' commands for GitHub remotes"
    );

    // gh issue must NOT appear before HOST_TYPE is set from REMOTE_HOST_TYPE
    let host_type_pos = command
        .find("HOST_TYPE")
        .expect("HOST_TYPE must exist in step-03");
    let first_gh_issue_pos = command
        .find("gh issue")
        .expect("'gh issue' must exist in step-03");
    assert!(
        first_gh_issue_pos > host_type_pos,
        "step-03 must not invoke 'gh issue' before consuming the remote host type"
    );
}

#[test]
fn workflow_prep_step_03_has_azdo_work_item_path() {
    let command = step_command("workflow-prep", "step-03-create-issue");

    // Must have Azure DevOps work item creation path
    assert!(
        command.contains("az boards"),
        "step-03 must use 'az boards' for Azure DevOps work item creation"
    );

    // Must support explicit work item ID (AB#NNN pattern)
    assert!(
        command.contains("AB#"),
        "step-03 must support explicit Azure Boards work item references (AB#NNN)"
    );
}

#[test]
fn workflow_prep_step_03_has_local_tracking_fallback() {
    let command = step_command("workflow-prep", "step-03-create-issue");

    // Must have a fallback for unknown remote types (not GitHub, not AzDO)
    assert!(
        command.contains("local-tracking")
            || command.contains("LOCAL_ISSUE")
            || command.contains("local_issue")
            || command.contains("local tracking"),
        "step-03 must have a local tracking fallback for unknown remote types"
    );

    // Must NOT hard-fail for unknown remotes
    assert!(
        !command.contains("gh auth login"),
        "step-03 must not suggest 'gh auth login' for non-GitHub remotes"
    );
}

#[test]
fn workflow_prep_step_03_no_host_specific_error_messages_for_wrong_host() {
    let command = step_command("workflow-prep", "step-03-create-issue");

    // The old error "none of the git remotes configured for this repository
    // point to a known GitHub host" must not appear as a possible output
    // for non-GitHub remotes.
    assert!(
        !command.contains("none of the git remotes"),
        "step-03 must not contain GitHub-specific error messages that leak to non-GitHub remotes"
    );
}

#[test]
fn workflow_prep_step_03b_handles_azdo_work_item_urls() {
    let command = step_command("workflow-prep", "step-03b-extract-issue-number");

    // Must handle Azure DevOps work item URLs (_workitems/edit/NNN)
    assert!(
        command.contains("_workitems/edit/"),
        "step-03b must extract issue numbers from AzDO work item URLs (_workitems/edit/NNN)"
    );

    // Must handle AB#NNN references
    assert!(
        command.contains("AB#"),
        "step-03b must extract issue numbers from Azure Boards references (AB#NNN)"
    );

    // Must still handle GitHub issue URLs (regression check)
    assert!(
        command.contains("issues/"),
        "step-03b must still handle GitHub issue URLs (issues/NNN)"
    );

    // Must still handle GitHub PR URLs (regression check)
    assert!(
        command.contains("pull/"),
        "step-03b must still handle GitHub PR URLs (pull/NNN)"
    );

    // local-tracking extraction must come before task-desc AB# fallback
    let local_pos = command
        .find("local-tracking:")
        .expect("step-03b must handle local-tracking:NNN");
    let task_ab_pos = command
        .find("$TASK_DESC_RAW\" =~ AB#")
        .or_else(|| command.find("TASK_DESC_RAW =~ AB#"))
        .expect("step-03b must fall back to task_description AB#");
    assert!(
        local_pos < task_ab_pos,
        "step-03b must check ISSUE_CREATION local-tracking before falling back to TASK_DESC_RAW"
    );
}

#[test]
fn workflow_prep_step_03b_local_tracking_system_succeeds_without_numeric_issue_number() {
    let run = run_step_03b_extract_issue_number(
        "tracking_system=local\ntracking_reference=local-123\ntracking_issue=local-123\nissue_creation=local-tracking\n",
        "",
    );

    assert!(
        run.status.success(),
        "local tracking must skip numeric extraction and exit successfully; stderr:\n{}",
        run.stderr
    );
    assert_eq!(
        run.stdout, "",
        "local tracking must leave issue_number empty instead of coercing local-123 to 123"
    );
}

#[test]
fn workflow_prep_step_03b_local_reference_prefix_succeeds_without_numeric_issue_number() {
    let run = run_step_03b_extract_issue_number(
        "tracking_reference=local-123\ntracking_issue=local-123\nissue_creation=local-tracking\n",
        "",
    );

    assert!(
        run.status.success(),
        "local-* references must be treated as local tracking, not remote issue numbers; stderr:\n{}",
        run.stderr
    );
    assert_eq!(
        run.stdout, "",
        "local-* references must preserve the local identifier without emitting issue_number=123"
    );
}

#[test]
fn workflow_prep_step_03b_legacy_local_tracking_reference_is_not_coerced_to_issue_number() {
    let run = run_step_03b_extract_issue_number(
        "tracking_reference=local-tracking:123\ntracking_issue=local-tracking:123\n",
        "",
    );

    assert!(
        run.status.success(),
        "legacy local-tracking:* references must skip numeric extraction; stderr:\n{}",
        run.stderr
    );
    assert_eq!(
        run.stdout, "",
        "local-tracking:123 is a local identifier and must not emit issue_number=123"
    );
}

#[test]
fn workflow_prep_step_03b_remote_references_keep_numeric_extraction_behavior() {
    let cases = [
        (
            "github issue URL",
            "https://github.com/owner/repo/issues/456",
            "456",
        ),
        (
            "github pull request URL",
            "https://github.com/owner/repo/pull/789",
            "789",
        ),
        (
            "azure devops work item URL",
            "https://dev.azure.com/org/project/_workitems/edit/321",
            "321",
        ),
        ("azure boards shorthand", "AB#654", "654"),
    ];

    for (name, issue_creation, expected) in cases {
        let run = run_step_03b_extract_issue_number(issue_creation, "");
        assert!(
            run.status.success(),
            "{name} must still extract a numeric issue/work-item id; stderr:\n{}",
            run.stderr
        );
        assert_eq!(
            run.stdout, expected,
            "{name} must preserve existing numeric extraction behavior"
        );
    }
}

// ==========================================================================
// Issue #684: mid-stage steps must fall back to REPO_PATH when the worktree
// variable is not propagated from sub-recipe context (step-04).
// ==========================================================================

#[test]
fn mid_stage_worktree_steps_fall_back_to_repo_path() {
    // These steps are mid-pipeline — they SHOULD hard-fail when REPO_PATH is
    // also empty (via :?), but MUST accept REPO_PATH as a defensive fallback
    // so that context propagation failures between sub-recipes don't crash
    // the entire workflow when a valid repo directory is available.
    let cases = [
        ("workflow-publish", "step-15-commit-push"),
        ("workflow-publish", "step-16-create-draft-pr"),
        ("workflow-tdd", "checkpoint-after-implementation"),
        (
            "workflow-refactor-review",
            "checkpoint-after-review-feedback",
        ),
    ];
    for (recipe, step_id) in cases {
        let command = step_command(recipe, step_id);
        assert!(
            command.contains(":-${REPO_PATH:-}"),
            "{recipe}/{step_id} must fall back to $REPO_PATH when worktree var is empty"
        );
        // Must still hard-fail if BOTH worktree and REPO_PATH are empty
        assert!(
            command.contains("WORKTREE_SETUP_WORKTREE_PATH:?"),
            "{recipe}/{step_id} must hard-fail when worktree AND REPO_PATH are both empty"
        );
    }
}

#[test]
fn step_18c_does_not_fall_back_to_repo_path() {
    // step-18c is early-stage and must keep hard-fail (per test
    // workflow_pr_review_fails_loud_for_required_worktree_context).
    // It must NOT have a REPO_PATH fallback in the default chain.
    let command = step_command("workflow-pr-review", "step-18c-push-feedback-changes");
    assert!(
        !command.contains(":-${REPO_PATH"),
        "step-18c must NOT fall back to REPO_PATH — it requires the worktree"
    );
}

// ==========================================================================
// Issue #684: step-16 must handle non-GitHub remotes gracefully.
// ==========================================================================

#[test]
fn workflow_publish_step_16_guards_non_github_remotes() {
    let command = step_command("workflow-publish", "step-16-create-draft-pr");

    // step-16 must detect remote host type before creating a PR
    assert!(
        command.contains("REMOTE_HOST_TYPE")
            || command.contains("remote_host_type")
            || command.contains("github.com"),
        "step-16 must detect the remote host type before attempting PR creation"
    );

    // step-16 must skip PR creation for ALL non-GitHub remotes (not just AzDO)
    assert!(
        command.contains("!= \"github\""),
        "step-16 must skip PR creation for all non-GitHub remotes, not just AzDO"
    );
}

// ==========================================================================
// Issue #684 + #682: step-21 must guard PR_URL before gh commands.
// ==========================================================================

#[test]
fn workflow_finalize_step_21_guards_pr_url_before_gh_commands() {
    let command = step_command("workflow-finalize", "step-21-pr-ready");

    // Must reference PR_URL variable
    assert!(
        command.contains("PR_URL"),
        "step-21 must reference PR_URL to guard against empty values"
    );

    // Must check PR_URL is non-empty (whitespace-tolerant) before calling gh pr ready
    assert!(
        command.contains("-z \"$PR_URL\"")
            || command.contains("-n \"$PR_URL\"")
            || command.contains("[ -z \"${PR_URL")
            || command.contains("[ -n \"${PR_URL")
            || command.contains("[^[:space:]]"),
        "step-21 must check PR_URL is non-empty before invoking gh commands"
    );

    // gh pr ready must NOT be invoked unconditionally at the top level
    // It should be inside a conditional that checks PR_URL
    let pr_url_check_pos = command
        .find("PR_URL")
        .expect("PR_URL must appear in step-21");
    let gh_pr_ready_pos = command
        .find("gh pr ready")
        .expect("'gh pr ready' must still exist for GitHub PRs");
    assert!(
        gh_pr_ready_pos > pr_url_check_pos,
        "step-21 must check PR_URL before invoking 'gh pr ready'"
    );
}
