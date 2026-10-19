//! Review fixes for PR #1643: the PR head commit must be the local HEAD, a
//! valid `pr_number` takes precedence over a malformed `pr_url`, `publish_state_reached`
//! counts as a publish signal, userinfo cannot smuggle a foreign host, and each
//! fail-closed case names its reason.

use std::fs;

use serde_json::json;

use super::fixture::*;

fn green_env(fx: &Fixture) -> Vec<(&'static str, String)> {
    let mut envs = resumed_env(&fx.repo_path());
    envs.push(("PR_URL", PR_URL.to_owned()));
    envs
}

// A resumed run committed locally but never pushed: the PR's green checks ran
// on an older commit and must not vouch for this run's code.
#[test]
fn local_head_ahead_of_the_pr_head_fails_closed() {
    let fx = Fixture::new();
    fx.gh_view(&open_pr(green_rollup()));
    fs::write(fx.repo.join("README.md"), "unpushed\n").expect("edit");
    fx.git(&["commit", "-q", "-am", "unpushed work"]);

    let outcome = fx.validate(&as_refs(&green_env(&fx)));
    assert_failed_implementation(&outcome, "local HEAD ahead of PR head");
    assert_eq!(outcome.field("pr_number"), "");
    assert!(
        outcome.stderr.contains("head at the local HEAD"),
        "stderr must name the head mismatch\n{}",
        outcome.describe()
    );

    // Same with typed verification evidence: precommit passed on a commit the
    // PR does not contain.
    let mut envs = green_env(&fx);
    envs.push(("VERIFICATION_COMPLETED", "true".to_owned()));
    let mut view = open_pr(json!([check_run("IN_PROGRESS", json!(null))]));
    view["headRefOid"] = json!("0".repeat(40));
    fx.gh_view(&view);
    let outcome = fx.validate(&as_refs(&envs));
    assert_failed_implementation(&outcome, "precommit on an unpushed commit");
}

// An unborn run branch has no HEAD commit to compare: fail closed before gh.
#[test]
fn unresolvable_local_head_fails_closed_without_calling_gh() {
    let fx = Fixture::new();
    fx.gh_view(&open_pr(green_rollup()));
    fx.git(&["switch", "-q", "--detach"]);
    fx.git(&["branch", "-q", "-D", BRANCH]);
    fx.git(&["switch", "-q", "--orphan", BRANCH]);

    let outcome = fx.signal(BRANCH, Some("7"));
    assert!(!outcome.success(), "unborn HEAD is no signal");
    assert_eq!(outcome.stdout, "", "no stdout on failure");
    assert!(
        outcome
            .stderr
            .contains("no open-PR signal: local HEAD could not be resolved"),
        "stderr must name the HEAD check\n{}",
        outcome.describe()
    );
    fx.assert_gh_not_called("unresolvable local HEAD");
}

#[test]
fn missing_or_malformed_pr_head_oid_fails_closed() {
    for (name, value) in [
        ("missing", serde_json::Value::Null),
        ("number", json!(7)),
        ("short sha", json!("abc123")),
        ("empty", json!("")),
    ] {
        let fx = Fixture::new();
        let mut view = open_pr(green_rollup());
        view["headRefOid"] = value;
        fx.gh_view(&view);
        let outcome = fx.signal(BRANCH, Some("7"));
        assert!(!outcome.success(), "{name} headRefOid is no signal");
        assert_eq!(outcome.stdout, "", "{name}: no stdout on failure");
    }
}

#[test]
fn valid_pr_number_takes_precedence_over_a_malformed_pr_url() {
    for bad_url in [
        "https://github.com/rysweet/fixture/pull/7/",
        "https://github.com/rysweet/fixture/pull/7/files",
        "https://github.com/RySweet/Fixture/pull/7",
    ] {
        let fx = Fixture::new();
        fx.gh_view(&open_pr(green_rollup()));
        let mut envs = resumed_env(&fx.repo_path());
        envs.push(("PR_URL", bad_url.to_owned()));
        envs.push(("PR_NUMBER", "7".to_owned()));
        let outcome = fx.validate(&as_refs(&envs));
        assert_eq!(
            outcome.field("terminal_state"),
            "EXISTING_OPEN_PR",
            "{bad_url}: valid pr_number must still be used\n{}",
            outcome.describe()
        );
        assert_eq!(outcome.field("pr_url"), PR_URL);
        fx.assert_gh_reads_only();
        assert_eq!(fx.gh_calls().len(), 1, "{bad_url}: one pr view, no retry");
    }

    // Without a pr_number there is nothing to fall back to.
    let fx = Fixture::new();
    fx.gh_view(&open_pr(green_rollup()));
    let mut envs = resumed_env(&fx.repo_path());
    envs.push((
        "PR_URL",
        "https://github.com/rysweet/fixture/pull/7/".to_owned(),
    ));
    let outcome = fx.validate(&as_refs(&envs));
    assert_failed_implementation(&outcome, "malformed url, no number");
    assert!(
        outcome.stderr.contains("no open-PR signal: PR reference"),
        "stderr must name the URL check\n{}",
        outcome.describe()
    );
    fx.assert_gh_not_called("malformed url, no number");
}

#[test]
fn publish_state_reached_alone_is_a_publish_signal() {
    let fx = Fixture::new();
    fx.gh_view(&open_pr(green_rollup()));
    fx.gh_list(&json!([{"number": 7, "url": PR_URL, "isCrossRepository": false}]));
    let mut envs = resumed_env(&fx.repo_path());
    envs.push(("PUBLISH_STATE_REACHED", "true".to_owned()));
    let outcome = fx.validate(&as_refs(&envs));
    assert_eq!(
        outcome.field("terminal_state"),
        "EXISTING_OPEN_PR",
        "{}",
        outcome.describe()
    );
    fx.assert_gh_reads_only();
}

#[test]
fn userinfo_cannot_hide_a_foreign_host() {
    for origin in [
        "https://evil.example#@github.com/rysweet/fixture",
        "https://evil.example?@github.com/rysweet/fixture",
        "https://evil.example\\@github.com/rysweet/fixture",
    ] {
        let fx = Fixture::new();
        fx.git(&["remote", "set-url", "origin", origin]);
        fx.gh_view(&open_pr(green_rollup()));
        let outcome = fx.signal(BRANCH, Some("7"));
        assert!(!outcome.success(), "{origin} is no signal");
        fx.assert_gh_not_called(origin);
    }

    // Plain userinfo (a token) is still accepted.
    let fx = Fixture::new();
    fx.git(&[
        "remote",
        "set-url",
        "origin",
        "https://x-access-token:abc@github.com/rysweet/fixture.git",
    ]);
    fx.gh_view(&open_pr(green_rollup()));
    let outcome = fx.signal(BRANCH, Some("7"));
    assert!(outcome.success(), "{}", outcome.describe());
    assert!(!outcome.stderr.contains("abc"), "token must not be echoed");
}

// gh pr list failing is no signal: nothing is inspected and the run stays failed.
#[test]
fn failing_pr_list_is_no_signal() {
    let fx = Fixture::new();
    fx.gh_fail("list", 1);
    fx.gh_view(&open_pr(green_rollup()));
    let mut envs = resumed_env(&fx.repo_path());
    envs.push((
        "TERMINAL_STATE_PUBLISH_STATUS",
        "existing-open-pr".to_owned(),
    ));
    let outcome = fx.validate(&as_refs(&envs));
    assert_failed_implementation(&outcome, "pr list exit 1");
    assert!(
        outcome
            .stderr
            .contains("no open-PR signal: gh pr list failed"),
        "{}",
        outcome.describe()
    );
    let calls = fx.gh_calls();
    assert!(calls.iter().any(|c| c.starts_with("pr list ")), "{calls:?}");
    assert!(
        !calls.iter().any(|c| c.starts_with("pr view ")),
        "{calls:?}"
    );
}

// A .git directory is not a worktree: the dirty probe cannot see untracked
// files there, so the open-PR path must not vouch for it.
#[test]
fn git_dir_target_is_not_a_worktree() {
    let fx = Fixture::new();
    fs::write(fx.repo.join("untracked.txt"), "dirty\n").expect("dirty file");
    fx.gh_view(&open_pr(green_rollup()));
    let git_dir = fx.repo.join(".git").to_string_lossy().into_owned();
    let mut envs = resumed_env(&git_dir);
    envs.push(("PR_URL", PR_URL.to_owned()));
    let outcome = fx.validate(&as_refs(&envs));
    assert!(!outcome.success(), "{}", outcome.describe());
    assert_ne!(outcome.field("terminal_state"), "EXISTING_OPEN_PR");
    assert!(
        outcome
            .stderr
            .contains("no open-PR signal: target directory is not a git worktree"),
        "{}",
        outcome.describe()
    );
    fx.assert_gh_not_called(".git target");
}
