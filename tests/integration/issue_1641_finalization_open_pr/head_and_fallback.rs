//! Review fixes for PR #1643: the PR head commit must be the local HEAD, a
//! valid `pr_number` takes precedence over a malformed `pr_url`, userinfo
//! cannot smuggle a foreign host, and each unread fact names its reason.
//!
//! Since #2343 the head comparison is reported, not judged: the facts carry
//! `local_head`, `pr_head` and what `git merge-base --is-ancestor` answered
//! about them, and assess-finalization decides what that means.

use std::fs;

use serde_json::json;

use super::fixture::*;

fn green_env(fx: &Fixture) -> Vec<(&'static str, String)> {
    let mut envs = resumed_env(&fx.repo_path());
    envs.push(("PR_URL", PR_URL.to_owned()));
    envs
}

// A resumed run committed locally but never pushed: the PR's green checks ran
// on an older commit, and the facts say so plainly -- the PR head is not the
// local head, and the local head is not an ancestor of it.
#[test]
fn local_head_ahead_of_the_pr_head_is_reported_as_a_head_that_does_not_descend() {
    let fx = Fixture::new();
    fx.gh_view(&open_pr(green_rollup()));
    let pushed = fx.head();
    fs::write(fx.repo.join("README.md"), "unpushed\n").expect("edit");
    fx.git(&["commit", "-q", "-am", "unpushed work"]);
    let local = fx.head();

    let (facts, outcome) = fx.facts(BRANCH, Some(PR_URL));

    assert_eq!(facts["local_head"], local, "{}", outcome.describe());
    assert_eq!(facts["pr_head"], pushed, "{}", outcome.describe());
    assert_eq!(
        facts["pr_head_descends_local_head"],
        "false",
        "an older PR head does not contain the local head\n{}",
        outcome.describe()
    );
    assert_ne!(facts["pr_head"], facts["local_head"]);
}

// A PR head that is not in the local object store and cannot be fetched is an
// unread fact, not a "no": the facts leave the answer empty and say why.
#[test]
fn an_unfetchable_pr_head_leaves_the_ancestry_unread() {
    let fx = Fixture::new();
    let mut view = open_pr(json!([check_run("IN_PROGRESS", json!(null))]));
    view["headRefOid"] = json!("0".repeat(40));
    fx.gh_view(&view);

    let (facts, outcome) = fx.facts(BRANCH, Some(PR_URL));

    assert_eq!(facts["pr_head"], "0".repeat(40), "{}", outcome.describe());
    assert_eq!(
        facts["pr_head_descends_local_head"],
        "",
        "an unobtainable commit cannot be answered about\n{}",
        outcome.describe()
    );
    assert!(
        facts["unread"]
            .as_str()
            .expect("unread is a String")
            .contains("could not obtain"),
        "the unread fact must name the commit it could not get: {facts}"
    );
}

// With the same facts and nothing deciding them, finalization reaches no
// terminal state: an unpushed local head never becomes a success by default.
#[test]
fn an_unpushed_local_head_never_becomes_a_success_by_default() {
    let fx = Fixture::new();
    fx.gh_view(&open_pr(green_rollup()));
    fs::write(fx.repo.join("README.md"), "unpushed\n").expect("edit");
    fx.git(&["commit", "-q", "-am", "unpushed work"]);
    let mut envs = green_env(&fx);
    envs.push(("VERIFICATION_COMPLETED", "true".to_owned()));

    let outcome = fx.validate(&as_refs(&envs));

    assert_no_decision(&outcome, "local HEAD ahead of PR head");
    assert_ne!(outcome.field("terminal_success"), "true");
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

    // The facts reader reports the same unread fact, and never asks gh about a
    // worktree whose HEAD it could not resolve.
    let fx = Fixture::new();
    fx.gh_view(&open_pr(green_rollup()));
    fx.git(&["switch", "-q", "--detach"]);
    fx.git(&["branch", "-q", "-D", BRANCH]);
    fx.git(&["switch", "-q", "--orphan", BRANCH]);

    let (facts, _) = fx.facts(BRANCH, Some("7"));

    assert_eq!(facts["local_head"], "");
    assert!(
        facts["unread"]
            .as_str()
            .expect("unread is a String")
            .contains("local HEAD"),
        "the unread fact must name the HEAD: {facts}"
    );
    fx.assert_gh_not_called("unresolvable local HEAD, facts reader");
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
        view["headRefOid"] = value.clone();
        fx.gh_view(&view);
        let outcome = fx.signal(BRANCH, Some("7"));
        assert!(!outcome.success(), "{name} headRefOid is no signal");
        assert_eq!(outcome.stdout, "", "{name}: no stdout on failure");

        // The facts reader reports no head at all rather than a malformed one.
        let fx = Fixture::new();
        let mut view = open_pr(green_rollup());
        view["headRefOid"] = value;
        fx.gh_view(&view);
        let (facts, _) = fx.facts(BRANCH, Some("7"));
        assert_eq!(
            facts["pr_head"], "",
            "{name}: no malformed head is reported"
        );
        assert_eq!(
            facts["pr_head_descends_local_head"], "",
            "{name}: nothing to ask about"
        );
    }
}
