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
use std::collections::HashSet;
use std::path::PathBuf;

const BRICK_LIMIT: usize = 400;

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

fn recipe_text(name: &str) -> String {
    let path = recipe_path(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn load(name: &str) -> Recipe {
    let path = recipe_path(name);
    let text = recipe_text(name);
    serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

fn load_yaml(name: &str) -> serde_yaml::Value {
    let path = recipe_path(name);
    let text = recipe_text(name);
    serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

fn step_command(recipe: &str, step_id: &str) -> String {
    load(recipe)
        .steps
        .into_iter()
        .find(|step| step.id == step_id)
        .unwrap_or_else(|| panic!("{recipe}.yaml missing step {step_id}"))
        .command
        .unwrap_or_else(|| panic!("{recipe}.yaml step {step_id} must be a bash step"))
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
    // Phase 6: workflow-publish (steps 14-16b)
    "step-14-bump-version",
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
    // Phase 8: workflow-finalize (steps 20-22b + complete)
    "step-20-final-cleanup",
    "step-20b-push-cleanup",
    "step-20c-quality-audit",
    "step-21-pr-ready",
    "step-22-ensure-mergeable",
    "step-22b-final-status",
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
        for step in load(name).steps {
            if !seen.insert(step.id.clone()) {
                dups.push(step.id);
            }
        }
    }
    assert!(dups.is_empty(), "duplicate step IDs found: {dups:?}");
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
        .into_iter()
        .map(|step| step.id)
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

    for (step, command) in [
        ("step-18c-push-feedback-changes", push_command.as_str()),
        ("step-19c-zero-bs-verification", zero_bs_command.as_str()),
    ] {
        assert!(
            command.contains("set -euo pipefail"),
            "{step} must fail loudly on shell errors"
        );
        assert!(
            command.contains("WORKTREE_SETUP_WORKTREE_PATH:?"),
            "{step} must require worktree_setup.worktree_path instead of inventing a default"
        );
    }

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
        .flat_map(|name| load(name).steps.into_iter().map(|s| s.id))
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
