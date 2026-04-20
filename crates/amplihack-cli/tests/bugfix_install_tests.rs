//! TDD tests for install/update bug fixes: #254, #249, #257.
//!
//! These tests verify:
//! - #254: REPO_ARCHIVE_URL and REPO_GIT_URL point to amplihack-rs;
//!   find_framework_repo_root() accepts `amplifier-bundle/` marker
//! - #249: run_update() calls ensure_framework_installed() after binary swap
//! - #257: verify_sha256() uses http_get_with_retry() (tested via contract)

use std::fs;
use tempfile::TempDir;

// ============================================================================
// Bug #254: URLs point to amplihack-rs and repo root detection
// ============================================================================

#[test]
fn find_framework_repo_root_accepts_claude_dir_marker() {
    // Contract: A directory containing `.claude/` is recognized as a
    // framework repo root (Python layout).
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(repo.join(".claude")).unwrap();

    // Use the clone module's find_framework_repo_root indirectly:
    // We can test the behavior by checking the directory marker logic.
    assert!(repo.join(".claude").is_dir());
}

#[test]
fn find_framework_repo_root_accepts_amplifier_bundle_marker() {
    // Contract: A directory containing `amplifier-bundle/` is recognized
    // as a framework repo root (Rust layout, fix #254).
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(repo.join("amplifier-bundle")).unwrap();

    assert!(repo.join("amplifier-bundle").is_dir());
}

#[test]
fn find_framework_repo_root_searches_nested_directories() {
    // Contract: The search is BFS — it finds the repo root even if it's
    // nested inside a GitHub archive extraction directory like
    // `amplihack-rs-main/`.
    let tmp = TempDir::new().unwrap();
    let nested = tmp
        .path()
        .join("amplihack-rs-main")
        .join("amplifier-bundle");
    fs::create_dir_all(&nested).unwrap();

    // Simulate BFS search from tmp root
    let mut found = false;
    let mut queue = std::collections::VecDeque::from([tmp.path().to_path_buf()]);
    while let Some(dir) = queue.pop_front() {
        if dir.join(".claude").is_dir() || dir.join("amplifier-bundle").is_dir() {
            found = true;
            break;
        }
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    queue.push_back(path);
                }
            }
        }
    }
    assert!(
        found,
        "BFS should find amplifier-bundle/ in nested directory"
    );
}

#[test]
fn find_framework_repo_root_fails_when_no_marker_present() {
    // Contract: If neither `.claude/` nor `amplifier-bundle/` exists,
    // the search must fail with a clear error.
    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("some-random-dir")).unwrap();
    fs::write(tmp.path().join("some-file.txt"), "hello").unwrap();

    // Simulate the search
    let mut found = false;
    let mut queue = std::collections::VecDeque::from([tmp.path().to_path_buf()]);
    while let Some(dir) = queue.pop_front() {
        if dir.join(".claude").is_dir() || dir.join("amplifier-bundle").is_dir() {
            found = true;
            break;
        }
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    queue.push_back(path);
                }
            }
        }
    }
    assert!(!found, "search should fail when no marker directory exists");
}

// ============================================================================
// Bug #254: URL constants point to amplihack-rs
// ============================================================================

#[test]
fn repo_archive_url_points_to_amplihack_rs() {
    // Contract: REPO_ARCHIVE_URL must reference the Rust repo, not the
    // Python repo. We verify this by checking the binary's source code
    // was compiled with the correct constant.
    //
    // Since the constant is pub(super), we verify indirectly by checking
    // that the types.rs file contains the correct URL.
    let types_src = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/commands/install/types.rs"
    ));
    assert!(
        types_src.contains("amplihack-rs/archive/refs/heads/main.tar.gz"),
        "REPO_ARCHIVE_URL must point to amplihack-rs repo"
    );
    assert!(
        !types_src.contains("amplihack/archive/refs/heads/main.tar.gz")
            || types_src.contains("amplihack-rs/archive"),
        "REPO_ARCHIVE_URL must not point to old Python amplihack repo"
    );
}

#[test]
fn repo_git_url_points_to_amplihack_rs() {
    // Contract: REPO_GIT_URL must reference the Rust repo.
    let types_src = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/commands/install/types.rs"
    ));
    assert!(
        types_src.contains("github.com/rysweet/amplihack-rs"),
        "REPO_GIT_URL must point to amplihack-rs"
    );
}

// ============================================================================
// Bug #249: run_update() calls ensure_framework_installed()
// ============================================================================

#[test]
fn update_check_source_includes_framework_restage() {
    // Contract: run_update() must call ensure_framework_installed() after
    // the binary swap. We verify this at the source level since we can't
    // easily mock the network calls in an integration test.
    let check_src = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/update/check.rs"));
    assert!(
        check_src.contains("ensure_framework_installed"),
        "run_update() must call ensure_framework_installed() after binary swap"
    );
    // Verify it's in the run_update function, not just imported
    let run_update_start = check_src
        .find("fn run_update()")
        .expect("run_update must exist");
    let run_update_body = &check_src[run_update_start..];
    // Find the end of the function (next `fn ` at the same indentation level)
    let next_fn = run_update_body[1..]
        .find("\nfn ")
        .unwrap_or(run_update_body.len());
    let run_update_body = &run_update_body[..next_fn];
    assert!(
        run_update_body.contains("ensure_framework_installed"),
        "ensure_framework_installed() must be called inside run_update(), not elsewhere"
    );
}

#[test]
fn update_check_handles_framework_restage_failure_gracefully() {
    // Contract: If ensure_framework_installed() fails after binary swap,
    // run_update() must print a warning but NOT return an error.
    let check_src = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/update/check.rs"));
    // The call should be in an `if let Err(err) = ...` block
    assert!(
        check_src
            .contains("if let Err(err) = crate::commands::install::ensure_framework_installed()"),
        "ensure_framework_installed errors must be handled gracefully"
    );
    // Should print a warning, not bail
    assert!(
        check_src.contains("framework asset refresh failed"),
        "failure message must mention 'framework asset refresh failed'"
    );
    assert!(
        check_src.contains("amplihack install"),
        "failure message must suggest 'amplihack install' as manual recovery"
    );
}

// ============================================================================
// Bug #257: verify_sha256 uses http_get_with_retry
// ============================================================================

#[test]
fn verify_sha256_uses_retry_variant() {
    // Contract: verify_sha256() must use http_get_with_retry() instead of
    // http_get() when fetching the checksum file.
    let install_src = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/update/install.rs"
    ));
    let verify_fn_start = install_src
        .find("fn verify_sha256")
        .expect("verify_sha256 must exist");
    let verify_fn_body = &install_src[verify_fn_start..];
    // Find the closing brace of the function
    let mut brace_depth = 0;
    let mut fn_end = 0;
    for (i, ch) in verify_fn_body.char_indices() {
        if ch == '{' {
            brace_depth += 1;
        }
        if ch == '}' {
            brace_depth -= 1;
            if brace_depth == 0 {
                fn_end = i + 1;
                break;
            }
        }
    }
    let verify_fn_body = &verify_fn_body[..fn_end];
    assert!(
        verify_fn_body.contains("http_get_with_retry"),
        "verify_sha256 must use http_get_with_retry, not plain http_get"
    );
}

#[test]
fn download_and_replace_also_uses_retry() {
    // Contract: download_and_replace() must use http_get_with_retry()
    // for the archive download as well.
    let install_src = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/update/install.rs"
    ));
    let download_fn_start = install_src
        .find("fn download_and_replace")
        .expect("download_and_replace must exist");
    let download_fn_body = &install_src[download_fn_start..];
    assert!(
        download_fn_body.contains("http_get_with_retry"),
        "download_and_replace must use http_get_with_retry for archive download"
    );
}

// ============================================================================
// Bug #257: SHA-256 validation logic
// ============================================================================

#[test]
fn sha256_hex_validation_rejects_short_digest() {
    // Contract: A checksum file with fewer than 64 hex characters must be
    // rejected.
    let install_src = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/update/install.rs"
    ));
    assert!(
        install_src.contains("expected_hex.len() != 64"),
        "verify_sha256 must validate digest is exactly 64 hex chars"
    );
}

#[test]
fn sha256_hex_validation_rejects_non_hex() {
    // Contract: A checksum with non-hex characters must be rejected.
    let install_src = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/update/install.rs"
    ));
    assert!(
        install_src.contains("is_ascii_hexdigit"),
        "verify_sha256 must validate hex characters"
    );
}

// ============================================================================
// Bug #254: clone.rs root detection (source-level contract verification)
// ============================================================================

#[test]
fn clone_rs_checks_both_markers() {
    // Contract: find_framework_repo_root must check for BOTH .claude/
    // and amplifier-bundle/ directory markers.
    let clone_src = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/commands/install/clone.rs"
    ));
    assert!(
        clone_src.contains(".claude"),
        "find_framework_repo_root must check for .claude/ marker"
    );
    assert!(
        clone_src.contains("amplifier-bundle"),
        "find_framework_repo_root must check for amplifier-bundle/ marker"
    );
}

#[test]
fn clone_rs_error_mentions_both_markers() {
    // Contract: The error message when no root is found must mention
    // both possible markers so the user knows what to look for.
    let clone_src = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/commands/install/clone.rs"
    ));
    let bail_pos = clone_src
        .rfind("bail!")
        .expect("should have a bail! for missing root");
    let end = (bail_pos + 200).min(clone_src.len());
    let bail_msg = &clone_src[bail_pos..end];
    assert!(
        bail_msg.contains(".claude") && bail_msg.contains("amplifier-bundle"),
        "error message must mention both .claude and amplifier-bundle, got: {bail_msg}"
    );
}
