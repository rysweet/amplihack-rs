//! Which pull request the facts reader reports, and which it refuses to
//! adopt (#1641, #2343).
//!
//! A pull request that is not this run's is a matter of what the reader
//! printed, not of a verdict: it records what `gh` answered, never widens a
//! recorded reference to a search, and never asks about a repository the run
//! did not name. The terminal-state cases that used to live beside these are
//! in `fail_closed.rs`.

use serde_json::{Value, json};

use super::fixture::*;

// An open PR at the run's head whose checks cannot be classified is an unread
// fact. Checks that are merely pending, or not reported at all, are not among
// them: #2343 and #2784 hand that PR on instead (success.rs). Failing checks
// read as `failing` (blocked_ci.rs).
#[test]
fn an_open_pr_with_an_unclassifiable_check_is_read_once_and_reported_unknown() {
    for rollup in [
        json!([check_run("COMPLETED", json!(null))]),
        json!([{"__typename": "MysteryCheck", "state": "SUCCESS"}]),
    ] {
        let fx = Fixture::new();
        fx.gh_view(&open_pr(rollup.clone()));

        let (facts, _) = fx.facts(BRANCH, Some(PR_URL));

        assert_eq!(facts["checks"], "unknown", "rollup {rollup}");
        let views = fx
            .gh_calls()
            .into_iter()
            .filter(|c| c.starts_with("pr view 7 "))
            .count();
        assert_eq!(views, 1, "rollup {rollup}: exactly one live pr view");
    }
}

// The live PR's own state, draft flag, head and cross-repository flag are
// reported exactly as `gh` printed them, and a value of the wrong type is
// reported as unread rather than coerced into a usable one. The pull request
// the facts name is always the one that was ASKED for -- the recorded number
// and the canonical URL for the origin repository -- so a view that echoes a
// different identity cannot rename it.
#[test]
fn a_pr_that_is_not_the_runs_open_same_repo_pr_is_reported_as_it_is() {
    let cases: Vec<(&str, &str, Value, &str, &str)> = vec![
        ("closed", "state", json!("CLOSED"), "pr_state", "CLOSED"),
        ("merged", "state", json!("MERGED"), "pr_state", "MERGED"),
        (
            "cross repo",
            "isCrossRepository",
            json!(true),
            "pr_cross_repository",
            "true",
        ),
        (
            "cross repo as string",
            "isCrossRepository",
            json!("false"),
            "pr_cross_repository",
            "",
        ),
        (
            "cross repo missing",
            "isCrossRepository",
            Value::Null,
            "pr_cross_repository",
            "",
        ),
        (
            "head mismatch",
            "headRefName",
            json!("feat/other"),
            "pr_number",
            "7",
        ),
        ("other number", "number", json!(8), "pr_number", "7"),
        (
            "other repo url",
            "url",
            json!("https://github.com/evil/fixture/pull/7"),
            "pr_url",
            PR_URL,
        ),
        (
            "url with suffix",
            "url",
            json!("https://github.com/rysweet/fixture/pull/7/files"),
            "pr_url",
            PR_URL,
        ),
    ];
    for (name, key, value, fact, expected) in cases {
        let fx = Fixture::new();
        let mut view = open_pr(green_rollup());
        view[key] = value;
        fx.gh_view(&view);

        let (facts, outcome) = fx.facts(BRANCH, Some(PR_URL));

        assert_eq!(facts[fact], expected, "{name}: {}", outcome.describe());
        // Whatever the view said, the identity in the facts is the requested
        // one, read from the origin repository with --repo.
        assert_eq!(facts["pr_number"], "7", "{name}");
        assert_eq!(facts["pr_url"], PR_URL, "{name}");
        fx.assert_gh_reads_only();
    }
}

// A closed pull request is not a published outcome: with the facts saying so,
// the decided state is the failure, and the result still names no success.
#[test]
fn a_closed_pr_does_not_become_a_success() {
    let fx = Fixture::new();
    let mut view = open_pr(green_rollup());
    view["state"] = json!("CLOSED");
    fx.gh_view(&view);
    let mut envs = assessed_env(&fx.repo_path(), "FAILED_CLOSED_UNMERGED", "7");
    envs.push(("PR_URL", PR_URL.to_owned()));

    let outcome = fx.validate(&as_refs(&envs));

    assert!(!outcome.success(), "{}", outcome.describe());
    assert_eq!(outcome.field("terminal_state"), "FAILED_CLOSED_UNMERGED");
    assert_eq!(outcome.field("terminal_success"), "false");
    assert_eq!(
        outcome.field("publish_state_reached"),
        "false",
        "a closed pull request is not a reached publish state\n{}",
        outcome.describe()
    );
}

#[test]
fn gh_failure_or_malformed_json_is_no_signal_and_does_not_leak() {
    let failing = Fixture::new();
    failing.gh_fail("view", 1);

    let (facts, outcome) = failing.facts(BRANCH, Some(PR_URL));

    assert_eq!(facts["pr_state"], "", "gh exit 1 leaves the state unread");
    assert!(
        facts["unread"]
            .as_str()
            .expect("unread is a String")
            .contains("gh pr view failed"),
        "the unread fact must name the tool that failed: {facts}"
    );
    assert_no_leak(&outcome);

    for body in ["not json", "{\"number\":", "[]", "\"OPEN\""] {
        let fx = Fixture::new();
        fx.gh_view_raw(body);

        let (facts, outcome) = fx.facts(BRANCH, Some(PR_URL));

        assert_eq!(facts["pr_state"], "", "malformed view body {body:?}");
        assert_eq!(facts["pr_head"], "", "malformed view body {body:?}");
        assert_eq!(
            facts["checks"], "unknown",
            "malformed view body {body:?} must not read as green"
        );
        assert_no_leak(&outcome);
    }
}

// A pr_url naming another repo is not widened to a pr list search: there is
// nothing to read, and gh is never called.
#[test]
fn a_pr_url_for_another_repo_is_never_read_and_never_widened() {
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

        let (facts, _) = fx.facts(BRANCH, Some(url));

        assert_eq!(facts["pr_number"], "", "{url}");
        assert_eq!(facts["open_pr_count"], "", "{url}: no list fallback");
        assert!(
            facts["unread"]
                .as_str()
                .expect("unread is a String")
                .contains("canonical URL"),
            "{url}: the unusable reference must be named: {facts}"
        );
        fx.assert_gh_not_called(url);
    }
}

// The pr list search must find exactly one PR, and it must be same-repo. The
// count it found is reported either way.
#[test]
fn ambiguous_or_foreign_pr_list_results_name_no_pull_request() {
    for (list, count) in [
        (json!([]), "0"),
        (
            json!([{"number": 7, "url": PR_URL, "isCrossRepository": true}]),
            "1",
        ),
        (
            json!([
                {"number": 7, "url": PR_URL, "isCrossRepository": false},
                {"number": 9, "url": "https://github.com/fork/fixture/pull/9", "isCrossRepository": true}
            ]),
            "2",
        ),
        (json!({"number": 7}), ""),
    ] {
        let fx = Fixture::new();
        fx.gh_list(&list);
        fx.gh_view(&open_pr(green_rollup()));

        let (facts, _) = fx.facts(BRANCH, None);

        assert_eq!(facts["pr_number"], "", "pr list {list}");
        assert_eq!(facts["open_pr_count"], count, "pr list {list}");
        assert!(
            !fx.gh_calls().iter().any(|c| c.starts_with("pr view ")),
            "pr list {list}: no PR may be inspected without a unique same-repo match"
        );
    }
}

// #1641 D5: a run that recorded a non-GitHub remote has no GitHub pull request
// to read, whatever this checkout's origin happens to be. The remote is never
// asked, and the facts say why; the local facts are still collected, so a run
// with no GitHub pull request to find can still be judged on what it did.
#[test]
fn a_recorded_non_github_remote_is_never_asked_about_a_pull_request() {
    let fx = Fixture::new();
    fx.gh_view(&open_pr(green_rollup()));
    let mut envs = assessed_env(&fx.repo_path(), "IMPLEMENTED_VERIFIED", "");
    envs.push(("PR_URL", PR_URL.to_owned()));
    envs.push(("REMOTE_HOST_TYPE", "azdo".to_owned()));

    let outcome = fx.validate(&as_refs(&envs));

    fx.assert_gh_not_called("a run that declared a non-GitHub remote");
    let evidence = outcome.field("evidence_used");
    assert!(
        evidence.contains("outcome.pr=none"),
        "no pull request may be adopted for a non-GitHub run: {evidence}\n{}",
        outcome.describe()
    );
    assert_eq!(outcome.field("terminal_state"), "IMPLEMENTED_VERIFIED");
    assert_eq!(outcome.field("terminal_success"), "true");

    // The facts say it in the reader's own words, and still carry the branch
    // and the local head the decision was made on.
    let (value, _) = fx.facts_for_host(BRANCH, None, "azdo");
    assert_eq!(value["declared_remote_host"], "azdo");
    assert_eq!(value["pr_number"], "");
    assert_eq!(value["local_head"], fx.head());
    assert!(
        value["unread"]
            .as_str()
            .expect("unread is a String")
            .contains("azdo"),
        "the reason must name the remote the run recorded: {value}"
    );
}
