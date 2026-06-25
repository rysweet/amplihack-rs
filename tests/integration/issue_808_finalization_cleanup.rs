//! Regression tests for issue #808 — default-workflow finalization cleanup.
//!
//! When a `default-workflow` run hits a denied force-push, its push-fallback
//! path could spray throwaway branches to the shared remote and leave nested
//! worktrees behind, with **no finalization cleanup** to remove them. A human
//! had to delete the stray remote branches and the leaked nested worktree by
//! hand.
//!
//! These tests pin the deterministic finalization cleanup contract implemented
//! in `amplifier-bundle/tools/workflow_runtime_artifacts.sh` and wired into
//! `amplifier-bundle/recipes/workflow-finalize.yaml`:
//!
//!   * On a denied force-push, the run-created fallback branch is deleted from
//!     the **shared remote** and locally, leaving only the intended PR branch.
//!   * Nested worktrees created under the task worktree are removed **and**
//!     their administrative registrations are pruned (no dangling worktree
//!     metadata — the #780/#755 regression), and their orphaned branch is
//!     deleted from the remote and locally too.
//!   * Cleanup is defensive (never aborts the caller) and idempotent.
//!
//! The tests drive the real bash helper against a real tempdir git repo with a
//! bare local origin — no network, no mocking of git.

#![allow(clippy::too_many_lines)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

fn workspace_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop(); // bins/amplihack -> bins
    p.pop(); // bins -> workspace root
    p
}

fn runtime_artifact_helper() -> PathBuf {
    workspace_root().join("amplifier-bundle/tools/workflow_runtime_artifacts.sh")
}

fn finalize_recipe() -> PathBuf {
    workspace_root().join("amplifier-bundle/recipes/workflow-finalize.yaml")
}

// ---------------------------------------------------------------------------
// Git helpers
// ---------------------------------------------------------------------------

fn git(cwd: &Path, args: &[&str]) -> Output {
    let out = Command::new("git")
        .args(["-c", "user.email=test@test", "-c", "user.name=test"])
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {args:?} failed:\nstdout:{}\nstderr:{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    out
}

fn git_stdout(cwd: &Path, args: &[&str]) -> String {
    String::from_utf8_lossy(&git(cwd, args).stdout).into_owned()
}

/// True if `branch` exists on the bare origin remote.
fn remote_branch_exists(repo: &Path, branch: &str) -> bool {
    let out = Command::new("git")
        .args(["ls-remote", "--heads", "origin", branch])
        .current_dir(repo)
        .output()
        .expect("spawn git ls-remote");
    !String::from_utf8_lossy(&out.stdout).trim().is_empty()
}

/// True if `branch` exists locally.
fn local_branch_exists(repo: &Path, branch: &str) -> bool {
    !git_stdout(repo, &["branch", "--list", branch])
        .trim()
        .is_empty()
}

/// All worktree paths currently registered for the repo.
fn registered_worktrees(repo: &Path) -> Vec<String> {
    git_stdout(repo, &["worktree", "list", "--porcelain"])
        .lines()
        .filter_map(|l| l.strip_prefix("worktree "))
        .map(str::to_owned)
        .collect()
}

/// A task-worktree fixture: a real repo with a bare origin, checked out on the
/// intended PR branch, mirroring the shape of a `default-workflow` task worktree.
struct TaskWorktree {
    _origin: TempDir,
    _repo: TempDir,
    repo: PathBuf,
    intended: String,
}

impl TaskWorktree {
    /// Build a repo on `intended` with `main` as the base, both pushed to a bare
    /// origin. `intended` is the branch checked out in the (task) worktree.
    fn new(intended: &str) -> Self {
        let origin = TempDir::new().expect("origin tempdir");
        let origin_path = origin.path().to_path_buf();
        Command::new("git")
            .args(["init", "--bare", "-b", "main"])
            .arg(&origin_path)
            .output()
            .expect("git init --bare");

        let repo_dir = TempDir::new().expect("repo tempdir");
        let repo = repo_dir.path().to_path_buf();
        git(&repo, &["init", "-b", "main"]);
        git(
            &repo,
            &["remote", "add", "origin", origin_path.to_str().unwrap()],
        );
        std::fs::write(repo.join("README.md"), "init\n").unwrap();
        git(&repo, &["add", "README.md"]);
        git(&repo, &["commit", "-m", "init"]);
        git(&repo, &["push", "-u", "origin", "HEAD:main"]);

        // Intended PR branch (the one finalization must keep).
        git(&repo, &["checkout", "-b", intended]);
        std::fs::write(repo.join("feature.txt"), "work\n").unwrap();
        git(&repo, &["add", "feature.txt"]);
        git(&repo, &["commit", "-m", "feature work"]);
        git(&repo, &["push", "-u", "origin", intended]);

        Self {
            _origin: origin,
            _repo: repo_dir,
            repo,
            intended: intended.to_owned(),
        }
    }

    /// Simulate a push-fallback that left a throwaway branch on the shared
    /// remote and locally (e.g. `<base>-rebased`).
    fn push_stray_fallback_branch(&self, branch: &str) {
        git(&self.repo, &["branch", branch, "main"]);
        git(&self.repo, &["push", "origin", branch]);
    }

    /// Simulate the leaked nested worktree under the task worktree, on its own
    /// run-created branch that was also pushed to the shared remote.
    fn add_nested_worktree(&self, rel_path: &str, branch: &str) {
        git(
            &self.repo,
            &["worktree", "add", rel_path, "-b", branch, "main"],
        );
        git(&self.repo, &["push", "origin", branch]);
    }

    fn repo_str(&self) -> &str {
        self.repo.to_str().unwrap()
    }
}

/// Run a bash snippet with the runtime-artifact helper sourced. Returns the
/// process exit status (cleanup must never abort: status is always success).
fn run_with_helper(snippet: &str) -> Output {
    let script = format!(
        "set -uo pipefail\nsource \"{}\"\n{}\n",
        runtime_artifact_helper().display(),
        snippet
    );
    Command::new("bash")
        .arg("-c")
        .arg(script)
        .output()
        .expect("spawn bash")
}

// ---------------------------------------------------------------------------
// Behavioural regression tests
// ---------------------------------------------------------------------------

#[test]
fn finalization_deletes_tracked_fallback_branch_from_shared_remote_and_locally() {
    let fx = TaskWorktree::new("feat/degradation-events");
    fx.push_stray_fallback_branch("feat/degradation-events-rebased");

    assert!(
        remote_branch_exists(&fx.repo, "feat/degradation-events-rebased"),
        "precondition: stray fallback branch is on the shared remote"
    );

    let out = run_with_helper(&format!(
        "record_run_created_branch \"{repo}\" \"feat/degradation-events-rebased\"\n\
         finalize_workflow_runtime_artifacts \"{repo}\" \"{intended}\"",
        repo = fx.repo_str(),
        intended = fx.intended,
    ));
    assert!(
        out.status.success(),
        "finalization cleanup must never abort the caller\nstderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert!(
        !remote_branch_exists(&fx.repo, "feat/degradation-events-rebased"),
        "stray fallback branch must be deleted from the shared remote"
    );
    assert!(
        !local_branch_exists(&fx.repo, "feat/degradation-events-rebased"),
        "stray fallback branch must be deleted locally"
    );
    // The intended PR branch and the base branch must survive, remote and local.
    assert!(
        remote_branch_exists(&fx.repo, &fx.intended) && local_branch_exists(&fx.repo, &fx.intended),
        "intended PR branch must be preserved"
    );
    assert!(
        remote_branch_exists(&fx.repo, "main") && local_branch_exists(&fx.repo, "main"),
        "protected base branch must be preserved"
    );
}

#[test]
fn finalization_removes_nested_worktree_with_no_dangling_registration_and_deletes_its_branch() {
    let fx = TaskWorktree::new("feat/degradation-events");
    fx.add_nested_worktree("worktrees/feat/issue-491-nested", "feat/issue-491-nested");

    let nested_dir = fx.repo.join("worktrees/feat/issue-491-nested");
    assert!(nested_dir.exists(), "precondition: nested worktree exists");
    assert!(
        registered_worktrees(&fx.repo)
            .iter()
            .any(|w| w.ends_with("worktrees/feat/issue-491-nested")),
        "precondition: nested worktree is registered"
    );
    assert!(
        remote_branch_exists(&fx.repo, "feat/issue-491-nested"),
        "precondition: nested branch pushed to shared remote"
    );

    let out = run_with_helper(&format!(
        "finalize_workflow_runtime_artifacts \"{repo}\" \"{intended}\"",
        repo = fx.repo_str(),
        intended = fx.intended,
    ));
    assert!(out.status.success(), "cleanup must never abort the caller");

    assert!(
        !nested_dir.exists(),
        "nested worktree directory must be removed"
    );
    assert!(
        !fx.repo.join("worktrees").exists(),
        "leaked worktrees/ directory must be removed"
    );
    // The #780/#755 regression: a bare `rm -rf` leaves a dangling registration.
    assert!(
        !registered_worktrees(&fx.repo)
            .iter()
            .any(|w| w.contains("issue-491-nested")),
        "nested worktree must be deregistered (no dangling worktree metadata)"
    );
    // The task worktree itself must remain registered.
    assert_eq!(
        registered_worktrees(&fx.repo).len(),
        1,
        "only the task worktree itself should remain registered"
    );
    // The orphaned nested branch must be deleted from the remote and locally.
    assert!(
        !remote_branch_exists(&fx.repo, "feat/issue-491-nested"),
        "leaked nested-worktree branch must be deleted from the shared remote"
    );
    assert!(
        !local_branch_exists(&fx.repo, "feat/issue-491-nested"),
        "leaked nested-worktree branch must be deleted locally"
    );
    assert!(
        local_branch_exists(&fx.repo, &fx.intended),
        "intended PR branch must be preserved"
    );
}

#[test]
fn finalization_never_deletes_the_intended_branch_even_if_recorded() {
    let fx = TaskWorktree::new("feat/degradation-events");

    // Even a buggy caller that records the intended branch must not lose it.
    let out = run_with_helper(&format!(
        "record_run_created_branch \"{repo}\" \"{intended}\"\n\
         record_run_created_branch \"{repo}\" \"main\"\n\
         finalize_workflow_runtime_artifacts \"{repo}\" \"{intended}\"",
        repo = fx.repo_str(),
        intended = fx.intended,
    ));
    assert!(out.status.success());

    assert!(
        remote_branch_exists(&fx.repo, &fx.intended) && local_branch_exists(&fx.repo, &fx.intended),
        "intended PR branch must never be deleted"
    );
    assert!(
        remote_branch_exists(&fx.repo, "main") && local_branch_exists(&fx.repo, "main"),
        "protected base branch must never be deleted"
    );
}

#[test]
fn finalization_is_idempotent_and_defensive_when_artifacts_are_already_gone() {
    let fx = TaskWorktree::new("feat/degradation-events");
    fx.push_stray_fallback_branch("feat/degradation-events-rebased");

    let first = run_with_helper(&format!(
        "record_run_created_branch \"{repo}\" \"feat/degradation-events-rebased\"\n\
         finalize_workflow_runtime_artifacts \"{repo}\" \"{intended}\"",
        repo = fx.repo_str(),
        intended = fx.intended,
    ));
    assert!(first.status.success());
    assert!(!remote_branch_exists(
        &fx.repo,
        "feat/degradation-events-rebased"
    ));

    // Second run over already-clean state must still succeed (idempotent) and
    // must not disturb the surviving branches.
    let second = run_with_helper(&format!(
        "finalize_workflow_runtime_artifacts \"{repo}\" \"{intended}\"",
        repo = fx.repo_str(),
        intended = fx.intended,
    ));
    assert!(
        second.status.success(),
        "a second finalization over clean state must be a no-op success"
    );
    assert!(remote_branch_exists(&fx.repo, &fx.intended));
    assert!(remote_branch_exists(&fx.repo, "main"));
}

#[test]
fn preflight_deregisters_nested_worktree_without_dangling_registration() {
    // The narrow preflight cleanup must remove a real nested git worktree AND
    // prune its registration (the #780/#755 regression a bare `rm -rf` re-leaks),
    // while preserving the task worktree itself.
    let fx = TaskWorktree::new("feat/degradation-events");
    fx.add_nested_worktree("worktrees/nested-agent", "nested-agent");

    let out = run_with_helper(&format!(
        "preflight_known_workflow_runtime_artifacts \"{repo}\"",
        repo = fx.repo_str(),
    ));
    assert!(
        out.status.success(),
        "preflight must succeed on an untracked nested worktree\nstderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !fx.repo.join("worktrees").exists(),
        "preflight must remove the nested worktree directory"
    );
    assert!(
        !registered_worktrees(&fx.repo)
            .iter()
            .any(|w| w.contains("nested-agent")),
        "preflight must leave no dangling nested-worktree registration"
    );
    assert_eq!(
        registered_worktrees(&fx.repo).len(),
        1,
        "the task worktree itself must survive preflight"
    );
}

// ---------------------------------------------------------------------------
// Recipe-wiring contract tests
// ---------------------------------------------------------------------------

fn finalize_recipe_text() -> String {
    std::fs::read_to_string(finalize_recipe()).expect("read workflow-finalize.yaml")
}

#[test]
fn finalize_recipe_wires_unconditional_finalization_cleanup_step() {
    let recipe: serde_yaml::Value =
        serde_yaml::from_str(&finalize_recipe_text()).expect("parse workflow-finalize.yaml");
    let steps = recipe
        .get("steps")
        .and_then(serde_yaml::Value::as_sequence)
        .expect("recipe has steps");

    let cleanup = steps
        .iter()
        .find(|s| {
            s.get("id").and_then(serde_yaml::Value::as_str) == Some("step-22c-finalization-cleanup")
        })
        .expect("workflow-finalize must declare step-22c-finalization-cleanup");

    assert!(
        cleanup.get("condition").is_none(),
        "finalization cleanup must run unconditionally (success AND non-success terminal states)"
    );

    let command = cleanup
        .get("command")
        .and_then(serde_yaml::Value::as_str)
        .expect("cleanup step is a bash command");
    for required in [
        "workflow_runtime_artifacts.sh",
        "finalize_workflow_runtime_artifacts",
        "trap '_run_finalization_cleanup' EXIT",
    ] {
        assert!(
            command.contains(required),
            "finalization cleanup step must use the deterministic, trap-guarded helper; missing `{required}`"
        );
    }
}

#[test]
fn finalize_recipe_helper_defines_deterministic_cleanup_contract() {
    let helper = std::fs::read_to_string(runtime_artifact_helper())
        .expect("read workflow_runtime_artifacts.sh");
    for required in [
        "record_run_created_branch",
        "cleanup_run_created_branches",
        "cleanup_nested_worktrees",
        "finalize_workflow_runtime_artifacts",
        "push origin --delete",
        "worktree prune",
    ] {
        assert!(
            helper.contains(required),
            "runtime-artifact helper must define the deterministic finalization cleanup contract; missing `{required}`"
        );
    }
}
