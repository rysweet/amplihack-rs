//! Defect 3 wiring: `workflow_pr_ready.sh` sources the auth gate, keeps the
//! contract error, and refuses an account without push permission before any
//! PR call.

use std::path::Path;
use std::process::Output;

use super::auth::{AUTH_ERROR, count_calls, sandbox};
use super::{Sandbox, assert_line, dump, text};

// --- workflow_pr_ready.sh wiring -------------------------------------------

fn pr_ready(sb: &Sandbox, dir: &Path, env: &[(&str, &str)]) -> Output {
    sb.run_helper(dir, "workflow_pr_ready.sh", &[], env)
}

fn github_repo(sb: &Sandbox) -> std::path::PathBuf {
    let repo = sb.root().join("repo");
    sb.git(
        sb.root(),
        &["init", "-q", "-b", "main", repo.to_str().unwrap()],
    );
    sb.commit_file(&repo, "README.md", "base\n", "base");
    sb.git(
        &repo,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/owner/repo.git",
        ],
    );
    sb.git(&repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    sb.git(&repo, &["switch", "-q", "-c", "feature"]);
    sb.commit_file(&repo, "f.txt", "f\n", "feature");
    repo
}

#[test]
fn pr_ready_fails_with_exit_2_when_the_gate_helper_is_missing() {
    let sb = sandbox();
    let repo = github_repo(&sb);
    let out = pr_ready(
        &sb,
        &repo,
        &[("WORKFLOW_GH_AUTH_GATE_HELPER", "/nonexistent/gate.sh")],
    );
    assert_eq!(out.status.code(), Some(2), "{}", dump(&out));
    assert!(text(&out.stderr).contains("ERROR"), "{}", dump(&out));
    assert_eq!(count_calls(&sb, "auth status"), 0);
}

#[test]
fn pr_ready_retries_a_transient_auth_drop() {
    let sb = sandbox();
    let repo = github_repo(&sb);
    let out = pr_ready(&sb, &repo, &[("GH_AUTH_FAILS", "2")]);
    assert!(
        count_calls(&sb, "auth status") >= 3,
        "{}\n{}",
        sb.gh_calls(),
        dump(&out)
    );
    assert!(!text(&out.stderr).contains(AUTH_ERROR), "{}", dump(&out));
}

#[test]
fn pr_ready_keeps_the_contract_error_when_auth_never_returns() {
    let sb = sandbox();
    let repo = github_repo(&sb);
    let out = pr_ready(
        &sb,
        &repo,
        &[
            ("GH_AUTH_FAILS", "always"),
            ("WORKFLOW_GH_AUTH_ATTEMPTS", "2"),
        ],
    );
    assert_eq!(out.status.code(), Some(1), "{}", dump(&out));
    assert_line(&text(&out.stderr), AUTH_ERROR, "pr_ready auth");
    assert_eq!(count_calls(&sb, "auth status"), 2);
}

#[test]
fn pr_ready_refuses_an_account_without_push_permission() {
    let sb = sandbox();
    let repo = github_repo(&sb);
    let out = pr_ready(
        &sb,
        &repo,
        &[
            ("GH_PUSH_SEQ", "false"),
            ("WORKFLOW_GH_AUTH_ATTEMPTS", "2"),
            ("PR_URL", "https://github.com/owner/repo/pull/7"),
        ],
    );
    assert_eq!(out.status.code(), Some(1), "{}", dump(&out));
    assert_line(
        &text(&out.stderr),
        "ERROR: active gh account 'tester' lacks push permission on owner/repo",
        "pr_ready push",
    );
    assert!(
        !sb.gh_calls().lines().any(|l| l.starts_with("pr ")),
        "no PR call after denial\n{}",
        sb.gh_calls()
    );
}
