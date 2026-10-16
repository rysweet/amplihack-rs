//! Structural guards for Signal-enabled shipped binaries.
//!
//! These tests read workflow YAML only. They ensure official CI, release, and
//! snapshot builds compile the shipped `amplihack-hooks` binary with the Signal
//! feature while preserving existing job names and artifact layout.

use std::path::PathBuf;

fn workspace_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path
}

fn read_repo_file(path: &str) -> String {
    let full_path = workspace_root().join(path);
    std::fs::read_to_string(&full_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", full_path.display()))
}

fn assert_signal_release_command(workflow: &str, workflow_name: &str) {
    assert!(
        workflow.contains("-p amplihack -p amplihack-hooks-bin"),
        "{workflow_name} must build exactly the shipped amplihack and amplihack-hooks packages"
    );
    assert!(
        workflow.contains("--features amplihack-hooks-bin/signal"),
        "{workflow_name} must enable the hook binary's Signal feature for shipped Unix builds"
    );
}

#[test]
fn ci_required_build_jobs_compile_signal_enabled_hooks() {
    let ci = read_repo_file(".github/workflows/ci.yml");
    assert_signal_release_command(&ci, "ci.yml");

    for required_name in [
        "Build ${{ matrix.target }}",
        "cargo build --release --locked --target ${{ matrix.target }}",
        "cross build --release --locked --target ${{ matrix.target }}",
    ] {
        assert!(
            ci.contains(required_name),
            "ci.yml must preserve required build job/command marker `{required_name}`"
        );
    }
}

#[test]
fn stable_release_packages_signal_enabled_hooks() {
    let release = read_repo_file(".github/workflows/release.yml");
    assert_signal_release_command(&release, "release.yml");
    assert!(
        release.contains("cp target/${{ matrix.target }}/release/amplihack-hooks"),
        "release.yml must continue packaging amplihack-hooks from the target release directory"
    );
}

#[test]
fn stable_release_builds_no_windows_target() {
    // Owner decision: no Windows builds. Every remaining release target is
    // Unix, so the hook Signal feature is enabled unconditionally.
    let release = read_repo_file(".github/workflows/release.yml");
    for forbidden in ["windows-msvc", "windows-latest", "exe_suffix"] {
        assert!(
            !release.contains(forbidden),
            "release.yml must not reintroduce a Windows build leg (found `{forbidden}`)"
        );
    }
    assert_eq!(
        release
            .matches("--features amplihack-hooks-bin/signal")
            .count(),
        1,
        "release.yml must enable the hook Signal feature exactly once in the build step"
    );
}

#[test]
fn snapshot_release_packages_signal_enabled_hooks() {
    let snapshot = read_repo_file(".github/workflows/publish-snapshot.yml");
    assert_signal_release_command(&snapshot, "publish-snapshot.yml");
    assert!(
        snapshot.contains("amplihack amplihack-hooks"),
        "publish-snapshot.yml must continue packaging both shipped binaries"
    );
}
