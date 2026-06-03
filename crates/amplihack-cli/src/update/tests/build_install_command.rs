//! TDD tests for `build_install_command` (issue #683).
//!
//! These tests define the contract for the subprocess re-exec approach:
//! after downloading a new binary, `run_update` must spawn the NEW binary
//! as a subprocess with `install --force-refresh`, not call `run_install`
//! in-process with the OLD binary's compiled-in code.
//!
//! ## Test status (TDD Step 7)
//! - `uses_provided_binary_path` → PASSES (stub has correct program)
//! - All other tests → FAIL (stub is missing args/env vars)
//! - Tests will PASS once implementation is complete (Step 8)

use std::path::PathBuf;

use super::super::check::build_install_command;

// ============================================================================
// Program path tests
// ============================================================================

/// The function must use the exact path passed in, not `current_exe()`.
/// On Linux, `current_exe()` resolves to `/proc/self/exe` which points to
/// the deleted inode after atomic rename. Using the explicit path returned
/// by `download_and_replace` avoids this.
#[test]
fn uses_provided_binary_path() {
    let fake_path = PathBuf::from("/tmp/test-amplihack/bin/amplihack");
    let cmd = build_install_command(&fake_path);
    assert_eq!(
        cmd.get_program(),
        fake_path.as_os_str(),
        "must use the provided binary path, not current_exe() or a hardcoded path"
    );
}

/// Edge case: paths containing spaces must be preserved exactly.
/// The `Command` API handles quoting automatically, but we verify the
/// path is passed through without modification.
#[test]
fn handles_path_with_spaces() {
    let fake_path = PathBuf::from("/home/user name/local/bin/amplihack");
    let cmd = build_install_command(&fake_path);
    assert_eq!(
        cmd.get_program(),
        fake_path.as_os_str(),
        "must preserve paths with spaces exactly as provided"
    );
}

// ============================================================================
// Argument tests
// ============================================================================

/// The subprocess must pass `install` as the first argument (subcommand).
#[test]
fn includes_install_subcommand_arg() {
    let fake_path = PathBuf::from("/usr/local/bin/amplihack");
    let cmd = build_install_command(&fake_path);
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();
    assert!(
        args.contains(&"install".to_string()),
        "must include 'install' subcommand, got args: {args:?}"
    );
}

/// The subprocess must pass `--force-refresh` so the new binary knows this
/// install was triggered by an update (not a manual `amplihack install`).
#[test]
fn includes_force_refresh_flag() {
    let fake_path = PathBuf::from("/usr/local/bin/amplihack");
    let cmd = build_install_command(&fake_path);
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();
    assert!(
        args.contains(&"--force-refresh".to_string()),
        "must include '--force-refresh' flag, got args: {args:?}"
    );
}

/// Arguments must be in the correct order: subcommand first, then flags.
/// clap requires the subcommand name before any flags.
#[test]
fn args_in_correct_order() {
    let fake_path = PathBuf::from("/usr/local/bin/amplihack");
    let cmd = build_install_command(&fake_path);
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();
    let install_pos = args.iter().position(|a| a == "install");
    let force_refresh_pos = args.iter().position(|a| a == "--force-refresh");
    assert!(
        install_pos.is_some() && force_refresh_pos.is_some(),
        "both 'install' and '--force-refresh' must be present, got: {args:?}"
    );
    assert!(
        install_pos.unwrap() < force_refresh_pos.unwrap(),
        "'install' must come before '--force-refresh', got: {args:?}"
    );
}

// ============================================================================
// Environment variable tests
// ============================================================================

/// The subprocess must set `AMPLIHACK_NO_UPDATE_CHECK=1` to prevent the new
/// binary from re-checking for updates (infinite recursion guard).
#[test]
fn sets_no_update_check_env() {
    let fake_path = PathBuf::from("/usr/local/bin/amplihack");
    let cmd = build_install_command(&fake_path);
    let envs: Vec<(String, Option<String>)> = cmd
        .get_envs()
        .map(|(k, v)| {
            (
                k.to_string_lossy().to_string(),
                v.map(|v| v.to_string_lossy().to_string()),
            )
        })
        .collect();
    assert!(
        envs.contains(&(
            "AMPLIHACK_NO_UPDATE_CHECK".to_string(),
            Some("1".to_string())
        )),
        "must set AMPLIHACK_NO_UPDATE_CHECK=1 to prevent update recursion, got envs: {envs:?}"
    );
}

/// The subprocess must set `AMPLIHACK_NONINTERACTIVE=1` to suppress any
/// interactive prompts during the automated install step.
#[test]
fn sets_noninteractive_env() {
    let fake_path = PathBuf::from("/usr/local/bin/amplihack");
    let cmd = build_install_command(&fake_path);
    let envs: Vec<(String, Option<String>)> = cmd
        .get_envs()
        .map(|(k, v)| {
            (
                k.to_string_lossy().to_string(),
                v.map(|v| v.to_string_lossy().to_string()),
            )
        })
        .collect();
    assert!(
        envs.contains(&(
            "AMPLIHACK_NONINTERACTIVE".to_string(),
            Some("1".to_string())
        )),
        "must set AMPLIHACK_NONINTERACTIVE=1 to suppress prompts, got envs: {envs:?}"
    );
}
