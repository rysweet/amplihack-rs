//! Locks the investigation-workflow brick decomposition into CI.

use serde::Deserialize;
use std::collections::HashSet;
use std::path::PathBuf;

const LIMIT: usize = 400;

const PHASE_RECIPES: &[&str] = &[
    "investigation-prep",
    "investigation-explore",
    "investigation-verify",
    "investigation-report",
];

const EXPECTED_STEP_INVENTORY: &[&str] = &[
    "preflight-validation",
    "normalize-question",
    "init-tracking",
    "scope-definition",
    "clarify-ambiguities",
    "exploration-strategy",
    "check-past-investigations",
    "historical-research",
    "deep-dive-primary",
    "deep-dive-secondary",
    "deep-dive-tertiary",
    "deep-dive-specialist",
    "consolidate-findings",
    "formulate-hypotheses",
    "execute-verification",
    "validate-verification",
    "identify-patterns",
    "synthesis",
    "validate-synthesis",
    "update-discoveries",
    "update-patterns",
    "create-investigation-report",
    "transition-guidance",
    "efficiency-report",
    "final-output",
];

/// Critical investigation steps that must never be silently dropped.
const MANDATORY_STEPS: &[&str] = &[
    "preflight-validation",        // input validation
    "scope-definition",            // bound the investigation
    "exploration-strategy",        // structured exploration
    "consolidate-findings",        // converge deep dives
    "formulate-hypotheses",        // testable claims
    "execute-verification",        // verify hypotheses
    "validate-verification",       // verification quality gate
    "synthesis",                   // pattern → conclusion
    "validate-synthesis",          // synthesis quality gate
    "create-investigation-report", // observable output
    "final-output",                // user-facing result
];

#[derive(Debug, Deserialize)]
struct Step {
    id: String,
    #[serde(rename = "type")]
    step_type: Option<String>,
    recipe: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Recipe {
    steps: Vec<Step>,
}

fn recipes_dir() -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut dir = crate_dir.clone();
    while !dir.join("amplifier-bundle").exists() {
        if !dir.pop() {
            panic!("could not find amplifier-bundle from {crate_dir:?}");
        }
    }
    dir.join("amplifier-bundle/recipes")
}

fn load(name: &str) -> Recipe {
    let path = recipes_dir().join(format!("{name}.yaml"));
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
    serde_yaml::from_str(&raw).unwrap_or_else(|e| panic!("parse {path:?}: {e}"))
}

#[test]
fn composer_calls_four_phase_subrecipes_in_order() {
    let composer = load("investigation-workflow");
    let calls: Vec<&str> = composer
        .steps
        .iter()
        .filter(|s| s.step_type.as_deref() == Some("recipe"))
        .map(|s| s.recipe.as_deref().unwrap_or(""))
        .collect();
    assert_eq!(
        calls, PHASE_RECIPES,
        "composer must call exactly the 4 phase sub-recipes in order"
    );
}

#[test]
fn every_phase_subrecipe_loads_and_has_steps() {
    for name in PHASE_RECIPES {
        let r = load(name);
        assert!(!r.steps.is_empty(), "{name} must have steps");
    }
}

#[test]
fn composed_step_inventory_matches_expected_in_order() {
    let mut composed: Vec<String> = Vec::new();
    for name in PHASE_RECIPES {
        for s in load(name).steps {
            composed.push(s.id);
        }
    }
    let expected: Vec<String> = EXPECTED_STEP_INVENTORY
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(
        composed, expected,
        "composed step inventory must match v2 byte-for-byte"
    );
}

#[test]
fn no_duplicate_step_ids_across_subrecipes() {
    let mut seen = HashSet::new();
    for name in PHASE_RECIPES {
        for s in load(name).steps {
            assert!(
                seen.insert(s.id.clone()),
                "step id `{}` duplicated across phase recipes",
                s.id
            );
        }
    }
}

#[test]
fn every_phase_subrecipe_under_400_lines() {
    for name in PHASE_RECIPES {
        let path = recipes_dir().join(format!("{name}.yaml"));
        let n = std::fs::read_to_string(&path).unwrap().lines().count();
        assert!(
            n <= LIMIT,
            "{name}.yaml is {n} lines, must be ≤ {LIMIT} (brick rule)"
        );
    }
}

#[test]
fn mandatory_investigation_steps_present() {
    let mut composed = HashSet::new();
    for name in PHASE_RECIPES {
        for s in load(name).steps {
            composed.insert(s.id);
        }
    }
    for must in MANDATORY_STEPS {
        assert!(
            composed.contains(*must),
            "MANDATORY step `{must}` missing from composed investigation-workflow"
        );
    }
}
