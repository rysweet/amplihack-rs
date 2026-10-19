//! Fail-closed cases the validator exists for. The open-PR path may only turn
//! the default `FAILED_IMPLEMENTATION` into success when every proof holds; it
//! never masks an earlier blocker and never succeeds on one signal alone.

use serde_json::{Value, json};

use super::fixture::*;

fn with_url(fx: &Fixture) -> Vec<(&'static str, String)> {
    let mut envs = resumed_env(&fx.repo_path());
    envs.push(("PR_URL", PR_URL.to_owned()));
    envs
}

// Core regression for #1641 (must still hold): nothing published, no evidence.
#[test]
fn run_with_no_pr_and_no_evidence_is_still_failed_implementation() {
    let fx = Fixture::new();
    fx.gh_view(&open_pr(green_rollup()));
    fx.gh_list(&json!([{"number": 7, "url": PR_URL, "isCrossRepository": false}]));

    let outcome = fx.validate(&as_refs(&resumed_env(&fx.repo_path())));

    assert_failed_implementation(&outcome, "no publish signal");
    assert_eq!(outcome.field("publish_state_reached"), "false");
    fx.assert_gh_not_called("without a publish signal the validator must not search GitHub");
}

// An open PR is implementation proof only; it still needs green checks or
// typed verification evidence.
#[test]
fn open_pr_without_a_passing_verification_signal_fails_closed() {
    for rollup in [
        json!([check_run("COMPLETED", json!("FAILURE"))]),
        json!([
            check_run("COMPLETED", json!("SUCCESS")),
            check_run("COMPLETED", json!("CANCELLED"))
        ]),
        json!([check_run("IN_PROGRESS", json!(null))]),
        json!([check_run("QUEUED", json!(null))]),
        json!([check_run("COMPLETED", json!(null))]),
        json!([status_context("PENDING")]),
        json!([status_context("FAILURE")]),
        json!([{"__typename": "MysteryCheck", "state": "SUCCESS"}]),
        json!([]),
        Value::Null,
    ] {
        let fx = Fixture::new();
        fx.gh_view(&open_pr(rollup.clone()));
        let outcome = fx.validate(&as_refs(&with_url(&fx)));
        assert_failed_implementation(&outcome, &format!("rollup {rollup}"));
    }
}

// The live PR must be OPEN, on the run branch, same-repo, and be the PR asked
// for at the exact canonical URL.
#[test]
fn pr_that_is_not_the_runs_open_same_repo_pr_fails_closed() {
    let cases: Vec<(&str, &str, Value)> = vec![
        ("closed", "state", json!("CLOSED")),
        ("merged", "state", json!("MERGED")),
        ("head mismatch", "headRefName", json!("feat/other")),
        ("cross repo", "isCrossRepository", json!(true)),
        ("cross repo as string", "isCrossRepository", json!("false")),
        ("cross repo missing", "isCrossRepository", Value::Null),
        ("other number", "number", json!(8)),
        (
            "other repo url",
            "url",
            json!("https://github.com/evil/fixture/pull/7"),
        ),
        (
            "url with suffix",
            "url",
            json!("https://github.com/rysweet/fixture/pull/7/files"),
        ),
    ];
    for (name, key, value) in cases {
        let fx = Fixture::new();
        let mut view = open_pr(green_rollup());
        view[key] = value;
        fx.gh_view(&view);
        let outcome = fx.validate(&as_refs(&with_url(&fx)));
        assert_failed_implementation(&outcome, name);
        assert_eq!(
            outcome.field("pr_number"),
            "",
            "{name}: unconfirmed PR must not be reported as confirmed"
        );
    }
}

#[test]
fn gh_failure_or_malformed_json_is_no_signal_and_does_not_leak() {
    let failing = Fixture::new();
    failing.gh_fail("view", 1);
    let outcome = failing.validate(&as_refs(&with_url(&failing)));
    assert_failed_implementation(&outcome, "gh pr view exit 1");
    assert_no_leak(&outcome);

    for body in ["not json", "{\"number\":", "[]", "\"OPEN\""] {
        let fx = Fixture::new();
        fx.gh_view_raw(body);
        let outcome = fx.validate(&as_refs(&with_url(&fx)));
        assert_failed_implementation(&outcome, &format!("malformed view body {body:?}"));
    }
}

// A pr_url naming another repo fails at once: no pr list fallback, no gh call.
#[test]
fn pr_url_for_another_repo_fails_closed_without_calling_gh() {
    for url in [
        "https://github.com/evil/fixture/pull/7",
        "https://github.com/rysweet/fixture-evil/pull/7",
        "https://github.com/rysweet/fixtur./pull/7",
        "https://user@github.com/rysweet/fixture/pull/7",
        "https://github.com/rysweet/fixture/pull/7?x=1",
        "https://github.com/rysweet/fixture/pull/0",
    ] {
        let fx = Fixture::new();
        fx.gh_view(&open_pr(green_rollup()));
        fx.gh_list(&json!([{"number": 7, "url": PR_URL, "isCrossRepository": false}]));
        let mut envs = resumed_env(&fx.repo_path());
        envs.push(("PR_URL", url.to_owned()));
        let outcome = fx.validate(&as_refs(&envs));
        assert_failed_implementation(&outcome, url);
        fx.assert_gh_not_called(url);
    }
}

// The pr list fallback must find exactly one PR, and it must be same-repo.
#[test]
fn ambiguous_or_foreign_pr_list_results_fail_closed() {
    for list in [
        json!([]),
        json!([{"number": 7, "url": PR_URL, "isCrossRepository": true}]),
        json!([
            {"number": 7, "url": PR_URL, "isCrossRepository": false},
            {"number": 9, "url": "https://github.com/fork/fixture/pull/9", "isCrossRepository": true}
        ]),
        json!({"number": 7}),
    ] {
        let fx = Fixture::new();
        fx.gh_list(&list);
        fx.gh_view(&open_pr(green_rollup()));
        let mut envs = resumed_env(&fx.repo_path());
        envs.push((
            "TERMINAL_STATE_PUBLISH_STATUS",
            "existing-open-pr".to_owned(),
        ));
        let outcome = fx.validate(&as_refs(&envs));
        assert_failed_implementation(&outcome, &format!("pr list {list}"));
        assert!(
            !fx.gh_calls().iter().any(|c| c.starts_with("pr view ")),
            "pr list {list}: no PR may be inspected without a unique same-repo match"
        );
    }
}

// Guards that must hold before gh is ever called.
#[test]
fn unusable_run_scope_fails_closed_without_calling_gh() {
    // Target directory fell back to '.': cwd is the repo, but REPO_PATH unset.
    let fx = Fixture::new();
    fx.gh_view(&open_pr(green_rollup()));
    let outcome = fx.validate(&[
        ("BRANCH_NAME", BRANCH),
        ("PR_URL", PR_URL),
        ("FINALIZER_STEP_STATUS", "ok"),
    ]);
    assert_failed_implementation(&outcome, "'.' target fallback");
    fx.assert_gh_not_called("'.' target fallback");

    // AzDO remotes: only GitHub checks are evaluated.
    let fx = Fixture::new();
    fx.gh_view(&open_pr(green_rollup()));
    let mut envs = with_url(&fx);
    envs.push(("REMOTE_HOST_TYPE", "azdo".to_owned()));
    assert_failed_implementation(&fx.validate(&as_refs(&envs)), "azdo remote");
    fx.assert_gh_not_called("azdo remote");

    // Metadata branch disagrees with the live branch.
    let fx = Fixture::new();
    fx.gh_view(&open_pr(green_rollup()));
    let mut envs = with_url(&fx);
    envs[1].1 = "feat/other".to_owned();
    assert_failed_implementation(&fx.validate(&as_refs(&envs)), "branch mismatch");
    fx.assert_gh_not_called("branch mismatch");

    // Runs on the default branch never qualify.
    for branch in ["main", "master"] {
        let fx = Fixture::on_branch(branch);
        let mut view = open_pr(green_rollup());
        view["headRefName"] = json!(branch);
        fx.gh_view(&view);
        let mut envs = with_url(&fx);
        envs[1].1 = branch.to_owned();
        assert_failed_implementation(&fx.validate(&as_refs(&envs)), branch);
        fx.assert_gh_not_called(branch);
    }
}

// Every earlier blocker keeps its state, and the open-PR path never runs.
#[test]
fn blockers_keep_their_state_and_are_not_masked_by_an_open_green_pr() {
    let cases: [(&str, &str, &str); 4] = [
        (
            "FINALIZATION_EVIDENCE_GIT_DIRTY_WORKTREE",
            "true",
            "FAILED_DIRTY_WORKTREE",
        ),
        (
            "FINALIZATION_EVIDENCE_AGENT_OUTPUTS_HOLLOW_SUCCESS_SIGNALS",
            "true",
            "HOLLOW_SUCCESS",
        ),
        ("TERMINAL_STATE_TERMINAL_STATE", "BLOCKED_CI", "BLOCKED_CI"),
        ("FINALIZER_STEP_STATUS", "failed", "FAILED_IMPLEMENTATION"),
    ];
    for (key, value, expected) in cases {
        let fx = Fixture::new();
        fx.gh_view(&open_pr(green_rollup()));
        let mut envs = with_url(&fx);
        envs.retain(|(k, _)| *k != key);
        envs.push((key, value.to_owned()));
        let outcome = fx.validate(&as_refs(&envs));
        assert!(!outcome.success(), "{key}={value}\n{}", outcome.describe());
        assert_eq!(outcome.field("terminal_state"), expected, "{key}={value}");
        fx.assert_gh_not_called(&format!("{key}={value}"));
    }

    // A live dirty worktree (no collected evidence) is also a blocker.
    let fx = Fixture::new();
    fx.gh_view(&open_pr(green_rollup()));
    std::fs::write(fx.repo.join("untracked.txt"), "dirty\n").expect("dirty file");
    let outcome = fx.validate(&as_refs(&with_url(&fx)));
    assert_eq!(
        outcome.field("terminal_state"),
        "FAILED_DIRTY_WORKTREE",
        "{}",
        outcome.describe()
    );
    fx.assert_gh_not_called("live dirty worktree");
}

// The publish-state override is internal; the environment cannot spoof it.
#[test]
fn ambient_publish_state_override_cannot_spoof_publication() {
    let fx = Fixture::new();
    let mut envs = resumed_env(&fx.repo_path());
    envs.push(("publish_state_override", "true".to_owned()));
    envs.push(("PUBLISH_STATE_OVERRIDE", "true".to_owned()));
    let outcome = fx.validate(&as_refs(&envs));
    assert_failed_implementation(&outcome, "spoofed override");
    assert_eq!(outcome.field("publish_state_reached"), "false");
    fx.assert_gh_not_called("spoofed override");
}
