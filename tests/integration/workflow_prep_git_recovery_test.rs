//! Issue #582: workspace preparation must distinguish hard repository
//! validation from recoverable git probes.

use serde::Deserialize;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

#[derive(Debug, Deserialize)]
struct Recipe {
    #[serde(default)]
    steps: Vec<Step>,
}

#[derive(Debug, Deserialize)]
struct Step {
    id: String,
    #[serde(default)]
    command: Option<String>,
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn recipe_path(name: &str) -> PathBuf {
    workspace_root()
        .join("amplifier-bundle")
        .join("recipes")
        .join(format!("{name}.yaml"))
}

fn step_command(recipe: &str, step_id: &str) -> String {
    let path = recipe_path(recipe);
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let parsed: Recipe =
        serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));
    parsed
        .steps
        .into_iter()
        .find(|step| step.id == step_id)
        .unwrap_or_else(|| panic!("{recipe}.yaml missing step {step_id}"))
        .command
        .unwrap_or_else(|| panic!("{recipe}.yaml step {step_id} must have command"))
}

fn write_executable(path: &Path, content: &str) {
    let mut file = std::fs::File::create(path).expect("create executable");
    file.write_all(content.as_bytes())
        .expect("write executable");
    drop(file);
    let mut perms = std::fs::metadata(path).expect("metadata").permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).expect("chmod executable");
}

fn run_prepare_workspace(fake_git: &str) -> Output {
    let temp = tempfile::tempdir().expect("tempdir");
    let bin_dir = temp.path().join("bin");
    std::fs::create_dir_all(&bin_dir).expect("mkdir bin");
    write_executable(&bin_dir.join("git"), fake_git);

    let path = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let command = step_command("workflow-prep", "step-01-prepare-workspace");

    Command::new("bash")
        .arg("-c")
        .arg(command)
        .env_clear()
        .env("PATH", path)
        .env("REPO_PATH", temp.path())
        .env("TASK_DESCRIPTION", "issue #582 regression")
        .env("SKIP_PRE_AGENT_VALIDATION", "true")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run workflow-prep step")
}

#[test]
fn prepare_workspace_continues_when_git_status_exits_128_after_repo_validation() {
    let output = run_prepare_workspace(
        r#"#!/bin/sh
case "$1:$2" in
  rev-parse:--is-inside-work-tree) exit 0 ;;
  status:*) echo "fatal: index file corrupt enough to make status fail" >&2; exit 128 ;;
  fetch:*) exit 0 ;;
  branch:--show-current) echo "main"; exit 0 ;;
  remote:get-url) echo "https://github.com/rysweet/eatme"; exit 0 ;;
  *) echo "unexpected git invocation: $*" >&2; exit 2 ;;
esac
"#,
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "valid repos must continue when only `git status` fails with exit 128 after rev-parse succeeds; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("git status failed") && stderr.contains("continuing"),
        "recoverable status failure must emit clear degraded-diagnostics warning; stderr:\n{stderr}"
    );
}

/// Run step-01 in a real directory using the real `git` binary, with the
/// given extra env vars applied. Used to exercise the issue #900 auto-init
/// path where an actual repository must be created on disk.
fn run_prepare_workspace_real_git(repo_dir: &Path, extra_env: &[(&str, &str)]) -> Output {
    let command = step_command("workflow-prep", "step-01-prepare-workspace");
    let mut cmd = Command::new("bash");
    cmd.arg("-c")
        .arg(command)
        .env("REPO_PATH", repo_dir)
        .env("TASK_DESCRIPTION", "issue #900 regression")
        .env("SKIP_PRE_AGENT_VALIDATION", "true")
        // Keep git deterministic regardless of the host user's config.
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in extra_env {
        cmd.env(key, value);
    }
    cmd.output().expect("run workflow-prep step")
}

#[test]
fn prepare_workspace_auto_inits_repo_in_non_git_dir() {
    let temp = tempfile::tempdir().expect("tempdir");
    let output = run_prepare_workspace_real_git(temp.path(), &[]);

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "issue #900: step-01 must auto-init a repo in a non-git dir instead of failing; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("[init] no git repo found — initialized a new one for repo-creation task"),
        "auto-init must emit the informational [init] line; stdout:\n{stdout}"
    );
    assert!(
        temp.path().join(".git").exists(),
        "auto-init must create a real .git repository in REPO_PATH"
    );
    // Performance: a freshly auto-initialized repo has no remotes, so the
    // network `git fetch --all` must be skipped instead of spawned pointlessly.
    assert!(
        stdout.contains("No remotes configured — skipping fetch"),
        "auto-init path must skip the network fetch when no remotes exist; stdout:\n{stdout}"
    );

    // The new repository should be on the `main` branch.
    let head = Command::new("git")
        .args(["-C"])
        .arg(temp.path())
        .args(["symbolic-ref", "--short", "HEAD"])
        .output()
        .expect("read HEAD");
    let branch = String::from_utf8_lossy(&head.stdout);
    assert_eq!(
        branch.trim(),
        "main",
        "auto-init must default the initial branch to main; got '{}'",
        branch.trim()
    );
}

#[test]
fn prepare_workspace_still_fails_for_non_git_paths_when_auto_init_disabled() {
    let temp = tempfile::tempdir().expect("tempdir");
    let output = run_prepare_workspace_real_git(temp.path(), &[("AUTO_INIT_REPO", "false")]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "AUTO_INIT_REPO=false must preserve the hard non-git guard"
    );
    assert!(
        stderr.contains("requires a git repo"),
        "disabled auto-init failure should stay explicit; stderr:\n{stderr}"
    );
    assert!(
        !temp.path().join(".git").exists(),
        "disabled auto-init must not create a repository"
    );
}

#[test]
fn prepare_workspace_succeeds_in_existing_checkout() {
    let temp = tempfile::tempdir().expect("tempdir");
    // Create a real existing checkout.
    let init = Command::new("git")
        .arg("-C")
        .arg(temp.path())
        .args(["init", "-b", "main"])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("git init");
    assert!(init.status.success(), "test setup: git init must succeed");

    let output = run_prepare_workspace_real_git(temp.path(), &[]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "existing checkout must still succeed; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        !stdout.contains("[init] no git repo found"),
        "existing checkout must not trigger auto-init; stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("--- Git Status ---") && stdout.contains("--- Current Branch ---"),
        "existing checkout must still emit full status/branch diagnostics; stdout:\n{stdout}"
    );
}

#[test]
fn worktree_setup_treats_fetch_failure_as_recoverable_after_repo_validation() {
    let command = step_command("workflow-worktree", "step-04-setup-worktree");
    assert!(
        command.contains("git rev-parse --is-inside-work-tree"),
        "worktree setup must keep hard repository validation"
    );
    assert!(
        command.contains("WARNING: git fetch origin failed")
            && command.contains("continuing with local")
            && !command.contains("ERROR: git fetch origin failed after"),
        "after repository validation, fetch failures should warn and continue with local refs instead of aborting issue #582/eatme-style repos"
    );
}
