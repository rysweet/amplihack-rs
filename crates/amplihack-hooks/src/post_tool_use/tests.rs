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

/// The project key of the blarify marker: the first 16 lowercase hex
/// characters of the SHA-256 of the canonicalised cwd (issue #1963).
fn blarify_project_key(project: &std::path::Path) -> String {
    use sha2::{Digest, Sha256};
    let canonical = project
        .canonicalize()
        .unwrap_or_else(|_| project.to_path_buf());
    let digest = Sha256::digest(canonical.as_os_str().as_encoded_bytes());
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

fn run_post_tool_use(tool_name: &str, tool_input: Value, session_id: Option<&str>) -> Value {
    PostToolUseHook
        .process(HookInput::PostToolUse {
            tool_name: tool_name.to_string(),
            tool_input,
            tool_result: None,
            session_id: session_id.map(str::to_string),
        })
        .unwrap()
}

#[test]
fn blarify_stale_marker_written_for_code_file_edit() {
    // cwd and HOME are process-global state; hold env_lock to prevent races
    // with other tests that change them in parallel.
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let _state = crate::test_support::isolate_state_home(home.path());
    let cwd = crate::test_support::CwdGuard::set(dir.path()).unwrap();

    let input = serde_json::json!({
        "file_path": "src/main.rs",
        "old_string": "foo",
        "new_string": "bar",
    });
    run_post_tool_use("Edit", input, Some("blarify-1"));

    drop(cwd);

    let marker = home
        .path()
        .join(".amplihack/runtime/projects")
        .join(blarify_project_key(dir.path()))
        .join("blarify_stale");
    assert!(marker.exists(), "blarify_stale marker should be written");
    let content = fs::read_to_string(&marker).unwrap();
    let parsed: Value = serde_json::from_str(&content).unwrap();
    assert_eq!(parsed["stale"], true);
    assert_eq!(parsed["tool"], "Edit");
    assert_eq!(parsed["reason"], "code_file_modified");
    assert!(
        !dir.path().join(".amplihack").exists(),
        "the marker must not be written under the project"
    );
}

#[test]
fn blarify_stale_marker_not_written_for_non_code_file() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let _state = crate::test_support::isolate_state_home(home.path());
    let cwd = crate::test_support::CwdGuard::set(dir.path()).unwrap();

    let input = serde_json::json!({
        "file_path": "docs/README.md",
        "old_string": "a",
        "new_string": "b",
    });
    run_post_tool_use("Edit", input, Some("blarify-2"));

    drop(cwd);

    assert!(
        !home.path().join(".amplihack/runtime/projects").exists(),
        "blarify_stale marker should NOT be written for non-code files"
    );
    assert!(!dir.path().join(".amplihack").exists());
}

#[test]
fn allows_all_tools() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let _state = crate::test_support::isolate_state_home(home.path());
    let cwd = crate::test_support::CwdGuard::set(dir.path()).unwrap();

    let result = run_post_tool_use("Bash", serde_json::json!({"command": "ls"}), None);

    drop(cwd);

    assert!(result.as_object().unwrap().is_empty());
    assert!(
        fs::read_dir(dir.path()).unwrap().next().is_none(),
        "post-tool-use must not write into the cwd"
    );
}

#[test]
fn handles_unknown_events() {
    let hook = PostToolUseHook;
    let result = hook.process(HookInput::Unknown).unwrap();
    assert!(result.as_object().unwrap().is_empty());
}

/// `<home>/.amplihack/runtime/sessions/<sid>/workflow_state.json`.
fn workflow_state_path(home: &std::path::Path, session: &str) -> std::path::PathBuf {
    home.join(".amplihack/runtime/sessions")
        .join(session)
        .join("workflow_state.json")
}

#[test]
fn dev_skill_invocation_starts_workflow_tracking() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let _state = crate::test_support::isolate_state_home(home.path());
    let cwd = crate::test_support::CwdGuard::set(dir.path()).unwrap();

    let result = run_post_tool_use(
        "Skill",
        serde_json::json!({"skill": "dev-orchestrator"}),
        Some("session-1"),
    );

    drop(cwd);

    assert!(result.get("warnings").is_none());
    let raw = fs::read_to_string(workflow_state_path(home.path(), "session-1"))
        .expect("workflow state is written under the state root");
    let state: WorkflowEnforcementState = serde_json::from_str(&raw).unwrap();
    assert_eq!(state.tool_calls_since, 0);
    assert!(!dir.path().join(".claude").exists());
}

#[test]
fn workflow_evidence_clears_tracking_state() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let _state = crate::test_support::isolate_state_home(home.path());
    let cwd = crate::test_support::CwdGuard::set(dir.path()).unwrap();
    let state_path = workflow_state_path(home.path(), "session-1");
    fs::create_dir_all(state_path.parent().unwrap()).unwrap();
    fs::write(
        &state_path,
        serde_json::to_vec(&WorkflowEnforcementState {
            dev_invoked_at: 1,
            tool_calls_since: 1,
            warning_emitted: false,
        })
        .unwrap(),
    )
    .unwrap();

    let result = run_post_tool_use(
        "Bash",
        serde_json::json!({"command": "amplihack recipe run smart-orchestrator -c task_description='fix bug' -c repo_path=."}),
        Some("session-1"),
    );

    drop(cwd);

    assert!(result.get("warnings").is_none());
    assert!(!state_path.exists(), "evidence clears the workflow state");
}

#[test]
fn workflow_bypass_warning_fires_once_at_threshold() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let _state = crate::test_support::isolate_state_home(home.path());
    let cwd = crate::test_support::CwdGuard::set(dir.path()).unwrap();
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

    drop(cwd);

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
    assert!(!dir.path().join(".claude").exists());
}
