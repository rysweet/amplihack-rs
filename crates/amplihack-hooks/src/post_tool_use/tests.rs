use super::*;
use crate::test_support::env_lock;

#[test]
fn categorizes_tools_correctly() {
    assert_eq!(categorize_tool("Bash"), "bash_commands");
    assert_eq!(categorize_tool("Write"), "file_operations");
    assert_eq!(categorize_tool("Edit"), "file_operations");
    assert_eq!(categorize_tool("grep"), "search_operations");
    assert_eq!(categorize_tool("CustomTool"), "other");
}

#[test]
fn validates_edit_errors() {
    let result = serde_json::json!({"error": "file not found"});
    let warning = validate_tool_result("Edit", Some(&result));
    assert!(warning.is_some());
    assert!(warning.unwrap().contains("file not found"));
}

#[test]
fn validates_success_false() {
    let result = serde_json::json!({"success": false, "message": "permission denied"});
    let warning = validate_tool_result("Write", Some(&result));
    assert!(warning.is_some());
    assert!(warning.unwrap().contains("permission denied"));
}

#[test]
fn no_warning_on_success() {
    let result = serde_json::json!({"success": true});
    assert!(validate_tool_result("Edit", Some(&result)).is_none());
}

#[test]
fn no_warning_for_bash() {
    let result = serde_json::json!({"error": "something"});
    assert!(validate_tool_result("Bash", Some(&result)).is_none());
}

#[test]
fn is_code_file_detects_known_extensions() {
    assert!(is_code_file("src/main.rs"));
    assert!(is_code_file("app/module.py"));
    assert!(is_code_file("index.ts"));
    assert!(is_code_file("Component.tsx"));
    assert!(!is_code_file("README.md"));
    assert!(!is_code_file("config.yaml"));
    assert!(!is_code_file("image.png"));
}

#[test]
fn is_code_file_case_insensitive() {
    assert!(is_code_file("Main.RS"));
    assert!(is_code_file("App.PY"));
}

#[test]
fn extract_written_paths_write_tool() {
    let input = serde_json::json!({"path": "src/main.rs", "content": "fn main() {}"});
    let paths = extract_written_paths("Write", &input);
    assert_eq!(paths, vec!["src/main.rs"]);
}

#[test]
fn extract_written_paths_edit_tool_file_path() {
    let input =
        serde_json::json!({"file_path": "src/lib.rs", "old_string": "a", "new_string": "b"});
    let paths = extract_written_paths("Edit", &input);
    assert_eq!(paths, vec!["src/lib.rs"]);
}

#[test]
fn extract_written_paths_multiedit_tool() {
    let input = serde_json::json!({
        "edits": [
            {"file_path": "src/a.rs", "old_string": "a", "new_string": "b"},
            {"file_path": "src/b.rs", "old_string": "c", "new_string": "d"},
        ]
    });
    let paths = extract_written_paths("MultiEdit", &input);
    assert_eq!(paths, vec!["src/a.rs", "src/b.rs"]);
}

#[test]
fn extract_written_paths_bash_returns_empty() {
    let input = serde_json::json!({"command": "ls"});
    let paths = extract_written_paths("Bash", &input);
    assert!(paths.is_empty());
}

#[test]
fn blarify_stale_marker_written_for_code_file_edit() {
    // cwd is process-global state; hold env_lock to prevent races with
    // other tests that also call set_current_dir() in parallel.
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().unwrap();
    // Temporarily change cwd for ProjectDirs resolution.
    let original = std::env::current_dir().ok();
    let _ = std::env::set_current_dir(dir.path());

    let input = serde_json::json!({
        "file_path": "src/main.rs",
        "old_string": "foo",
        "new_string": "bar",
    });
    mark_blarify_stale_if_needed("Edit", &input);

    if let Some(orig) = original {
        let _ = std::env::set_current_dir(orig);
    }

    let marker = dir.path().join(".amplihack").join("blarify_stale");
    assert!(marker.exists(), "blarify_stale marker should be written");
    let content = fs::read_to_string(&marker).unwrap();
    let parsed: Value = serde_json::from_str(&content).unwrap();
    assert_eq!(parsed["stale"], true);
    assert_eq!(parsed["tool"], "Edit");
    assert_eq!(parsed["reason"], "code_file_modified");
}

#[test]
fn blarify_stale_marker_not_written_for_non_code_file() {
    // cwd is process-global state; hold env_lock to prevent races with
    // other tests that also call set_current_dir() in parallel.
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let original = std::env::current_dir().ok();
    let _ = std::env::set_current_dir(dir.path());

    let input = serde_json::json!({
        "file_path": "docs/README.md",
        "old_string": "a",
        "new_string": "b",
    });
    mark_blarify_stale_if_needed("Edit", &input);

    if let Some(orig) = original {
        let _ = std::env::set_current_dir(orig);
    }

    let marker = dir.path().join(".amplihack").join("blarify_stale");
    assert!(
        !marker.exists(),
        "blarify_stale marker should NOT be written for non-code files"
    );
}

#[test]
fn allows_all_tools() {
    let hook = PostToolUseHook;
    let input = HookInput::PostToolUse {
        tool_name: "Bash".to_string(),
        tool_input: serde_json::json!({"command": "ls"}),
        tool_result: None,
        session_id: None,
    };
    let result = hook.process(input).unwrap();
    assert!(result.as_object().unwrap().is_empty());
}

#[test]
fn handles_unknown_events() {
    let hook = PostToolUseHook;
    let result = hook.process(HookInput::Unknown).unwrap();
    assert!(result.as_object().unwrap().is_empty());
}

#[test]
fn dev_skill_invocation_starts_workflow_tracking() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let original = std::env::current_dir().ok();
    let _ = std::env::set_current_dir(dir.path());

    let warning = update_workflow_enforcement(
        "Skill",
        &serde_json::json!({"skill": "dev-orchestrator"}),
        Some("session-1"),
    );
    let state = read_workflow_state(&ProjectDirs::from_cwd(), Some("session-1"));

    if let Some(orig) = original {
        let _ = std::env::set_current_dir(orig);
    }

    assert!(warning.is_none());
    assert!(state.is_some());
    assert_eq!(state.unwrap().tool_calls_since, 0);
}

#[test]
fn workflow_evidence_clears_tracking_state() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let original = std::env::current_dir().ok();
    let _ = std::env::set_current_dir(dir.path());
    let dirs = ProjectDirs::from_cwd();
    write_workflow_state(
        &dirs,
        Some("session-1"),
        &WorkflowEnforcementState {
            dev_invoked_at: 1,
            tool_calls_since: 1,
            warning_emitted: false,
        },
    )
    .unwrap();

    let warning = update_workflow_enforcement(
        "Bash",
        &serde_json::json!({"command": "PYTHONPATH=src python3 -c 'from amplihack.recipes import run_recipe_by_name'"}),
        Some("session-1"),
    );
    let state = read_workflow_state(&dirs, Some("session-1"));

    if let Some(orig) = original {
        let _ = std::env::set_current_dir(orig);
    }

    assert!(warning.is_none());
    assert!(state.is_none());
}

#[test]
fn workflow_bypass_warning_fires_once_at_threshold() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let original = std::env::current_dir().ok();
    let _ = std::env::set_current_dir(dir.path());
    let hook = PostToolUseHook;

    hook.process(HookInput::PostToolUse {
        tool_name: "Skill".to_string(),
        tool_input: serde_json::json!({"skill": "dev-orchestrator"}),
        tool_result: None,
        session_id: Some("session-1".to_string()),
    })
    .unwrap();

    for _ in 0..(TOOL_CALL_THRESHOLD - 1) {
        let result = hook
            .process(HookInput::PostToolUse {
                tool_name: "View".to_string(),
                tool_input: serde_json::json!({"path": "/tmp/random.txt"}),
                tool_result: None,
                session_id: Some("session-1".to_string()),
            })
            .unwrap();
        assert!(result.as_object().unwrap().is_empty());
    }

    let warning_result = hook
        .process(HookInput::PostToolUse {
            tool_name: "View".to_string(),
            tool_input: serde_json::json!({"path": "/tmp/random.txt"}),
            tool_result: None,
            session_id: Some("session-1".to_string()),
        })
        .unwrap();
    let repeat_result = hook
        .process(HookInput::PostToolUse {
            tool_name: "View".to_string(),
            tool_input: serde_json::json!({"path": "/tmp/random.txt"}),
            tool_result: None,
            session_id: Some("session-1".to_string()),
        })
        .unwrap();

    if let Some(orig) = original {
        let _ = std::env::set_current_dir(orig);
    }

    let warnings = warning_result["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1);
    assert!(
        warnings[0]
            .as_str()
            .unwrap()
            .contains("WORKFLOW BYPASS DETECTED")
    );
    assert_eq!(
        warning_result["metadata"]["tool_calls_without_evidence"],
        TOOL_CALL_THRESHOLD
    );
    assert!(repeat_result.as_object().unwrap().is_empty());
}
