//! TDD-red contracts for conservative hygiene cleanup automation.
//!
//! The cleanup command must be opt-in, dry-run by default, and guarded against
//! deleting active worktrees, current repo targets, running sessions, or recent
//! session artifacts.

use amplihack_cli::Cli;
use clap::CommandFactory;

#[test]
fn top_level_help_exposes_hygiene_cleanup_command() {
    let mut command = Cli::command();
    let help = command.render_long_help().to_string();

    assert!(
        help.contains("hygiene"),
        "top-level help must expose the hygiene command"
    );
    assert!(
        help.contains("cleanup"),
        "top-level help must make the cleanup surface discoverable"
    );
}

#[test]
fn cleanup_accepts_explicit_categories_repo_and_age_without_apply() {
    let parsed = Cli::try_parse_from([
        "amplihack",
        "hygiene",
        "cleanup",
        "--worktrees",
        "--cargo-targets",
        "--sessions",
        "--older-than",
        "14d",
        "--repo",
        "/tmp/example-repo",
    ]);

    assert!(
        parsed.is_ok(),
        "hygiene cleanup must parse explicit categories, --older-than, and --repo while defaulting to dry-run: {parsed:?}"
    );
}

#[test]
fn cleanup_rejects_apply_without_cleanup_category() {
    let error = Cli::try_parse_from([
        "amplihack",
        "hygiene",
        "cleanup",
        "--apply",
        "--older-than",
        "14d",
    ])
    .expect_err("--apply without at least one cleanup category must be rejected");

    let message = error.to_string();
    assert!(
        message.contains("category")
            && message.contains("--worktrees")
            && message.contains("--cargo-targets")
            && message.contains("--sessions"),
        "error must explain that --apply requires an explicit cleanup category; got: {message}"
    );
}

#[test]
fn cleanup_rejects_apply_without_age_guardrail() {
    let error = Cli::try_parse_from(["amplihack", "hygiene", "cleanup", "--apply", "--worktrees"])
        .expect_err("--apply without --older-than must be rejected");

    let message = error.to_string();
    assert!(
        message.contains("--older-than") && message.contains("guardrail"),
        "error must explain that destructive cleanup requires an --older-than guardrail; got: {message}"
    );
}

#[test]
fn cleanup_exposes_safety_skip_reasons_in_contract_help() {
    let error = Cli::try_parse_from(["amplihack", "hygiene", "cleanup", "--help"])
        .expect_err("clap help exits through an error-like display result");
    let help = error.to_string();

    for required in [
        "dry-run",
        "--apply",
        "active worktree",
        "current repository",
        "running session",
        "recent session",
    ] {
        assert!(
            help.contains(required),
            "hygiene cleanup help must document safety guard `{required}`"
        );
    }
}
