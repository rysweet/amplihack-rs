//! Integration tests: memory command behaviour without the kuzu-backend feature.
//!
//! These tests exercise the compiled `amplihack` binary (default feature set,
//! no kuzu / cmake dependency) to verify:
//!
//! 1. `amplihack memory --help` exits 0 regardless of feature flags.
//! 2. `amplihack memory tree` can run successfully against the sqlite backend.
//! 3. `amplihack memory tree --backend kuzu` exits non-zero and surfaces an
//!    actionable error message when the kuzu-backend feature is not enabled.
//!
//! Run these tests (default features, no cmake required):
//!   cargo test --test memory_no_kuzu_test
//!
//! NOTE: These tests skip themselves gracefully when the binary is not yet
//! compiled so that CI does not fail during `cargo test --no-run`.

use std::path::PathBuf;
use std::process::Command;

/// Return the path to the debug amplihack binary (the one under test).
fn amplihack_bin() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // CARGO_MANIFEST_DIR for this test file is the workspace root's `tests/`
    // folder; pop it to reach the workspace root.
    path.pop(); // tests/
    path.pop(); // workspace root
    path.push("target/debug/amplihack");
    path
}

/// Run a command, return (exit_success, stdout, stderr).
fn run(cmd: &mut Command) -> (bool, String, String) {
    let output = cmd
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn process: {e}"));
    let success = output.status.success();
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    (success, stdout, stderr)
}

/// Skip the test if the binary has not been built yet.
macro_rules! require_bin {
    ($bin:expr) => {
        if !$bin.exists() {
            eprintln!(
                "Skipping: binary not found at {:?} — run `cargo build` first",
                $bin
            );
            return;
        }
    };
}

// ---------------------------------------------------------------------------
// Help is always available
// ---------------------------------------------------------------------------

#[test]
fn memory_help_exits_zero_without_kuzu() {
    let bin = amplihack_bin();
    require_bin!(bin);
    let (ok, _out, err) = run(Command::new(&bin).args(["memory", "--help"]));
    assert!(
        ok,
        "memory --help should exit 0 without the kuzu-backend feature; stderr: {err}"
    );
}

#[test]
fn memory_tree_help_exits_zero_without_kuzu() {
    let bin = amplihack_bin();
    require_bin!(bin);
    let (ok, _out, err) = run(Command::new(&bin).args(["memory", "tree", "--help"]));
    assert!(
        ok,
        "memory tree --help should exit 0 without kuzu; stderr: {err}"
    );
}

#[test]
fn memory_export_help_exits_zero_without_kuzu() {
    let bin = amplihack_bin();
    require_bin!(bin);
    let (ok, _out, err) = run(Command::new(&bin).args(["memory", "export", "--help"]));
    assert!(
        ok,
        "memory export --help should exit 0 without kuzu; stderr: {err}"
    );
}

#[test]
fn memory_import_help_exits_zero_without_kuzu() {
    let bin = amplihack_bin();
    require_bin!(bin);
    let (ok, _out, err) = run(Command::new(&bin).args(["memory", "import", "--help"]));
    assert!(
        ok,
        "memory import --help should exit 0 without kuzu; stderr: {err}"
    );
}

// ---------------------------------------------------------------------------
// SQLite backend is always available
// ---------------------------------------------------------------------------

#[test]
fn memory_tree_sqlite_backend_exits_zero() {
    let bin = amplihack_bin();
    require_bin!(bin);
    // Use a throwaway HOME so the test does not touch the real ~/.amplihack.
    let tmp = tempfile::tempdir().expect("failed to create tempdir");
    let (ok, _out, err) = run(Command::new(&bin)
        .args(["memory", "tree", "--backend", "sqlite"])
        .env("HOME", tmp.path()));
    assert!(
        ok,
        "memory tree --backend sqlite should succeed without kuzu; stderr: {err}"
    );
}

// ---------------------------------------------------------------------------
// Kuzu backend: actionable error when feature is disabled
// ---------------------------------------------------------------------------

/// This test only runs when the binary was compiled WITHOUT the kuzu-backend
/// feature (the default `cargo install` path).  When kuzu-backend IS enabled
/// the test would need kuzu files on disk, so we skip it.
///
/// IMPORTANT: this test will FAIL before the feature-gating implementation is
/// correct — that is intentional TDD behaviour.
#[cfg(not(feature = "kuzu-backend"))]
#[test]
fn memory_tree_kuzu_backend_exits_nonzero_without_feature() {
    let bin = amplihack_bin();
    require_bin!(bin);
    let tmp = tempfile::tempdir().expect("failed to create tempdir");
    let (ok, _out, err) = run(Command::new(&bin)
        .args(["memory", "tree", "--backend", "kuzu"])
        .env("HOME", tmp.path()));
    assert!(
        !ok,
        "memory tree --backend kuzu must exit non-zero when kuzu-backend feature is disabled"
    );
    // The combined output must guide the user to reinstall with the feature.
    let combined = format!("{_out}{err}");
    assert!(
        combined.contains("--features kuzu-backend"),
        "output must contain '--features kuzu-backend' to guide the user, got:\n{combined}"
    );
}

#[cfg(not(feature = "kuzu-backend"))]
#[test]
fn memory_export_kuzu_format_exits_nonzero_without_feature() {
    let bin = amplihack_bin();
    require_bin!(bin);
    let tmp = tempfile::tempdir().expect("failed to create tempdir");
    let out_path = tmp.path().join("out.kuzu");
    let (ok, _out, err) = run(Command::new(&bin)
        .args([
            "memory",
            "export",
            "--agent",
            "test-agent",
            "--output",
            out_path.to_str().unwrap(),
            "--format",
            "kuzu",
        ])
        .env("HOME", tmp.path()));
    assert!(
        !ok,
        "memory export --format kuzu must fail without kuzu-backend feature"
    );
    let combined = format!("{_out}{err}");
    assert!(
        combined.contains("--features kuzu-backend"),
        "export error must mention '--features kuzu-backend', got:\n{combined}"
    );
}

#[cfg(not(feature = "kuzu-backend"))]
#[test]
fn memory_import_kuzu_format_exits_nonzero_without_feature() {
    let bin = amplihack_bin();
    require_bin!(bin);
    let tmp = tempfile::tempdir().expect("failed to create tempdir");
    let fake_input = tmp.path().join("data.kuzu");
    std::fs::write(&fake_input, b"").expect("write temp file");
    let (ok, _out, err) = run(Command::new(&bin)
        .args([
            "memory",
            "import",
            "--agent",
            "test-agent",
            "--input",
            fake_input.to_str().unwrap(),
            "--format",
            "kuzu",
        ])
        .env("HOME", tmp.path()));
    assert!(
        !ok,
        "memory import --format kuzu must fail without kuzu-backend feature"
    );
    let combined = format!("{_out}{err}");
    assert!(
        combined.contains("--features kuzu-backend"),
        "import error must mention '--features kuzu-backend', got:\n{combined}"
    );
}
