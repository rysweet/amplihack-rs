//! #1714 crusty round 4, counterpart to `probe.rs`: the terminal-state
//! probe's fast-forward arm does not widen into stale metadata. A diverged PR
//! head, a rewound remote, and a probe run without `accept_pr_head_ahead`
//! still end `FAILED_INVALID_INPUT`, and the validator never hands that state
//! to the loops, even inside auto-drive.

use std::fs;

use serde_json::Value;

use super::ancestor::commit_run_work;
use super::fixture::*;
use super::probe::{answer, assert_stale, merged_on_remote_before_finalize, probe, validate_after};

/// Assert that the validator, even in auto-drive, fails closed on `json`.
fn assert_invalid_input_not_handed_off(fx: &Fixture, json: &Value, why: &str) {
    let outcome = validate_after(fx, json, true);
    assert!(!outcome.success(), "{why}\n{}", outcome.describe());
    assert_eq!(
        outcome.field("terminal_state"),
        "FAILED_INVALID_INPUT",
        "{why}"
    );
    assert_no_handoff_key(&outcome, why);
}

// The counterpart: a PR head that does not contain the local HEAD is still
// stale metadata, and FAILED_INVALID_INPUT is never handed off.
#[test]
fn a_diverged_pr_head_still_ends_failed_invalid_input_without_hand_off() {
    let fx = Fixture::new();
    commit_run_work(&fx);
    fx.git(&["switch", "-q", "-c", "sibling", "main"]);
    fs::write(fx.repo.join("OTHER.md"), "other\n").expect("write other");
    fx.git(&["add", "OTHER.md"]);
    fx.git(&["commit", "-q", "-m", "other work"]);
    let diverged = fx.head();
    fx.git(&["switch", "-q", BRANCH]);
    answer(&fx, &diverged, green_rollup());
    for named in [true, false] {
        let p = probe(&fx, named, true);
        let json = assert_stale(&p, &format!("diverged, named={named}"));
        assert_invalid_input_not_handed_off(&fx, &json, &format!("diverged, named={named}"));
    }
}

// origin/<branch> once held a commit that M does not contain: the remote was
// rewound or rewritten, so the head is stale even though L is M's ancestor.
#[test]
fn a_rewound_remote_still_ends_failed_invalid_input() {
    let (fx, _, pr_head) = merged_on_remote_before_finalize();
    fx.git(&["switch", "-q", "-c", "lost", BRANCH]);
    fs::write(fx.repo.join("LOST.md"), "pushed, then removed\n").expect("write lost");
    fx.git(&["add", "LOST.md"]);
    fx.git(&["commit", "-q", "-m", "pushed then rewound"]);
    let lost = fx.head();
    fx.git(&["switch", "-q", BRANCH]);
    fx.git(&[
        "update-ref",
        "--create-reflog",
        &format!("refs/remotes/origin/{BRANCH}"),
        &lost,
    ]);
    answer(&fx, &pr_head, green_rollup());
    let p = probe(&fx, false, true);
    let json = assert_stale(&p, "rewound");
    assert!(
        p.stderr.contains("history rewritten or rewound"),
        "{}",
        p.describe()
    );
    assert_invalid_input_not_handed_off(&fx, &json, "rewound");
}

// Only workflow-finalize opts in. Run as workflow-publish runs it, the probe
// keeps the exact-head rule, and the recipes say so.
#[test]
fn only_workflow_finalize_accepts_a_pr_head_ahead() {
    let (fx, _, pr_head) = merged_on_remote_before_finalize();
    answer(&fx, &pr_head, green_rollup());
    for named in [true, false] {
        assert_stale(
            &probe(&fx, named, false),
            &format!("opt-out, named={named}"),
        );
    }
    let finalize_recipe = recipe_yaml("workflow-finalize");
    let finalize = step(&finalize_recipe, "finalize-terminal-state");
    assert_eq!(
        finalize["context"]["accept_pr_head_ahead"].as_str(),
        Some("true")
    );
    let probe_recipe = recipe_yaml("workflow-terminal-state");
    assert_eq!(
        probe_recipe["context"]["accept_pr_head_ahead"].as_str(),
        Some("false")
    );
    assert!(
        !read(&recipe_path("workflow-publish")).contains("accept_pr_head_ahead"),
        "workflow-publish pushes without pulling, so it must not opt in"
    );
}
