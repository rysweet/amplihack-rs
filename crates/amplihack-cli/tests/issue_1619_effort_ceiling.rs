//! Issue #1619, B3(c) (TDD): the shipped `strong` effort steps down rather
//! than failing on an account whose model does not accept `xhigh`.
//!
//! The review asked that the Codex `strong` tier fall back rather than fail
//! there. Amplihack cannot learn what a model accepts without making a live
//! call, and the brief excludes probing, retries and timers, so the ceiling
//! is a declaration about the operator's own account: an optional
//! `max_effort` inside a column, applied before the effort is written.
//!
//! The shipped `codex.strong = "xhigh"` is unchanged. The issue's 3 November
//! comment requires `xhigh` for orchestration and formal steps; an account
//! that cannot run it says so in its own file.
//!
//! These start the real `amplihack` binary against an argv-recording fake
//! harness, so the assertion is over what Codex is actually told.

#![cfg(target_os = "linux")]

use crate::support::tier_harness::Fixture;

#[test]
fn a_declared_ceiling_steps_the_strong_tier_down_on_codex() {
    let f = Fixture::new();
    f.write_override("version = 1\n[codex]\nmax_effort = \"high\"\n");
    let launch = f.run("codex", &["exec", "--model", "strong", "-"]);
    assert_eq!(
        launch.effort(),
        Some("high"),
        "the shipped `strong` asks for xhigh and this account declares high as its \
         ceiling. argv: {:?}",
        launch.argv
    );
}

#[test]
fn a_capped_step_runs_rather_than_failing() {
    let f = Fixture::new();
    f.write_override("version = 1\n[codex]\nmax_effort = \"low\"\n");
    let launch = f.run("codex", &["exec", "--model", "strong", "-"]);
    assert!(
        launch.output.status.success(),
        "stepping an effort down is not a failure. stderr:\n{}",
        launch.stderr()
    );
    assert!(
        launch.argv.iter().any(|a| a == "exec"),
        "and the step still reaches the harness. argv: {:?}",
        launch.argv
    );
}

#[test]
fn a_ceiling_never_raises_an_effort() {
    let f = Fixture::new();
    f.write_override("version = 1\n[codex]\nmax_effort = \"xhigh\"\n");
    for (tier, expected) in [("fast", "low"), ("standard", "high"), ("strong", "xhigh")] {
        let launch = f.run("codex", &["exec", "--model", tier, "-"]);
        assert_eq!(
            launch.effort(),
            Some(expected),
            "`{tier}` is at or below the ceiling and must be untouched. argv: {:?}",
            launch.argv
        );
    }
}

#[test]
fn no_ceiling_leaves_the_shipped_efforts_alone() {
    let f = Fixture::new();
    for (tier, expected) in [("fast", "low"), ("standard", "high"), ("strong", "xhigh")] {
        let launch = f.run("codex", &["exec", "--model", tier, "-"]);
        assert_eq!(
            launch.effort(),
            Some(expected),
            "the shipped mapping declares no ceiling. argv: {:?}",
            launch.argv
        );
    }
}

#[test]
fn a_max_effort_that_is_not_an_effort_is_reported_and_changes_nothing() {
    let f = Fixture::new();
    f.write_override("version = 1\n[codex]\nmax_effort = \"enormous\"\n");
    let launch = f.run("codex", &["exec", "--model", "strong", "-"]);
    assert_eq!(
        launch.effort(),
        Some("xhigh"),
        "a typo fails open: applying some default cap instead would silently change \
         what every Codex step runs at. argv: {:?}",
        launch.argv
    );
    assert!(
        !launch.stderr().is_empty(),
        "and the operator is told where they wrote it"
    );
}

#[test]
fn a_ceiling_never_becomes_a_model_or_another_argument() {
    let f = Fixture::new();
    f.write_override("version = 1\n[codex]\nmax_effort = \"medium\"\n");
    let launch = f.run("codex", &["exec", "--model", "strong", "-"]);
    assert!(
        !launch.argv.iter().any(|a| a == "max_effort"),
        "the key is config, not argv. argv: {:?}",
        launch.argv
    );
    assert_ne!(
        launch.model(),
        Some("medium"),
        "an effort is not a model. argv: {:?}",
        launch.argv
    );
}

/// Claude Code has no launch control for reasoning effort, so a declared
/// effort there is recorded and documented as not a launch control. A ceiling
/// must not invent one.
#[test]
fn a_ceiling_writes_no_effort_onto_a_harness_that_has_no_control_for_one() {
    let f = Fixture::new();
    f.write_override(
        "version = 1\n[claude]\nmax_effort = \"low\"\n[claude.strong]\n\
         model = \"claude-opus-5[1m]\"\neffort = \"xhigh\"\n",
    );
    let launch = f.run("claude", &["--model", "strong", "-p"]);
    assert_eq!(
        launch.effort(),
        None,
        "no reasoning-effort argument belongs on a Claude command line. argv: {:?}",
        launch.argv
    );
    assert_eq!(launch.model(), Some("claude-opus-5[1m]"));
}
