//! Which pull request reference the facts reader follows, and what it does
//! when it cannot follow one (#1643, #2343).
//!
//! A canonical `pr_number` wins over a malformed `pr_url`; a URL whose host is
//! smuggled through userinfo is not this origin; a `pr list` that will not
//! answer is an unread fact with its reason, not an absent pull request. The
//! head-comparison cases are in `head_and_fallback.rs`.

use std::fs;

use serde_json::json;

use super::fixture::*;

#[test]
fn valid_pr_number_takes_precedence_over_a_malformed_pr_url() {
    for bad_url in [
        "https://github.com/rysweet/fixture/pull/7/",
        "https://github.com/rysweet/fixture/pull/7/files",
        "https://github.com/RySweet/Fixture/pull/7",
    ] {
        let fx = Fixture::new();
        fx.gh_view(&open_pr(green_rollup()));
        let mut envs = assessed_env(&fx.repo_path(), "EXISTING_OPEN_PR", "7");
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
        assert!(
            outcome.field("evidence_used").contains("outcome.pr=7"),
            "{bad_url}: the facts must have read PR 7\n{}",
            outcome.describe()
        );
        fx.assert_gh_reads_only();
        assert_eq!(fx.gh_calls().len(), 1, "{bad_url}: one pr view, no retry");
    }

    // Without a pr_number there is nothing to fall back to: the malformed URL
    // is reported unread, and no pull request is read or widened to.
    let fx = Fixture::new();
    fx.gh_view(&open_pr(green_rollup()));
    let mut envs = resumed_env(&fx.repo_path());
    envs.push((
        "PR_URL",
        "https://github.com/rysweet/fixture/pull/7/".to_owned(),
    ));

    let outcome = fx.validate(&as_refs(&envs));

    assert_no_decision(&outcome, "malformed url, no number");
    assert!(
        outcome.field("terminal_reason").contains("canonical URL"),
        "the result must name the URL check\n{}",
        outcome.describe()
    );
    assert!(
        outcome.field("evidence_used").contains("outcome.pr=none"),
        "no pull request may be adopted\n{}",
        outcome.describe()
    );
    fx.assert_gh_not_called("malformed url, no number");
}

// #2343: the run's own `publish_state_reached` no longer gates the look. With
// no pull request reference at all the branch's open pull requests are
// searched, and the decided state is reported on what they say.
#[test]
fn a_run_with_no_pr_reference_is_still_read_from_the_remote() {
    for publish_state in ["true", "false", ""] {
        let fx = Fixture::new();
        fx.gh_view(&open_pr(green_rollup()));
        fx.gh_list(&json!([{"number": 7, "url": PR_URL, "isCrossRepository": false}]));
        let mut envs = assessed_env(&fx.repo_path(), "EXISTING_OPEN_PR", "7");
        if !publish_state.is_empty() {
            envs.push(("PUBLISH_STATE_REACHED", publish_state.to_owned()));
        }

        let outcome = fx.validate(&as_refs(&envs));

        assert_eq!(
            outcome.field("terminal_state"),
            "EXISTING_OPEN_PR",
            "publish_state_reached={publish_state:?}\n{}",
            outcome.describe()
        );
        assert!(
            fx.gh_calls().iter().any(|c| c.starts_with("pr list ")),
            "publish_state_reached={publish_state:?}: the remote is read either way: {:?}",
            fx.gh_calls()
        );
        fx.assert_gh_reads_only();
    }
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

        // The facts reader rejects the same origins: not a github.com repo, so
        // there is no pull request to read and gh is never asked.
        let fx = Fixture::new();
        fx.git(&["remote", "set-url", "origin", origin]);
        fx.gh_view(&open_pr(green_rollup()));
        let (facts, _) = fx.facts(BRANCH, Some("7"));
        assert_eq!(facts["github_origin"], "false", "{origin}");
        assert_eq!(facts["remote"], "", "{origin}");
        fx.assert_gh_not_called(origin);
    }

    // Plain userinfo (a token) is still accepted, and never echoed.
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

// gh pr list failing is an unread fact: nothing is inspected, and the result
// names what could not be read rather than guessing at it.
#[test]
fn failing_pr_list_is_an_unread_fact() {
    let fx = Fixture::new();
    fx.gh_fail("list", 1);
    fx.gh_view(&open_pr(green_rollup()));

    let (facts, outcome) = fx.facts(BRANCH, None);

    assert_eq!(facts["pr_number"], "");
    assert_eq!(facts["open_pr_count"], "");
    assert!(
        facts["unread"]
            .as_str()
            .expect("unread is a String")
            .contains("gh pr list failed"),
        "the unread fact must name the failed list: {facts}"
    );
    assert_no_leak(&outcome);
    let calls = fx.gh_calls();
    assert!(calls.iter().any(|c| c.starts_with("pr list ")), "{calls:?}");
    assert!(
        !calls.iter().any(|c| c.starts_with("pr view ")),
        "{calls:?}"
    );

    // Through the validator the same gap reaches the result, and nothing
    // decides a terminal state on it.
    let fx = Fixture::new();
    fx.gh_fail("list", 1);
    fx.gh_view(&open_pr(green_rollup()));
    let outcome = fx.validate(&as_refs(&resumed_env(&fx.repo_path())));
    assert_no_decision(&outcome, "pr list exit 1");
    assert!(
        outcome
            .field("terminal_reason")
            .contains("gh pr list failed"),
        "{}",
        outcome.describe()
    );
}

// A .git directory is not a worktree: the dirty probe cannot see untracked
// files there, so nothing about it may be read as this run's outcome.
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
            .field("terminal_reason")
            .contains("not a git worktree"),
        "{}",
        outcome.describe()
    );
    fx.assert_gh_not_called(".git target");
}
