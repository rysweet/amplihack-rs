//! TDD regression test for `amplihack orch run <ws_file>` (refs #248).
//!
//! Verifies the new native subcommand that ports the legacy
//! `python3 multitask/orchestrator.py` invocation in
//! `amplifier-bundle/recipes/smart-orchestrator.yaml`.
//!
//! Contract:
//! - Subcommand `orch run` accepts a positional `<WS_FILE>` path argument.
//! - Empty workstreams JSON arrays are rejected before execution.
//! - `orch run --help` is visible (subcommand wired into clap).

use tempfile::TempDir;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_amplihack")
}

#[test]
fn orch_run_help_is_available() {
    let output = crate::isolated_amplihack::isolated(bin())
        .args(["orch", "run", "--help"])
        .output()
        .expect("failed to execute amplihack orch run --help");

    assert!(
        output.status.success(),
        "orch run --help should succeed; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.to_lowercase().contains("ws_file") || stdout.contains("WS_FILE"),
        "help should mention WS_FILE positional; got: {stdout}"
    );
}

#[test]
fn orch_run_empty_workstreams_is_rejected_before_execution() {
    let tmp = TempDir::new().expect("tempdir");
    let ws = tmp.path().join("ws.json");
    std::fs::write(&ws, "[]").expect("write ws.json");

    let output = crate::isolated_amplihack::isolated(bin())
        .args(["orch", "run"])
        .arg(&ws)
        .output()
        .expect("failed to execute amplihack orch run");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "orch run must reject an empty plan; stderr={stderr}"
    );
    assert!(
        stderr.contains("plan.workstreams must be a non-empty array"),
        "expected a clear empty-plan diagnostic; got: {stderr}"
    );
}

#[test]
fn orch_run_missing_file_fails() {
    let tmp = TempDir::new().expect("tempdir");
    let missing = tmp.path().join("does_not_exist.json");

    let output = crate::isolated_amplihack::isolated(bin())
        .args(["orch", "run"])
        .arg(&missing)
        .output()
        .expect("failed to execute amplihack orch run");

    assert!(
        !output.status.success(),
        "orch run with missing file should fail"
    );
}
