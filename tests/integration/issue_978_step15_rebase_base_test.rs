//! tests/integration/issue_978_step15_rebase_base_test.rs
//!
//! Regression coverage for issue #978: default-workflow Step 15 must be
//! ancestry-aware. It must NOT blindly `git pull --rebase` a temporary
//! workstream branch onto its configured upstream — when that upstream has
//! diverged (unrelated/stale), rebasing replays already-integrated commits and
//! produces add/add conflicts. Step 15 must instead:
//!   1. fast-forward push when HEAD is strictly ahead of upstream (behind == 0),
//!      preserving the tested commit identities (no rebase / no history rewrite);
//!   2. fail closed with structured merge-base/ahead/behind evidence when the
//!      histories genuinely diverge (behind > 0), instead of silently rebasing.

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

fn step_command(recipe: &Value, id: &str) -> String {
    recipe
        .get("steps")
        .and_then(|s| s.as_sequence())
        .and_then(|steps| {
            steps
                .iter()
                .find(|step| step.get("id").and_then(|v| v.as_str()) == Some(id))
        })
        .and_then(|step| step.get("command"))
        .and_then(|c| c.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| panic!("step {id} must be a bash command step"))
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

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("run git {args:?} in {}: {e}", dir.display()));
    assert!(
        output.status.success(),
        "git {args:?} failed in {}\nstdout:\n{}\nstderr:\n{}",
        dir.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn git_stdout(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("run git {args:?} in {}: {e}", dir.display()));
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn run_step15(repo: &Path, command: &str) -> std::process::Output {
    Command::new("bash")
        .arg("-c")
        .arg(command)
        .current_dir(repo)
        .env("WORKTREE_SETUP_WORKTREE_PATH", repo)
        .env(
            "WORKFLOW_RUNTIME_ARTIFACT_HELPER",
            workspace_helper_path("workflow_runtime_artifacts.sh"),
        )
        .env("TASK_DESCRIPTION", "publish workstream branch (issue #978)")
        .env("ISSUE_NUMBER", "978")
        .env("REMOTE_HOST_TYPE", "github")
        .output()
        .expect("run step-15 commit-push command")
}

/// Build a clone whose `feature` branch is `ahead` of `origin/feature`. Returns
/// the working repo path (inside `tmp`).
fn init_repo_with_feature_branch(tmp: &Path) -> (PathBuf, PathBuf) {
    let origin = tmp.join("origin.git");
    let repo = tmp.join("repo");
    git(tmp, &["init", "--bare", origin.to_str().unwrap()]);
    git(
        tmp,
        &["clone", origin.to_str().unwrap(), repo.to_str().unwrap()],
    );
    git(&repo, &["switch", "-c", "main"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "Workflow Test"]);
    write_file(&repo.join("README.md"), "base\n");
    git(&repo, &["add", "README.md"]);
    git(&repo, &["commit", "-m", "base"]);
    git(&repo, &["push", "-u", "origin", "main"]);
    // Workstream branch, published so it has an upstream.
    git(&repo, &["switch", "-c", "feature"]);
    write_file(&repo.join("feature.txt"), "f1\n");
    git(&repo, &["add", "feature.txt"]);
    git(&repo, &["commit", "-m", "feature commit 1"]);
    git(&repo, &["push", "-u", "origin", "feature"]);
    (origin, repo)
}

#[test]
fn step15_fast_forwards_ahead_branch_without_rebase() {
    let recipe = load_publish_recipe();
    let command = step_command(&recipe, "step-15-commit-push");
    let tmp = TempDir::new().expect("tempdir");
    let (_origin, repo) = init_repo_with_feature_branch(tmp.path());

    // Local-only descendant commit: strictly ahead of origin/feature (behind == 0).
    write_file(&repo.join("feature.txt"), "f1\nf2\n");
    git(&repo, &["add", "feature.txt"]);
    git(&repo, &["commit", "-m", "feature commit 2"]);
    let head_before = git_stdout(&repo, &["rev-parse", "HEAD"]);

    let output = run_step15(&repo, &command);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "fast-forwardable branch must publish successfully\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains(r#""pushed":"true""#),
        "step-15 must push the fast-forward commit, stdout:\n{stdout}"
    );
    // Commit identity preserved (no rebase/rewrite): HEAD unchanged and remote matches.
    let head_after = git_stdout(&repo, &["rev-parse", "HEAD"]);
    assert_eq!(
        head_before, head_after,
        "step-15 must not rewrite commit identities during a fast-forward publish"
    );
    let remote_feature = git_stdout(&repo, &["rev-parse", "origin/feature"]);
    assert_eq!(
        head_after, remote_feature,
        "origin/feature must fast-forward to the local HEAD"
    );
    assert!(
        !stderr.contains("Rebasing"),
        "step-15 must not run a rebase for a fast-forward publish, stderr:\n{stderr}"
    );
}

#[test]
fn step15_fails_closed_on_divergent_upstream_instead_of_rebasing() {
    let recipe = load_publish_recipe();
    let command = step_command(&recipe, "step-15-commit-push");
    let tmp = TempDir::new().expect("tempdir");
    let (origin, repo) = init_repo_with_feature_branch(tmp.path());

    // Simulate an unrelated/diverged upstream: a second clone pushes a
    // *conflicting* commit onto origin/feature touching the same file.
    let other = tmp.path().join("other");
    git(
        tmp.path(),
        &["clone", origin.to_str().unwrap(), other.to_str().unwrap()],
    );
    git(&other, &["config", "user.email", "other@example.com"]);
    git(&other, &["config", "user.name", "Other Worker"]);
    git(&other, &["switch", "feature"]);
    write_file(&other.join("feature.txt"), "f1\nUPSTREAM-CONFLICT\n");
    git(&other, &["add", "feature.txt"]);
    git(&other, &["commit", "-m", "conflicting upstream commit"]);
    git(&other, &["push", "origin", "feature"]);

    // Meanwhile the workstream repo makes its own descendant commit: now ahead>0
    // AND behind>0 relative to the (diverged) upstream.
    write_file(&repo.join("feature.txt"), "f1\nLOCAL-WORK\n");
    git(&repo, &["add", "feature.txt"]);
    git(&repo, &["commit", "-m", "local workstream commit 2"]);
    let head_before = git_stdout(&repo, &["rev-parse", "HEAD"]);
    let file_before = fs::read_to_string(repo.join("feature.txt")).unwrap();

    let output = run_step15(&repo, &command);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "divergent upstream must fail closed, not rebase\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("issue #978") && stderr.contains("refuses to rebase divergent history"),
        "step-15 must fail with an explicit #978 divergence message, stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("ahead=") && stderr.contains("behind=") && stderr.contains("merge_base="),
        "step-15 must emit structured ancestry evidence, stderr:\n{stderr}"
    );
    assert!(
        stdout.contains(r#""reason":"diverged-upstream"#) || stdout.contains(r#""pushed":"false""#),
        "step-15 must not report a successful push on divergence, stdout:\n{stdout}"
    );
    // No history rewrite and no rebase conflict markers left behind.
    let head_after = git_stdout(&repo, &["rev-parse", "HEAD"]);
    assert_eq!(
        head_before, head_after,
        "step-15 must not rewrite HEAD when it refuses a divergent rebase"
    );
    let file_after = fs::read_to_string(repo.join("feature.txt")).unwrap();
    assert_eq!(
        file_before, file_after,
        "step-15 must leave the working tree untouched on a refused divergent rebase"
    );
    assert!(
        !file_after.contains("<<<<<<<") && !file_after.contains(">>>>>>>"),
        "step-15 must not leave add/add conflict markers, file:\n{file_after}"
    );
    assert!(
        !repo.join(".git/rebase-merge").exists() && !repo.join(".git/rebase-apply").exists(),
        "step-15 must not leave an in-progress rebase behind"
    );
}
