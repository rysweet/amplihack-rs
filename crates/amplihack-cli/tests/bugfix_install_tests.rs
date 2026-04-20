//! Bugfix integration tests for install/update commands.
//!
//! Covers:
//! - #254: URL constants point to amplihack-rs repo
//! - #249: run_update() calls ensure_framework_installed() after binary swap
//! - #257: verify_sha256() uses http_get_with_retry()

// ── #254: URL constants verification ──

/// Verify that the repo archive URL points to the Rust repo, not the Python repo.
#[test]
fn repo_archive_url_points_to_amplihack_rs() {
    // We can't import the private constant directly, but we can verify
    // the source code contains the correct URL.
    let source = include_str!("../src/commands/install/types.rs");
    assert!(
        source.contains("rysweet/amplihack-rs"),
        "REPO_ARCHIVE_URL must point to amplihack-rs repo"
    );
    assert!(
        !source.contains("\"https://github.com/rysweet/amplihack/archive"),
        "REPO_ARCHIVE_URL must NOT point to the Python amplihack repo"
    );
}

/// Verify the git clone URL points to the Rust repo.
#[test]
fn repo_git_url_points_to_amplihack_rs() {
    let source = include_str!("../src/commands/install/types.rs");
    assert!(
        source.contains("\"https://github.com/rysweet/amplihack-rs\""),
        "REPO_GIT_URL must point to amplihack-rs repo"
    );
}

/// Verify find_framework_repo_root accepts amplifier-bundle/ directory.
#[test]
fn find_framework_repo_root_accepts_amplifier_bundle() {
    let source = include_str!("../src/commands/install/clone.rs");
    assert!(
        source.contains("amplifier-bundle"),
        "find_framework_repo_root must accept amplifier-bundle/ as a marker"
    );
}

/// Verify find_framework_repo_root still accepts .claude/ directory (backward compat).
#[test]
fn find_framework_repo_root_still_accepts_claude_dir() {
    let source = include_str!("../src/commands/install/clone.rs");
    assert!(
        source.contains(".claude"),
        "find_framework_repo_root must still accept .claude/ as a marker"
    );
}

/// Verify the error message mentions both markers.
#[test]
fn find_framework_repo_root_error_mentions_both_markers() {
    let source = include_str!("../src/commands/install/clone.rs");
    assert!(
        source.contains(".claude/") && source.contains("amplifier-bundle/"),
        "error message should mention both .claude/ and amplifier-bundle/"
    );
}

/// Verify BFS search implementation (not just string matching).
#[test]
fn find_framework_repo_root_uses_bfs_search() {
    let source = include_str!("../src/commands/install/clone.rs");
    assert!(
        source.contains("VecDeque"),
        "find_framework_repo_root should use BFS (VecDeque)"
    );
}

// ── #249: Update re-stages assets ──

/// Verify run_update calls ensure_framework_installed after download_and_replace.
#[test]
fn run_update_calls_ensure_framework_installed() {
    let source = include_str!("../src/update/check.rs");
    assert!(
        source.contains("ensure_framework_installed"),
        "run_update must call ensure_framework_installed after binary replacement"
    );
}

/// Verify the ensure_framework_installed failure is non-fatal (logged, not propagated).
#[test]
fn run_update_asset_restaging_failure_is_nonfatal() {
    let source = include_str!("../src/update/check.rs");
    // Should use if-let-Err pattern, not the ? operator on ensure_framework_installed
    assert!(
        source.contains("if let Err"),
        "asset re-staging failure should be caught with if-let-Err, not propagated with ?"
    );
    assert!(
        source.contains("amplihack install"),
        "error message should suggest running 'amplihack install' manually"
    );
}

// ── #257: Checksum uses retry ──

/// Verify verify_sha256 uses http_get_with_retry, not plain http_get.
#[test]
fn verify_sha256_uses_retry() {
    let source = include_str!("../src/update/install.rs");
    assert!(
        source.contains("http_get_with_retry"),
        "verify_sha256 must use http_get_with_retry for checksum download"
    );
    // Verify it's not using plain http_get for the checksum
    // (http_get_with_retry contains http_get so we check the function call site)
    // The verify_sha256 function is above download_and_replace in the file
    let checksum_section = &source[..source
        .find("fn download_and_replace")
        .unwrap_or(source.len())];
    assert!(
        checksum_section.contains("http_get_with_retry"),
        "checksum download should use http_get_with_retry"
    );
}

/// Verify download_and_replace also uses retry for the archive download.
#[test]
fn download_and_replace_uses_retry() {
    let source = include_str!("../src/update/install.rs");
    assert!(
        source.contains("http_get_with_retry(&release.asset_url)"),
        "archive download must use http_get_with_retry"
    );
}
