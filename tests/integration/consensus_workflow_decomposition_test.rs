//! Consensus-workflow decomposition parity tests.
//!
//! `consensus-workflow.yaml` was decomposed (v3.0.0) from a 2492-line monolith
//! into a thin composer that calls 11 phase sub-recipes. The contract is the
//! same 15-step / 7-consensus-gate flow as v2; every debate round, panel
//! perspective, and consensus gate is preserved verbatim.
//!
//! The user's design constraint: the LLM converges on correct behaviour
//! through many recursive review layers, so no review layer may be silently
//! dropped. These tests lock that contract — if a future edit drops a step,
//! reorders the inventory, duplicates an ID, or breaks the brick budget
//! (≤400 LOC per sub-recipe), CI fails before the regression ships.

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
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

const PHASE_RECIPES: &[&str] = &[
    "consensus-preflight",
    "consensus-requirements-debate",
    "consensus-issue-worktree",
    "consensus-design",
    "consensus-tdd-impl",
    "consensus-panel-cleanup",
    "consensus-test-fix",
    "consensus-publish",
    "consensus-pr-review",
    "consensus-pr-feedback",
    "consensus-merge-finalize",
];

const EXPECTED_STEP_INVENTORY: &[&str] = &[
    // consensus-preflight
    "step0-preflight",
    "step1-requirements-analysis",
    "step1-codebase-analysis",
    // consensus-requirements-debate
    "step1-debate-round1-architect",
    "step1-debate-round1-security",
    "step1-debate-round1-api",
    "step1-debate-round1-database",
    "step1-debate-round1-tester",
    "step1-debate-round2-cross-examination",
    "step1-debate-round3-consensus",
    "step1-finalize-requirements",
    // consensus-issue-worktree
    "step2-create-issue",
    "step3-setup-worktree",
    // consensus-design (round 1: 5 perspectives, round 2: cross-exam, round 3: consensus)
    "step4-design-round1-architect",
    "step4-design-round1-api",
    "step4-design-round1-database",
    "step4-design-round1-security",
    "step4-design-round1-tester",
    "step4-design-round2-cross-examination",
    "step4-design-round3-consensus",
    // consensus-tdd-impl (tests + standard impl + N-version 3 builders + vote)
    "step4-write-tests",
    "step5-standard-implementation",
    "step5-nversion-identify-critical",
    "step5-nversion-builder1-conservative",
    "step5-nversion-builder2-pragmatic",
    "step5-nversion-builder3-minimalist",
    "step5-nversion-compare",
    "step5-nversion-synthesize",
    "step5-nversion-vote",
    // consensus-panel-cleanup (4-perspective panel + apply)
    "step6-panel-cleanup",
    "step6-panel-optimizer",
    "step6-panel-reviewer",
    "step6-panel-patterns",
    "step6-panel-consensus",
    "step6-apply-refactoring",
    // consensus-test-fix
    "step7-run-tests",
    "step7-fix-issues",
    "step8-local-testing",
    // consensus-publish
    "step9-commit",
    "step10-create-pr",
    // consensus-pr-review (5-perspective panel + consensus + first feedback step)
    "step11-pr-review-reviewer",
    "step11-pr-review-security",
    "step11-pr-review-optimizer",
    "step11-pr-review-patterns",
    "step11-pr-review-tester",
    "step11-pr-review-consensus",
    "step12-implement-feedback",
    // consensus-pr-feedback (push + 4-perspective philosophy panel)
    "step12-push-updates",
    "step13-philosophy-reviewer",
    "step13-philosophy-patterns",
    "step13-philosophy-cleanup",
    "step13-philosophy-guardian",
    // consensus-merge-finalize (CI + mergeable + final-quality panel)
    "step14-check-ci",
    "step14-diagnose-ci",
    "step14-verify-mergeable",
    "step15-final-cleanup",
    "step15-final-reviewer",
    "step15-final-patterns",
    "step15-final-consensus",
    "final-output",
];

#[test]
fn composer_calls_eleven_phase_subrecipes_in_order() {
    let composer = load("consensus-workflow");
    assert_eq!(composer.name, "consensus-workflow");
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
            "composer step {i}: expected recipe '{expected}', got {:?}",
            step.recipe
        );
        assert_eq!(step.id, *expected, "composer step id mismatch at {i}");
    }
}

#[test]
fn every_phase_subrecipe_loads_and_has_steps() {
    for name in PHASE_RECIPES {
        let r = load(name);
        assert_eq!(&r.name, name);
        assert!(!r.steps.is_empty(), "{name} must declare ≥1 step");
    }
}

#[test]
fn composed_step_inventory_matches_expected_in_order() {
    let mut composed: Vec<String> = Vec::new();
    for name in PHASE_RECIPES {
        composed.extend(load(name).steps.into_iter().map(|s| s.id));
    }
    assert_eq!(
        composed.len(),
        EXPECTED_STEP_INVENTORY.len(),
        "composed has {} steps, expected {}",
        composed.len(),
        EXPECTED_STEP_INVENTORY.len()
    );
    for (i, (c, e)) in composed.iter().zip(EXPECTED_STEP_INVENTORY).enumerate() {
        assert_eq!(c, e, "step #{i}: composed='{c}', expected='{e}'");
    }
}

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
    assert!(dups.is_empty(), "duplicate step IDs: {dups:?}");
}

#[test]
fn every_phase_subrecipe_under_400_lines() {
    const LIMIT: usize = 400;
    let mut violations: Vec<(String, usize)> = Vec::new();
    for name in PHASE_RECIPES
        .iter()
        .chain(std::iter::once(&"consensus-workflow"))
    {
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

/// The 7 mandatory consensus gates documented in the v2 header must remain
/// present after decomposition. Each gate maps to a specific step ID in the
/// composed inventory.
#[test]
fn seven_mandatory_consensus_gates_present() {
    let composed: HashSet<String> = PHASE_RECIPES
        .iter()
        .flat_map(|n| load(n).steps.into_iter().map(|s| s.id))
        .collect();
    // (gate label, representative step ID that proves the gate exists)
    let gates: &[(&str, &str)] = &[
        (
            "Gate 1: Requirements debate (3 rounds)",
            "step1-debate-round3-consensus",
        ),
        (
            "Gate 2: Design consensus (3 rounds)",
            "step4-design-round3-consensus",
        ),
        (
            "Gate 3: N-version implementation vote",
            "step5-nversion-vote",
        ),
        ("Gate 4: Refactoring expert panel", "step6-panel-consensus"),
        (
            "Gate 5: PR-review expert panel",
            "step11-pr-review-consensus",
        ),
        (
            "Gate 6: Philosophy compliance panel",
            "step13-philosophy-guardian",
        ),
        ("Gate 7: Final quality panel", "step15-final-consensus"),
    ];
    for (label, step) in gates {
        assert!(
            composed.contains(*step),
            "MANDATORY {label}: step '{step}' missing from decomposed workflow"
        );
    }
}
