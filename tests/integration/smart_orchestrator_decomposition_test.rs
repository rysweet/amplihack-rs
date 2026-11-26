//! Checks on the smart-orchestrator phase sub-recipes.
//!
//!   * every phase recipe parses and has steps
//!   * brick rule (≤400 LOC) on every phase recipe
//!   * no duplicate step IDs across phases
//!   * the quick-change conditions on reflection and validation (#1740, D1)
//!
//! The checks that pinned the step inventory, its order and the composer's
//! calls were removed under #3129: tests check values and behaviour, never
//! wording, step lists or step order (Ryan, 23 Nov 2026).

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

#[derive(Debug, Deserialize)]
struct Step {
    id: String,
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
fn every_phase_subrecipe_loads_and_has_steps() {
    for name in PHASE_RECIPES {
        let r = load(name);
        assert!(!r.steps.is_empty(), "{name} must have steps");
    }
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

/// Issue #1740 (D1): a quick-change run skips orchestrator reflection and
/// outside-in validation; summarize and complete-session still run.
const SKIPPED_AFTER_QUICK_CHANGE: &[(&str, &str)] = &[
    ("smart-reflect-loop", "reflect-round-1"),
    ("smart-reflect-loop", "execute-round-2"),
    ("smart-reflect-loop", "reflect-round-2"),
    ("smart-reflect-loop", "execute-round-3"),
    ("smart-reflect-loop", "reflect-final"),
    ("smart-validate-summarize", "validate-outside-in-testing"),
];

fn raw_condition(recipe: &str, id: &str) -> String {
    let path = recipes_dir().join(format!("{recipe}.yaml"));
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
    let doc: serde_yaml::Value =
        serde_yaml::from_str(&raw).unwrap_or_else(|e| panic!("parse {path:?}: {e}"));
    let step = doc["steps"]
        .as_sequence()
        .expect("steps")
        .iter()
        .find(|s| s["id"].as_str() == Some(id))
        .unwrap_or_else(|| panic!("{recipe}: step {id} missing"));
    step["condition"]
        .as_str()
        .unwrap_or("")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn reflection_and_validation_exclude_quick_change() {
    for (recipe, id) in SKIPPED_AFTER_QUICK_CHANGE {
        let c = raw_condition(recipe, id);
        assert!(
            c.contains("and selected_workflow != 'quick-change'"),
            "{recipe}/{id} must exclude quick-change runs: {c}"
        );
    }
    for id in ["summarize", "complete-session"] {
        let c = raw_condition("smart-validate-summarize", id);
        assert!(!c.contains("selected_workflow"), "{id} must still run: {c}");
    }
}

#[test]
fn quick_change_round_1_requires_the_selection() {
    let c = raw_condition(
        "smart-execute-routing",
        "execute-single-round-1-quick-change",
    );
    assert!(c.contains("selected_workflow == 'quick-change'"), "{c}");
    let c = raw_condition(
        "smart-execute-routing",
        "execute-single-round-1-development",
    );
    assert!(c.contains("selected_workflow != 'quick-change'"), "{c}");
}
