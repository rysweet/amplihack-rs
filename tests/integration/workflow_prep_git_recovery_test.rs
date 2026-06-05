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

#[test]
fn prepare_workspace_still_fails_for_non_git_paths() {
    let output = run_prepare_workspace(
        r#"#!/bin/sh
if [ "$1:$2" = "rev-parse:--is-inside-work-tree" ]; then
  echo "fatal: not a git repository" >&2
  exit 128
fi
echo "unexpected git invocation: $*" >&2
exit 2
"#,
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "hard repo validation must still fail for non-git paths"
    );
    assert!(
        stderr.contains("requires a git repo"),
        "non-git failure should stay explicit; stderr:\n{stderr}"
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
