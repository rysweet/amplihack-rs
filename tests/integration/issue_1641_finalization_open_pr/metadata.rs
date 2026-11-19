//! Metadata integrity for the outcome read (#1641 audit): invalid or
//! non-canonical context values never widen the lookup and never reach a
//! decision, and the real recipe `worktree_setup` transport is honoured.
//!
//! Since #2343 the guard is earlier than it was: metadata that cannot be read
//! leaves the facts ungathered, and the assessment is consulted only after the
//! facts are in hand. A claimed success therefore cannot be applied to a run
//! whose own metadata says nothing readable.

use serde_json::json;

use super::fixture::*;

// A non-canonical pr_number with no pr_url is rejected by the reader; it never
// widens to the pr list search, and gh is never called.
#[test]
fn bad_pr_number_without_url_does_not_fall_back_to_pr_list() {
    for bad in ["abc", "0", "#7", "99999999999"] {
        let fx = Fixture::new();
        fx.gh_list(&json!([{"number": 7, "url": PR_URL, "isCrossRepository": false}]));
        fx.gh_view(&open_pr(green_rollup()));
        let mut envs = resumed_env(&fx.repo_path());
        envs.push(("PR_NUMBER", bad.to_owned()));

        let outcome = fx.validate(&as_refs(&envs));

        assert_no_decision(&outcome, bad);
        assert!(
            outcome.field("evidence_used").contains("outcome.pr=none"),
            "{bad}: no pull request may be adopted\n{}",
            outcome.describe()
        );
        fx.assert_gh_not_called(bad);
    }
}

// Invalid authoritative metadata (wrong JSON type) leaves the facts
// ungathered, and no assessment can be applied to facts that do not exist.
#[test]
fn invalid_metadata_leaves_the_outcome_ungathered() {
    for terminal_state in [
        json!({"publish_status": "existing-open-pr", "pr_number": {"x": 1}}),
        json!({"pr_number": 7, "terminal_state": ["BLOCKED_CI"]}),
    ] {
        let fx = Fixture::new();
        fx.gh_list(&json!([{"number": 7, "url": PR_URL, "isCrossRepository": false}]));
        fx.gh_view(&open_pr(green_rollup()));
        let repo_path = fx.repo_path();
        let terminal_state = terminal_state.to_string();
        let (_, assessment_json) = assessment("EXISTING_OPEN_PR", "none", "7");

        let outcome = fx.validate(&[
            ("RECIPE_VAR_repo_path", repo_path.as_str()),
            ("RECIPE_VAR_branch_name", BRANCH),
            ("RECIPE_VAR_terminal_state", terminal_state.as_str()),
            ("RECIPE_VAR_finalizer_step_status", r#"{"status":"ok"}"#),
            (
                "RECIPE_VAR_finalization_assessment",
                assessment_json.as_str(),
            ),
        ]);

        assert!(
            !outcome.success(),
            "{terminal_state}\n{}",
            outcome.describe()
        );
        assert_eq!(
            outcome.field("terminal_state"),
            "FAILED_MISSING_TERMINAL_EVIDENCE",
            "{terminal_state}: a claimed success cannot be applied to ungathered facts\n{}",
            outcome.describe()
        );
        assert!(
            outcome
                .field("terminal_reason")
                .contains("workflow metadata is invalid"),
            "{terminal_state}: the reason must name the metadata\n{}",
            outcome.describe()
        );
        fx.assert_gh_not_called(&terminal_state);
    }
}

// Real recipe runs supply the directory through worktree_setup. A valid one
// reaches the outcome read; an invalid one must not silently fall back to
// repo_path (which may be a clean checkout of another directory).
#[test]
fn worktree_setup_transport_is_used_and_invalid_values_fail_closed() {
    let fx = Fixture::new();
    fx.gh_view(&open_pr(green_rollup()));
    let setup = json!({"worktree_path": fx.repo_path()}).to_string();
    let (_, assessment_json) = assessment("EXISTING_OPEN_PR", "none", "7");
    let outcome = fx.validate(&[
        ("RECIPE_VAR_worktree_setup", setup.as_str()),
        ("RECIPE_VAR_branch_name", BRANCH),
        ("RECIPE_VAR_pr_number", "7"),
        ("RECIPE_VAR_finalizer_step_status", r#"{"status":"ok"}"#),
        (
            "RECIPE_VAR_finalization_assessment",
            assessment_json.as_str(),
        ),
    ]);
    assert_eq!(
        outcome.field("terminal_state"),
        "EXISTING_OPEN_PR",
        "{}",
        outcome.describe()
    );
    assert!(
        outcome.field("evidence_used").contains("outcome.pr=7"),
        "the worktree_setup path must be what was read\n{}",
        outcome.describe()
    );

    let fx = Fixture::new();
    fx.gh_view(&open_pr(green_rollup()));
    let repo_path = fx.repo_path();
    let outcome = fx.validate(&[
        ("RECIPE_VAR_worktree_setup", r#"{"worktree_path":5}"#),
        ("RECIPE_VAR_repo_path", repo_path.as_str()),
        ("RECIPE_VAR_branch_name", BRANCH),
        ("RECIPE_VAR_pr_number", "7"),
        ("RECIPE_VAR_finalizer_step_status", r#"{"status":"ok"}"#),
        (
            "RECIPE_VAR_finalization_assessment",
            assessment_json.as_str(),
        ),
    ]);
    assert!(!outcome.success(), "{}", outcome.describe());
    assert_ne!(outcome.field("terminal_state"), "EXISTING_OPEN_PR");
    fx.assert_gh_not_called("invalid worktree_setup");
}
