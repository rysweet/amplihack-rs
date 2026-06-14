//! TDD-red contracts for workflow finalization idempotency.
//!
//! Finalization must treat already terminal successful states as success while
//! still surfacing active CI failures and closed-unmerged PRs as actionable
//! failures.

use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use serde_json::Value as JsonValue;
use serde_yaml::Value;
use tempfile::TempDir;

fn workspace_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path
}

fn load_finalize_recipe() -> Value {
    let path = workspace_root()
        .join("amplifier-bundle")
        .join("recipes")
        .join("workflow-finalize.yaml");
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

fn run_agentic_finalization(mode: &str, envs: &[(&str, &str)]) -> std::process::Output {
    let mut command = Command::new("bash");
    command
        .arg(workspace_helper_path("workflow_agentic_finalization.sh"))
        .arg(mode);
    for (key, value) in envs {
        command.env(key, value);
    }
    command
        .output()
        .expect("run workflow_agentic_finalization.sh")
}

fn step_command(recipe: &Value, id: &str) -> String {
    recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .expect("recipe must contain top-level steps")
        .iter()
        .find(|step| step.get("id").and_then(Value::as_str) == Some(id))
        .and_then(|step| step.get("command").and_then(Value::as_str))
        .unwrap_or_else(|| panic!("step {id} must be a bash command step"))
        .to_owned()
}

#[test]
fn ready_step_distinguishes_merged_closed_after_merge_and_closed_unmerged() {
    let recipe = load_finalize_recipe();
    let command = step_command(&recipe, "step-21-pr-ready");

    for required in [
        "mergedAt",
        "closed-after-merge",
        "closed-unmerged",
        "terminal_status",
        "exit 1",
    ] {
        assert!(
            command.contains(required),
            "step-21 must classify merged/closed PR states explicitly; missing `{required}`"
        );
    }

    assert!(
        !command.contains("PR is closed — no ready-for-review action possible")
            && !command.contains("PR is closed - no ready-for-review action possible"),
        "closed-unmerged PRs must not be silently skipped as successful finalization"
    );
}

#[test]
fn final_status_accepts_no_diff_and_already_merged_as_successful_terminal_outcomes() {
    let recipe = load_finalize_recipe();
    let command = step_command(&recipe, "step-22b-final-status");

    for required in [
        "git diff --quiet",
        "no-diff",
        "already-merged",
        "closed-after-merge",
        "terminal_status",
    ] {
        assert!(
            command.contains(required),
            "final status must report idempotent successful terminal outcomes; missing `{required}`"
        );
    }
}

#[test]
fn final_status_uses_resilient_github_status_lookup() {
    let command = helper_text("workflow_final_status.sh");

    for required in [
        "sanitize_gh_stderr",
        "is_transient_gh_error",
        "gh_pr_view_with_retry",
        "timeout 60 gh pr view",
        "for attempt in 1 2 3",
        "retrying (${attempt}/3)",
        "final PR status lookup failed",
    ] {
        assert!(
            command.contains(required),
            "final status must use bounded, visible GitHub status lookup resilience; missing `{required}`"
        );
    }
}

#[test]
fn pr_ready_helper_retries_github_lookup_ready_and_comment_calls() {
    let command = helper_text("workflow_pr_ready.sh");

    for required in [
        "sanitize_gh_stderr",
        "is_transient_gh_error",
        "gh_with_retry",
        "timeout 60 gh",
        "for attempt in 1 2 3",
        "retrying (${attempt}/3)",
        "workflow_pr_scope.sh",
        "gh_with_retry \"pr view\"",
        "gh_with_retry \"pr ready\"",
        "gh_with_retry \"pr comment\"",
        "isCrossRepository",
    ] {
        assert!(
            command.contains(required),
            "PR-ready helper must use bounded GitHub retries; missing `{required}`"
        );
    }
    assert!(
        !command.contains("baseRepository"),
        "PR-ready helper must request only gh-supported fields; gh pr view does not support baseRepository"
    );
}

#[test]
fn workflow_complete_json_reports_terminal_outcome_instead_of_unconditional_merge_ready() {
    let recipe = load_finalize_recipe();
    let command = step_command(&recipe, "workflow-complete");

    assert!(
        command.contains("terminal_outcome"),
        "workflow-complete JSON must include the terminal outcome that made finalization succeed"
    );
    assert!(
        !command.contains("ready_to_merge: true"),
        "workflow-complete must not unconditionally claim merge readiness for no-diff or already-merged terminal states"
    );
}

#[test]
fn pr_ready_helper_fails_closed_when_pr_view_metadata_is_unavailable() {
    let tmp = TempDir::new().expect("tempdir");
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("create bin dir");
    let gh = bin_dir.join("gh");
    write_file(
        &gh,
        r#"#!/usr/bin/env bash
set -euo pipefail
if [ "${1:-}" = "auth" ] && [ "${2:-}" = "status" ]; then
  exit 0
fi
if [ "${1:-}" = "pr" ] && [ "${2:-}" = "view" ]; then
  echo "https://token@example.com/hidden failure" >&2
  exit 42
fi
echo "unexpected gh call: $*" >&2
exit 99
"#,
    );
    make_executable(&gh);

    let old_path = std::env::var("PATH").unwrap_or_default();
    let output = Command::new("bash")
        .arg(workspace_helper_path("workflow_pr_ready.sh"))
        .env("PATH", format!("{}:{old_path}", bin_dir.display()))
        .env("PR_URL", "https://github.com/owner/repo/pull/7")
        .output()
        .expect("run workflow_pr_ready.sh");

    assert!(
        !output.status.success(),
        "PR-ready helper must fail closed when explicit PR metadata cannot be inspected\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("current branch is empty") || stderr.contains("pr_metadata_unavailable"),
        "scoped validation must fail closed before ambiguous PR mutation, stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("https://token@example.com"),
        "scoped validation must not leak credential-bearing URLs, stderr:\n{stderr}"
    );
}

#[test]
fn pr_ready_helper_fails_closed_when_gh_auth_is_unavailable() {
    let tmp = TempDir::new().expect("tempdir");
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("create bin dir");
    let gh = bin_dir.join("gh");
    write_file(
        &gh,
        r#"#!/usr/bin/env bash
set -euo pipefail
if [ "${1:-}" = "auth" ] && [ "${2:-}" = "status" ]; then
  echo "not logged in" >&2
  exit 42
fi
echo "unexpected gh call: $*" >&2
exit 99
"#,
    );
    make_executable(&gh);

    let old_path = std::env::var("PATH").unwrap_or_default();
    let output = Command::new("bash")
        .arg(workspace_helper_path("workflow_pr_ready.sh"))
        .env("PATH", format!("{}:{old_path}", bin_dir.display()))
        .env("PR_URL", "https://github.com/owner/repo/pull/7")
        .output()
        .expect("run workflow_pr_ready.sh");

    assert!(
        !output.status.success(),
        "PR-ready helper must fail closed when gh auth is unavailable"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cannot verify or mutate GitHub PR readiness"),
        "auth failure must be surfaced as a hard blocker, stderr:\n{stderr}"
    );
}

#[test]
fn pr_ready_helper_fails_closed_when_branch_discovery_fails() {
    let tmp = TempDir::new().expect("tempdir");
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo).expect("create repo");
    for args in [
        vec!["init", "-b", "main"],
        vec!["config", "user.email", "test@example.com"],
        vec!["config", "user.name", "Workflow Test"],
    ] {
        let status = Command::new("git")
            .args(args)
            .current_dir(&repo)
            .status()
            .expect("git setup");
        assert!(status.success(), "git setup command failed");
    }
    write_file(&repo.join("README.md"), "base\n");
    for args in [
        vec!["add", "README.md"],
        vec!["commit", "-m", "base"],
        vec!["switch", "-c", "feature"],
    ] {
        let status = Command::new("git")
            .args(args)
            .current_dir(&repo)
            .status()
            .expect("git setup");
        assert!(status.success(), "git setup command failed");
    }

    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("create bin dir");
    let gh = bin_dir.join("gh");
    write_file(
        &gh,
        r#"#!/usr/bin/env bash
set -euo pipefail
if [ "${1:-}" = "auth" ] && [ "${2:-}" = "status" ]; then
  exit 0
fi
if [ "${1:-}" = "pr" ] && [ "${2:-}" = "list" ]; then
  echo "discovery unavailable" >&2
  exit 42
fi
echo "unexpected gh call: $*" >&2
exit 99
"#,
    );
    make_executable(&gh);

    let old_path = std::env::var("PATH").unwrap_or_default();
    let output = Command::new("bash")
        .arg(workspace_helper_path("workflow_pr_ready.sh"))
        .current_dir(&repo)
        .env("PATH", format!("{}:{old_path}", bin_dir.display()))
        .output()
        .expect("run workflow_pr_ready.sh");

    assert!(
        !output.status.success(),
        "PR-ready helper must fail closed when branch PR discovery is ambiguous"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unable to determine current GitHub repo identity")
            || stderr.contains("scoped PR validation failed"),
        "branch discovery failure must not become a no-PR success, stderr:\n{stderr}"
    );
}

#[test]
fn pr_ready_helper_validates_pr_identity_before_mutation() {
    let tmp = TempDir::new().expect("tempdir");
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo).expect("create repo");
    Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(&repo)
        .status()
        .expect("git init");
    Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(&repo)
        .status()
        .expect("git config email");
    Command::new("git")
        .args(["config", "user.name", "Workflow Test"])
        .current_dir(&repo)
        .status()
        .expect("git config name");
    write_file(&repo.join("README.md"), "base\n");
    Command::new("git")
        .args(["add", "README.md"])
        .current_dir(&repo)
        .status()
        .expect("git add");
    Command::new("git")
        .args(["commit", "-m", "base"])
        .current_dir(&repo)
        .status()
        .expect("git commit");
    Command::new("git")
        .args([
            "remote",
            "add",
            "origin",
            "https://x-access-token:secret@github.com/owner/repo.git",
        ])
        .current_dir(&repo)
        .status()
        .expect("git remote add");
    Command::new("git")
        .args(["switch", "-c", "feature"])
        .current_dir(&repo)
        .status()
        .expect("git switch");
    Command::new("git")
        .args(["update-ref", "refs/remotes/origin/main", "main"])
        .current_dir(&repo)
        .status()
        .expect("git update remote main");
    write_file(&repo.join("feature.txt"), "feature\n");
    Command::new("git")
        .args(["add", "feature.txt"])
        .current_dir(&repo)
        .status()
        .expect("git add feature");
    Command::new("git")
        .args(["commit", "-m", "feature"])
        .current_dir(&repo)
        .status()
        .expect("git commit feature");

    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("create bin dir");
    let gh_log = tmp.path().join("gh.log");
    let gh = bin_dir.join("gh");
    write_file(
        &gh,
        &format!(
            r#"#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> {gh_log:?}
if [ "${{1:-}}" = "auth" ] && [ "${{2:-}}" = "status" ]; then
  exit 0
fi
if [ "${{1:-}}" = "pr" ] && [ "${{2:-}}" = "view" ]; then
  cat <<'JSON'
{{"url":"https://github.com/owner/repo/pull/7","number":7,"state":"OPEN","isDraft":true,"mergedAt":"","headRefName":"feature","baseRefName":"main","headRefOid":"0000000000000000000000000000000000000000","headRepositoryOwner":{{"login":"owner"}},"headRepository":{{"name":"repo"}},"isCrossRepository":false}}
JSON
  exit 0
fi
if [ "${{1:-}}" = "pr" ] && {{ [ "${{2:-}}" = "ready" ] || [ "${{2:-}}" = "comment" ]; }}; then
  echo "mutation must not be reached" >&2
  exit 77
fi
echo "unexpected gh call: $*" >&2
exit 99
"#,
            gh_log = gh_log.display().to_string()
        ),
    );
    make_executable(&gh);

    let old_path = std::env::var("PATH").unwrap_or_default();
    let output = Command::new("bash")
        .arg(workspace_helper_path("workflow_pr_ready.sh"))
        .current_dir(&repo)
        .env("PATH", format!("{}:{old_path}", bin_dir.display()))
        .env("PR_URL", "https://github.com/owner/repo/pull/7")
        .output()
        .expect("run workflow_pr_ready.sh");

    assert!(
        !output.status.success(),
        "PR-ready helper must fail before mutation when PR identity is stale\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no_scoped_pr") || stderr.contains("headRefOid"),
        "identity failure must explain stale scoped PR metadata, stderr:\n{stderr}"
    );
    let log = fs::read_to_string(&gh_log).expect("read gh log");
    assert!(
        !log.contains("pr ready") && !log.contains("pr comment"),
        "PR mutation commands must not run after identity validation failure; gh log:\n{log}"
    );
}

#[test]
fn pr_ready_helper_fails_closed_when_ready_mutation_fails() {
    let tmp = TempDir::new().expect("tempdir");
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo).expect("create repo");
    for args in [
        vec!["init", "-b", "main"],
        vec!["config", "user.email", "test@example.com"],
        vec!["config", "user.name", "Workflow Test"],
    ] {
        let status = Command::new("git")
            .args(args)
            .current_dir(&repo)
            .status()
            .expect("git setup");
        assert!(status.success(), "git setup command failed");
    }
    write_file(&repo.join("README.md"), "base\n");
    for args in [
        vec!["add", "README.md"],
        vec!["commit", "-m", "base"],
        vec![
            "remote",
            "add",
            "origin",
            "https://x-access-token:secret@github.com/owner/repo.git",
        ],
        vec!["switch", "-c", "feature"],
    ] {
        let status = Command::new("git")
            .args(args)
            .current_dir(&repo)
            .status()
            .expect("git setup");
        assert!(status.success(), "git setup command failed");
    }
    let status = Command::new("git")
        .args(["update-ref", "refs/remotes/origin/main", "main"])
        .current_dir(&repo)
        .status()
        .expect("git update remote main");
    assert!(status.success(), "git update remote main failed");
    write_file(&repo.join("feature.txt"), "feature\n");
    for args in [vec!["add", "feature.txt"], vec!["commit", "-m", "feature"]] {
        let status = Command::new("git")
            .args(args)
            .current_dir(&repo)
            .status()
            .expect("git feature setup");
        assert!(status.success(), "git feature command failed");
    }

    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("create bin dir");
    let gh_log = tmp.path().join("gh-ready-fail.log");
    let gh = bin_dir.join("gh");
    write_file(
        &gh,
        &format!(
            r#"#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> {gh_log:?}
if [ "${{1:-}}" = "auth" ] && [ "${{2:-}}" = "status" ]; then
  exit 0
fi
if [ "${{1:-}}" = "pr" ] && [ "${{2:-}}" = "view" ]; then
  head_oid="$(git rev-parse HEAD)"
  cat <<JSON
{{"url":"https://github.com/owner/repo/pull/7","number":7,"state":"OPEN","isDraft":true,"mergedAt":"","headRefName":"feature","baseRefName":"main","headRefOid":"$head_oid","headRepositoryOwner":{{"login":"owner"}},"headRepository":{{"name":"repo"}},"isCrossRepository":false}}
JSON
  exit 0
fi
if [ "${{1:-}}" = "pr" ] && [ "${{2:-}}" = "ready" ]; then
  echo "ready mutation failed" >&2
  exit 42
fi
if [ "${{1:-}}" = "pr" ] && [ "${{2:-}}" = "comment" ]; then
  echo "comment must not be reached" >&2
  exit 77
fi
echo "unexpected gh call: $*" >&2
exit 99
"#,
            gh_log = gh_log.display().to_string()
        ),
    );
    make_executable(&gh);

    let old_path = std::env::var("PATH").unwrap_or_default();
    let output = Command::new("bash")
        .arg(workspace_helper_path("workflow_pr_ready.sh"))
        .current_dir(&repo)
        .env("PATH", format!("{}:{old_path}", bin_dir.display()))
        .env("PR_URL", "https://github.com/owner/repo/pull/7")
        .output()
        .expect("run workflow_pr_ready.sh");

    assert!(
        !output.status.success(),
        "PR-ready helper must fail closed when ready mutation fails\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("exit 42") && stderr.contains("refusing to report successful finalization"),
        "ready mutation failure must preserve exit status and be fatal, stderr:\n{stderr}"
    );
    let log = fs::read_to_string(&gh_log).expect("read gh log");
    assert!(log.contains("pr ready"), "test must reach ready mutation");
    assert!(
        !log.contains("pr comment"),
        "helper must not post a ready comment after ready mutation failure; gh log:\n{log}"
    );
}

#[test]
fn pr_ready_helper_fails_closed_when_base_branch_cannot_be_proven() {
    let tmp = TempDir::new().expect("tempdir");
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo).expect("create repo");
    for args in [
        vec!["init", "-b", "main"],
        vec!["config", "user.email", "test@example.com"],
        vec!["config", "user.name", "Workflow Test"],
    ] {
        let status = Command::new("git")
            .args(args)
            .current_dir(&repo)
            .status()
            .expect("git setup");
        assert!(status.success(), "git setup command failed");
    }
    write_file(&repo.join("README.md"), "base\n");
    for args in [
        vec!["add", "README.md"],
        vec!["commit", "-m", "base"],
        vec![
            "remote",
            "add",
            "origin",
            "https://github.com/owner/repo.git",
        ],
        vec!["switch", "-c", "feature"],
    ] {
        let status = Command::new("git")
            .args(args)
            .current_dir(&repo)
            .status()
            .expect("git setup");
        assert!(status.success(), "git setup command failed");
    }
    write_file(&repo.join("feature.txt"), "feature\n");
    for args in [vec!["add", "feature.txt"], vec!["commit", "-m", "feature"]] {
        let status = Command::new("git")
            .args(args)
            .current_dir(&repo)
            .status()
            .expect("git feature setup");
        assert!(status.success(), "git feature command failed");
    }

    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("create bin dir");
    let gh_log = tmp.path().join("gh-missing-base.log");
    let gh = bin_dir.join("gh");
    write_file(
        &gh,
        &format!(
            r#"#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> {gh_log:?}
if [ "${{1:-}}" = "auth" ] && [ "${{2:-}}" = "status" ]; then
  exit 0
fi
if [ "${{1:-}}" = "pr" ] && [ "${{2:-}}" = "view" ]; then
  head_oid="$(git rev-parse HEAD)"
  cat <<JSON
{{"url":"https://github.com/owner/repo/pull/7","number":7,"state":"OPEN","isDraft":true,"mergedAt":"","headRefName":"feature","baseRefName":"main","headRefOid":"$head_oid","headRepositoryOwner":{{"login":"owner"}},"headRepository":{{"name":"repo"}},"isCrossRepository":false}}
JSON
  exit 0
fi
if [ "${{1:-}}" = "pr" ] && [ "${{2:-}}" = "ready" ]; then
  echo "ready must not be reached" >&2
  exit 77
fi
echo "unexpected gh call: $*" >&2
exit 99
"#,
            gh_log = gh_log.display().to_string()
        ),
    );
    make_executable(&gh);

    let old_path = std::env::var("PATH").unwrap_or_default();
    let output = Command::new("bash")
        .arg(workspace_helper_path("workflow_pr_ready.sh"))
        .current_dir(&repo)
        .env("PATH", format!("{}:{old_path}", bin_dir.display()))
        .env("PR_URL", "https://github.com/owner/repo/pull/7")
        .output()
        .expect("run workflow_pr_ready.sh");

    assert!(
        !output.status.success(),
        "PR-ready helper must fail closed when base branch cannot be proven"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unable to resolve expected base branch"),
        "missing base proof must be surfaced before mutation, stderr:\n{stderr}"
    );
    let log = fs::read_to_string(&gh_log).expect("read gh log");
    assert!(
        !log.contains("pr ready"),
        "helper must not mutate when base branch cannot be proven; gh log:\n{log}"
    );
}

#[test]
fn final_status_retry_helper_preserves_failing_gh_exit_status() {
    let tmp = TempDir::new().expect("tempdir");
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("create bin dir");
    let gh = bin_dir.join("gh");
    write_file(
        &gh,
        r#"#!/usr/bin/env bash
set -euo pipefail
echo "https://token@example.com/final status failure" >&2
exit 42
"#,
    );
    make_executable(&gh);

    let old_path = std::env::var("PATH").unwrap_or_default();
    let output = Command::new("bash")
        .arg(workspace_helper_path("workflow_final_status.sh"))
        .env("PATH", format!("{}:{old_path}", bin_dir.display()))
        .env("REMOTE_HOST_TYPE", "github")
        .env("PR_URL", "https://github.com/owner/repo/pull/7")
        .env("TASK_DESCRIPTION", "test task")
        .env("ISSUE_NUMBER", "7")
        .output()
        .expect("run workflow_final_status.sh");

    assert!(
        !output.status.success(),
        "final status helper must fail closed when scoped PR validation cannot run"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("scoped final PR validation")
            || stderr.contains("lacks repo, branch, headRefOid, or baseRefName context"),
        "final-status helper must explain scoped validation failure, stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("exit 0"),
        "final-status retry helper must not convert failed gh calls into success, stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("https://token@example.com"),
        "final-status helper must not expose credential-bearing URLs, stderr:\n{stderr}"
    );
}

#[test]
fn final_status_does_not_confirm_no_diff_success_with_dirty_worktree() {
    let tmp = TempDir::new().expect("tempdir");
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo).expect("create repo");
    for args in [
        vec!["init", "-b", "main"],
        vec!["config", "user.email", "test@example.com"],
        vec!["config", "user.name", "Workflow Test"],
    ] {
        let status = Command::new("git")
            .args(args)
            .current_dir(&repo)
            .status()
            .expect("git setup");
        assert!(status.success(), "git setup command failed");
    }
    write_file(&repo.join("README.md"), "base\n");
    for args in [
        vec!["add", "README.md"],
        vec!["commit", "-m", "base"],
        vec![
            "remote",
            "add",
            "origin",
            "https://github.com/owner/repo.git",
        ],
        vec!["update-ref", "refs/remotes/origin/main", "main"],
    ] {
        let status = Command::new("git")
            .args(args)
            .current_dir(&repo)
            .status()
            .expect("git setup");
        assert!(status.success(), "git setup command failed");
    }
    write_file(&repo.join("dirty.txt"), "uncommitted\n");

    let output = Command::new("bash")
        .arg(workspace_helper_path("workflow_final_status.sh"))
        .current_dir(&repo)
        .env("REMOTE_HOST_TYPE", "other")
        .env("PR_PUBLISH_RESULT_STATE", "no-diff")
        .env("TASK_DESCRIPTION", "test task")
        .env("ISSUE_NUMBER", "7")
        .output()
        .expect("run workflow_final_status.sh");

    assert!(
        !output.status.success(),
        "final status helper must fail when dirty worktree prevents no-diff proof"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains("terminal_status=NO_DIFF_SUCCESS"),
        "dirty worktree must not be confirmed as NO_DIFF_SUCCESS\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("clean-worktree diff could not confirm"),
        "dirty no-diff claim must produce a visible error, stderr:\n{stderr}"
    );
}

#[test]
fn final_status_does_not_confirm_closed_obsolete_with_dirty_worktree() {
    let tmp = TempDir::new().expect("tempdir");
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo).expect("create repo");
    for args in [
        vec!["init", "-b", "main"],
        vec!["config", "user.email", "test@example.com"],
        vec!["config", "user.name", "Workflow Test"],
    ] {
        let status = Command::new("git")
            .args(args)
            .current_dir(&repo)
            .status()
            .expect("git setup");
        assert!(status.success(), "git setup command failed");
    }
    write_file(&repo.join("README.md"), "base\n");
    for args in [
        vec!["add", "README.md"],
        vec!["commit", "-m", "base"],
        vec![
            "remote",
            "add",
            "origin",
            "https://github.com/owner/repo.git",
        ],
        vec!["update-ref", "refs/remotes/origin/main", "main"],
    ] {
        let status = Command::new("git")
            .args(args)
            .current_dir(&repo)
            .status()
            .expect("git setup");
        assert!(status.success(), "git setup command failed");
    }
    write_file(&repo.join("dirty.txt"), "uncommitted\n");

    let output = Command::new("bash")
        .arg(workspace_helper_path("workflow_final_status.sh"))
        .current_dir(&repo)
        .env("REMOTE_HOST_TYPE", "other")
        .env("PR_PUBLISH_RESULT_STATE", "CLOSED_OBSOLETE")
        .env("TASK_DESCRIPTION", "test task")
        .env("ISSUE_NUMBER", "7")
        .output()
        .expect("run workflow_final_status.sh");

    assert!(
        !output.status.success(),
        "final status helper must fail when dirty worktree prevents obsolete proof"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains("All 23 workflow steps completed successfully"),
        "dirty obsolete state must not report successful completion\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("clean-worktree diff could not confirm"),
        "dirty obsolete claim must produce a visible error, stderr:\n{stderr}"
    );
}

#[test]
fn agentic_finalization_validate_fails_closed_for_malformed_output() {
    let output = run_agentic_finalization("validate", &[("AGENTIC_FINALIZER_OUTPUT", "not json")]);

    assert!(
        !output.status.success(),
        "malformed finalizer output must fail closed"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.contains("FAILED_FINALIZER_OUTPUT"),
        "failure JSON must preserve FAILED_FINALIZER_OUTPUT, stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("not a single JSON object"),
        "stderr must explain malformed finalizer output, stderr:\n{stderr}"
    );
}

#[test]
fn agentic_finalization_rejects_low_confidence_success() {
    let output = run_agentic_finalization(
        "validate",
        &[(
            "AGENTIC_FINALIZER_OUTPUT",
            r#"{"schema_version":1,"terminal_state":"IMPLEMENTED_VERIFIED","terminal_success":true,"confidence":"medium","reason":"implementation and verification evidence exists","required_next_action":"No action required.","hollow_success_detected":false,"evidence_used":["implementation_completed=true","verification_completed=true"]}"#,
        )],
    );

    assert!(
        !output.status.success(),
        "terminal_success=true with medium confidence must fail closed"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("terminal_success=true requires confidence=high"),
        "failure output must explain confidence invariant, stdout:\n{stdout}"
    );
}

#[test]
fn agentic_finalization_rejects_success_when_collected_evidence_is_dirty() {
    let output = run_agentic_finalization(
        "validate",
        &[
            (
                "AGENTIC_FINALIZER_OUTPUT",
                r#"{"schema_version":1,"terminal_state":"IMPLEMENTED_VERIFIED","terminal_success":true,"confidence":"high","reason":"implementation and verification evidence exists","required_next_action":"No action required.","hollow_success_detected":false,"evidence_used":["implementation_completed=true","verification_completed=true"]}"#,
            ),
            (
                "FINALIZATION_EVIDENCE",
                r#"{"git":{"dirty_worktree":"true"},"tooling":{"missing":"","gh_required":"false"},"prior_terminal_state":{"terminal_state":""}}"#,
            ),
        ],
    );

    assert!(
        !output.status.success(),
        "dirty collected evidence must override an optimistic agent success"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("FAILED_DIRTY_WORKTREE"),
        "dirty evidence must produce FAILED_DIRTY_WORKTREE, stdout:\n{stdout}"
    );
}

#[test]
fn agentic_finalization_rejects_success_when_required_github_tooling_is_missing() {
    let output = run_agentic_finalization(
        "validate",
        &[
            (
                "AGENTIC_FINALIZER_OUTPUT",
                r#"{"schema_version":1,"terminal_state":"FOLLOWUP_CREATED","terminal_success":true,"confidence":"high","reason":"PR publication evidence exists","required_next_action":"No action required.","hollow_success_detected":false,"evidence_used":["pr.present=true"]}"#,
            ),
            (
                "FINALIZATION_EVIDENCE",
                r#"{"git":{"dirty_worktree":"false"},"tooling":{"missing":"gh","gh_required":"true"},"prior_terminal_state":{"terminal_state":""}}"#,
            ),
        ],
    );

    assert!(
        !output.status.success(),
        "missing required gh tooling must override an optimistic agent success"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("FAILED_MISSING_TOOLING"),
        "missing gh evidence must produce FAILED_MISSING_TOOLING, stdout:\n{stdout}"
    );
}

#[test]
fn agentic_finalization_validate_and_complete_emit_canonical_success_json() {
    let tmp = TempDir::new().expect("tempdir");
    let repo_path = tmp.path().to_string_lossy().to_string();
    let validate = run_agentic_finalization(
        "validate",
        &[
            (
                "AGENTIC_FINALIZER_OUTPUT",
                r#"{"schema_version":1,"terminal_state":"IMPLEMENTED_VERIFIED","terminal_success":true,"confidence":"high","reason":"implementation and verification evidence exists","required_next_action":"No action required.","hollow_success_detected":false,"evidence_used":["implementation_completed=true","verification_completed=true"]}"#,
            ),
            ("IMPLEMENTATION_COMPLETED", "true"),
            ("VERIFICATION_COMPLETED", "true"),
            ("REPO_PATH", &repo_path),
        ],
    );

    assert!(
        validate.status.success(),
        "valid high-confidence implementation/verification success should pass\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&validate.stdout),
        String::from_utf8_lossy(&validate.stderr)
    );
    let workflow_result: JsonValue =
        serde_json::from_slice(&validate.stdout).expect("validate emits JSON");
    assert_eq!(workflow_result["terminal_state"], "IMPLEMENTED_VERIFIED");
    assert_eq!(workflow_result["terminal_success"], "true");
    assert_eq!(workflow_result["finalizer_output_valid"], "true");

    let complete = run_agentic_finalization(
        "complete",
        &[
            ("TASK_DESCRIPTION", "test task"),
            ("ISSUE_NUMBER", "7"),
            (
                "RECIPE_VAR_workflow_result__terminal_state",
                "IMPLEMENTED_VERIFIED",
            ),
            ("RECIPE_VAR_workflow_result__terminal_success", "true"),
            (
                "RECIPE_VAR_workflow_result__terminal_reason",
                "implementation and verification evidence exists",
            ),
            (
                "RECIPE_VAR_workflow_result__required_next_action",
                "No action required.",
            ),
            (
                "RECIPE_VAR_workflow_result__hollow_success_detected",
                "false",
            ),
            (
                "RECIPE_VAR_workflow_result__evidence_used",
                "implementation_completed=true,verification_completed=true",
            ),
            ("RECIPE_VAR_workflow_result__finalizer_schema_version", "1"),
            ("RECIPE_VAR_workflow_result__finalizer_confidence", "high"),
            ("RECIPE_VAR_workflow_result__finalizer_output_valid", "true"),
            ("RECIPE_VAR_workflow_result__terminal_failure", "false"),
        ],
    );

    assert!(
        complete.status.success(),
        "workflow-complete helper should emit canonical completion JSON\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&complete.stdout),
        String::from_utf8_lossy(&complete.stderr)
    );
    let completion: JsonValue = serde_json::from_slice(&complete.stdout).expect("complete JSON");
    assert_eq!(
        completion["workflow_result"]["terminal_state"],
        "IMPLEMENTED_VERIFIED"
    );
    assert_eq!(
        completion["workflow_result"]["finalizer_confidence"],
        "high"
    );
    assert_eq!(completion["terminal_vocabulary"][5], "IMPLEMENTED_VERIFIED");
}

#[test]
fn cleanup_push_logging_redacts_embedded_remote_credentials() {
    let command = step_command(&load_finalize_recipe(), "step-20b-push-cleanup");

    assert!(
        command.contains("redact_sensitive_output"),
        "cleanup push step must route remote output through a redaction boundary"
    );
    assert!(
        command.contains("https?://"),
        "cleanup redaction must cover both http:// and https:// credential-bearing URLs"
    );
    assert!(
        command.contains("pull_output_file")
            && command.contains("git --no-pager pull --rebase >\"$pull_output_file\" 2>&1"),
        "cleanup push step must capture git pull output before logging it"
    );
    assert!(
        !command.contains("cat \"$push_output_file\""),
        "cleanup push step must not print raw git push output"
    );
}
