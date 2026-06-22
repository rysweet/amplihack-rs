//! TDD contract tests for `amplihack orch helper workflow-log-inventory` (#799).
//!
//! The helper must be deterministic, read-only, metadata-only, and available in
//! both text and JSON forms for investigation workflows.

use serde_json::Value;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_amplihack")
}

fn write_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .unwrap_or_else(|e| panic!("create {}: {e}", parent.display()));
    }
    std::fs::write(path, content).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

fn run_inventory(root: &Path, format: Option<&str>) -> std::process::Output {
    let mut command = Command::new(bin());
    command
        .args(["orch", "helper", "workflow-log-inventory", "--root"])
        .arg(root)
        .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1");
    if let Some(format) = format {
        command.args(["--format", format]);
    }
    command.output().expect("run workflow-log-inventory")
}

#[test]
fn workflow_log_inventory_help_is_wired_under_orch_helper() {
    let output = Command::new(bin())
        .args(["orch", "helper", "workflow-log-inventory", "--help"])
        .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
        .output()
        .expect("run workflow-log-inventory --help");

    assert!(
        output.status.success(),
        "helper help should succeed; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--root"),
        "help must document --root; got:\n{stdout}"
    );
    assert!(
        stdout.contains("--format"),
        "help must document --format; got:\n{stdout}"
    );
}

#[test]
fn workflow_log_inventory_json_is_sorted_metadata_only_and_non_recursive() {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path();
    write_file(
        &root.join(".amplihack/workflows/z-last.log"),
        "SECRET_TOKEN=must-never-appear\n",
    );
    write_file(
        &root.join(".amplihack/recipes/a-first.log"),
        "another secret line\n",
    );
    write_file(
        &root.join("nested/random.log"),
        "not a deterministic workflow location\n",
    );
    write_file(&root.join("recipe-runner.log"), "runner secret\n");

    let output = run_inventory(root, Some("json"));
    assert!(
        output.status.success(),
        "json inventory should succeed; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "successful inventory should not warn; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    for forbidden in ["SECRET_TOKEN", "another secret line", "runner secret"] {
        assert!(
            !stdout.contains(forbidden),
            "inventory must emit metadata only, not log contents: {forbidden}"
        );
    }

    let json: Value = serde_json::from_str(&stdout).expect("valid inventory json");
    assert_eq!(
        json["root"].as_str(),
        root.canonicalize().unwrap().to_str(),
        "root must be canonicalized"
    );
    let artifacts = json["artifacts"].as_array().expect("artifacts array");
    let paths: Vec<&str> = artifacts
        .iter()
        .map(|artifact| artifact["path"].as_str().expect("relative path"))
        .collect();
    assert_eq!(
        paths,
        vec![
            ".amplihack/recipes/a-first.log",
            ".amplihack/workflows/z-last.log",
            "recipe-runner.log",
        ],
        "inventory must use deterministic known locations and stable path sorting"
    );

    for artifact in artifacts {
        assert!(
            artifact["kind"].is_string(),
            "kind must be present: {artifact}"
        );
        assert!(
            artifact["size_bytes"].as_u64().is_some(),
            "size_bytes must be numeric metadata: {artifact}"
        );
        assert!(
            artifact["modified_utc"]
                .as_str()
                .is_some_and(|value| value.ends_with('Z')),
            "modified_utc must be UTC RFC3339-ish metadata: {artifact}"
        );
    }
}

#[test]
fn workflow_log_inventory_text_reports_empty_inventory() {
    let tmp = TempDir::new().expect("tempdir");
    let output = run_inventory(tmp.path(), None);

    assert!(
        output.status.success(),
        "empty inventory should succeed; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("workflow-log-inventory root="),
        "text output must include canonical root header; got:\n{stdout}"
    );
    assert!(
        stdout.contains("found 0 log artifacts"),
        "text output must report zero artifacts deterministically; got:\n{stdout}"
    );
}

#[test]
fn workflow_log_inventory_rejects_invalid_root() {
    let tmp = TempDir::new().expect("tempdir");
    let missing = tmp.path().join("missing");
    let output = run_inventory(&missing, Some("json"));

    assert!(
        !output.status.success(),
        "invalid roots must fail instead of falling back silently"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("root") || stderr.contains("No such file") || stderr.contains("not found"),
        "invalid-root failure should explain the root problem; stderr={stderr}"
    );
}
