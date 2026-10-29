//! Issue #1714, case 2 (mistt-ui-platform PR #138): the local HEAD was
//! 5174029 and the PR head was 50e41db, a merge of main pushed from another
//! checkout whose first parent was 5174029. The open-PR check required the two
//! to be equal, found no signal, and the run ended `FAILED_IMPLEMENTATION`
//! before the crusty and merge-ready loops.
//!
//! Here the same history is built in the fixture repo: the run branch holds
//! one commit of run work (L), and the PR head is a `--no-ff` merge of main
//! into L (M, first parent L). The tests cover:
//! - M present locally: the same PR (`head:"descendant"`);
//! - M absent locally: one fetch from the fixture's private remote, no ref or
//!   FETCH_HEAD written;
//! - M unfetchable: no signal (a missing object is never read as "ancestor");
//! - a diverged or rewritten PR head: no signal.
//!
//! The hand-off on that history is in `ancestor_handoff.rs`.

use std::fs;
use std::path::PathBuf;

use serde_json::json;

use super::fixture::*;
use crate::issue_1641_finalization_open_pr::fixture::assert_failed_implementation;

pub(super) const FLAG: (&str, &str) = ("RECIPE_VAR_finalization_handoff", "true");

/// The run's own commit L on the run branch; returns its id.
pub(super) fn commit_run_work(fx: &Fixture) -> String {
    fs::write(fx.repo.join("WORK.md"), "run work\n").expect("write work");
    fx.git(&["add", "WORK.md"]);
    fx.git(&["commit", "-q", "-m", "run work"]);
    fx.head()
}

/// Build M, a merge of a moved main into L whose first parent is L, on a
/// temporary branch `pushed`, and switch back to the run branch at L.
pub(super) fn merge_main_on_pushed(fx: &Fixture) -> (String, String) {
    let local = commit_run_work(fx);
    fx.git(&["switch", "-q", "main"]);
    fs::write(fx.repo.join("MAIN.md"), "main moved\n").expect("write main");
    fx.git(&["add", "MAIN.md"]);
    fx.git(&["commit", "-q", "-m", "main moves"]);
    fx.git(&["switch", "-q", "-c", "pushed", BRANCH]);
    fx.git(&["merge", "-q", "--no-ff", "--no-edit", "main"]);
    let pr_head = fx.head();
    fx.git(&["switch", "-q", BRANCH]);
    assert_eq!(fx.head(), local, "the run branch stays at L");
    assert_eq!(
        fx.git(&["rev-parse", &format!("{pr_head}^1")]).trim(),
        local,
        "M's first parent is L"
    );
    (local, pr_head)
}

pub(super) fn drop_pushed_branch(fx: &Fixture) {
    fx.git(&["branch", "-q", "-D", "pushed"]);
}

/// Remove every unreferenced object, so M is no longer in the object store.
fn prune(fx: &Fixture, oid: &str) {
    fx.git(&["reflog", "expire", "--expire=now", "--all"]);
    fx.git(&["gc", "-q", "--prune=now"]);
    assert!(!has_object(fx, oid), "{oid} must be gone after the prune");
}

fn has_object(fx: &Fixture, oid: &str) -> bool {
    let mut command = amplihack_git::command();
    command
        .current_dir(&fx.repo)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args(["cat-file", "-e", &format!("{oid}^{{commit}}")]);
    command.output().expect("run git").status.success()
}

/// A bare repo where `https://github.com/rysweet/fixture.git` resolves.
fn origin_remote(fx: &Fixture) -> PathBuf {
    let path = fx.remotes().join("rysweet/fixture.git");
    fs::create_dir_all(&path).expect("remote dir");
    fx.git(&["init", "-q", "--bare", &path.to_string_lossy()]);
    path
}

pub(super) fn view_at(fx: &Fixture, pr_head: &str, rollup: serde_json::Value) {
    let mut view = open_pr(rollup);
    view["headRefOid"] = json!(pr_head);
    fx.gh_view(&view);
}

pub(super) fn pr_env(fx: &Fixture) -> Vec<(&'static str, String)> {
    let mut envs = resumed_env(&fx.repo_path());
    envs.push(("PR_URL", PR_URL.to_owned()));
    envs
}

pub(super) fn pending() -> serde_json::Value {
    json!([check_run("IN_PROGRESS", json!(null))])
}

fn assert_descendant_signal(outcome: &Outcome, checks: &str, why: &str) {
    assert!(outcome.success(), "{why}\n{}", outcome.describe());
    let signal = outcome.json();
    assert_eq!(signal["pr_number"], "7", "{why}");
    assert_eq!(signal["pr_url"], PR_URL, "{why}");
    assert_eq!(signal["checks"], checks, "{why}");
    assert_eq!(signal["head"], "descendant", "{why}");
}

fn assert_no_signal(outcome: &Outcome, why: &str) {
    assert!(
        !outcome.success(),
        "{why}: must be no signal\n{}",
        outcome.describe()
    );
    assert_eq!(outcome.stdout, "", "{why}: no stdout on failure");
    assert!(
        outcome
            .stderr
            .contains("with its head at the local HEAD or a fast-forward of it"),
        "{why}: stderr must name the head rule\n{}",
        outcome.describe()
    );
}

fn assert_no_fetch_side_effects(fx: &Fixture) {
    assert_eq!(
        fx.git(&["for-each-ref", "refs/remotes"]).trim(),
        "",
        "the fetch must not write a remote-tracking ref"
    );
    assert!(
        !fx.repo.join(".git/FETCH_HEAD").exists(),
        "the fetch must not write FETCH_HEAD"
    );
}

// The mistt #138 history with green checks: the same PR, EXISTING_OPEN_PR.
#[test]
fn an_ancestor_local_head_with_green_checks_is_the_same_pr() {
    let fx = Fixture::new();
    let (local, pr_head) = merge_main_on_pushed(&fx);
    drop_pushed_branch(&fx);
    view_at(&fx, &pr_head, green_rollup());

    assert_descendant_signal(&fx.signal(BRANCH, Some("7")), "green", "signal");

    let outcome = validate(&fx, &as_refs(&pr_env(&fx)));
    assert!(outcome.success(), "{}", outcome.describe());
    assert_eq!(outcome.field("terminal_state"), "EXISTING_OPEN_PR");
    assert_eq!(outcome.field("pr_number"), "7");
    let evidence = outcome.field("evidence_used");
    assert!(
        evidence.contains("pr_head_sha_matches=false")
            && evidence.contains("pr_head_fast_forwards_local_head=true"),
        "the result records that the PR head is a fast-forward: {evidence}"
    );
    assert!(
        !outcome.stderr.contains("no open-PR signal"),
        "{}",
        outcome.describe()
    );
    assert_eq!(fx.head(), local, "the check never moves the local HEAD");
    fx.assert_gh_reads_only();
}

// M is not in the local object store: one fetch brings it in, and nothing
// else in the repository changes.
#[test]
fn a_pr_head_missing_locally_is_fetched_without_writing_refs() {
    let fx = Fixture::new();
    let (local, pr_head) = merge_main_on_pushed(&fx);
    origin_remote(&fx);
    fx.git(&[
        "push",
        "-q",
        "origin",
        &format!("pushed:refs/heads/{BRANCH}"),
    ]);
    drop_pushed_branch(&fx);
    for r in fx
        .git(&["for-each-ref", "--format=%(refname)", "refs/remotes"])
        .lines()
    {
        fx.git(&["update-ref", "-d", r]);
    }
    prune(&fx, &pr_head);
    view_at(&fx, &pr_head, green_rollup());

    assert_descendant_signal(&fx.signal(BRANCH, Some("7")), "green", "after fetch");
    assert!(has_object(&fx, &pr_head), "the fetch brought M in");
    assert_eq!(fx.head(), local);
    assert_no_fetch_side_effects(&fx);
}

// A missing object is never read as "ancestor": with no remote, or a remote
// whose branch is elsewhere, the check fails closed.
#[test]
fn an_unfetchable_pr_head_fails_closed() {
    let fx = Fixture::new();
    let (_, pr_head) = merge_main_on_pushed(&fx);
    drop_pushed_branch(&fx);
    prune(&fx, &pr_head);
    view_at(&fx, &pr_head, green_rollup());
    assert_no_signal(&fx.signal(BRANCH, Some("7")), "no remote");
    assert_failed_implementation(&validate(&fx, &as_refs(&pr_env(&fx))), "no remote");

    origin_remote(&fx);
    fx.git(&[
        "push",
        "-q",
        "origin",
        &format!("{BRANCH}:refs/heads/{BRANCH}"),
    ]);
    assert_no_signal(&fx.signal(BRANCH, Some("7")), "remote branch elsewhere");
    assert!(!has_object(&fx, &pr_head));
}

// A PR head that does not contain the local HEAD is not the same PR: a
// sibling commit (diverged), and a rewrite of the local commit (amended).
#[test]
fn a_diverged_or_rewritten_pr_head_fails_closed() {
    let fx = Fixture::new();
    let local = commit_run_work(&fx);
    fx.git(&["switch", "-q", "-c", "sibling", "main"]);
    fs::write(fx.repo.join("OTHER.md"), "other\n").expect("write other");
    fx.git(&["add", "OTHER.md"]);
    fx.git(&["commit", "-q", "-m", "other work"]);
    let diverged = fx.head();
    fx.git(&["switch", "-q", BRANCH]);
    fx.git(&["switch", "-q", "-c", "rewritten", BRANCH]);
    fx.git(&["commit", "-q", "--amend", "-m", "run work, amended"]);
    let rewritten = fx.head();
    fx.git(&["switch", "-q", BRANCH]);
    assert_eq!(fx.head(), local);

    for (name, pr_head) in [("diverged", &diverged), ("rewritten", &rewritten)] {
        view_at(&fx, pr_head, green_rollup());
        assert_no_signal(&fx.signal(BRANCH, Some("7")), name);
        let mut envs = pr_env(&fx);
        envs.push(("VERIFICATION_COMPLETED", "true".to_owned()));
        let outcome = validate(&fx, &as_refs(&envs));
        assert_failed_implementation(&outcome, name);
        assert_eq!(outcome.field("pr_number"), "", "{name}");
    }
}
