//! How the pull request behind an `EXISTING_OPEN_PR` success is read (#1641,
//! #2343).
//!
//! Reading it is `finalization_outcome_facts`, which reports what `gh`
//! printed and judges none of it, so these cases assert the facts. The cases
//! about the terminal state those facts are decided into are in `success.rs`.

use serde_json::json;

use super::fixture::*;
use super::success::{VIEW_FIELDS, assert_existing_open_pr};

// The read behind it: one bounded, read-only `pr view` scoped with `--repo` to
// the origin repository, asking for exactly the documented fields.
#[test]
fn the_facts_read_the_pr_read_only_with_repo_and_the_documented_fields() {
    let fx = Fixture::new();
    fx.gh_view(&open_pr(green_rollup()));

    let (facts, _) = fx.facts(BRANCH, Some(PR_URL));

    assert_eq!(facts["pr_number"], "7");
    assert_eq!(facts["pr_url"], PR_URL);
    assert_eq!(facts["pr_state"], "OPEN");
    assert_eq!(facts["checks"], "green");
    assert_eq!(facts["pr_head"], fx.head());
    // The pull request itself was read in full: nothing about it is unread.
    // (The fixture never pushes, so the ls-remote fact is unread; that is a
    // different fact and the reader says which.)
    assert!(
        !facts["unread"]
            .as_str()
            .expect("unread is a String")
            .contains("pull request"),
        "nothing about the pull request may be unread: {facts}"
    );
    let calls = fx.gh_calls();
    assert!(
        calls.iter().any(|c| c.starts_with("pr view 7 ")
            && c.contains(&format!("--repo {REPO}"))
            && c.contains("--json")
            && VIEW_FIELDS.iter().all(|f| c.contains(f))),
        "the facts reader must inspect the PR read-only with --repo and the documented fields: {calls:?}"
    );
    fx.assert_gh_reads_only();
}

// With no pull request reference of its own, the facts reader searches the
// branch's open pull requests and re-checks the single match with `pr view`.
#[test]
fn a_run_with_no_pr_reference_is_found_through_the_pr_list_search() {
    let fx = Fixture::new();
    fx.gh_list(&json!([{"number": 7, "url": PR_URL, "isCrossRepository": false}]));
    fx.gh_view(&open_pr(green_rollup()));

    let (facts, _) = fx.facts(BRANCH, None);

    assert_eq!(facts["open_pr_count"], "1");
    assert_eq!(facts["pr_number"], "7");
    assert_eq!(facts["checks"], "green");
    let calls = fx.gh_calls();
    assert!(
        calls.iter().any(|c| c.starts_with("pr list ")
            && c.contains(&format!("--head {BRANCH}"))
            && c.contains("--state open")),
        "the reader must search the open PRs of the run branch: {calls:?}"
    );
    assert!(
        calls.iter().any(|c| c.starts_with("pr view 7 ")),
        "the single match must be re-checked with pr view: {calls:?}"
    );
    fx.assert_gh_reads_only();
}

// #2343: the run's own publish bookkeeping no longer decides anything. Whatever
// publish status the run recorded -- including none at all -- the same facts are
// read and the same decided state is reported.
#[test]
fn the_recorded_publish_status_no_longer_changes_the_result() {
    for (alias, status) in [
        ("TERMINAL_STATE_PUBLISH_STATUS", "existing-open-pr"),
        ("PR_PUBLISH_RESULT_STATE", "existing-open-pr"),
        ("TERMINAL_STATE_PUBLISH_STATUS", "EXISTING_OPEN_PR"),
        ("TERMINAL_STATE_PUBLISH_STATUS", "FOLLOWUP_CREATED"),
        ("TERMINAL_STATE_TERMINAL_STATE", "existing-open-pr"),
        ("TERMINAL_STATE_PUBLISH_STATUS", ""),
    ] {
        let fx = Fixture::new();
        fx.gh_list(&json!([{"number": 7, "url": PR_URL, "isCrossRepository": false}]));
        fx.gh_view(&open_pr(green_rollup()));
        let mut envs = assessed_env(&fx.repo_path(), "EXISTING_OPEN_PR", "7");
        if !status.is_empty() {
            envs.push((alias, status.to_owned()));
        }

        let outcome = fx.validate(&as_refs(&envs));

        assert!(
            outcome.success(),
            "{alias}={status}\n{}",
            outcome.describe()
        );
        assert_existing_open_pr(&outcome, "green");
        fx.assert_gh_reads_only();
    }
}

#[test]
fn neutral_skipped_and_success_status_contexts_count_as_green() {
    let fx = Fixture::new();
    fx.gh_view(&open_pr(json!([
        check_run("COMPLETED", json!("SUCCESS")),
        check_run("COMPLETED", json!("NEUTRAL")),
        check_run("COMPLETED", json!("SKIPPED")),
        status_context("SUCCESS"),
    ])));

    assert_eq!(fx.fact(BRANCH, Some(PR_URL), "checks"), "green");
}

// Successful runs must not leave retry scratch files in the worktree, which
// would dirty it for the next run.
#[test]
fn open_pr_lookup_leaves_the_worktree_clean() {
    let fx = Fixture::new();
    fx.gh_view(&open_pr(green_rollup()));
    let mut envs = assessed_env(&fx.repo_path(), "EXISTING_OPEN_PR", "7");
    envs.push(("PR_URL", PR_URL.to_owned()));

    assert!(fx.validate(&as_refs(&envs)).success());
    assert_eq!(
        fx.git(&["status", "--porcelain"]),
        "",
        "worktree must stay clean"
    );
    assert!(
        !path_has_retry_scratch(&fx.repo),
        "no retry scratch in the worktree"
    );
    assert!(
        fs_is_empty(&fx.tmp.path().join("scratch")),
        "the private mktemp dir must be removed"
    );
}

fn fs_is_empty(dir: &std::path::Path) -> bool {
    std::fs::read_dir(dir).map_or(true, |mut entries| entries.next().is_none())
}
