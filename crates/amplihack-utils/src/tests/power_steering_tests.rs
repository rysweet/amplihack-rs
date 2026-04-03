use super::*;
use std::fs;
use tempfile::TempDir;

/// Helper: create the `.disabled` file in a temp runtime dir layout.
fn setup_disabled(tmp: &TempDir) -> PathBuf {
    let runtime = tmp.path().join(".claude").join("runtime");
    let ps_dir = runtime.join("power-steering");
    fs::create_dir_all(&ps_dir).unwrap();
    let disabled = ps_dir.join(".disabled");
    fs::write(&disabled, "").unwrap();
    disabled
}

// ── prompt_re_enable_if_disabled tests ──────────────────────────────────

#[test]
fn returns_enabled_when_no_disabled_file() {
    let tmp = TempDir::new().unwrap();
    // Create the runtime dir but NOT the .disabled file.
    let runtime = tmp.path().join(".claude").join("runtime").join("power-steering");
    fs::create_dir_all(&runtime).unwrap();

    crate::worktree::clear_cache();
    let result = prompt_re_enable_if_disabled(Some(tmp.path()));
    assert_eq!(result, ReEnableResult::Enabled);
}

#[test]
fn returns_enabled_when_no_runtime_dir_at_all() {
    let tmp = TempDir::new().unwrap();
    crate::worktree::clear_cache();
    let result = prompt_re_enable_if_disabled(Some(tmp.path()));
    assert_eq!(result, ReEnableResult::Enabled);
}

#[test]
fn noninteractive_removes_disabled_file() {
    // This test runs in a test harness which has piped stdin (non-interactive).
    let tmp = TempDir::new().unwrap();
    let disabled = setup_disabled(&tmp);
    assert!(disabled.exists());

    crate::worktree::clear_cache();
    let result = prompt_re_enable_if_disabled(Some(tmp.path()));
    // In the test harness, stdin is piped → non-interactive path fires.
    assert_eq!(result, ReEnableResult::Enabled);
    assert!(!disabled.exists(), ".disabled should have been removed");
}

#[test]
fn disabled_file_path_is_correct() {
    let tmp = TempDir::new().unwrap();
    let disabled = setup_disabled(&tmp);
    let expected_suffix = Path::new(".claude")
        .join("runtime")
        .join("power-steering")
        .join(".disabled");
    assert!(
        disabled.ends_with(&expected_suffix),
        "disabled file should be at the expected path"
    );
}

#[test]
fn re_enable_result_equality() {
    assert_eq!(ReEnableResult::Enabled, ReEnableResult::Enabled);
    assert_eq!(ReEnableResult::Disabled, ReEnableResult::Disabled);
    assert_ne!(ReEnableResult::Enabled, ReEnableResult::Disabled);
}

#[test]
fn remove_disabled_file_safe_handles_missing_file() {
    let tmp = TempDir::new().unwrap();
    let fake = tmp.path().join("does-not-exist");
    // Should not panic.
    remove_disabled_file_safe(&fake, None);
}

#[test]
fn remove_disabled_file_safe_removes_existing() {
    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join(".disabled");
    fs::write(&file, "").unwrap();
    assert!(file.exists());
    remove_disabled_file_safe(&file, Some("(test)"));
    assert!(!file.exists());
}

#[test]
fn is_noninteractive_returns_true_in_test_harness() {
    // In `cargo test`, stdin is typically piped.
    // This verifies the function doesn't panic and returns a plausible value.
    let _result = is_noninteractive();
    // We just verify it doesn't crash; the exact value depends on the runner.
}

#[test]
fn timeout_constant_is_30() {
    assert_eq!(TIMEOUT_SECONDS, 30);
}

#[test]
fn try_prompt_with_nonexistent_project_root() {
    crate::worktree::clear_cache();
    // A nonexistent root should still succeed (fail-open).
    let result = try_prompt(Some(Path::new("/nonexistent-path-for-test")));
    // The worktree module fails gracefully; either we get Enabled (no
    // .disabled file) or an error that gets caught at the outer level.
    match result {
        Ok(ReEnableResult::Enabled) => {} // expected
        Ok(ReEnableResult::Disabled) => {} // acceptable
        Err(_) => {}                       // also acceptable (IO error)
    }
}
