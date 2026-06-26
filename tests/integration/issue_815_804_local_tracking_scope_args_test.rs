//! Regression tests for issues #815 / #804 — downstream PR-scope consumers must
//! coerce a non-numeric local tracking `issue_number` (e.g. `local-5d904cff4398`)
//! to an empty `--issue` / `--work-item` before calling `workflow_pr_scope.sh`.
//!
//! `step-03b-extract-issue-number` now propagates a local tracking reference as
//! `issue_number`. PR-scope matching only understands numeric GitHub issue /
//! AzDO work-item ids, so every consumer that forwards `issue_number` to
//! `workflow_pr_scope.sh` must drop a non-numeric ref — otherwise the jq filter
//! rejects the legitimate current-work PR (`no_scoped_pr`). This locks that
//! contract for `workflow_pr_ready.sh` and `workflow_final_status.sh` (the
//! `workflow-terminal-state` consumer is covered in
//! `default_workflow_terminal_state.rs`).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop(); // bins/amplihack -> bins
    p.pop(); // bins -> workspace root
    p
}

fn helper_path(name: &str) -> PathBuf {
    workspace_root().join("amplifier-bundle/tools").join(name)
}

fn make_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("chmod");
    }
}

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(repo)
        .status()
        .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
    assert!(status.success(), "git {args:?} failed");
}

/// Minimal GitHub-remote repo on a `feature` branch with an `origin/main` base.
fn setup_repo(tmp: &Path) -> PathBuf {
    let repo = tmp.join("repo");
    fs::create_dir_all(&repo).expect("create repo");
    git(&repo, &["init", "-b", "main"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "Workflow Test"]);
    fs::write(repo.join("README.md"), "base\n").expect("write readme");
    git(&repo, &["add", "README.md"]);
    git(&repo, &["commit", "-m", "base"]);
    git(
        &repo,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/owner/repo.git",
        ],
    );
    git(&repo, &["update-ref", "refs/remotes/origin/main", "main"]);
    git(&repo, &["switch", "-c", "feature"]);
    fs::write(repo.join("feature.txt"), "feature\n").expect("write feature");
    git(&repo, &["add", "feature.txt"]);
    git(&repo, &["commit", "-m", "feature"]);
    repo
}

/// A stub `workflow_pr_scope.sh` that records every argument it receives (one
/// `[arg]` per line) and then fails closed with `no_scoped_pr`.
fn recorder_scope_helper(tmp: &Path, log: &Path) -> PathBuf {
    let stub = tmp.join("scope-recorder.sh");
    fs::write(
        &stub,
        format!(
            "#!/usr/bin/env bash\nfor a in \"$@\"; do printf '[%s]\\n' \"$a\" >> {log:?}; done\nprintf '{{\"ok\":false,\"reason\":\"no_scoped_pr\"}}\\n'\nexit 1\n",
            log = log.display().to_string()
        ),
    )
    .expect("write stub");
    make_executable(&stub);
    stub
}

/// A no-op `gh` so `command -v gh` succeeds where the consumer requires it.
fn stub_gh(bin_dir: &Path) {
    fs::create_dir_all(bin_dir).expect("create bin dir");
    let gh = bin_dir.join("gh");
    fs::write(&gh, "#!/usr/bin/env bash\nexit 0\n").expect("write gh");
    make_executable(&gh);
}

/// Returns the argument value immediately following `--<flag>` in the recorded
/// `[arg]`-per-line log.
fn arg_value(log: &str, flag: &str) -> String {
    let lines: Vec<&str> = log.lines().collect();
    let needle = format!("[{flag}]");
    let idx = lines
        .iter()
        .position(|l| *l == needle)
        .unwrap_or_else(|| panic!("flag {flag} not recorded; log:\n{log}"));
    let raw = lines
        .get(idx + 1)
        .unwrap_or_else(|| panic!("no value after {flag}; log:\n{log}"));
    raw.trim_start_matches('[')
        .trim_end_matches(']')
        .to_string()
}

fn run_pr_ready(repo: &Path, bin_dir: &Path, stub: &Path, issue_env: &[(&str, &str)]) {
    let old_path = std::env::var("PATH").unwrap_or_default();
    let mut cmd = Command::new("bash");
    cmd.arg(helper_path("workflow_pr_ready.sh"))
        .current_dir(repo)
        .env("PATH", format!("{}:{old_path}", bin_dir.display()))
        .env("WORKFLOW_PR_SCOPE_HELPER", stub);
    for (k, v) in issue_env {
        cmd.env(k, v);
    }
    let _ = cmd.output().expect("run workflow_pr_ready.sh");
}

fn run_final_status(repo: &Path, bin_dir: &Path, stub: &Path, issue_env: &[(&str, &str)]) {
    let old_path = std::env::var("PATH").unwrap_or_default();
    let mut cmd = Command::new("bash");
    cmd.arg(helper_path("workflow_final_status.sh"))
        .current_dir(repo)
        .env("PATH", format!("{}:{old_path}", bin_dir.display()))
        .env("WORKFLOW_PR_SCOPE_HELPER", stub)
        .env("REMOTE_HOST_TYPE", "github")
        .env("PR_URL", "https://github.com/owner/repo/pull/7")
        .env("TASK_DESCRIPTION", "test task");
    for (k, v) in issue_env {
        cmd.env(k, v);
    }
    let _ = cmd.output().expect("run workflow_final_status.sh");
}

#[test]
fn pr_ready_coerces_local_tracking_issue_number_to_empty() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = setup_repo(tmp.path());
    let bin_dir = tmp.path().join("bin");
    stub_gh(&bin_dir);
    let log = tmp.path().join("scope-args.log");
    let stub = recorder_scope_helper(tmp.path(), &log);

    run_pr_ready(
        &repo,
        &bin_dir,
        &stub,
        &[("RECIPE_VAR_issue_number", "local-5d904cff4398")],
    );

    let recorded = fs::read_to_string(&log).expect("read scope args log");
    assert_eq!(
        arg_value(&recorded, "--issue"),
        "",
        "pr_ready must pass an empty --issue for a local tracking reference; log:\n{recorded}"
    );
    assert_eq!(
        arg_value(&recorded, "--work-item"),
        "",
        "pr_ready must pass an empty --work-item for a local tracking reference; log:\n{recorded}"
    );
}

#[test]
fn pr_ready_preserves_numeric_issue_number() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = setup_repo(tmp.path());
    let bin_dir = tmp.path().join("bin");
    stub_gh(&bin_dir);
    let log = tmp.path().join("scope-args.log");
    let stub = recorder_scope_helper(tmp.path(), &log);

    run_pr_ready(&repo, &bin_dir, &stub, &[("ISSUE_NUMBER", "763")]);

    let recorded = fs::read_to_string(&log).expect("read scope args log");
    assert_eq!(
        arg_value(&recorded, "--issue"),
        "763",
        "pr_ready must preserve a real numeric issue number; log:\n{recorded}"
    );
}

#[test]
fn final_status_coerces_local_tracking_issue_number_to_empty() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = setup_repo(tmp.path());
    let bin_dir = tmp.path().join("bin");
    stub_gh(&bin_dir);
    let log = tmp.path().join("scope-args.log");
    let stub = recorder_scope_helper(tmp.path(), &log);

    run_final_status(
        &repo,
        &bin_dir,
        &stub,
        &[("RECIPE_VAR_issue_number", "local-5d904cff4398")],
    );

    let recorded = fs::read_to_string(&log).expect("read scope args log");
    assert_eq!(
        arg_value(&recorded, "--issue"),
        "",
        "final_status must pass an empty --issue for a local tracking reference; log:\n{recorded}"
    );
    assert_eq!(
        arg_value(&recorded, "--work-item"),
        "",
        "final_status must pass an empty --work-item for a local tracking reference; log:\n{recorded}"
    );
}

#[test]
fn final_status_preserves_numeric_issue_number() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = setup_repo(tmp.path());
    let bin_dir = tmp.path().join("bin");
    stub_gh(&bin_dir);
    let log = tmp.path().join("scope-args.log");
    let stub = recorder_scope_helper(tmp.path(), &log);

    run_final_status(&repo, &bin_dir, &stub, &[("ISSUE_NUMBER", "763")]);

    let recorded = fs::read_to_string(&log).expect("read scope args log");
    assert_eq!(
        arg_value(&recorded, "--issue"),
        "763",
        "final_status must preserve a real numeric issue number; log:\n{recorded}"
    );
}
