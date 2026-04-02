//! Tests for user prompt submit hook.

use super::*;
use crate::agent_memory::{detect_agent_references, detect_slash_command_agent};
use crate::post_tool_use::PostToolUseHook;
use crate::protocol::Hook;
use crate::test_support::env_lock;
use amplihack_cli::memory::PromptContextMemory;
use amplihack_types::{HookInput, ProjectDirs};
use serde_json::Value;
use std::fs;

#[test]
fn extract_prefs_from_table() {
    let content = r#"
| Setting | Value |
| --- | --- |
| Verbosity | balanced |
| Style | casual |
"#;
    let prefs = extract_preferences(content);
    assert_eq!(prefs.len(), 2);
    assert_eq!(prefs[0], ("Verbosity".to_string(), "balanced".to_string()));
    assert_eq!(prefs[1], ("Style".to_string(), "casual".to_string()));
}

#[test]
fn extract_prefs_from_headers() {
    let content = r#"
## Preferences

### Verbosity
balanced

### Style
casual and direct
"#;
    let prefs = extract_preferences(content);
    assert_eq!(prefs.len(), 2);
    assert_eq!(prefs[0], ("Verbosity".to_string(), "balanced".to_string()));
    assert_eq!(
        prefs[1],
        ("Style".to_string(), "casual and direct".to_string())
    );
}

#[test]
fn extract_prefs_skips_header() {
    let content = "| Setting | Value |\n| --- | --- |";
    let prefs = extract_preferences(content);
    assert!(prefs.is_empty());
}

#[test]
fn build_context() {
    let prefs = vec![("Key".to_string(), "Value".to_string())];
    let ctx = build_preference_context(&prefs);
    assert!(ctx.contains("**Key**: Value"));
}

#[test]
fn detects_explicit_agent_references() {
    let agents = detect_agent_references(
        "Please inspect @.claude/agents/amplihack/core/architect.md for this task",
    );
    assert_eq!(agents, vec!["architect".to_string()]);
}

#[test]
fn detects_slash_command_agent() {
    assert_eq!(
        detect_slash_command_agent("/analyze auth flow"),
        Some("analyzer")
    );
    assert_eq!(
        detect_slash_command_agent("please /analyze auth flow"),
        None
    );
}

#[test]
fn formats_agent_memory_context() {
    let context = format_agent_memory_context(
        &[String::from("analyzer")],
        &[PromptContextMemory {
            content: String::from("Fix CI by running cargo fmt before push."),
            code_context: Some(String::from(
                "**Related Files:**\n- src/example/module.py (python)",
            )),
        }],
    );
    assert!(context.contains("## Memory for analyzer Agent"));
    assert!(context.contains("Fix CI by running cargo fmt before push."));
    assert!(context.contains("relevance: 0.00"));
    assert!(context.contains("**Related Files:**"));
    assert!(context.contains("src/example/module.py"));
}

#[test]
fn handles_unknown_events() {
    let hook = UserPromptSubmitHook;
    let result = hook.process(HookInput::Unknown).unwrap();
    assert!(result.as_object().unwrap().is_empty());
}

#[test]
fn returns_additional_context_without_mutating_prompt() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let original = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();

    let hook = UserPromptSubmitHook;
    let result = hook
        .process(HookInput::UserPromptSubmit {
            user_prompt: Some("/dev continue parity audit".to_string()),
            session_id: Some("test-session".to_string()),
            extra: Value::Null,
        })
        .unwrap();

    let _ = std::env::set_current_dir(&original);

    assert_eq!(
        result["hookSpecificOutput"]["hookEventName"],
        "UserPromptSubmit"
    );
    let context = result["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(context.contains("/dev workflow detected"));
    assert!(
        result["hookSpecificOutput"]
            .get("userPromptContent")
            .is_none()
    );
}

#[test]
fn detects_python_parity_dev_variants_case_insensitively() {
    assert!(preferences::is_dev_invocation("/DEV implement caching"));
    assert!(preferences::is_dev_invocation(
        "/Dev add logging middleware"
    ));
    assert!(preferences::is_dev_invocation(
        "/amplihack:dev continue parity"
    ));
    assert!(preferences::is_dev_invocation(
        "Please use dev-orchestrator for this"
    ));
    assert!(!preferences::is_dev_invocation("/review this change"));
}

#[test]
fn extracts_prompt_from_extra_prompt_key() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let original = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();

    let hook = UserPromptSubmitHook;
    let result = hook
        .process(HookInput::UserPromptSubmit {
            user_prompt: None,
            session_id: Some("prompt-extra".to_string()),
            extra: serde_json::json!({ "prompt": "/dev continue parity audit" }),
        })
        .unwrap();

    let _ = std::env::set_current_dir(&original);

    assert!(
        result["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .contains("/dev workflow detected")
    );
}

#[test]
fn extracts_prompt_from_user_message_dict() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let original = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();

    let hook = UserPromptSubmitHook;
    let result = hook
        .process(HookInput::UserPromptSubmit {
            user_prompt: None,
            session_id: Some("message-dict".to_string()),
            extra: serde_json::json!({
                "userMessage": { "text": "/Dev implement auth", "metadata": { "source": "cli" } }
            }),
        })
        .unwrap();

    let _ = std::env::set_current_dir(&original);

    assert!(
        result["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .contains("/dev workflow detected")
    );
}

#[test]
fn dev_prompt_initializes_workflow_enforcement_state_and_warning_path() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let original = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();

    let hook = UserPromptSubmitHook;
    let result = hook
        .process(HookInput::UserPromptSubmit {
            user_prompt: Some("/dev continue parity audit".to_string()),
            session_id: Some("workflow-session".to_string()),
            extra: Value::Null,
        })
        .unwrap();

    let workflow_state = dir
        .path()
        .join(".claude/runtime/workflow_state/workflow-session.json");
    assert!(workflow_state.exists());
    assert!(
        result["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .contains("/dev workflow detected")
    );

    let post_tool_use = PostToolUseHook;
    for path in ["src/main.rs", "src/lib.rs"] {
        let result = post_tool_use
            .process(HookInput::PostToolUse {
                tool_name: "Read".to_string(),
                tool_input: serde_json::json!({ "path": path }),
                tool_result: None,
                session_id: Some("workflow-session".to_string()),
            })
            .unwrap();
        assert!(result.as_object().unwrap().get("warnings").is_none());
    }

    let warning = post_tool_use
        .process(HookInput::PostToolUse {
            tool_name: "Read".to_string(),
            tool_input: serde_json::json!({ "path": "src/extra.rs" }),
            tool_result: None,
            session_id: Some("workflow-session".to_string()),
        })
        .unwrap();

    let _ = std::env::set_current_dir(&original);

    assert!(
        warning["warnings"][0]
            .as_str()
            .unwrap()
            .contains("WORKFLOW BYPASS DETECTED")
    );
}

#[test]
fn load_user_preferences_uses_amplihack_root_override() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let project = tempfile::tempdir().unwrap();
    let framework = tempfile::tempdir().unwrap();
    fs::create_dir_all(framework.path().join(".claude/context")).unwrap();
    fs::write(
        framework.path().join(".claude/context/USER_PREFERENCES.md"),
        "| Setting | Value |\n| --- | --- |\n| Verbosity | concise |\n",
    )
    .unwrap();
    let previous = std::env::var_os("AMPLIHACK_ROOT");
    unsafe { std::env::set_var("AMPLIHACK_ROOT", framework.path()) };

    let (context, has_learned_patterns) =
        preferences::load_user_preferences_with_patterns(&ProjectDirs::new(project.path()));

    match previous {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_ROOT", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_ROOT") },
    }

    assert!(!has_learned_patterns);
    assert!(context.unwrap().contains("**Verbosity**: concise"));
}
