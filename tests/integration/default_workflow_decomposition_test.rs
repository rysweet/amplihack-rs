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
}

fn recipes_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("amplifier-bundle")
        .join("recipes")
}

fn load(name: &str) -> Recipe {
    let path = recipes_dir().join(format!("{name}.yaml"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_yaml::from_str(&text)
        .unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
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
    "step-08c-implementation-no-op-guard",
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
        assert!(
            !r.steps.is_empty(),
            "{name} must declare at least one step"
        );
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
    const LIMIT: usize = 400;
    let mut violations: Vec<(String, usize)> = Vec::new();
    for name in PHASE_RECIPES.iter().chain(std::iter::once(&"default-workflow")) {
        let path = recipes_dir().join(format!("{name}.yaml"));
        let lines = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
            .lines()
            .count();
        if lines > LIMIT {
            violations.push((name.to_string(), lines));
        }
    }
    assert!(
        violations.is_empty(),
        "brick rule violation (>{LIMIT} LOC): {violations:?}"
    );
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
