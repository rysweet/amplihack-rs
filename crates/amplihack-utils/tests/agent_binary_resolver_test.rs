//! TDD: Failing tests for `amplihack_utils::agent_binary` resolver.
//!
//! These tests define the contract for the unified agent-binary resolver:
//! 1. Precedence: `AMPLIHACK_AGENT_BINARY` env > `launcher_context.json` > "copilot" default.
//! 2. Strict allowlist: only {claude, copilot, codex, amplifier}.
//! 3. Walk-up boundary, file size cap, validation hardening.
//!
//! Run with: `TMPDIR=/tmp cargo test -p amplihack-utils --test agent_binary_resolver_test`
//!
//! NOTE: tests mutate process env so they share `serial_test::serial` (or a
//! single-threaded runner). Edition-2024 requires `unsafe` for env mutation.

#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::Path;
use tempfile::TempDir;

use amplihack_utils::agent_binary::{
    self, ALLOWED_BINARIES, DEFAULT_BINARY, ResolveError, resolve,
};

fn write_launcher_context(repo: &Path, launcher: &str) {
    let runtime = repo.join(".claude").join("runtime");
    fs::create_dir_all(&runtime).unwrap();
    let body =
        format!(r#"{{"launcher":"{launcher}","pid":1234,"created_at":"2026-01-01T00:00:00Z"}}"#);
    fs::write(runtime.join("launcher_context.json"), body).unwrap();
}

fn clear_env() {
    // SAFETY: tests are serialized; env mutation is unsafe in edition 2024.
    unsafe {
        std::env::remove_var("AMPLIHACK_AGENT_BINARY");
    }
}

fn set_env(value: &str) {
    // SAFETY: see clear_env.
    unsafe {
        std::env::set_var("AMPLIHACK_AGENT_BINARY", value);
    }
}

/// Some test inputs (NUL bytes, etc.) are rejected by the OS before reaching
/// our resolver. Skip those — the resolver's `validate_binary_name` unit tests
/// already cover the `\0` case directly. The security test below also exercises
/// these via `try_set_env`.
fn try_set_env(value: &str) -> bool {
    if value.bytes().any(|b| b == 0) {
        return false;
    }
    set_env(value);
    true
}

#[test]
fn default_is_copilot_when_nothing_set() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    let result = resolve(tmp.path()).unwrap();
    assert_eq!(result, "copilot");
    assert_eq!(DEFAULT_BINARY, "copilot");
}

#[test]
fn env_var_takes_precedence_over_file() {
    let tmp = TempDir::new().unwrap();
    write_launcher_context(tmp.path(), "claude");
    set_env("copilot");
    let result = resolve(tmp.path()).unwrap();
    assert_eq!(result, "copilot");
    clear_env();
}

#[test]
fn launcher_context_used_when_env_unset() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    write_launcher_context(tmp.path(), "claude");
    let result = resolve(tmp.path()).unwrap();
    assert_eq!(result, "claude");
}

#[test]
fn allowlist_contains_exactly_four_binaries() {
    let mut sorted: Vec<&str> = ALLOWED_BINARIES.to_vec();
    sorted.sort_unstable();
    assert_eq!(sorted, vec!["amplifier", "claude", "codex", "copilot"]);
}

#[test]
fn env_value_outside_allowlist_is_rejected() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    set_env("malicious");
    let result = resolve(tmp.path()).unwrap();
    // Rejected → falls through to default.
    assert_eq!(result, "copilot");
    clear_env();
}

#[test]
fn env_value_with_path_separator_is_rejected() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    for bad in &["/bin/sh", "..\\evil", "claude/../sh", "cla\0ude"] {
        if !try_set_env(bad) {
            // OS rejects NUL bytes in env values; the validator unit tests
            // cover that branch directly (see `validate_rejects_dangerous_inputs`).
            continue;
        }
        let result = resolve(tmp.path()).unwrap();
        assert_eq!(result, "copilot", "value {bad:?} should be rejected");
    }
    clear_env();
}

#[test]
fn env_value_is_trimmed_and_lowercased() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    set_env("  CLAUDE  ");
    let result = resolve(tmp.path()).unwrap();
    assert_eq!(result, "claude");
    clear_env();
}

#[test]
fn launcher_context_outside_allowlist_falls_back_to_default() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    write_launcher_context(tmp.path(), "rm-rf-slash");
    let result = resolve(tmp.path()).unwrap();
    assert_eq!(result, "copilot");
}

#[test]
fn oversized_launcher_context_is_rejected() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    let runtime = tmp.path().join(".claude").join("runtime");
    fs::create_dir_all(&runtime).unwrap();
    // > 64 KiB cap.
    let huge = "x".repeat(70 * 1024);
    let body = format!(r#"{{"launcher":"claude","junk":"{huge}"}}"#);
    fs::write(runtime.join("launcher_context.json"), body).unwrap();
    let result = resolve(tmp.path()).unwrap();
    assert_eq!(result, "copilot", "oversized file must be rejected");
}

#[test]
fn malformed_launcher_context_falls_back_to_default() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    let runtime = tmp.path().join(".claude").join("runtime");
    fs::create_dir_all(&runtime).unwrap();
    fs::write(runtime.join("launcher_context.json"), "{not valid json").unwrap();
    let result = resolve(tmp.path()).unwrap();
    assert_eq!(result, "copilot");
}

#[test]
fn walk_up_finds_context_in_ancestor() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    write_launcher_context(tmp.path(), "claude");
    let nested = tmp.path().join("a").join("b").join("c");
    fs::create_dir_all(&nested).unwrap();
    let result = resolve(&nested).unwrap();
    assert_eq!(result, "claude");
}

#[test]
fn walk_up_stops_at_git_boundary() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    // Outer repo with launcher_context = claude.
    write_launcher_context(tmp.path(), "claude");
    // Inner "repo" with .git boundary, no launcher_context.
    let inner = tmp.path().join("inner");
    fs::create_dir_all(inner.join(".git")).unwrap();
    let nested = inner.join("src");
    fs::create_dir_all(&nested).unwrap();
    let result = resolve(&nested).unwrap();
    // Should NOT find outer claude — boundary respected.
    assert_eq!(result, "copilot");
}

#[test]
fn resolve_function_signature_returns_result() {
    // Compile-time contract: resolve(&Path) -> Result<String, ResolveError>.
    clear_env();
    let tmp = TempDir::new().unwrap();
    let _: Result<String, ResolveError> = resolve(tmp.path());
}

#[test]
fn validate_binary_name_helper_exposed() {
    assert!(agent_binary::validate_binary_name("claude").is_some());
    assert!(agent_binary::validate_binary_name("COPILOT").is_some());
    assert!(agent_binary::validate_binary_name("evil/bin").is_none());
    assert!(agent_binary::validate_binary_name("").is_none());
    assert!(agent_binary::validate_binary_name(&"x".repeat(64)).is_none());
}
