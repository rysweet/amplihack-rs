//! Metadata integrity for the open-PR path (#1641 audit): invalid or
//! non-canonical context values never widen the lookup or feed a success, and
//! the real recipe `worktree_setup` transport is honoured.

use serde_json::json;

use super::fixture::*;

// A non-canonical pr_number with no pr_url is rejected by the helper; it never
// widens to the pr list search.
#[test]
fn bad_pr_number_without_url_does_not_fall_back_to_pr_list() {
    for bad in ["abc", "0", "#7", "99999999999"] {
        let fx = Fixture::new();
        fx.gh_list(&json!([{"number": 7, "url": PR_URL, "isCrossRepository": false}]));
        fx.gh_view(&open_pr(green_rollup()));
        let mut envs = resumed_env(&fx.repo_path());
        envs.push(("PR_NUMBER", bad.to_owned()));
        assert_failed_implementation(&fx.validate(&as_refs(&envs)), bad);
        fx.assert_gh_not_called(bad);
    }
}

// Invalid authoritative metadata (wrong JSON type) must never feed success.
#[test]
fn invalid_metadata_disables_the_open_pr_path() {
    for terminal_state in [
        json!({"publish_status": "existing-open-pr", "pr_number": {"x": 1}}),
        json!({"pr_number": 7, "terminal_state": ["BLOCKED_CI"]}),
    ] {
        let fx = Fixture::new();
        fx.gh_list(&json!([{"number": 7, "url": PR_URL, "isCrossRepository": false}]));
        fx.gh_view(&open_pr(green_rollup()));
        let repo_path = fx.repo_path();
        let terminal_state = terminal_state.to_string();
        let outcome = fx.validate(&[
            ("RECIPE_VAR_repo_path", repo_path.as_str()),
            ("RECIPE_VAR_branch_name", BRANCH),
            ("RECIPE_VAR_terminal_state", terminal_state.as_str()),
            ("RECIPE_VAR_finalizer_step_status", r#"{"status":"ok"}"#),
        ]);
        assert!(
            !outcome.success(),
            "{terminal_state}\n{}",
            outcome.describe()
        );
        assert_ne!(outcome.field("terminal_state"), "EXISTING_OPEN_PR");
        fx.assert_gh_not_called(&terminal_state);
    }
}

// Real recipe runs supply the directory through worktree_setup. A valid one
// reaches the open-PR path; an invalid one must not silently fall back to
// repo_path (which may be a clean checkout of another directory).
#[test]
fn worktree_setup_transport_is_used_and_invalid_values_fail_closed() {
    let fx = Fixture::new();
    fx.gh_view(&open_pr(green_rollup()));
    let setup = json!({"worktree_path": fx.repo_path()}).to_string();
    let outcome = fx.validate(&[
        ("RECIPE_VAR_worktree_setup", setup.as_str()),
        ("RECIPE_VAR_branch_name", BRANCH),
        ("RECIPE_VAR_pr_number", "7"),
        ("RECIPE_VAR_finalizer_step_status", r#"{"status":"ok"}"#),
    ]);
    assert_eq!(
        outcome.field("terminal_state"),
        "EXISTING_OPEN_PR",
        "{}",
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
    ]);
    assert!(!outcome.success(), "{}", outcome.describe());
    assert_ne!(outcome.field("terminal_state"), "EXISTING_OPEN_PR");
    fx.assert_gh_not_called("invalid worktree_setup");
}
