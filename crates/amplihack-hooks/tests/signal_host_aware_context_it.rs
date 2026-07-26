//! R2 — host-aware `additionalContext` injection (issue #1002).
//!
//! **The critical bug this file locks in:** inbound operator context was always
//! emitted as Claude Code's nested `hookSpecificOutput.additionalContext`, which
//! **Copilot CLI ignores**. Per the Copilot hooks reference, the
//! `userPromptSubmitted` hook must return a **top-level** `additionalContext`
//! **string** for the context to reach the agent. These tests pin the two
//! host-specific output shapes so a message from the Signal chat actually lands
//! in the Copilot agent's context.
//!
//! The seam under test is the pure output shaper
//! `amplihack_hooks::signal_integration::merge_additional_context`, plus the
//! host resolver `inject_host`. Keeping the shaper pure makes the contract
//! deterministic and parallel-safe (no process-global env/cwd mutation).
//!
//! RED: these symbols do not exist yet — this target fails to compile under
//! `--features signal` until R2 is implemented. Gated so the default build stays
//! green (preserving the 437 hook + 18 golden tests).
#![cfg(feature = "signal")]

use amplihack_hooks::signal_integration::{inject_host, merge_additional_context};
use serde_json::{Map, Value};

fn shape(host: &str, event: &str, ctx: &str) -> Map<String, Value> {
    let mut out = Map::new();
    merge_additional_context(&mut out, host, event, ctx);
    out
}

#[test]
fn copilot_emits_top_level_additional_context_string() {
    // Copilot reads a TOP-LEVEL `additionalContext` string. This is the exact
    // shape that was missing and caused inbound Signal messages to be dropped.
    let out = shape(
        "copilot",
        "userPromptSubmitted",
        "operator says: run the tests",
    );
    assert_eq!(
        out.get("additionalContext").and_then(Value::as_str),
        Some("operator says: run the tests"),
        "Copilot requires a top-level additionalContext string"
    );
}

#[test]
fn copilot_does_not_emit_nested_hook_specific_output() {
    // The nested Claude shape must NOT be present for Copilot — that is exactly
    // the payload Copilot ignores.
    let out = shape("copilot", "userPromptSubmitted", "hi");
    assert!(
        !out.contains_key("hookSpecificOutput"),
        "Copilot output must not carry Claude's nested hookSpecificOutput"
    );
}

#[test]
fn claude_emits_nested_hook_specific_output_unchanged() {
    // Claude Code behavior must be preserved byte-for-byte: nested
    // hookSpecificOutput with hookEventName + additionalContext.
    let out = shape(
        "claude",
        "UserPromptSubmit",
        "operator says: focus on the failing test",
    );
    let hso = out
        .get("hookSpecificOutput")
        .and_then(Value::as_object)
        .expect("claude must nest under hookSpecificOutput");
    assert_eq!(
        hso.get("hookEventName").and_then(Value::as_str),
        Some("UserPromptSubmit")
    );
    assert_eq!(
        hso.get("additionalContext").and_then(Value::as_str),
        Some("operator says: focus on the failing test")
    );
}

#[test]
fn claude_does_not_emit_top_level_additional_context() {
    let out = shape("claude", "UserPromptSubmit", "hi");
    assert!(
        !out.contains_key("additionalContext"),
        "Claude output must keep additionalContext nested, not top-level"
    );
}

#[test]
fn unknown_host_defaults_to_nested_claude_shape() {
    // Any non-copilot host (claude/codex/amplifier/unknown) uses the nested
    // shape. This preserves existing Claude-compatible behavior and never
    // accidentally strips context for a host we do not special-case.
    for host in ["codex", "amplifier", "somethingelse"] {
        let out = shape(host, "PostToolUse", "ctx");
        assert!(
            out.get("hookSpecificOutput").is_some(),
            "host {host:?} must fall back to nested hookSpecificOutput"
        );
        assert!(
            !out.contains_key("additionalContext"),
            "host {host:?} must not use the Copilot top-level shape"
        );
    }
}

#[test]
fn merge_preserves_existing_output_keys() {
    // PostToolUse builds an output map that may already carry `warnings` and
    // `metadata` (workflow enforcement). Merging Signal context must be additive
    // and never clobber those keys.
    let mut out = Map::new();
    out.insert("warnings".into(), serde_json::json!(["keep me"]));
    merge_additional_context(&mut out, "copilot", "postToolUse", "operator ctx");
    assert_eq!(out.get("warnings"), Some(&serde_json::json!(["keep me"])));
    assert_eq!(
        out.get("additionalContext").and_then(Value::as_str),
        Some("operator ctx")
    );
}

#[test]
fn inject_host_resolves_launcher_context_to_claude() {
    // `inject_host` is cwd-driven via the agent-binary resolver: a persisted
    // `.claude/runtime/launcher_context.json` selecting "claude" must resolve to
    // "claude" (parallel-safe: no env mutation, unique temp dir per test).
    let tmp = tempfile::tempdir().unwrap();
    let rt = tmp.path().join(".claude").join("runtime");
    std::fs::create_dir_all(&rt).unwrap();
    std::fs::write(
        rt.join("launcher_context.json"),
        serde_json::json!({ "launcher": "claude" }).to_string(),
    )
    .unwrap();

    // Only meaningful when the env override is absent (it takes precedence).
    if std::env::var_os("AMPLIHACK_AGENT_BINARY").is_none() {
        assert_eq!(inject_host(tmp.path()), "claude");
    }
}

#[test]
fn inject_host_returns_an_allowlisted_binary_name() {
    // Smoke: resolution always yields a known, safe binary identifier.
    let tmp = tempfile::tempdir().unwrap();
    let host = inject_host(tmp.path());
    assert!(
        ["copilot", "claude", "codex", "amplifier"].contains(&host.as_str()),
        "inject_host returned unexpected host {host:?}"
    );
}
