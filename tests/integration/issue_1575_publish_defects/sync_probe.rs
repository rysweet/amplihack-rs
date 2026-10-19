//! Defect 1 against the real probe: after `workflow_sync_pr_head.sh --mode
//! pre-probe`, `probe-terminal-state` no longer sees a stale PR head for runs
//! that continue an existing PR; on divergence the sync prints the #978 block,
//! pushes nothing, and the probe still fails closed.

use std::path::Path;

use serde_json::Value as JsonValue;
use serde_yaml::Value;

use super::sync::{commit_ahead, diverge_remote, sync};
use super::{Sandbox, assert_line, dump, helper_path, read, recipe_path, text};

// ---------------------------------------------------------------------------
// Probe harness: run the real workflow-terminal-state commands with a gh stub
// whose headRefOid is read from the bare remote at call time.
// ---------------------------------------------------------------------------

fn probe_command() -> String {
    let text = read(&recipe_path("workflow-terminal-state"));
    let recipe: Value = serde_yaml::from_str(&text).expect("parse workflow-terminal-state");
    recipe["steps"]
        .as_sequence()
        .expect("steps")
        .iter()
        .filter_map(|step| step.get("command").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
}

fn write_pr_stub(sb: &Sandbox, origin: &Path) {
    let script = format!(
        r#"#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "$GH_CALL_LOG"
oid="$(git --git-dir={origin:?} rev-parse refs/heads/feature)"
pr="$(jq -nc --arg oid "$oid" '{{url:"https://github.com/owner/repo/pull/7",number:7,state:"OPEN",mergedAt:"",title:"feat: issue #1575",body:"Refs #1575",createdAt:"2026-10-16T00:00:00Z",headRefName:"feature",baseRefName:"main",headRefOid:$oid,headRepositoryOwner:{{login:"owner"}},headRepository:{{name:"repo"}},isCrossRepository:false,statusCheckRollup:[]}}')"
case "${{1:-}} ${{2:-}}" in
  "auth status") exit 0 ;;
  "pr list") printf '[%s]\n' "$pr" ;;
  "pr view") printf '%s\n' "$pr" ;;
  *) echo "unexpected gh call: $*" >&2; exit 99 ;;
esac
"#,
        origin = origin.display().to_string()
    );
    sb.write_gh(&script);
}

struct ProbeRun {
    stderr: String,
    json: JsonValue,
}

/// Point origin at a GitHub URL (the stub answers for it) and run the probe.
fn run_probe(sb: &Sandbox, repo: &Path, origin: &Path, pr_number: Option<&str>) -> ProbeRun {
    write_pr_stub(sb, origin);
    sb.git(
        repo,
        &[
            "remote",
            "set-url",
            "origin",
            "https://github.com/owner/repo.git",
        ],
    );
    let output = sb
        .command("bash", repo)
        .arg("-c")
        .arg(probe_command())
        .env("REPO_PATH", repo)
        .env("RECIPE_VAR_worktree_setup__worktree_path", repo)
        .env("BRANCH_NAME", "feature")
        .env("BASE_REF", "main")
        .env("PR_NUMBER", pr_number.unwrap_or(""))
        .env("PR_URL", "")
        .env(
            "WORKFLOW_PR_SCOPE_HELPER",
            helper_path("workflow_pr_scope.sh"),
        )
        .env("GOAL_ALREADY_MET", "false")
        .output()
        .expect("run workflow-terminal-state probe");
    let stdout = text(&output.stdout);
    let line = stdout
        .lines()
        .rev()
        .find(|l| l.trim_start().starts_with('{'))
        .unwrap_or_else(|| panic!("probe emitted no JSON\n{}", dump(&output)));
    ProbeRun {
        stderr: text(&output.stderr),
        json: serde_json::from_str(line).expect("probe JSON"),
    }
}

// ---------------------------------------------------------------------------
// Ahead: every way of continuing an existing PR gets its commits pushed.
// ---------------------------------------------------------------------------

fn assert_ahead_branch_synced_before_probe(run_env: &[(&str, &str)], pr_number: Option<&str>) {
    let sb = Sandbox::new();
    let fx = sb.remote_fixture();
    let head = commit_ahead(&sb, &fx.repo, "resumed work 1");
    let head2 = commit_ahead(&sb, &fx.repo, "resumed work 2");
    assert_ne!(head, head2);

    let output = sync(&sb, &fx.repo, "pre-probe", run_env);
    assert_eq!(
        output.status.code(),
        Some(0),
        "pre-probe must exit 0\n{}",
        dump(&output)
    );
    assert!(
        output.stdout.is_empty(),
        "pre-probe prints nothing on stdout\n{}",
        dump(&output)
    );
    assert_eq!(
        sb.bare_ref(&fx.origin, "feature").as_deref(),
        Some(head2.as_str()),
        "the PR head on the remote must equal local HEAD after the sync\n{}",
        dump(&output)
    );
    assert_eq!(
        sb.head(&fx.repo),
        head2,
        "the sync must never rewrite local history"
    );

    let probe = run_probe(&sb, &fx.repo, &fx.origin, pr_number);
    assert!(
        !probe.stderr.contains("stale PR metadata") && !probe.stderr.contains("no_scoped_pr"),
        "after the sync the probe must not see a stale PR head\nstderr:\n{}",
        probe.stderr
    );
    assert_ne!(
        probe.json["terminal_state"], "FAILED_INVALID_INPUT",
        "{}",
        probe.json
    );
}

#[test]
fn baseline_probe_reports_stale_pr_metadata_without_sync() {
    // Harness check: proves the stub really reads the bare remote, so the
    // positive tests below are meaningful.
    let sb = Sandbox::new();
    let fx = sb.remote_fixture();
    commit_ahead(&sb, &fx.repo, "unpushed");
    let probe = run_probe(&sb, &fx.repo, &fx.origin, None);
    assert!(
        probe.stderr.contains("stale PR metadata"),
        "{}",
        probe.stderr
    );
}

#[test]
fn pre_probe_pushes_ahead_commits_for_existing_branch_runs() {
    assert_ahead_branch_synced_before_probe(&[("RECIPE_VAR_existing_branch", "feature")], None);
}

#[test]
fn pre_probe_pushes_ahead_commits_for_pr_number_runs() {
    assert_ahead_branch_synced_before_probe(&[("RECIPE_VAR_pr_number", "7")], Some("7"));
}

#[test]
fn pre_probe_pushes_ahead_commits_for_resume_checkpoint_runs() {
    assert_ahead_branch_synced_before_probe(
        &[(
            "RECIPE_VAR_resume_checkpoint",
            "checkpoint-after-review-feedback",
        )],
        None,
    );
}

#[test]
fn pre_probe_pushes_even_when_upstream_merge_points_at_main() {
    let sb = Sandbox::new();
    let fx = sb.remote_fixture();
    let main_before = sb.bare_ref(&fx.origin, "main");
    sb.git(
        &fx.repo,
        &["config", "branch.feature.merge", "refs/heads/main"],
    );
    let head = commit_ahead(&sb, &fx.repo, "local");
    let output = sync(&sb, &fx.repo, "pre-probe", &[]);
    assert_eq!(output.status.code(), Some(0), "{}", dump(&output));
    assert_eq!(
        sb.bare_ref(&fx.origin, "main"),
        main_before,
        "main must never move"
    );
    assert_eq!(
        sb.bare_ref(&fx.origin, "feature").as_deref(),
        Some(head.as_str())
    );
}

// ---------------------------------------------------------------------------
// Diverged: loud ERROR, nothing pushed, probe still fails closed.
// ---------------------------------------------------------------------------

#[test]
fn pre_probe_refuses_divergence_loudly_and_leaves_the_probe_to_fail_closed() {
    let sb = Sandbox::new();
    let fx = sb.remote_fixture();
    // The last commit both sides share; the local clone never fetched `remote`.
    let merge_base = sb.bare_ref(&fx.origin, "feature").expect("feature");
    let remote = diverge_remote(&sb, &fx.origin);
    let local = commit_ahead(&sb, &fx.repo, "amended locally");

    let output = sync(&sb, &fx.repo, "pre-probe", &[]);
    let stderr = text(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(0),
        "pre-probe exits 0 on divergence\n{}",
        dump(&output)
    );
    assert!(output.stdout.is_empty(), "{}", dump(&output));
    assert_line(
        &stderr,
        "ERROR: publish-sync-pr-head refuses to rebase divergent history onto upstream 'origin/feature' (issue #978).",
        "pre-probe divergence header",
    );
    for needle in [
        local.as_str(),
        remote.as_str(),
        &format!("merge_base={merge_base}"),
        "ahead=1",
        "behind=1",
    ] {
        assert!(
            stderr.contains(needle),
            "divergence block must name `{needle}`\n{stderr}"
        );
    }
    for cmd in [
        "git fetch origin",
        "git log --oneline @{u}..HEAD",
        "git log --oneline HEAD..@{u}",
        "git rebase @{u}",
        "git push --force-with-lease",
    ] {
        assert!(
            stderr.contains(cmd),
            "recovery command `{cmd}` missing\n{stderr}"
        );
    }
    assert_eq!(
        sb.bare_ref(&fx.origin, "feature").as_deref(),
        Some(remote.as_str())
    );
    assert_eq!(sb.head(&fx.repo), local, "no rebase, no rewrite");

    let probe = run_probe(&sb, &fx.repo, &fx.origin, None);
    assert!(
        probe.stderr.contains("stale PR metadata"),
        "{}",
        probe.stderr
    );
}

// ---------------------------------------------------------------------------
// The recipe step itself: never pushes from REPO_PATH, the caller's checkout
// (#1528), and never fails the recipe.
// ---------------------------------------------------------------------------

fn sync_step_command() -> String {
    let text = read(&recipe_path("workflow-publish"));
    let recipe: Value = serde_yaml::from_str(&text).expect("parse workflow-publish");
    recipe["steps"]
        .as_sequence()
        .expect("steps")
        .iter()
        .find(|s| s.get("id").and_then(Value::as_str) == Some("publish-sync-pr-head"))
        .and_then(|s| s.get("command").and_then(Value::as_str))
        .expect("publish-sync-pr-head command")
        .to_string()
}

fn run_sync_step(sb: &Sandbox, repo_path: &Path, worktree_path: &str) -> std::process::Output {
    sb.command("bash", sb.root())
        .arg("-c")
        .arg(sync_step_command())
        .env("REPO_PATH", repo_path)
        .env("RECIPE_VAR_worktree_setup__worktree_path", worktree_path)
        .output()
        .expect("run publish-sync-pr-head")
}

#[test]
fn sync_step_never_pushes_from_repo_path_without_a_worktree_path() {
    let sb = Sandbox::new();
    let fx = sb.remote_fixture();
    let before = sb.bare_ref(&fx.origin, "feature");
    commit_ahead(&sb, &fx.repo, "unpushed work in the caller's checkout");

    let output = run_sync_step(&sb, &fx.repo, "");
    assert_eq!(output.status.code(), Some(0), "{}", dump(&output));
    assert_line(
        &text(&output.stderr),
        "WARNING: publish-sync-pr-head: no usable worktree path; PR head not synced (#1575)",
        "empty worktree path",
    );
    assert_eq!(
        sb.bare_ref(&fx.origin, "feature"),
        before,
        "the caller's unpushed commits must not be pushed\n{}",
        dump(&output)
    );
}

#[test]
fn sync_step_refuses_a_path_that_is_not_a_linked_worktree() {
    let sb = Sandbox::new();
    let fx = sb.remote_fixture();
    let before = sb.bare_ref(&fx.origin, "feature");
    commit_ahead(&sb, &fx.repo, "unpushed work in the main checkout");

    let output = run_sync_step(&sb, &fx.repo, fx.repo.to_str().unwrap());
    assert_eq!(output.status.code(), Some(0), "{}", dump(&output));
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("is not a linked task worktree; PR head not synced (#1528/#1575)"),
        "{stderr}"
    );
    assert_eq!(sb.bare_ref(&fx.origin, "feature"), before);
}

#[test]
fn sync_step_pushes_from_a_linked_task_worktree() {
    let sb = Sandbox::new();
    let fx = sb.remote_fixture();
    sb.git(&fx.repo, &["switch", "-q", "main"]);
    let worktree = sb.root().join("worktree");
    sb.git(
        &fx.repo,
        &[
            "worktree",
            "add",
            "-q",
            worktree.to_str().unwrap(),
            "feature",
        ],
    );
    let head = commit_ahead(&sb, &worktree, "resumed work");

    let output = run_sync_step(&sb, &fx.repo, worktree.to_str().unwrap());
    assert_eq!(output.status.code(), Some(0), "{}", dump(&output));
    assert_eq!(
        sb.bare_ref(&fx.origin, "feature").as_deref(),
        Some(head.as_str()),
        "{}",
        dump(&output)
    );
}
