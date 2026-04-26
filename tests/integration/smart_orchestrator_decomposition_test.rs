//! Locks the smart-orchestrator brick decomposition into CI.
//!
//! These tests assert the v3 composer + 4 phase sub-recipes preserve
//! the v2 contract verbatim:
//!   * exact 29-step inventory in exact order
//!   * 4 sub-recipes in 4 calls, in order
//!   * brick rule (≤400 LOC) on every phase recipe
//!   * no duplicate step IDs across phases
//!   * critical routing/recovery steps still present

use serde::Deserialize;
use std::collections::HashSet;
use std::path::PathBuf;

const LIMIT: usize = 400;

const PHASE_RECIPES: &[&str] = &[
    "smart-classify-route",
    "smart-execute-routing",
    "smart-reflect-loop",
    "smart-validate-summarize",
];

const EXPECTED_STEP_INVENTORY: &[&str] = &[
    "preflight-validation",
    "classify-and-decompose",
    "parse-decomposition",
    "activate-workflow",
    "materialize-force-single-workstream",
    "materialize-allow-no-op",
    "setup-session",
    "handle-qa",
    "handle-ops-agent",
    "ops-file-change-check",
    "derive-recursion-guard",
    "execute-single-round-1-development",
    "execute-single-round-1-investigation",
    "create-workstreams-config",
    "launch-parallel-round-1",
    "execute-single-fallback-blocked-development",
    "execute-single-fallback-blocked-investigation",
    "detect-execution-gap",
    "file-routing-bug",
    "adaptive-execute-development",
    "adaptive-execute-investigation",
    "cleanup-helper",
    "reflect-round-1",
    "execute-round-2",
    "reflect-round-2",
    "execute-round-3",
    "reflect-final",
    "validate-outside-in-testing",
    "summarize",
    "complete-session",
];

/// Critical routing + recovery + reflection steps that must never be silently
/// dropped — they implement the orchestrator's safety guarantees.
const MANDATORY_STEPS: &[&str] = &[
    "preflight-validation",                          // input validation
    "classify-and-decompose",                        // task type detection
    "derive-recursion-guard",                        // recursion budget
    "execute-single-fallback-blocked-development",   // BLOCKED path
    "execute-single-fallback-blocked-investigation", // BLOCKED path
    "detect-execution-gap",                          // hollow-success detector
    "file-routing-bug",                              // observable failure
    "reflect-round-1",                               // goal-seeking gate
    "reflect-final",                                 // last-ditch reflection
    "validate-outside-in-testing",                   // outside-in gate
    "complete-session",                              // cleanup
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
            panic!("could not find amplifier-bundle from {:?}", crate_dir);
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
    let composer = load("smart-orchestrator");
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
fn critical_routing_and_recovery_steps_present() {
    let mut composed = HashSet::new();
    for name in PHASE_RECIPES {
        for s in load(name).steps {
            composed.insert(s.id);
        }
    }
    for must in MANDATORY_STEPS {
        assert!(
            composed.contains(*must),
            "MANDATORY step `{must}` missing from composed smart-orchestrator"
        );
    }
}
