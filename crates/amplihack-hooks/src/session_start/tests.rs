//! Integration tests for the session start hook process() method.

use super::*;
use crate::test_support::env_lock;
use std::fs;

fn generate_session_id() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    format!("session-{}", now.as_secs())
}

#[test]
fn handles_unknown_events() {
    let hook = SessionStartHook;
    let result = hook.process(HookInput::Unknown).unwrap();
    assert!(result.as_object().unwrap().is_empty());
}

#[test]
fn generate_session_id_format() {
    let id = generate_session_id();
    assert!(id.starts_with("session-"));
}

#[test]
fn session_start_captures_original_request_context() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let original = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();
    unsafe { std::env::set_var("AMPLIHACK_BLARIFY_MODE", "skip") };

    let hook = SessionStartHook;
    let result = hook
        .process(HookInput::SessionStart {
            session_id: Some("test-session".to_string()),
            cwd: None,
            extra: serde_json::json!({
                "prompt": "Implement complete hook parity. Do not regress tests. Ensure every user-visible hook output matches Python."
            }),
        })
        .unwrap();

    unsafe { std::env::remove_var("AMPLIHACK_BLARIFY_MODE") };
    let _ = std::env::set_current_dir(&original);

    let context = result["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(context.contains("## 🎯 ORIGINAL USER REQUEST - PRESERVE THESE REQUIREMENTS"));
    assert!(context.contains("**Constraints**:"));
    assert!(context.contains("**Success Criteria**:"));
    assert!(
        dir.path()
            .join(".claude/runtime/logs/test-session/ORIGINAL_REQUEST.md")
            .exists()
    );
    assert!(
        dir.path()
            .join(".claude/runtime/logs/test-session/original_request.json")
            .exists()
    );
}

#[test]
fn session_start_process_surfaces_code_graph_context_failure_notice() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let original = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();

    let broken_db = dir.path().join("broken-graph-db");
    fs::write(&broken_db, "not a graph db").unwrap();
    unsafe {
        std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", &broken_db);
        std::env::remove_var("AMPLIHACK_KUZU_DB_PATH");
        std::env::set_var("AMPLIHACK_BLARIFY_MODE", "skip");
    }

    let hook = SessionStartHook;
    let result = hook
        .process(HookInput::SessionStart {
            session_id: Some("test-session".to_string()),
            cwd: Some(dir.path().to_path_buf()),
            extra: Value::Object(serde_json::Map::new()),
        })
        .unwrap();

    let _ = std::env::set_current_dir(&original);
    unsafe {
        std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH");
        std::env::remove_var("AMPLIHACK_KUZU_DB_PATH");
        std::env::remove_var("AMPLIHACK_BLARIFY_MODE");
    }

    let context = result["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .expect("additional context");
    assert!(context.contains("## Code Graph Status"));
    assert!(context.contains("Code-graph context unavailable"));

    let warnings = result["warnings"]
        .as_array()
        .expect("warnings array expected");
    assert!(warnings.iter().any(|warning| {
        warning
            .as_str()
            .unwrap_or("")
            .contains("Code-graph context unavailable")
    }));
}

#[test]
fn session_start_process_surfaces_legacy_graph_env_alias_notice() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let original = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();

    let legacy_override = dir.path().join("legacy-graph-db");
    unsafe {
        std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH");
        std::env::set_var("AMPLIHACK_KUZU_DB_PATH", &legacy_override);
        std::env::set_var("AMPLIHACK_BLARIFY_MODE", "skip");
    }

    let hook = SessionStartHook;
    let result = hook
        .process(HookInput::SessionStart {
            session_id: Some("test-session".to_string()),
            cwd: Some(dir.path().to_path_buf()),
            extra: Value::Object(serde_json::Map::new()),
        })
        .unwrap();

    let _ = std::env::set_current_dir(&original);
    unsafe {
        std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH");
        std::env::remove_var("AMPLIHACK_KUZU_DB_PATH");
        std::env::remove_var("AMPLIHACK_BLARIFY_MODE");
    }

    let context = result["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .expect("additional context");
    assert!(context.contains("## Code Graph Status"));
    assert!(context.contains("AMPLIHACK_KUZU_DB_PATH"));
    assert!(context.contains("AMPLIHACK_GRAPH_DB_PATH"));
}

#[test]
fn session_start_process_surfaces_legacy_graph_store_notice() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let original = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();

    let legacy_store = dir.path().join(".amplihack").join("kuzu_db");
    fs::create_dir_all(&legacy_store).unwrap();
    unsafe {
        std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH");
        std::env::remove_var("AMPLIHACK_KUZU_DB_PATH");
        std::env::set_var("AMPLIHACK_BLARIFY_MODE", "skip");
    }

    let hook = SessionStartHook;
    let result = hook
        .process(HookInput::SessionStart {
            session_id: Some("test-session".to_string()),
            cwd: Some(dir.path().to_path_buf()),
            extra: Value::Object(serde_json::Map::new()),
        })
        .unwrap();

    let _ = std::env::set_current_dir(&original);
    unsafe {
        std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH");
        std::env::remove_var("AMPLIHACK_KUZU_DB_PATH");
        std::env::remove_var("AMPLIHACK_BLARIFY_MODE");
    }

    let context = result["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .expect("additional context");
    assert!(context.contains("## Code Graph Status"));
    assert!(context.contains(".amplihack/kuzu_db"));
    assert!(context.contains("graph_db"));
}

#[test]
fn session_start_process_surfaces_legacy_memory_env_alias_notice() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let original = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();

    let legacy_override = dir.path().join("legacy-memory-db");
    unsafe {
        std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH");
        std::env::set_var("AMPLIHACK_KUZU_DB_PATH", &legacy_override);
        std::env::set_var("AMPLIHACK_BLARIFY_MODE", "skip");
    }

    let hook = SessionStartHook;
    let result = hook
        .process(HookInput::SessionStart {
            session_id: Some("test-session".to_string()),
            cwd: Some(dir.path().to_path_buf()),
            extra: Value::Object(serde_json::Map::new()),
        })
        .unwrap();

    let _ = std::env::set_current_dir(&original);
    unsafe {
        std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH");
        std::env::remove_var("AMPLIHACK_KUZU_DB_PATH");
        std::env::remove_var("AMPLIHACK_BLARIFY_MODE");
    }

    let context = result["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .expect("additional context");
    assert!(context.contains("## Memory Store Status"));
    assert!(context.contains("AMPLIHACK_KUZU_DB_PATH"));
    assert!(context.contains("AMPLIHACK_GRAPH_DB_PATH"));
}

#[test]
fn session_start_process_surfaces_legacy_memory_store_notice() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let original = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();

    let legacy_store = home.path().join(".amplihack").join("memory_kuzu.db");
    fs::create_dir_all(legacy_store.parent().unwrap()).unwrap();
    fs::write(&legacy_store, "legacy-memory").unwrap();
    let previous_home = std::env::var_os("HOME");
    unsafe {
        std::env::set_var("HOME", home.path());
        std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH");
        std::env::remove_var("AMPLIHACK_KUZU_DB_PATH");
        std::env::set_var("AMPLIHACK_BLARIFY_MODE", "skip");
    }

    let hook = SessionStartHook;
    let result = hook
        .process(HookInput::SessionStart {
            session_id: Some("test-session".to_string()),
            cwd: Some(dir.path().to_path_buf()),
            extra: Value::Object(serde_json::Map::new()),
        })
        .unwrap();

    let _ = std::env::set_current_dir(&original);
    match previous_home {
        Some(value) => unsafe { std::env::set_var("HOME", value) },
        None => unsafe { std::env::remove_var("HOME") },
    }
    unsafe {
        std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH");
        std::env::remove_var("AMPLIHACK_KUZU_DB_PATH");
        std::env::remove_var("AMPLIHACK_BLARIFY_MODE");
    }

    let context = result["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .expect("additional context");
    assert!(context.contains("## Memory Store Status"));
    assert!(context.contains("memory_kuzu.db"));
    assert!(context.contains("memory_graph.db"));
}

#[test]
fn session_start_process_surfaces_blarify_setup_failure_notice() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/app.py"), "print('hi')\n").unwrap();
    let original = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();
    unsafe {
        std::env::set_var(
            "AMPLIHACK_AMPLIHACK_BINARY_PATH",
            dir.path().join("missing-amplihack"),
        );
        std::env::set_var("AMPLIHACK_BLARIFY_MODE", "background");
    }

    let hook = SessionStartHook;
    let result = hook
        .process(HookInput::SessionStart {
            session_id: Some("test-session".to_string()),
            cwd: Some(dir.path().to_path_buf()),
            extra: Value::Object(serde_json::Map::new()),
        })
        .unwrap();

    let _ = std::env::set_current_dir(&original);
    unsafe {
        std::env::remove_var("AMPLIHACK_AMPLIHACK_BINARY_PATH");
        std::env::remove_var("AMPLIHACK_BLARIFY_MODE");
    }

    let context = result["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .expect("additional context");
    assert!(context.contains("## Code Graph Status"));
    assert!(context.contains("Code-graph setup failed"));

    // AC: When the code-graph setup (binary lookup) fails, HookOutput must contain
    // a non-empty `warnings` field — the failure must not be silently swallowed.
    let warnings = result["warnings"]
        .as_array()
        .expect("HookOutput must have a 'warnings' array when blarify setup fails");
    assert!(
        !warnings.is_empty(),
        "warnings array must be non-empty on setup failure"
    );
    assert!(
        warnings
            .iter()
            .any(|w| w.as_str().unwrap_or("").contains("Code-graph setup failed")),
        "at least one warning must mention the setup failure"
    );

    // AC: indexing_status must be present and carry an error value.
    let status = result["hookSpecificOutput"]["indexing_status"]
        .as_str()
        .expect("indexing_status must be present in hookSpecificOutput");
    assert!(
        status.starts_with("error:"),
        "indexing_status must start with 'error:' on setup failure, got: {status}"
    );
}

#[test]
fn session_start_process_always_emits_indexing_status() {
    // AC: indexing_status must be present in hookSpecificOutput even when
    // there is no additionalContext (empty project directory).
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let original = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();
    // Disable blarify so we get a deterministic "complete" status.
    unsafe {
        std::env::set_var("AMPLIHACK_BLARIFY_MODE", "skip");
    }

    let hook = SessionStartHook;
    let result = hook
        .process(HookInput::SessionStart {
            session_id: Some("status-test".to_string()),
            cwd: Some(dir.path().to_path_buf()),
            extra: Value::Object(serde_json::Map::new()),
        })
        .unwrap();

    let _ = std::env::set_current_dir(&original);
    unsafe {
        std::env::remove_var("AMPLIHACK_BLARIFY_MODE");
    }

    let status = result["hookSpecificOutput"]["indexing_status"]
        .as_str()
        .expect("indexing_status must always be present in hookSpecificOutput");
    assert!(
        status == "started" || status == "complete" || status.starts_with("error:"),
        "indexing_status must be 'started', 'complete', or 'error:<reason>', got: {status}"
    );
}
