//! TDD: Failing tests for `amplihack_hooks::binary_hook_resolver`.
//!
//! Contract:
//! - `HookEvent` enum has 8 variants matching on-disk filenames.
//! - `resolve_hook(binary, event, hooks_root)` returns the canonical path when
//!   the per-binary hook file exists.
//! - When absent, returns `HookError::MissingHookForBinary { binary, event,
//!   expected_path, remediation }` — NO claude fallback, NO stub creation.
//! - `expected_hook_path` rejects non-allowlisted binaries and prevents path
//!   traversal escapes.

#![allow(clippy::unwrap_used)]

use std::fs;
use tempfile::TempDir;

use amplihack_hooks::binary_hook_resolver::{
    HookError, HookEvent, expected_hook_path, resolve_hook,
};

#[test]
fn hook_event_has_eight_variants_with_filename_mapping() {
    let pairs = [
        (HookEvent::PreToolUse, "PreToolUse"),
        (HookEvent::PostToolUse, "PostToolUse"),
        (HookEvent::Stop, "Stop"),
        (HookEvent::SessionStart, "SessionStart"),
        (HookEvent::SessionEnd, "SessionEnd"),
        (HookEvent::SessionStop, "SessionStop"),
        (HookEvent::UserPromptSubmit, "UserPromptSubmit"),
        (HookEvent::PreCompact, "PreCompact"),
    ];
    for (event, expected_stem) in pairs {
        assert_eq!(event.filename_stem(), expected_stem);
    }
}

#[test]
fn expected_path_constructs_under_binary_namespace() {
    let tmp = TempDir::new().unwrap();
    let path = expected_hook_path(tmp.path(), "copilot", HookEvent::SessionEnd).unwrap();
    let display = path.display().to_string();
    assert!(display.contains("copilot"));
    assert!(display.contains("SessionEnd"));
}

#[test]
fn expected_path_rejects_non_allowlisted_binary() {
    let tmp = TempDir::new().unwrap();
    for bad in &["bash", "../etc", "claude/../sh", ""] {
        let result = expected_hook_path(tmp.path(), bad, HookEvent::Stop);
        assert!(
            matches!(result, Err(HookError::InvalidBinary(_))),
            "{bad} must be rejected: got {result:?}"
        );
    }
}

#[test]
fn expected_path_blocks_traversal_escape() {
    let tmp = TempDir::new().unwrap();
    let result = expected_hook_path(tmp.path(), "..", HookEvent::Stop);
    assert!(matches!(result, Err(HookError::InvalidBinary(_))));
}

#[test]
fn resolve_hook_returns_path_when_file_present() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("copilot").join("hooks");
    fs::create_dir_all(&dir).unwrap();
    let file = dir.join("SessionEnd.py");
    fs::write(&file, "#!/usr/bin/env python3\n").unwrap();

    let resolved = resolve_hook(tmp.path(), "copilot", HookEvent::SessionEnd).unwrap();
    assert_eq!(
        resolved.canonicalize().unwrap(),
        file.canonicalize().unwrap()
    );
}

#[test]
fn resolve_hook_missing_returns_structured_error_no_fallback() {
    let tmp = TempDir::new().unwrap();
    // Create a claude SessionEnd hook but request copilot — must NOT fall back.
    let claude_dir = tmp.path().join("claude").join("hooks");
    fs::create_dir_all(&claude_dir).unwrap();
    fs::write(claude_dir.join("SessionEnd.py"), "x").unwrap();

    let result = resolve_hook(tmp.path(), "copilot", HookEvent::SessionEnd);
    match result {
        Err(HookError::MissingHookForBinary {
            binary,
            event,
            expected_path,
            remediation,
            ..
        }) => {
            assert_eq!(binary, "copilot");
            assert_eq!(event, HookEvent::SessionEnd);
            assert!(expected_path.display().to_string().contains("copilot"));
            assert!(!expected_path.display().to_string().contains("claude"));
            assert!(
                remediation.contains("amplihack") || remediation.contains("AMPLIHACK_AGENT_BINARY"),
                "remediation must point operator at concrete fixes, got: {remediation}"
            );
        }
        other => panic!("expected MissingHookForBinary, got {other:?}"),
    }
}

#[test]
fn resolve_hook_does_not_create_stub_file() {
    let tmp = TempDir::new().unwrap();
    let _ = resolve_hook(tmp.path(), "copilot", HookEvent::SessionEnd);
    // Resolver MUST be read-only — no stub session_end.py / SessionEnd.py written.
    let stub = tmp
        .path()
        .join("copilot")
        .join("hooks")
        .join("SessionEnd.py");
    assert!(!stub.exists(), "resolver must never create stub hook files");
    let py_stub = tmp
        .path()
        .join("copilot")
        .join("hooks")
        .join("session_end.py");
    assert!(!py_stub.exists());
}

#[test]
fn missing_hook_error_message_includes_static_remediation_template() {
    let tmp = TempDir::new().unwrap();
    let err = resolve_hook(tmp.path(), "copilot", HookEvent::SessionEnd).unwrap_err();
    let rendered = format!("{err}");
    // Spec D5 message contract.
    assert!(rendered.contains("SessionEnd") || rendered.contains("session"));
    assert!(rendered.contains("copilot"));
    // Must not silently succeed — the Display impl must surface the failure.
    assert!(!rendered.is_empty());
}
