//! Issue #1707: run the issue-claim check's shell suites from `cargo test`.
//!
//! `amplifier-bundle/tools/workflow_issue_claim_check.sh` is tested by two
//! shell suites that drive the real helper against real git repositories and
//! a fake `gh`. CI runs them as separate Lint steps, so the workspace test
//! suite never exercised the helper. This test runs both suites and fails if
//! either reports a failed check.
//!
//! The suites read recipe YAML with `python3` and PyYAML. When the system
//! `python3` cannot import `yaml`, the test builds a temporary virtual
//! environment with PyYAML using `uv` and puts it first on `PATH`. If that
//! is not possible the test fails rather than skipping, so a missing
//! dependency cannot pass as coverage.

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("repo root")
        .to_path_buf()
}

fn command_succeeds(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Returns a directory to prepend to `PATH` so that `python3` can import
/// `yaml`, or `None` when the system `python3` already can.
///
/// The fallback is a virtual environment built by `uv` in the test's temp
/// directory. A real interpreter is needed on `PATH`: a `python3` script that
/// calls `uv run` would be found again by uv's own interpreter discovery, and
/// the suites point `HOME` at a fake directory, which would hide uv's cache.
fn pyyaml_path_dir(tmp: &Path) -> Option<PathBuf> {
    if command_succeeds("python3", &["-c", "import yaml"]) {
        return None;
    }
    let venv = tmp.join("pyyaml-venv");
    let venv_str = venv.to_str().expect("utf-8 temp path");
    let python = venv.join("bin/python3");
    let python_str = python.to_str().expect("utf-8 temp path");
    assert!(
        command_succeeds("uv", &["venv", "--quiet", "--python", "python3", venv_str])
            && command_succeeds(
                "uv",
                &[
                    "pip", "install", "--quiet", "--python", python_str, "pyyaml"
                ]
            )
            && command_succeeds(python_str, &["-c", "import yaml"]),
        "the claim-check suites need python3 with PyYAML: install python3-yaml or uv"
    );
    Some(venv.join("bin"))
}

fn run_suite(script: &str) {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let mut path = std::env::var_os("PATH").unwrap_or_default();
    if let Some(dir) = pyyaml_path_dir(tmp.path()) {
        let mut joined = dir.into_os_string();
        joined.push(":");
        joined.push(&path);
        path = joined;
    }

    let root = repo_root();
    let output = Command::new("bash")
        .arg(root.join(script))
        .current_dir(&root)
        .env("PATH", path)
        .env("TMPDIR", tmp.path())
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
        .env_remove("GH_REPO")
        .output()
        .expect("run shell suite");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let failed: Vec<&str> = stdout
        .lines()
        .filter(|line| line.trim_start().starts_with("FAIL"))
        .collect();
    assert!(
        output.status.success() && failed.is_empty(),
        "{script} failed (status {:?}); failing checks: {failed:#?}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        output.status.code()
    );
}

#[test]
fn issue_1361_duplicate_pr_claim_check_suite_passes() {
    run_suite("tests/issue_1361_duplicate_pr_claim_check.sh");
}

#[test]
fn issue_1448_review_is_not_a_duplicate_suite_passes() {
    run_suite("tests/issue_1448_review_is_not_a_duplicate.sh");
}
