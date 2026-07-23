//! TDD-red contracts for workflow publish idempotency.
//!
//! These tests describe the publish behavior required before implementation:
//! no duplicate PRs, no empty PRs, and clean terminal success for already
//! handled branch states.

use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use serde_yaml::Value;
use tempfile::TempDir;

fn workspace_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path
}

fn load_publish_recipe() -> Value {
    let path = workspace_root()
        .join("amplifier-bundle")
        .join("recipes")
        .join("workflow-publish.yaml");
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

fn helper_text(name: &str) -> String {
    let path = workspace_root()
        .join("amplifier-bundle")
        .join("tools")
        .join(name);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn workspace_helper_path(name: &str) -> PathBuf {
    workspace_root()
        .join("amplifier-bundle")
        .join("tools")
        .join(name)
}

fn write_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|e| panic!("create {}: {e}", parent.display()));
    }
    fs::write(path, content).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

fn make_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))
            .unwrap_or_else(|e| panic!("chmod {}: {e}", path.display()));
    }
}

fn run_cmd(dir: &Path, program: &str, args: &[&str]) {
    let output = Command::new(program)
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("run {program} {args:?} in {}: {e}", dir.display()));
    assert!(
        output.status.success(),
        "command failed: {program} {args:?}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn step<'a>(recipe: &'a Value, id: &str) -> &'a Value {
    recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .expect("recipe must contain top-level steps")
        .iter()
        .find(|step| step.get("id").and_then(Value::as_str) == Some(id))
        .unwrap_or_else(|| panic!("missing recipe step {id}"))
}

fn step_command(recipe: &Value, id: &str) -> String {
    step(recipe, id)
        .get("command")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("step {id} must be a bash command step"))
        .to_owned()
}

#[test]
fn publish_step_emits_structured_terminal_outcome_for_idempotent_states() {
    let recipe = load_publish_recipe();
    let publish_step = step(&recipe, "step-16-create-draft-pr");

    assert_eq!(
        publish_step.get("parse_json").and_then(Value::as_bool),
        Some(true),
        "step-16 must emit structured JSON so downstream steps can distinguish created, existing, merged, no-diff, and skipped states"
    );
    assert_eq!(
        publish_step.get("output").and_then(Value::as_str),
        Some("pr_publish_result"),
        "step-16 output must be a structured publish result, not only a raw PR URL"
    );
}

#[test]
fn publish_checks_branch_diff_and_all_pr_states_before_creating_pr() {
    let recipe = load_publish_recipe();
    let command = step_command(&recipe, "step-16-create-draft-pr");

    for required in [
        "--state all",
        "gh pr view",
        "headRefName",
        "headRefOid",
        "mergedAt",
        "git diff --quiet",
        "no-diff",
        "existing-open-pr",
        "already-merged",
        "closed-after-merge",
        "closed-unmerged-with-diff",
    ] {
        assert!(
            command.contains(required),
            "publish must classify branch diff and all PR terminal states before creating a PR; missing `{required}`"
        );
    }
}

#[test]
fn publish_never_invokes_create_for_no_diff_or_existing_pr_states() {
    let recipe = load_publish_recipe();
    let command = step_command(&recipe, "step-16-create-draft-pr");

    let create_position = command
        .rfind("gh_pr_create_with_retry")
        .or_else(|| command.rfind("gh pr create"))
        .expect("publish step must still create PRs for the create-new-pr state");
    let guard_position = command
        .find("PUBLISH_STATE")
        .expect("publish step must compute a PUBLISH_STATE before PR creation");

    assert!(
        guard_position < create_position,
        "PUBLISH_STATE classification must happen before the first gh pr create invocation"
    );

    for terminal in ["no-diff", "existing-open-pr", "already-merged"] {
        assert!(
            command.contains(&format!("PUBLISH_STATE=\"{terminal}\""))
                || command.contains(&format!("PUBLISH_STATE='{terminal}'")),
            "terminal state `{terminal}` must return before PR creation"
        );
    }
}

#[test]
fn publish_retries_all_github_pr_metadata_and_mutation_calls() {
    let command = helper_text("workflow_publish_pr.sh");

    for required in [
        "sanitize_gh_stderr",
        "is_transient_gh_error",
        "gh_pr_list_with_retry",
        "gh_pr_view_with_retry",
        "gh_pr_create_with_retry",
        "timeout 60 gh pr list",
        "timeout 60 gh pr view",
        "timeout 60 gh pr create",
        "retrying (${attempt}/3)",
        "refusing to risk duplicate PR creation",
    ] {
        assert!(
            command.contains(required),
            "publish helper contract must include resilient GitHub call handling; missing `{required}`"
        );
    }

    assert!(
        !command.contains("VIEW_JSON=\"$(gh pr view"),
        "publish helper must not inspect existing PRs with a raw gh pr view call"
    );
}

#[test]
fn publish_helper_discovers_prs_with_exact_identity_validation() {
    let command = helper_text("workflow_publish_pr.sh");

    for required in [
        "--head \"$CURRENT_BRANCH\"",
        "baseRefName",
        "headRefName",
        "headRefOid",
        "headRepositoryOwner",
        "headRepository",
        "isCrossRepository",
        "workflow_pr_scope.sh",
        "scoped_pr_lookup",
        "parse_github_repo_identity",
        "scoped PR lookup failed",
    ] {
        assert!(
            command.contains(required),
            "publish helper must validate exact PR identity before trusting an existing PR; missing `{required}`"
        );
    }

    assert!(
        !command.contains("test(\"issue-$ISSUE_NUM\")")
            && !command.contains("test(\\\"issue-$ISSUE_NUM"),
        "publish helper must not use broad issue-number PR fallback matching"
    );
    assert!(
        !command.contains("baseRepository"),
        "publish helper must request only gh-supported fields; gh pr list/view do not support baseRepository"
    );
}

#[test]
fn publish_treats_non_github_hosts_as_structured_successful_skip() {
    let recipe = load_publish_recipe();
    let command = step_command(&recipe, "step-16-create-draft-pr");

    assert!(
        command.contains("non-github") && command.contains("terminal_status"),
        "non-GitHub hosts must emit a structured successful skip instead of an empty PR URL"
    );
    assert!(
        !command.contains("printf ''"),
        "non-GitHub skip must not be represented as an empty string output"
    );
}

#[test]
fn publish_commit_push_redacts_pull_and_push_failure_output() {
    let recipe = load_publish_recipe();
    let command = step_command(&recipe, "step-15-commit-push");

    assert!(
        command.contains("redact_sensitive_file"),
        "publish commit/push step must use a shared redaction helper"
    );
    assert!(
        command.contains("git fetch origin >\"$fetch_output_file\" 2>&1"),
        "publish commit/push step must capture fetch output before logging (issue #978)"
    );
    assert!(
        !command.contains("cat \"$push_stderr_file\""),
        "publish commit/push step must not print raw push stderr"
    );
    assert!(
        command.contains("https?://"),
        "publish redaction must cover both http:// and https:// credential-bearing URLs"
    );
}

#[test]
fn publish_helper_fails_dirty_no_diff_instead_of_terminal_success() {
    let tmp = TempDir::new().expect("tempdir");
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo).expect("create repo");
    run_cmd(&repo, "git", &["init", "-b", "main"]);
    run_cmd(&repo, "git", &["config", "user.email", "test@example.com"]);
    run_cmd(&repo, "git", &["config", "user.name", "Workflow Test"]);
    write_file(&repo.join("README.md"), "base\n");
    run_cmd(&repo, "git", &["add", "README.md"]);
    run_cmd(&repo, "git", &["commit", "-m", "base"]);
    run_cmd(
        &repo,
        "git",
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/owner/repo.git",
        ],
    );
    run_cmd(
        &repo,
        "git",
        &["update-ref", "refs/remotes/origin/main", "main"],
    );
    write_file(&repo.join("dirty.txt"), "uncommitted\n");

    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("create bin dir");
    let gh = bin_dir.join("gh");
    write_file(
        &gh,
        r#"#!/usr/bin/env bash
set -euo pipefail
echo "gh must not be called before dirty worktree failure: $*" >&2
exit 77
"#,
    );
    make_executable(&gh);

    let old_path = std::env::var("PATH").unwrap_or_default();
    let output = Command::new("bash")
        .arg(workspace_helper_path("workflow_publish_pr.sh"))
        .current_dir(&repo)
        .env("PATH", format!("{}:{old_path}", bin_dir.display()))
        .env("REMOTE_HOST_TYPE", "github")
        .env("ISSUE_NUMBER", "7")
        .output()
        .expect("run workflow_publish_pr.sh");

    assert!(
        !output.status.success(),
        "dirty no-diff worktree must fail instead of producing NO_DIFF_SUCCESS\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(r#""state":"FAILED_DIRTY_WORKTREE""#),
        "publish helper must emit structured dirty-worktree failure, stdout:\n{stdout}"
    );
    assert!(
        !stdout.contains("NO_DIFF_SUCCESS"),
        "dirty worktree must not be represented as no-diff success, stdout:\n{stdout}"
    );
}

#[test]
fn commit_push_marks_already_pushed_branch_as_publishable() {
    let recipe = load_publish_recipe();
    let command = step_command(&recipe, "step-15-commit-push");
    let tmp = TempDir::new().expect("tempdir");
    let origin = tmp.path().join("origin.git");
    let repo = tmp.path().join("repo");

    run_cmd(
        tmp.path(),
        "git",
        &["init", "--bare", origin.to_str().unwrap()],
    );
    run_cmd(
        tmp.path(),
        "git",
        &["clone", origin.to_str().unwrap(), repo.to_str().unwrap()],
    );
    run_cmd(&repo, "git", &["switch", "-c", "main"]);
    run_cmd(&repo, "git", &["config", "user.email", "test@example.com"]);
    run_cmd(&repo, "git", &["config", "user.name", "Workflow Test"]);
    write_file(&repo.join("README.md"), "base\n");
    run_cmd(&repo, "git", &["add", "README.md"]);
    run_cmd(&repo, "git", &["commit", "-m", "base"]);
    run_cmd(&repo, "git", &["push", "-u", "origin", "main"]);
    run_cmd(&repo, "git", &["switch", "-c", "feature"]);
    write_file(&repo.join("feature.txt"), "feature\n");
    run_cmd(&repo, "git", &["add", "feature.txt"]);
    run_cmd(&repo, "git", &["commit", "-m", "feature"]);
    run_cmd(&repo, "git", &["push", "-u", "origin", "feature"]);

    let output = Command::new("bash")
        .arg("-c")
        .arg(command)
        .current_dir(&repo)
        .env("WORKTREE_SETUP_WORKTREE_PATH", &repo)
        .env(
            "WORKFLOW_RUNTIME_ARTIFACT_HELPER",
            workspace_helper_path("workflow_runtime_artifacts.sh"),
        )
        .env("TASK_DESCRIPTION", "publish existing feature branch")
        .env("ISSUE_NUMBER", "7")
        .env("REMOTE_HOST_TYPE", "github")
        .output()
        .expect("run step-15 commit-push command");

    assert!(
        output.status.success(),
        "already-pushed branch must continue to PR publication\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(r#""pushed":"true""#) && stdout.contains(r#""reason":"already-pushed""#),
        "step-15 must mark already-pushed work as publishable, stdout:\n{stdout}"
    );
}
