//! Issue #582: workspace preparation must distinguish hard repository
//! validation from recoverable git probes.
//!
//! Issue #900: `step-01-prepare-workspace` must NOT hard-fail on non-git
//! workspaces. When `REPO_PATH` is not inside a git work tree it must
//! initialize a fresh repo (`git init -b main`), print a clear informational
//! line, and CONTINUE — so repo-creation tasks can proceed. The auto-init is
//! gated by `AUTO_INIT_REPO` (default enabled); `AUTO_INIT_REPO=false`
//! restores the historical hard-fail for locked-down contexts.

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

/// Run step-01 with a *fake* `git` on PATH, driven by the provided shell
/// script. Used to model recoverable diagnostic failures deterministically.
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

/// Run step-01 with the *real* `git` in `repo_path`. `auto_init` controls the
/// `AUTO_INIT_REPO` env var (`None` leaves it unset to exercise the default).
fn run_prepare_workspace_real_git(repo_path: &Path, auto_init: Option<&str>) -> Output {
    let command = step_command("workflow-prep", "step-01-prepare-workspace");

    let mut cmd = Command::new("bash");
    cmd.arg("-c")
        .arg(command)
        .env("REPO_PATH", repo_path)
        .env("TASK_DESCRIPTION", "issue #900 repo-creation task")
        .env("SKIP_PRE_AGENT_VALIDATION", "true")
        // Keep git deterministic/offline regardless of host config.
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    match auto_init {
        Some(value) => {
            cmd.env("AUTO_INIT_REPO", value);
        }
        None => {
            cmd.env_remove("AUTO_INIT_REPO");
        }
    }

    cmd.output().expect("run workflow-prep step (real git)")
}

/// Create a real git checkout on `main` with one commit, to model an existing
/// repository for the "preserve existing behavior" contract.
fn init_real_checkout(dir: &Path) {
    let ok = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("run git setup");
        assert!(status.success(), "git {args:?} failed setting up checkout");
    };
    ok(&["init", "-b", "main"]);
    ok(&["config", "user.email", "test@example.com"]);
    ok(&["config", "user.name", "Test"]);
    ok(&["commit", "--allow-empty", "-m", "seed"]);
}

fn current_branch(dir: &Path) -> String {
    // Use symbolic-ref so this resolves correctly on an *unborn* branch
    // (freshly `git init`ed repo with no commits yet), where
    // `rev-parse --abbrev-ref HEAD` would report "HEAD".
    let out = Command::new("git")
        .args(["symbolic-ref", "--short", "HEAD"])
        .current_dir(dir)
        .output()
        .expect("git symbolic-ref");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

// ─────────────────────────────────────────────────────────────────────────
// Issue #900: auto-init contract
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn prepare_workspace_auto_inits_repo_in_non_git_dir() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path();

    let output = run_prepare_workspace_real_git(repo, None); // default enabled

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "non-git workspace must auto-init and continue (issue #900); stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        repo.join(".git").exists(),
        "step-01 must initialize a real .git directory in a non-git workspace; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(
        current_branch(repo),
        "main",
        "auto-initialized repo must be on the `main` branch"
    );
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("[init]") && combined.contains("no git repo found"),
        "auto-init must print a clear informational line about initializing a new repo; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

#[test]
fn prepare_workspace_hard_fails_when_auto_init_disabled() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path();

    let output = run_prepare_workspace_real_git(repo, Some("false"));

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "AUTO_INIT_REPO=false must restore the historical hard-fail for non-git paths; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        !repo.join(".git").exists(),
        "disabled auto-init must NOT create a .git directory; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("requires a git repo"),
        "disabled auto-init failure should stay explicit; stderr:\n{stderr}"
    );
}

#[test]
fn prepare_workspace_preserves_existing_checkout_diagnostics() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path();
    init_real_checkout(repo);

    let output = run_prepare_workspace_real_git(repo, None);

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "existing checkout must continue to succeed; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let combined = format!("{stdout}{stderr}");
    assert!(
        !combined.contains("[init]"),
        "existing checkout must NOT trigger the auto-init path; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("--- Git Status ---") && stdout.contains("--- Current Branch ---"),
        "existing checkout must preserve the status/branch diagnostics; stdout:\n{stdout}"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// Issue #582: recoverable-diagnostic contract (unchanged)
// ─────────────────────────────────────────────────────────────────────────

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
