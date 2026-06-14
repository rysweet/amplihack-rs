//! TDD contract tests for automated GitHub Actions failure investigations.
//!
//! These tests are written before implementation. They pin the behavior needed
//! for reports like "many scheduled action runs are failing": prove true
//! schedule-event evidence first, classify actual failed automation from gh
//! logs, fix only repo-caused failures, and close through a PR with validation.

use std::path::PathBuf;

fn repo_root() -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut dir = crate_dir.clone();
    while !dir.join("amplifier-bundle").exists() {
        if !dir.pop() {
            panic!("could not find amplifier-bundle from {crate_dir:?}");
        }
    }
    dir
}

fn workflow_recipe_contract_text() -> String {
    let recipes_dir = repo_root().join("amplifier-bundle/recipes");
    let mut combined = String::new();
    for entry in std::fs::read_dir(&recipes_dir)
        .unwrap_or_else(|e| panic!("read recipes dir {}: {e}", recipes_dir.display()))
    {
        let entry = entry.unwrap_or_else(|e| panic!("read recipe dir entry: {e}"));
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("yaml") {
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read recipe {}: {e}", path.display()));
            combined.push_str("\n--- ");
            combined.push_str(&path.display().to_string());
            combined.push_str(" ---\n");
            combined.push_str(&text);
        }
    }
    combined
}

#[test]
fn workflow_contract_checks_schedule_event_runs_before_broad_failures() {
    let recipes = workflow_recipe_contract_text();

    for required in [
        "gh run list --event schedule --limit 50",
        "gh run list --status failure --limit 50",
        "No recent `schedule` event",
    ] {
        assert!(
            recipes.contains(required),
            "workflow recipes must require `{required}` so user-reported \
             scheduled action failures are proven from GitHub Actions evidence \
             before any code change"
        );
    }
}

#[test]
fn workflow_contract_collects_failed_step_logs_and_evidence_table_fields() {
    let recipes = workflow_recipe_contract_text();

    for required in [
        "gh run view RUN_ID",
        "--log-failed",
        "workflow",
        "run URL",
        "event",
        "branch/SHA",
        "failing job",
        "failing step",
        "root-cause excerpt",
        "classification",
    ] {
        assert!(
            recipes.contains(required),
            "workflow recipes must require failed-run evidence field `{required}`"
        );
    }
}

#[test]
fn workflow_contract_handles_github_service_failures_visibly() {
    let recipes = workflow_recipe_contract_text();

    for required in [
        "gh auth status",
        "existing authenticated session",
        "permission",
        "authentication",
        "rate limit",
        "network",
        "missing log access",
        "Retry once",
        "external-transient or inaccessible",
        "do not infer",
    ] {
        assert!(
            recipes.contains(required),
            "workflow recipes must encode GitHub service integration handling `{required}`"
        );
    }
}

#[test]
fn workflow_contract_limits_fixes_to_proven_repo_caused_failures() {
    let recipes = workflow_recipe_contract_text();

    for required in [
        "repo-caused",
        "generated-template-caused",
        "external-transient",
        "stale",
        "unrelated",
        "inaccessible",
        "do not patch",
        "narrowest fix",
    ] {
        assert!(
            recipes.contains(required),
            "workflow recipes must encode classification/fix-selection rule `{required}`"
        );
    }
}

#[test]
fn workflow_contract_requires_validation_pr_and_ci_closure() {
    let recipes = workflow_recipe_contract_text();

    for required in [
        "pre-commit run --all-files",
        "regression coverage",
        "targeted tests",
        "gh pr create",
        "gh pr checks --watch",
        "merge-ready",
    ] {
        assert!(
            recipes.contains(required),
            "workflow recipes must require PR closure evidence `{required}`"
        );
    }
}
