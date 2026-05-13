//! Failing TDD tests for issue #621: `amplihack copilot` subprocess-safe defaults.
//!
//! These tests SPECIFY the contract for:
//!   * `command::resolve_subprocess_safe`               — pure decision function
//!   * `command::should_inject_subprocess_safe_flags`   — pure per-flag duplicate suppression
//!   * `command::resolve_no_reflection`                 — pure precedence resolver
//!   * `command::build_command_for_dir`                 — extended with `subprocess_safe: bool`
//!
//! All tests in this file are expected to FAIL TO COMPILE until the
//! implementation in `command.rs` introduces the corresponding helpers and
//! extends `build_command_for_dir`'s signature. That compile failure IS the
//! "red" state of TDD; no `#[ignore]`, no stub helpers — implement them next.
//!
//! Layering note: `build_command_for_dir` already injects the broad
//! `--allow-all` for copilot (issue #303). When `subprocess_safe=true`, the
//! granular `--allow-all-tools` and `--allow-all-paths` flags are ALSO injected
//! (defense-in-depth — satisfies issue #621 acceptance criterion #1 that the
//! literal granular flags appear in argv). Tests below assert both expectations.

#![allow(clippy::bool_assert_comparison)]

use super::command::{
    build_command_for_dir, resolve_no_reflection, resolve_subprocess_safe,
    should_inject_subprocess_safe_flags,
};
use crate::binary_finder::BinaryInfo;
use crate::test_support::home_env_lock;
use std::path::PathBuf;

// ── Helpers ────────────────────────────────────────────────────────────────

fn copilot_binary() -> BinaryInfo {
    BinaryInfo {
        name: "copilot".to_string(),
        path: PathBuf::from("/usr/bin/copilot"),
        version: None,
    }
}

fn claude_binary() -> BinaryInfo {
    BinaryInfo {
        name: "claude".to_string(),
        path: PathBuf::from("/usr/bin/claude"),
        version: None,
    }
}

fn arg_strings(cmd: &std::process::Command) -> Vec<String> {
    cmd.get_args()
        .map(|s| s.to_string_lossy().into_owned())
        .collect()
}

// ── R6 / #1: resolve_subprocess_safe — pure decision function ──────────────

/// #1: explicit_flag=true MUST always return true regardless of other inputs.
#[test]
fn resolve_subprocess_safe_explicit_flag_true_always_returns_true() {
    assert!(resolve_subprocess_safe(true, None, true));
    assert!(resolve_subprocess_safe(true, Some("copilot"), true));
    assert!(resolve_subprocess_safe(true, None, false));
    assert!(resolve_subprocess_safe(true, Some(""), true));
}

/// #2: AMPLIHACK_AGENT_BINARY set non-empty MUST trigger subprocess-safe even at TTY.
#[test]
fn resolve_subprocess_safe_agent_binary_set_returns_true_even_with_tty() {
    assert!(resolve_subprocess_safe(false, Some("copilot"), true));
    assert!(resolve_subprocess_safe(false, Some("claude"), true));
    assert!(resolve_subprocess_safe(false, Some("anything"), true));
}

/// #3: All streams non-TTY MUST trigger subprocess-safe even with no env signals.
#[test]
fn resolve_subprocess_safe_non_tty_returns_true() {
    assert!(resolve_subprocess_safe(false, None, false));
}

/// #4: Interactive context with no signals MUST return false.
/// (Security-critical invariant: must not silently expand permissions on a TTY.)
#[test]
fn resolve_subprocess_safe_interactive_no_signals_returns_false() {
    assert_eq!(resolve_subprocess_safe(false, None, true), false);
}

/// #5: AMPLIHACK_AGENT_BINARY set to empty string MUST be treated as unset.
/// (Empty string is the sentinel "no delegation" marker — a process that
/// inherits an empty agent-binary env var must not be auto-classified as a
/// subprocess delegate.)
#[test]
fn resolve_subprocess_safe_empty_agent_binary_treated_as_unset() {
    assert_eq!(resolve_subprocess_safe(false, Some(""), true), false);
}

// ── R6 / #2-#6 + #7-#10: should_inject_subprocess_safe_flags ───────────────

/// #6: When subprocess_safe=true AND no user flags present, both granular
/// flags MUST be injected. Returns (inject_tools, inject_paths).
#[test]
fn should_inject_both_flags_when_subprocess_safe_and_no_user_flags() {
    let user_args: Vec<String> = vec![];
    let (inject_tools, inject_paths) = should_inject_subprocess_safe_flags(true, &user_args);
    assert!(inject_tools, "must inject --allow-all-tools");
    assert!(inject_paths, "must inject --allow-all-paths");
}

/// #7: When user supplied --allow-all-tools, MUST suppress that injection.
/// (Idempotency / no duplicate user-supplied flag.)
#[test]
fn should_skip_tools_when_user_supplied_allow_all_tools() {
    let user_args = vec!["--allow-all-tools".to_string()];
    let (inject_tools, inject_paths) = should_inject_subprocess_safe_flags(true, &user_args);
    assert_eq!(
        inject_tools, false,
        "must not duplicate user --allow-all-tools"
    );
    assert!(
        inject_paths,
        "user --allow-all-tools does not imply --allow-all-paths; must still inject paths"
    );
}

/// #8: When user supplied --allow-all-paths, MUST suppress that injection.
#[test]
fn should_skip_paths_when_user_supplied_allow_all_paths() {
    let user_args = vec!["--allow-all-paths".to_string()];
    let (inject_tools, inject_paths) = should_inject_subprocess_safe_flags(true, &user_args);
    assert!(
        inject_tools,
        "user --allow-all-paths does not imply --allow-all-tools; must still inject tools"
    );
    assert_eq!(
        inject_paths, false,
        "must not duplicate user --allow-all-paths"
    );
}

/// #9: User-supplied --allow-all is a SUPERSET — MUST suppress BOTH granular flags.
#[test]
fn should_skip_both_when_user_supplied_allow_all_superset() {
    let user_args = vec!["--allow-all".to_string()];
    let (inject_tools, inject_paths) = should_inject_subprocess_safe_flags(true, &user_args);
    assert_eq!(
        inject_tools, false,
        "--allow-all is superset of --allow-all-tools"
    );
    assert_eq!(
        inject_paths, false,
        "--allow-all is superset of --allow-all-paths"
    );
}

/// #10: When subprocess_safe=false, NEVER inject the granular flags
/// (regardless of what user passed). This is the security-critical invariant:
/// the change must NOT silently expand permissions on a TTY.
#[test]
fn should_inject_nothing_when_subprocess_safe_false() {
    let user_args: Vec<String> = vec![];
    let (inject_tools, inject_paths) = should_inject_subprocess_safe_flags(false, &user_args);
    assert_eq!(inject_tools, false);
    assert_eq!(inject_paths, false);

    // Even with user args present, subprocess_safe=false → no granular injection.
    let user_args = vec![
        "--remote".to_string(),
        "--model".to_string(),
        "gpt-5".to_string(),
    ];
    let (inject_tools, inject_paths) = should_inject_subprocess_safe_flags(false, &user_args);
    assert_eq!(inject_tools, false);
    assert_eq!(inject_paths, false);
}

// ── R6 / #19': resolve_no_reflection — precedence resolver ─────────────────
//
// Precedence (highest → lowest):
//   1. explicit --reflection (true)              → reflection ON  → return false
//   2. explicit --no-reflection (true)           → reflection OFF → return true
//   3. subprocess-safe context active (true)     → reflection OFF → return true
//   4. default                                   → reflection ON  → return false

/// #11: --reflection (explicit opt-in) overrides --no-reflection AND subprocess-safe.
/// (clap conflicts_with prevents --reflection + --no-reflection both being true,
/// but the resolver itself is defense-in-depth.)
#[test]
fn resolve_no_reflection_explicit_reflection_wins_over_subprocess_safe() {
    // explicit_reflection=true, no_reflection=false, subprocess_safe=true
    assert_eq!(resolve_no_reflection(true, false, true), false);
    // explicit_reflection=true, subprocess_safe=false
    assert_eq!(resolve_no_reflection(true, false, false), false);
}

/// #12: --no-reflection (explicit opt-out) takes effect when --reflection not passed.
#[test]
fn resolve_no_reflection_explicit_no_reflection_returns_true() {
    // explicit_reflection=false, no_reflection=true, subprocess_safe=false
    assert_eq!(resolve_no_reflection(false, true, false), true);
    // explicit_reflection=false, no_reflection=true, subprocess_safe=true
    assert_eq!(resolve_no_reflection(false, true, true), true);
}

/// #13: subprocess-safe context implies --no-reflection by default.
#[test]
fn resolve_no_reflection_subprocess_safe_default_returns_true() {
    // explicit_reflection=false, no_reflection=false, subprocess_safe=true
    assert_eq!(resolve_no_reflection(false, false, true), true);
}

/// #14: Default — no explicit flags, no subprocess-safe → reflection ON.
#[test]
fn resolve_no_reflection_default_returns_false() {
    // explicit_reflection=false, no_reflection=false, subprocess_safe=false
    assert_eq!(resolve_no_reflection(false, false, false), false);
}

// ── R6 / #7-#12: build_command_for_dir integration ─────────────────────────
//
// These tests call build_command_for_dir directly with the new
// `subprocess_safe: bool` parameter. The signature change from the design spec
// is:
//
//   pub(super) fn build_command_for_dir(
//       binary: &BinaryInfo,
//       resume: bool,
//       continue_session: bool,
//       skip_permissions: bool,
//       extra_args: &[String],
//       add_dir_override: Option<&Path>,
//       subprocess_safe: bool,        // ← NEW
//   ) -> Command;

/// #15: Copilot + subprocess_safe=true MUST inject BOTH --allow-all-tools and --allow-all-paths.
#[test]
fn copilot_subprocess_safe_injects_allow_all_tools_and_paths() {
    let _guard = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    // Clear opt-out so the existing #303 default --allow-all also fires (we
    // verify defense-in-depth layering).
    unsafe {
        std::env::remove_var("AMPLIHACK_COPILOT_NO_ALLOW_ALL");
    }

    let cmd = build_command_for_dir(
        &copilot_binary(),
        false,
        false,
        false,
        &[],
        None,
        true, // subprocess_safe
    );
    let args = arg_strings(&cmd);
    assert!(
        args.iter().any(|a| a == "--allow-all-tools"),
        "subprocess_safe copilot must inject --allow-all-tools; got {args:?}"
    );
    assert!(
        args.iter().any(|a| a == "--allow-all-paths"),
        "subprocess_safe copilot must inject --allow-all-paths; got {args:?}"
    );
}

/// #16: Subprocess_safe must NOT duplicate --allow-all-tools when user supplied it.
#[test]
fn copilot_subprocess_safe_skips_injection_when_user_supplied_allow_all_tools() {
    let _guard = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    unsafe {
        std::env::remove_var("AMPLIHACK_COPILOT_NO_ALLOW_ALL");
    }

    let extra = vec!["--allow-all-tools".to_string()];
    let cmd = build_command_for_dir(
        &copilot_binary(),
        false,
        false,
        false,
        &extra,
        None,
        true, // subprocess_safe
    );
    let args = arg_strings(&cmd);
    let tools_count = args
        .iter()
        .filter(|a| a.as_str() == "--allow-all-tools")
        .count();
    assert_eq!(
        tools_count, 1,
        "must not duplicate user-supplied --allow-all-tools; got {args:?}"
    );
}

/// #17: Subprocess_safe must NOT duplicate --allow-all-paths when user supplied it.
#[test]
fn copilot_subprocess_safe_skips_injection_when_user_supplied_allow_all_paths() {
    let _guard = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    unsafe {
        std::env::remove_var("AMPLIHACK_COPILOT_NO_ALLOW_ALL");
    }

    let extra = vec!["--allow-all-paths".to_string()];
    let cmd = build_command_for_dir(&copilot_binary(), false, false, false, &extra, None, true);
    let args = arg_strings(&cmd);
    let paths_count = args
        .iter()
        .filter(|a| a.as_str() == "--allow-all-paths")
        .count();
    assert_eq!(
        paths_count, 1,
        "must not duplicate user-supplied --allow-all-paths; got {args:?}"
    );
}

/// #18: User-supplied --allow-all (broader) suppresses BOTH granular flags.
#[test]
fn copilot_subprocess_safe_skips_both_granular_when_allow_all_present() {
    let _guard = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    // User passed --allow-all themselves; the existing #303 logic also skips
    // injecting an extra --allow-all because user already supplied it.
    unsafe {
        std::env::remove_var("AMPLIHACK_COPILOT_NO_ALLOW_ALL");
    }

    let extra = vec!["--allow-all".to_string()];
    let cmd = build_command_for_dir(&copilot_binary(), false, false, false, &extra, None, true);
    let args = arg_strings(&cmd);
    assert!(
        !args.iter().any(|a| a == "--allow-all-tools"),
        "user --allow-all is superset; must NOT inject --allow-all-tools; got {args:?}"
    );
    assert!(
        !args.iter().any(|a| a == "--allow-all-paths"),
        "user --allow-all is superset; must NOT inject --allow-all-paths; got {args:?}"
    );
}

/// #19 (security-critical): Copilot WITHOUT subprocess_safe must NOT inject
/// the granular subprocess-safe flags. The preexisting `--allow-all` default
/// from issue #303 may still appear (separate code path), but the granular
/// `--allow-all-tools` / `--allow-all-paths` must be absent. This locks the
/// invariant that an interactive TTY user does not silently get expanded
/// permission semantics from this PR.
#[test]
fn copilot_interactive_does_not_inject_subprocess_safe_granular_flags() {
    let _guard = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    // Suppress the existing default --allow-all so we can isolate the
    // subprocess-safe injection contract.
    unsafe {
        std::env::set_var("AMPLIHACK_COPILOT_NO_ALLOW_ALL", "1");
    }

    let cmd = build_command_for_dir(
        &copilot_binary(),
        false,
        false,
        false,
        &[],
        None,
        false, // subprocess_safe = false
    );
    let args = arg_strings(&cmd);

    unsafe {
        std::env::remove_var("AMPLIHACK_COPILOT_NO_ALLOW_ALL");
    }

    assert!(
        !args.iter().any(|a| a == "--allow-all-tools"),
        "interactive copilot must NOT receive --allow-all-tools; got {args:?}"
    );
    assert!(
        !args.iter().any(|a| a == "--allow-all-paths"),
        "interactive copilot must NOT receive --allow-all-paths; got {args:?}"
    );
}

/// #20: Non-copilot binaries (claude) must NEVER receive the subprocess-safe
/// granular flags, even when subprocess_safe=true. The contract is scoped to
/// the Copilot subcommand.
#[test]
fn non_copilot_binaries_never_get_subprocess_safe_granular_flags() {
    let _guard = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());

    let cmd = build_command_for_dir(
        &claude_binary(),
        false,
        false,
        false,
        &[],
        None,
        true, // subprocess_safe is true but this is a claude binary
    );
    let args = arg_strings(&cmd);
    assert!(
        !args.iter().any(|a| a == "--allow-all-tools"),
        "claude must NOT get --allow-all-tools; got {args:?}"
    );
    assert!(
        !args.iter().any(|a| a == "--allow-all-paths"),
        "claude must NOT get --allow-all-paths; got {args:?}"
    );
}

/// #21: Codex binary likewise must NEVER receive the subprocess-safe flags.
#[test]
fn codex_does_not_get_subprocess_safe_granular_flags() {
    let _guard = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());

    let codex = BinaryInfo {
        name: "codex".to_string(),
        path: PathBuf::from("/usr/bin/codex"),
        version: None,
    };
    let cmd = build_command_for_dir(&codex, false, false, false, &[], None, true);
    let args = arg_strings(&cmd);
    assert!(
        !args.iter().any(|a| a == "--allow-all-tools"),
        "codex must NOT get --allow-all-tools; got {args:?}"
    );
    assert!(
        !args.iter().any(|a| a == "--allow-all-paths"),
        "codex must NOT get --allow-all-paths; got {args:?}"
    );
}

/// #22: Argv ordering — granular flags injected by amplihack must appear
/// BEFORE user-supplied trailing args. This way, if the user passes a
/// duplicate, their value comes last in argv (typical CLI semantics: last
/// occurrence wins).
#[test]
fn copilot_subprocess_safe_flags_appear_before_user_trailing_args() {
    let _guard = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    unsafe {
        std::env::set_var("AMPLIHACK_COPILOT_NO_ALLOW_ALL", "1");
        std::env::set_var("AMPLIHACK_COPILOT_NO_REMOTE", "1");
    }

    let user_marker = "USER_TRAILING_ARG";
    let extra = vec![user_marker.to_string()];
    let cmd = build_command_for_dir(&copilot_binary(), false, false, false, &extra, None, true);
    let args = arg_strings(&cmd);

    unsafe {
        std::env::remove_var("AMPLIHACK_COPILOT_NO_ALLOW_ALL");
        std::env::remove_var("AMPLIHACK_COPILOT_NO_REMOTE");
    }

    let tools_pos = args
        .iter()
        .position(|a| a == "--allow-all-tools")
        .expect("--allow-all-tools must be injected");
    let paths_pos = args
        .iter()
        .position(|a| a == "--allow-all-paths")
        .expect("--allow-all-paths must be injected");
    let user_pos = args
        .iter()
        .position(|a| a == user_marker)
        .expect("user trailing arg must be present");

    assert!(
        tools_pos < user_pos,
        "--allow-all-tools must precede user trailing args; got {args:?}"
    );
    assert!(
        paths_pos < user_pos,
        "--allow-all-paths must precede user trailing args; got {args:?}"
    );
}
