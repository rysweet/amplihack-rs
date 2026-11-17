//! Issue #1619, B3(b) (TDD): two bundled steps are on the wrong tier.
//!
//! The first cut puts mechanical steps on `fast` — task read-back,
//! prerequisite lookups, worktree setup, loop-result verification, summaries
//! and cleanup — and keeps design, implementation and debugging on `strong`.
//! Two steps were classed as mechanical that are not: each verifies a pull
//! request's head against local state, adjudicates who wrote a review
//! comment, and reports blockers a later step acts on. That is judgement, and
//! a wrong answer sends a drive at the wrong commit or treats an unresolved
//! review as settled.
//!
//! The tier a step declares is the whole of the feature's input, so it is
//! asserted on the recipe rather than on any prose about it.

use serde_yaml::Value;
use std::path::{Path, PathBuf};

fn recipes_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../amplifier-bundle/recipes")
}

fn load(name: &str) -> Value {
    assert!(
        !name.is_empty() && !name.contains("..") && !name.contains('/') && !name.contains('\\'),
        "unsafe recipe reference: {name}"
    );
    let path = recipes_dir().join(format!("{name}.yaml"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
    serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("{path:?}: {e}"))
}

/// The tier one named step of one recipe declares.
fn tier_of(recipe: &str, step_id: &str) -> Option<String> {
    let doc = load(recipe);
    let steps = doc["steps"]
        .as_sequence()
        .unwrap_or_else(|| panic!("{recipe}: no steps"));
    let step = steps
        .iter()
        .find(|step| step["id"].as_str() == Some(step_id))
        .unwrap_or_else(|| panic!("{recipe}: no step `{step_id}`"));
    step["model"].as_str().map(str::to_string)
}

#[test]
fn the_crusty_round_context_step_is_not_mechanical() {
    assert_eq!(
        tier_of("autodrive-crusty-round", "step-01-round-context").as_deref(),
        Some("standard"),
        "this step verifies a PR's actual head against local state, recovers a stale \
         or unpublished head without discarding work, and refuses to review against \
         a different commit. A wrong answer reviews the wrong code."
    );
}

#[test]
fn the_crusty_evidence_step_is_not_mechanical() {
    assert_eq!(
        tier_of("autodrive-merge-round", "step-01b-crusty-evidence").as_deref(),
        Some("standard"),
        "this step adjudicates a review verdict from the PR's own comments, deciding \
         comment authorship with no tool parsing it and no state file to fall back \
         on. A wrong answer treats an unresolved review as settled."
    );
}
