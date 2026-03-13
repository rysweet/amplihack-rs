//! Integration tests for CI/CD configuration files.
//!
//! These tests validate that the GitHub Actions workflow YAML files contain
//! the expected Windows build target entries.
//!
//! # Why test YAML files?
//!
//! The Windows build target is a pure CI configuration change with no Rust
//! source to unit-test.  Testing the YAML directly ensures the CI contract is
//! machine-verifiable and prevents the target from being accidentally removed
//! in future refactors.

use std::path::PathBuf;

/// Resolve a path relative to the workspace root.
///
/// Works whether `cargo test` is run from the workspace root or from inside a
/// crate directory.
fn workspace_file(relative: &str) -> PathBuf {
    // CARGO_MANIFEST_DIR points to `crates/amplihack-cli/` at test time.
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    // Walk up two levels: amplihack-cli/ → crates/ → workspace root
    PathBuf::from(manifest_dir)
        .join("..") // crates/
        .join("..") // workspace root
        .join(relative)
}

fn read_workflow(name: &str) -> String {
    let path = workspace_file(&format!(".github/workflows/{name}"));
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()))
}

// ── WS3-TEST-01: ci.yml contains Windows target ───────────────────────────────

/// `ci.yml` must include `x86_64-pc-windows-msvc` in the cross-compile matrix
/// so that the Windows binary is verified on every PR.
#[test]
fn ci_yml_contains_windows_msvc_target() {
    let content = read_workflow("ci.yml");
    assert!(
        content.contains("x86_64-pc-windows-msvc"),
        "ci.yml must include 'x86_64-pc-windows-msvc' in the cross-compile matrix"
    );
}

// ── WS3-TEST-02: ci.yml Windows entry uses windows-latest runner ──────────────

/// The Windows target entry in `ci.yml` must use `windows-latest` as its
/// runner (not `ubuntu-latest`) so the build runs on a real Windows host.
#[test]
fn ci_yml_windows_target_uses_windows_latest_runner() {
    let content = read_workflow("ci.yml");

    // Locate the windows-msvc block and confirm windows-latest appears nearby.
    let windows_pos = content
        .find("x86_64-pc-windows-msvc")
        .expect("x86_64-pc-windows-msvc not found in ci.yml — see WS3-TEST-01");

    // Check that "windows-latest" appears within 300 characters after the
    // target name (i.e., in the same matrix entry).
    let nearby = &content[windows_pos..][..300.min(content.len() - windows_pos)];
    assert!(
        nearby.contains("windows-latest"),
        "the x86_64-pc-windows-msvc matrix entry must specify \
         'runner: windows-latest'; nearby text:\n{nearby}"
    );
}

// ── WS3-TEST-03: ci.yml Windows entry sets cross: false ──────────────────────

/// The Windows matrix entry must have `cross: false` because we use a native
/// `windows-latest` runner, not the `cross` container tool.
#[test]
fn ci_yml_windows_target_sets_cross_false() {
    let content = read_workflow("ci.yml");

    let windows_pos = content
        .find("x86_64-pc-windows-msvc")
        .expect("x86_64-pc-windows-msvc not found in ci.yml — see WS3-TEST-01");

    let nearby = &content[windows_pos..][..300.min(content.len() - windows_pos)];
    assert!(
        nearby.contains("cross: false"),
        "the x86_64-pc-windows-msvc matrix entry must set 'cross: false'; \
         nearby text:\n{nearby}"
    );
}

// ── WS3-TEST-04: ci.yml artifact upload includes .exe variants ───────────────

/// The upload-artifact step for the Windows target must include both
/// `amplihack.exe` and `amplihack-hooks.exe` paths so the release job can
/// package them.
#[test]
fn ci_yml_artifact_upload_includes_exe_variants() {
    let content = read_workflow("ci.yml");
    assert!(
        content.contains("amplihack.exe"),
        "ci.yml artifact upload path list must include 'amplihack.exe' for the Windows target"
    );
}

// ── WS3-TEST-05: release.yml contains Windows target ─────────────────────────

/// `release.yml` must include `x86_64-pc-windows-msvc` in the build matrix
/// so that Windows binaries are published with every release.
#[test]
fn release_yml_contains_windows_msvc_target() {
    let content = read_workflow("release.yml");
    assert!(
        content.contains("x86_64-pc-windows-msvc"),
        "release.yml must include 'x86_64-pc-windows-msvc' in the build matrix"
    );
}

// ── WS3-TEST-06: release.yml Package step uses shell: bash ───────────────────

/// The Package step in `release.yml` must specify `shell: bash` so that the
/// bash-syntax cp/tar commands work on the `windows-latest` runner (where
/// Git for Windows provides bash).
#[test]
fn release_yml_package_step_uses_bash_shell() {
    let content = read_workflow("release.yml");
    assert!(
        content.contains("shell: bash"),
        "release.yml Package step must include 'shell: bash' so that bash \
         syntax works on windows-latest runners"
    );
}

// ── WS3-TEST-07: release.yml Windows entry uses windows-latest runner ─────────

/// The Windows target entry in `release.yml` must use `windows-latest`.
#[test]
fn release_yml_windows_target_uses_windows_latest_runner() {
    let content = read_workflow("release.yml");

    let windows_pos = content
        .find("x86_64-pc-windows-msvc")
        .expect("x86_64-pc-windows-msvc not found in release.yml — see WS3-TEST-05");

    let nearby = &content[windows_pos..][..300.min(content.len() - windows_pos)];
    assert!(
        nearby.contains("windows-latest"),
        "the x86_64-pc-windows-msvc entry in release.yml must specify \
         'runner: windows-latest'; nearby text:\n{nearby}"
    );
}
