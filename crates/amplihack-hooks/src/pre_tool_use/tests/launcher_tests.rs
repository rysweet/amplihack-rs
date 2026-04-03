use super::*;
use crate::test_support::env_lock;
use amplihack_cli::launcher_context::{LauncherContext, write_launcher_context};
use std::collections::BTreeMap;

#[test]
fn unknown_launcher_by_default() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    set_launcher_env(None, None, None, None);
    let result = detect_launcher();
    assert!(
        matches!(result, LauncherType::Unknown),
        "Expected Unknown when no launcher env vars set, got: {result:?}"
    );
}

#[test]
fn detects_copilot_from_persisted_launcher_context() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    set_launcher_env(None, None, None, None);
    let dir = tempfile::tempdir().unwrap();
    write_launcher_context(
        dir.path(),
        LauncherKind::Copilot,
        "amplihack copilot",
        BTreeMap::from([("AMPLIHACK_LAUNCHER".to_string(), "copilot".to_string())]),
    )
    .unwrap();

    let result = detect_launcher_for_dirs(&ProjectDirs::new(dir.path()));

    assert!(matches!(result, LauncherType::Copilot));
}

#[test]
fn ignores_stale_persisted_launcher_context() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    set_launcher_env(None, None, None, None);
    let dir = tempfile::tempdir().unwrap();
    let dirs = ProjectDirs::new(dir.path());
    fs::create_dir_all(&dirs.runtime).unwrap();
    fs::write(
        dirs.launcher_context_file(),
        serde_json::to_string_pretty(&LauncherContext {
            launcher: LauncherKind::Copilot,
            command: "amplihack copilot".to_string(),
            timestamp: "2000-01-01T00:00:00+00:00".to_string(),
            environment: BTreeMap::new(),
        })
        .unwrap(),
    )
    .unwrap();

    let result = detect_launcher_for_dirs(&dirs);

    assert!(matches!(result, LauncherType::Unknown));
}

#[test]
fn inject_context_does_not_panic() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    set_launcher_env(None, None, None, None);
    let dir = tempfile::tempdir().unwrap();
    let dirs = ProjectDirs::new(dir.path());
    let input = serde_json::json!({});
    // inject_context is side-effect-only; verify it completes without panic.
    inject_context(&dirs, &input);
    // The temp dir should still exist (no destructive side effects).
    assert!(
        dir.path().exists(),
        "temp dir should survive inject_context"
    );
}

#[test]
fn inject_context_writes_agents_file_for_copilot() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    set_launcher_env(Some("1"), None, None, None);
    let dir = tempfile::tempdir().unwrap();
    let dirs = ProjectDirs::new(dir.path());
    let input = serde_json::json!({"tool_name": "Bash", "tool_input": {"command": "ls"}});

    inject_context(&dirs, &input);

    let content = fs::read_to_string(dir.path().join("AGENTS.md")).unwrap();
    // Restore env before lock drops so other tests see a clean environment.
    set_launcher_env(None, None, None, None);
    assert!(content.contains(CONTEXT_MARKER_START));
    assert!(content.contains("\"tool_name\": \"Bash\""));
    assert!(content.contains("Copilot CLI (via amplihack)"));
}

#[test]
fn inject_context_replaces_existing_marker_block() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    set_launcher_env(Some("1"), None, None, None);
    let dir = tempfile::tempdir().unwrap();
    let dirs = ProjectDirs::new(dir.path());
    let agents = dir.path().join("AGENTS.md");
    fs::write(
        &agents,
        format!(
            "# Amplihack Agents\n\n{CONTEXT_MARKER_START}\nold\n{CONTEXT_MARKER_END}\n\nkeep me\n"
        ),
    )
    .unwrap();

    inject_context(&dirs, &serde_json::json!({"tool_name": "Read"}));

    let content = fs::read_to_string(&agents).unwrap();
    // Restore env before lock drops so other tests see a clean environment.
    set_launcher_env(None, None, None, None);
    assert_eq!(content.matches(CONTEXT_MARKER_START).count(), 1);
    assert!(content.contains("\"tool_name\": \"Read\""));
    assert!(content.contains("keep me"));
    assert!(!content.contains("\nold\n"));
}

#[test]
fn content_hash_gating_skips_redundant_write() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    set_launcher_env(Some("1"), None, None, None);
    let dir = tempfile::tempdir().unwrap();
    let dirs = ProjectDirs::new(dir.path());
    let input = serde_json::json!({"tool_name": "Bash", "tool_input": {"command": "ls"}});

    // First two calls stabilize content (trailing newline normalization).
    inject_context(&dirs, &input);
    inject_context(&dirs, &input);

    let agents_path = dir.path().join("AGENTS.md");
    let marker_path = dir.path().join(HASH_MARKER_DIR).join(HASH_MARKER_FILE);
    let content_before = fs::read_to_string(&agents_path).unwrap();
    let hash_before = fs::read_to_string(&marker_path).unwrap();
    let mtime_before = fs::metadata(&agents_path).unwrap().modified().unwrap();

    // Small sleep so any real write would have a different mtime.
    std::thread::sleep(std::time::Duration::from_millis(50));

    // Third call with same input: should skip the write.
    inject_context(&dirs, &input);

    let content_after = fs::read_to_string(&agents_path).unwrap();
    let hash_after = fs::read_to_string(&marker_path).unwrap();
    let mtime_after = fs::metadata(&agents_path).unwrap().modified().unwrap();
    set_launcher_env(None, None, None, None);

    assert_eq!(content_before, content_after, "Content should be unchanged");
    assert_eq!(hash_before, hash_after, "Hash marker should be unchanged");
    assert_eq!(mtime_before, mtime_after, "File should not have been rewritten");
}

#[test]
fn content_hash_gating_writes_on_changed_content() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    set_launcher_env(Some("1"), None, None, None);
    let dir = tempfile::tempdir().unwrap();
    let dirs = ProjectDirs::new(dir.path());

    // First call + stabilization call.
    let input1 = serde_json::json!({"tool_name": "Bash", "tool_input": {"command": "ls"}});
    inject_context(&dirs, &input1);
    inject_context(&dirs, &input1);
    let marker_path = dir.path().join(HASH_MARKER_DIR).join(HASH_MARKER_FILE);
    let hash1 = fs::read_to_string(&marker_path).unwrap();

    // Call with different input: should write.
    let input2 = serde_json::json!({"tool_name": "Read", "tool_input": {"path": "/a"}});
    inject_context(&dirs, &input2);
    let hash2 = fs::read_to_string(&marker_path).unwrap();
    let content = fs::read_to_string(dir.path().join("AGENTS.md")).unwrap();
    set_launcher_env(None, None, None, None);

    assert_ne!(hash1, hash2, "Hash should change with different content");
    assert!(content.contains("\"tool_name\": \"Read\""));
}
