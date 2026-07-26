//! Issue #1025 — the goal-seeking reflection loop kept running expensive
//! reflection/re-validation rounds long after the workstream's deliverable PR
//! was already open and green, because the reviewer (`amplihack:core:reviewer`)
//! reflect steps had no authoritative "PR is green -> done" short-circuit and
//! relied only on local validation (which can false-fail, e.g. a test scanning
//! sibling `./worktrees/` copies).
//!
//! The recipe-level fix adds a config-guarded, safety-first early-exit
//! instruction to every reviewer/reflect step in `smart-reflect-loop.yaml`:
//! when the deliverable PR is definitively OPEN and all required checks are
//! green, conclude `GOAL_STATUS: ACHIEVED` and stop; otherwise
//! (pending / failing / closed / draft / no PR / unknown) evaluate normally
//! and prefer continuing.
//!
//! These tests lock that guard — and its four decision cases — into CI so a
//! future prompt edit cannot silently drop the short-circuit or, worse,
//! short-circuit on a non-green state.

use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
struct Recipe {
    steps: Vec<Step>,
}

#[derive(Debug, Deserialize)]
struct Step {
    id: String,
    #[serde(default)]
    agent: String,
    #[serde(default)]
    prompt: String,
}

/// The reviewer/reflect steps that emit `GOAL_STATUS` and therefore gate
/// whether another round runs.
const REVIEWER_STEPS: &[&str] = &["reflect-round-1", "reflect-round-2", "reflect-final"];

/// The full 5-step inventory of the reflect loop. The #1025 fix must be a
/// prompt-only change: it must NOT add, remove, or rename steps (the
/// smart_orchestrator_decomposition contract test locks the global inventory).
const EXPECTED_STEPS: &[&str] = &[
    "reflect-round-1",
    "execute-round-2",
    "reflect-round-2",
    "execute-round-3",
    "reflect-final",
];

/// Env flag that disables the early-exit (fail-safe kill switch).
const DISABLE_FLAG: &str = "AMPLIHACK_DISABLE_PR_GREEN_SHORTCIRCUIT";

fn load_reflect_loop() -> Recipe {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut dir = crate_dir.clone();
    while !dir.join("amplifier-bundle").exists() {
        if !dir.pop() {
            panic!("could not find amplifier-bundle from {crate_dir:?}");
        }
    }
    let path = dir.join("amplifier-bundle/recipes/smart-reflect-loop.yaml");
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
    serde_yaml::from_str(&raw).unwrap_or_else(|e| panic!("parse {path:?}: {e}"))
}

fn step<'a>(recipe: &'a Recipe, id: &str) -> &'a Step {
    recipe
        .steps
        .iter()
        .find(|s| s.id == id)
        .unwrap_or_else(|| panic!("step {id} not found in smart-reflect-loop.yaml"))
}

#[test]
fn reflect_loop_step_inventory_is_unchanged_by_the_fix() {
    let recipe = load_reflect_loop();
    let ids: Vec<&str> = recipe.steps.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(
        ids, EXPECTED_STEPS,
        "#1025 must be a prompt-only change: reflect-loop step inventory changed"
    );
}

#[test]
fn every_reviewer_step_has_the_pr_green_early_exit_guard() {
    let recipe = load_reflect_loop();
    for id in REVIEWER_STEPS {
        let s = step(&recipe, id);
        assert_eq!(
            s.agent, "amplihack:core:reviewer",
            "{id} is expected to be the GOAL_STATUS-emitting reviewer step"
        );
        let p = &s.prompt;
        assert!(
            p.contains("issue #1025"),
            "{id} is missing the #1025 early-exit guard marker"
        );
        // Case (a): PR open + all checks green -> ACHIEVED (stop).
        assert!(
            p.contains("ACHIEVED") && p.to_lowercase().contains("green"),
            "{id} guard must map an open+green PR to GOAL_STATUS: ACHIEVED"
        );
        // The guard must tell the reviewer HOW to check authoritative required
        // checks, not broad optional status rollups or a local re-build.
        assert!(
            p.contains("gh pr checks --required"),
            "{id} guard must query required PR CI status via gh pr checks --required"
        );
        assert!(
            p.contains("gh pr view --json state,isDraft"),
            "{id} guard must query PR open/draft state separately"
        );
        // Kill switch so the behavior can be disabled without a code change.
        assert!(
            p.contains(DISABLE_FLAG),
            "{id} guard must be gated by {DISABLE_FLAG}"
        );
        // Safety: never short-circuit on a non-green state.
        assert!(
            p.contains("NEVER"),
            "{id} guard must forbid short-circuiting on non-green states"
        );
    }
}

/// The most detailed guard (round 1) must explicitly enumerate all four
/// decision cases so the "fail toward continuing" contract is unambiguous:
///   (a) green    -> ACHIEVED / stop
///   (b) pending  -> continue
///   (c) failing  -> continue
///   (d) no PR    -> continue
#[test]
fn round_one_guard_enumerates_all_four_decision_cases() {
    let recipe = load_reflect_loop();
    let p = step(&recipe, "reflect-round-1").prompt.to_lowercase();
    assert!(p.contains("green"), "case (a) open+green missing");
    assert!(p.contains("pending"), "case (b) pending-checks missing");
    assert!(p.contains("failing"), "case (c) failing-checks missing");
    assert!(p.contains("no pr"), "case (d) no-PR-yet missing");
    assert!(
        p.contains("empty"),
        "guard must not treat an empty required-check set as green"
    );
    assert!(
        p.contains("fail toward continuing"),
        "guard must state the fail-safe default of continuing the loop"
    );
}

/// The short-circuit must live only in the reviewer/reflect steps — the
/// builder `execute-round-*` steps must not carry it (they do work; they do
/// not decide goal status).
#[test]
fn executor_steps_do_not_carry_the_early_exit_guard() {
    let recipe = load_reflect_loop();
    for id in ["execute-round-2", "execute-round-3"] {
        let s = step(&recipe, id);
        assert!(
            !s.prompt.contains("issue #1025"),
            "{id} (a builder step) must not carry the reviewer early-exit guard"
        );
    }
}
