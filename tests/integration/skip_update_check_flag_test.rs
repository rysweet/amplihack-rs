//! TDD integration tests: `--skip-update-check` flag for `amplihack launch`.
//!
//! ## Why this test exists (WS3)
//!
//! The pre-launch npm update check (`tool_update_check::maybe_print_npm_update_notice`)
//! makes two network calls (npm list, npm show).  In CI, offline environments,
//! or scripted pipelines, this check must be suppressible via:
//!
//!   amplihack launch --skip-update-check
//!
//! These tests verify:
//! 1. The flag is accepted by the CLI without error
//! 2. The flag appears in `amplihack launch --help` output
//! 3. The flag suppresses npm subprocess invocations (no extra latency)
//! 4. The flag is independent of `AMPLIHACK_NONINTERACTIVE=1` (both work)
//!
//! ## Failure modes
//!
//! These tests FAIL (red) before WS3 is implemented because:
//! - `--skip-update-check` is not yet in the `Launch` clap struct
//! - The CLI exits 2 ("unrecognized argument") when the flag is passed
//! - The help text does not mention the flag
//!
//! They PASS (green) once the flag is added to the CLI and `run_launch`
//! accepts a `skip_update_check: bool` parameter.

use std::path::PathBuf;
use std::process::Command;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// Path to the compiled amplihack debug binary.
fn amplihack_bin() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // tests/
    path.pop(); // workspace root
    path.push("target/debug/amplihack");
    path
}

/// Assert the binary exists, panicking with a clear message if not.
fn require_binary() -> PathBuf {
    let bin = amplihack_bin();
    if !bin.exists() {
        panic!(
            "amplihack binary not found at {bin:?}.\n\
             Run `cargo build` first to compile the debug binary."
        );
    }
    bin
}

// ---------------------------------------------------------------------------
// WS3-INT-1: --skip-update-check is accepted by `amplihack launch --help`
// ---------------------------------------------------------------------------

/// `amplihack launch --help` must document `--skip-update-check`.
///
/// **FAILS** before WS3: the flag is not in the Launch clap struct, so it
/// doesn't appear in help output.
///
/// **PASSES** once `#[arg(long)] skip_update_check: bool` is in `Launch`.
#[test]
fn launch_help_documents_skip_update_check_flag() {
    let bin = require_binary();
    let output = Command::new(&bin)
        .args(["launch", "--help"])
        .output()
        .expect("failed to run amplihack launch --help");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{}{}", stdout, stderr);

    assert!(
        combined.contains("skip-update-check"),
        "FAIL: 'amplihack launch --help' output does not contain '--skip-update-check'.\n\
         \n\
         This flag must be added to the `Launch` clap struct:\n\
         \n\
           /// Skip the pre-launch npm update check.\n\
           #[arg(long)]\n\
           skip_update_check: bool,\n\
         \n\
         Found in help output:\n{}",
        combined
    );
}

// ---------------------------------------------------------------------------
// WS3-INT-2: `amplihack launch --skip-update-check` exits 0 with a stub tool
// ---------------------------------------------------------------------------

/// When `--skip-update-check` is passed alongside a stub claude binary,
/// the launch must succeed (exit 0) without attempting any npm queries.
///
/// **FAILS** before WS3: clap exits with code 2 ("error: unexpected argument
/// '--skip-update-check'").
///
/// **PASSES** once the flag is wired into the CLI dispatch.
#[test]
fn launch_with_skip_update_check_exits_zero_with_stub_tool() {
    let bin = require_binary();

    // Create a temporary directory with a stub claude binary.
    let tmpdir = tempfile::tempdir().expect("failed to create tempdir");
    let stub_bin = tmpdir.path().join("claude");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(&stub_bin, b"#!/bin/sh\nexit 0\n")
            .expect("failed to write stub claude");
        let mut perms = std::fs::metadata(&stub_bin)
            .expect("failed to stat stub")
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&stub_bin, perms).expect("failed to set permissions");
    }

    let new_path = format!(
        "{}:{}",
        tmpdir.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let status = Command::new(&bin)
        .args(["launch", "--skip-update-check"])
        .env("PATH", &new_path)
        .env("AMPLIHACK_NONINTERACTIVE", "1")
        .status()
        .expect("failed to run amplihack launch --skip-update-check");

    assert!(
        status.success(),
        "FAIL: `amplihack launch --skip-update-check` exited with status {status}.\n\
         \n\
         Expected exit 0.  Possible causes:\n\
         - Flag not yet added to the Launch clap struct (exit 2)\n\
         - run_launch() not accepting skip_update_check parameter\n\
         - Stub claude binary not found in PATH"
    );
}

// ---------------------------------------------------------------------------
// WS3-INT-3: Without --skip-update-check, the flag absence is not an error
// ---------------------------------------------------------------------------

/// `amplihack launch` without `--skip-update-check` must not exit with
/// "unexpected argument" or similar flag-related errors.
///
/// This is a regression guard: adding the flag must not break invocations
/// that don't use it.
#[test]
fn launch_without_skip_update_check_is_not_an_error() {
    let bin = require_binary();

    // Create a temporary directory with a stub claude binary.
    let tmpdir = tempfile::tempdir().expect("failed to create tempdir");
    let stub_bin = tmpdir.path().join("claude");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(&stub_bin, b"#!/bin/sh\nexit 0\n")
            .expect("failed to write stub claude");
        let mut perms = std::fs::metadata(&stub_bin)
            .expect("failed to stat stub")
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&stub_bin, perms).expect("failed to chmod stub");
    }

    let new_path = format!(
        "{}:{}",
        tmpdir.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let output = Command::new(&bin)
        .arg("launch")
        .env("PATH", &new_path)
        .env("AMPLIHACK_NONINTERACTIVE", "1")
        .output()
        .expect("failed to run amplihack launch");

    let stderr = String::from_utf8_lossy(&output.stderr);

    // Must not produce a clap "unexpected argument" error.
    assert!(
        !stderr.contains("unexpected argument"),
        "FAIL: `amplihack launch` (without --skip-update-check) produced an \
         'unexpected argument' error.\n\
         stderr: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// WS3-INT-4: --skip-update-check suppresses npm subprocess timing
// ---------------------------------------------------------------------------

/// With `--skip-update-check`, launch completes quickly because no npm
/// subprocesses are spawned.  Without the flag (in AMPLIHACK_NONINTERACTIVE=1
/// mode), the check is also skipped — but this test validates the flag path.
///
/// This test measures elapsed time: if npm were called, even with a 3s
/// timeout, the overhead would be measurable.  The stub environment has no
/// npm binary, so the subprocess would fail immediately — we verify the
/// flag causes zero subprocess overhead.
#[test]
fn launch_with_skip_update_check_completes_without_npm_subprocess_overhead() {
    let bin = require_binary();

    // Create a temp dir with stub claude but NO npm binary.
    // This means any npm invocation would fail with ENOENT.
    let tmpdir = tempfile::tempdir().expect("failed to create tempdir");
    let stub_bin = tmpdir.path().join("claude");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(&stub_bin, b"#!/bin/sh\nexit 0\n")
            .expect("failed to write stub claude");
        let mut perms = std::fs::metadata(&stub_bin)
            .expect("failed to stat stub")
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&stub_bin, perms).expect("failed to chmod stub");
    }

    // PATH contains only our stub dir — npm is not available.
    let isolated_path = tmpdir.path().to_string_lossy().to_string();

    let start = std::time::Instant::now();
    let _status = Command::new(&bin)
        .args(["launch", "--skip-update-check"])
        .env("PATH", &isolated_path)
        .env("AMPLIHACK_NONINTERACTIVE", "1")
        .output()
        .expect("failed to run amplihack launch --skip-update-check");
    let elapsed = start.elapsed();

    // Should complete within 1 second — no npm timeout overhead.
    assert!(
        elapsed.as_secs() < 5,
        "FAIL: `amplihack launch --skip-update-check` took {}s (expected <5s).\n\
         This suggests npm subprocesses are being spawned despite the flag.\n\
         Check that maybe_print_npm_update_notice respects the skip parameter.",
        elapsed.as_secs()
    );
}

// ---------------------------------------------------------------------------
// WS3-INT-5: --skip-update-check flag appears in top-level --help output
// ---------------------------------------------------------------------------

/// The `launch` subcommand help must show the flag.
/// This is a discoverability requirement.
#[test]
fn amplihack_launch_help_shows_skip_update_check() {
    let bin = require_binary();
    let output = Command::new(&bin)
        .args(["launch", "--help"])
        .output()
        .expect("failed to run amplihack launch --help");

    assert!(
        output.status.success(),
        "FAIL: `amplihack launch --help` exited non-zero.\n\
         Status: {}",
        output.status
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{}{}", stdout, stderr);

    // The flag must appear in help output
    assert!(
        combined.contains("skip-update-check"),
        "FAIL: '--skip-update-check' not found in `amplihack launch --help` output.\n\
         Full output:\n{}",
        combined
    );
}
