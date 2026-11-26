//! AC17 (#3049, live-install safety): runs started on 1.2.213 source
//! `amplifier-bundle/tools` from the live bundle at step time. Each test runs
//! one changed tool the way a 1.2.213 caller runs it: the same arguments, the
//! output the caller reads and the exit code it acts on. The invocations were
//! copied from `git show v1.2.213:<recipe>` when this file was written; no
//! test reads recipe text.
//!
//! | Tool | 1.2.213 caller |
//! |---|---|
//! | `workflow_issue_claim_check.sh` | workflow-prep `step-03c-issue-claim-check`; `workflow_publish_pr.sh` |
//! | `workflow_issue_tracking.sh` | workflow-prep `step-03-create-issue` |
//! | `workflow_publish_pr.sh` | workflow-publish `step-16-create-draft-pr` |
//! | `workflow_pr_scope.sh` | workflow-terminal-state (and the PR ready and publish tools) |
//! | `workflow_pr_ready.sh` | workflow-finalize `step-21-pr-ready` |

#![cfg(target_os = "linux")]

use super::tools::{World, git, text, tools_dir};
use crate::support::exec_stub::write_executable;
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use std::process::Command;

fn claim(number: u32, head: &str) -> Value {
    json!({"number": number, "url": format!("https://github.com/o/r/pull/{number}"),
        "title": "Runner content follow-ups", "state": "OPEN", "mergedAt": null,
        "headRefName": head, "isCrossRepository": false,
        "closingIssuesReferences": [{"number": 463}], "baseRefName": "main",
        "body": "Fixes #463"})
}

fn pr(head_ref: &str, base: &str, head_oid: &str) -> Value {
    json!({"number": 7, "url": "https://github.com/o/r/pull/7", "state": "OPEN",
        "isDraft": true, "headRefName": head_ref, "baseRefName": base,
        "headRefOid": head_oid, "isCrossRepository": false,
        "title": "T", "body": "Fixes #1", "createdAt": "2026-11-23T00:00:00Z",
        "headRepository": {"name": "r"}, "headRepositoryOwner": {"login": "o"}})
}

/// 1.2.213 step-03c: `bash "$C" --issue "${RECIPE_VAR_issue_number:-}"
/// --repo-path "${REPO_PATH:-.}"`. Its stdout becomes the step's
/// `issue_claim_check` output, and a non-zero exit fails the step.
#[test]
fn claim_check_called_as_step_03c_exits_zero_with_one_claimed_key_line() {
    let w = World::new();
    w.prs(&json!([claim(481, "feat/other")]));
    let out = w
        .tool("workflow_issue_claim_check.sh", &w.repo)
        .args(["--issue", "463", "--repo-path"])
        .arg(&w.repo)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "issue_claim_check: claimed issue=463 repo=o/r pr=https://github.com/o/r/pull/481\n",
        "{}",
        text(&out)
    );

    let empty = w
        .tool("workflow_issue_claim_check.sh", &w.repo)
        .args(["--issue", "", "--repo-path"])
        .arg(&w.repo)
        .output()
        .unwrap();
    assert_eq!(empty.status.code(), Some(0), "{}", text(&empty));
    assert_eq!(
        String::from_utf8_lossy(&empty.stdout),
        "issue_claim_check: skipped reason=no-issue-number\n"
    );
}

/// 1.2.213 `workflow_publish_pr.sh`: `--issue N --repo OWNER/REPO --head
/// BRANCH`, stdout discarded, stderr kept; a non-zero exit was
/// FAILED_DUPLICATE_CLAIM.
#[test]
fn claim_check_called_as_publish_calls_it_exits_zero_and_names_the_claim() {
    let w = World::new();
    w.prs(&json!([claim(481, "feat/other")]));
    let out = w
        .tool("workflow_issue_claim_check.sh", &w.repo)
        .args(["--issue", "463", "--repo", "o/r", "--head", "feat/new"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("#481"),
        "{}",
        text(&out)
    );
}

/// 1.2.213 step-03 sources the tracking helper and calls
/// `select_github_tracking_issue` with EXISTING_ISSUE_NUMBER, printing the
/// issue URL, and `emit_local_metadata` for local tracking.
#[test]
fn tracking_functions_step_03_calls_still_answer_as_before() {
    let w = World::new();
    let script = format!(
        ". '{}/workflow_issue_tracking.sh'; select_github_tracking_issue; echo rc=$?; emit_local_metadata",
        tools_dir().display()
    );
    // The tool is sourced by the step, not run, so bash gets it through -c.
    let out = Command::new("bash")
        .args(["-c", &script])
        .current_dir(&w.repo)
        .env_clear()
        .env("PATH", format!("{}:/usr/bin:/bin", w.bin.display()))
        .env("HOME", &w.base)
        .env("TMPDIR", &w.base)
        .env("GH_STATE", &w.gh)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("EXISTING_ISSUE_NUMBER", "463")
        .env("TASK_DESC", "Build the widget. Fixes #463.")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines[0], "https://github.com/o/r/issues/463", "{stdout}");
    assert_eq!(lines[1], "rc=0", "{stdout}");
    assert!(
        lines.contains(&"tracking_system=local"),
        "emit_local_metadata keeps its key lines: {stdout}"
    );
}

/// A pushed feature branch with a real diff against origin/main, and origin
/// renamed to the GitHub URL the stand-in gh answers for.
fn pushed_branch(w: &World, branch: &str) {
    let origin = w.base.join("origin.git");
    git(
        &w.base,
        &[
            "init",
            "-q",
            "--bare",
            "-b",
            "main",
            origin.to_str().unwrap(),
        ],
    );
    git(
        &w.repo,
        &["remote", "set-url", "origin", origin.to_str().unwrap()],
    );
    git(&w.repo, &["push", "-q", "-u", "origin", "main"]);
    git(&w.repo, &["checkout", "-q", "-b", branch]);
    fs::write(w.repo.join("b.txt"), "b\n").unwrap();
    git(&w.repo, &["add", "b.txt"]);
    git(&w.repo, &["commit", "-qm", "change"]);
    git(&w.repo, &["push", "-q", "-u", "origin", branch]);
    git(
        &w.repo,
        &["remote", "set-url", "origin", "git@github.com:o/r.git"],
    );
}

/// A `gh` in front of the shared stand-in that answers `pr create` with a
/// PR URL and passes every other call on. Returns the PATH to use.
fn with_pr_create(w: &World) -> String {
    let bin2 = w.base.join("bin2");
    fs::create_dir_all(&bin2).unwrap();
    write_executable(
        &bin2.join("gh"),
        format!(
            "#!/usr/bin/env bash\nif [ \"${{1:-}} ${{2:-}}\" = \"pr create\" ]; then printf '%s\\n' \"$*\" >> \"$GH_STATE/calls.log\"; echo https://github.com/o/r/pull/9; exit 0; fi\nexec '{}' \"$@\"\n",
            w.bin.join("gh").display()
        ),
        0o755,
    );
    format!("{}:{}:/usr/bin:/bin", bin2.display(), w.bin.display())
}

/// 1.2.213 step-16 runs `bash "$PUBLISH_HELPER"` with no arguments in the
/// run's worktree and keeps its stdout JSON as `pr_publish_result`. A claim by
/// another branch's PR is now information: the PR is opened and the result is
/// a success, never FAILED_DUPLICATE_CLAIM.
#[test]
fn publish_called_as_step_16_opens_the_pr_beside_another_branchs_claim() {
    let w = World::new();
    pushed_branch(&w, "feat/new");
    w.prs(&json!([claim(481, "feat/other")]));
    let mut c = w.tool("workflow_publish_pr.sh", &w.repo);
    for k in [
        "DESIGN_SPEC",
        "design_spec",
        "WORKFLOW_PR_LABELS",
        "EXPECTED_PR_TITLE_PREFIX",
        "PR_EXPECTED_TITLE_PREFIX",
        "WORKFLOW_STARTED_AT",
        "RECIPE_STARTED_AT",
        "TASK_STARTED_AT",
        "GH_REPO",
        "GH_HOST",
        "WORKFLOW_ISSUE_CLAIM_CHECK_HELPER",
        "WORKFLOW_PR_SCOPE_HELPER",
    ] {
        c.env_remove(k);
    }
    let out = c
        .env("PATH", with_pr_create(&w))
        .env("REMOTE_HOST_TYPE", "github")
        .env("ISSUE_NUMBER", "463")
        .env("RECIPE_VAR_base_branch", "main")
        .env("RECIPE_VAR_task_description", "Build the widget")
        .env("GIT_AUTHOR_NAME", "T")
        .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
        .env("GIT_COMMITTER_NAME", "T")
        .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let result: Value =
        serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{e}: {}", text(&out)));
    assert_eq!(result["terminal_status"], "success", "{result}");
    assert_eq!(
        result["pr_url"], "https://github.com/o/r/pull/9",
        "{result}"
    );
    assert_ne!(result["state"], "FAILED_DUPLICATE_CLAIM", "{result}");
    assert!(
        w.calls().iter().any(|c| c.starts_with("pr create")),
        "{:?}",
        w.calls()
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("#481"),
        "the claim is printed as information: {}",
        text(&out)
    );
}

/// 1.2.213 workflow-terminal-state calls the scope tool with
/// `--repo --head --base --issue --work-item --head-sha --allow-head-behind`
/// and a PR number, and reads stdout's `.reason` and the exit code.
#[test]
fn scope_called_as_terminal_state_keeps_reason_exit_and_first_error_line() {
    let w = World::new();
    git(&w.repo, &["checkout", "-q", "-b", "feat/x"]);
    let head = git(&w.repo, &["rev-parse", "HEAD"]);
    w.pr_view(&pr("feat/x", "release", &head));
    let out = w
        .tool("workflow_pr_scope.sh", &w.repo)
        .args([
            "--repo",
            "o/r",
            "--head",
            "feat/x",
            "--base",
            "main",
            "--issue",
            "",
            "--work-item",
            "",
            "--head-sha",
            &head,
            "--allow-head-behind",
            "--pr-number",
            "7",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout.lines().count(), 1, "{}", text(&out));
    let v: Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(v["reason"], "no_scoped_pr");
    assert_eq!(v["rejected"][0]["number"], 7, "{v}");
    assert_eq!(v["rejected"][0]["rejected_by"], json!(["base"]), "{v}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        stderr.lines().next(),
        Some(
            "ERROR: workflow_pr_scope.sh: no_scoped_pr: no PR matched the explicit workflow scope"
        ),
        "{}",
        text(&out)
    );
    assert!(
        stderr.contains("#7") && stderr.contains("release"),
        "{}",
        text(&out)
    );
}

fn ready_from(w: &World, dir: &Path, base: &str) -> std::process::Output {
    // 1.2.213 step-21: `bash "$READY_HELPER" --base "${RECIPE_VAR_base_branch:-}"`,
    // with the PR known from step-16's pr_publish_result.
    w.tool("workflow_pr_ready.sh", dir)
        .args(["--base", base])
        .env(
            "RECIPE_VAR_pr_publish_result",
            json!({"pr_url": "https://github.com/o/r/pull/7", "pr_number": "7",
                "base_branch": "main"})
            .to_string(),
        )
        .env("REPO_PATH", &w.repo)
        .output()
        .unwrap()
}

/// #2787 as 1.2.213 meets it: step-21 could not resolve worktree_setup and
/// fell back to REPO_PATH, the launch checkout on main. The tool finds the
/// PR's worktree from the PR's own head branch and marks the PR ready.
#[test]
fn pr_ready_called_as_step_21_from_the_launch_checkout_on_main_marks_the_pr_ready() {
    let w = World::new();
    // A clone has origin/main; the recorded base is checked against it.
    git(&w.repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    git(&w.repo, &["checkout", "-q", "-b", "feat/x"]);
    fs::write(w.repo.join("b"), "1").unwrap();
    git(&w.repo, &["add", "b"]);
    git(&w.repo, &["commit", "-qm", "one"]);
    let head = git(&w.repo, &["rev-parse", "HEAD"]);
    git(&w.repo, &["checkout", "-q", "main"]);
    let wt = w.base.join("wt");
    git(
        &w.repo,
        &["worktree", "add", "-q", wt.to_str().unwrap(), "feat/x"],
    );
    w.prs(&json!([pr("feat/x", "main", &head)]));
    w.pr_view(&pr("feat/x", "main", &head));
    let out = ready_from(&w, &w.repo, "");
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert!(
        w.calls().iter().any(|c| c.starts_with("pr ready")),
        "{}\n{:?}",
        text(&out),
        w.calls()
    );
}

/// A failed validation of the run's own PR prints the reason, exits 0 and
/// records no `pr ready` call at all.
#[test]
fn pr_ready_with_a_failed_validation_records_no_pr_ready_call() {
    let w = World::new();
    git(&w.repo, &["checkout", "-q", "-b", "feat/x"]);
    let head = git(&w.repo, &["rev-parse", "HEAD"]);
    w.prs(&json!([pr("feat/x", "release", &head)]));
    w.pr_view(&pr("feat/x", "release", &head));
    let out = ready_from(&w, &w.repo, "main");
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert!(
        !w.calls().iter().any(|c| c.starts_with("pr ready")),
        "{:?}",
        w.calls()
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("WARNING"),
        "{}",
        text(&out)
    );
}
