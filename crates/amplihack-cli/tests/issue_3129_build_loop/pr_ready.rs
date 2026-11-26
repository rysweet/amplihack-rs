//! C1 (#2787): `workflow_pr_ready.sh` marks the run's own PR ready from the
//! PR worktree, accepts a head one commit behind the local head, and never
//! ends a finished run when it cannot validate its own PR.

#![cfg(target_os = "linux")]

use super::tools::{World, git, text};
use serde_json::json;

fn pr_json(head: &str) -> serde_json::Value {
    json!({"number": 7, "url": "https://github.com/o/r/pull/7", "state": "OPEN",
        "isDraft": true, "headRefName": "feat/x", "baseRefName": "main",
        "headRefOid": head, "isCrossRepository": false,
        "title": "T", "body": "Fixes #1", "createdAt": "2026-11-23T00:00:00Z",
        "headRepository": {"name": "r"}, "headRepositoryOwner": {"login": "o"}})
}

fn on_branch_with_two_commits(w: &World) -> (String, String) {
    git(&w.repo, &["checkout", "-q", "-b", "feat/x"]);
    std::fs::write(w.repo.join("b"), "1").unwrap();
    git(&w.repo, &["add", "."]);
    git(&w.repo, &["commit", "-qm", "one"]);
    let first = git(&w.repo, &["rev-parse", "HEAD"]);
    std::fs::write(w.repo.join("b"), "2").unwrap();
    git(&w.repo, &["commit", "-qam", "two"]);
    (first, git(&w.repo, &["rev-parse", "HEAD"]))
}

fn run(w: &World, dir: &std::path::Path) -> std::process::Output {
    let mut c = w.tool("workflow_pr_ready.sh", dir);
    c.args(["--base", "main"])
        .env("RECIPE_VAR_pr_number", "7")
        .env("RECIPE_VAR_pr_url", "https://github.com/o/r/pull/7");
    c.output().unwrap()
}

#[test]
fn a_pr_head_one_commit_behind_the_local_head_is_marked_ready() {
    let w = World::new();
    let (first, _second) = on_branch_with_two_commits(&w);
    w.prs(&json!([pr_json(&first)]));
    w.pr_view(&pr_json(&first));
    let out = run(&w, &w.repo);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert!(
        w.gh.join("ready-called").exists(),
        "{}\n{:?}",
        text(&out),
        w.calls()
    );
}

#[test]
fn run_from_a_checkout_on_main_it_reads_the_branch_and_head_from_the_pr_worktree() {
    let w = World::new();
    let (_first, second) = on_branch_with_two_commits(&w);
    git(&w.repo, &["checkout", "-q", "main"]);
    let wt = w.base.join("wt");
    git(
        &w.repo,
        &["worktree", "add", "-q", wt.to_str().unwrap(), "feat/x"],
    );
    w.prs(&json!([pr_json(&second)]));
    w.pr_view(&pr_json(&second));
    let mut c = w.tool("workflow_pr_ready.sh", &w.repo);
    c.args(["--base", "main"])
        .env("RECIPE_VAR_pr_number", "7")
        .env("RECIPE_VAR_pr_url", "https://github.com/o/r/pull/7")
        .env("RECIPE_VAR_existing_branch", "feat/x")
        .env("REPO_PATH", &w.repo);
    let out = c.output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert!(
        w.gh.join("ready-called").exists(),
        "{}\n{:?}",
        text(&out),
        w.calls()
    );
}

#[test]
fn an_own_pr_that_cannot_be_validated_stays_draft_and_the_tool_exits_zero() {
    let w = World::new();
    let (_f, second) = on_branch_with_two_commits(&w);
    let mut other = pr_json(&second);
    other["baseRefName"] = json!("release");
    w.prs(&json!([other.clone()]));
    w.pr_view(&other);
    let out = run(&w, &w.repo);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert!(!w.gh.join("ready-called").exists(), "{}", text(&out));
    assert!(
        !String::from_utf8_lossy(&out.stderr).trim().is_empty(),
        "the reason must be printed"
    );
}

#[test]
fn a_cross_repository_pr_is_never_marked_ready() {
    let w = World::new();
    let (_f, second) = on_branch_with_two_commits(&w);
    let mut x = pr_json(&second);
    x["isCrossRepository"] = json!(true);
    w.prs(&json!([x.clone()]));
    w.pr_view(&x);
    let _ = run(&w, &w.repo);
    assert!(!w.gh.join("ready-called").exists());
}
