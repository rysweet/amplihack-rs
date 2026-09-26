//! Tests for user prompt submit hook.

use super::*;
use crate::agent_memory::{detect_agent_references, detect_slash_command_agent};
use crate::post_tool_use::PostToolUseHook;
use crate::protocol::Hook;
use crate::session_start::is_workflow_active;
use crate::test_support::{EnvVarGuard, env_lock};
use amplihack_memory::cli_memory::PromptContextMemory;
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
fn build_context_renders_selected_workflow_as_canonical_skill_recipe() {
    let prefs = vec![(
        "Selected".to_string(),
        "DEFAULT_WORKFLOW (`@~/.amplihack/.claude/workflows/DEFAULT_WORKFLOW.md`)".to_string(),
    )];

    let ctx = build_preference_context(&prefs);

    assert!(ctx.contains("- **Selected**: `default-workflow` skill/recipe"));
    assert!(!ctx.contains("DEFAULT_WORKFLOW.md"));
    assert!(!ctx.contains(".claude/workflow/DEFAULT_WORKFLOW.md"));
    assert!(!ctx.contains(".claude/workflows/DEFAULT_WORKFLOW.md"));
}

#[test]
fn build_context_preserves_unrelated_preferences_without_workflow_rewrite() {
    let prefs = vec![
        ("Verbosity".to_string(), "balanced".to_string()),
        ("Consensus Depth".to_string(), "balanced".to_string()),
    ];

    let ctx = build_preference_context(&prefs);

    assert!(ctx.contains("- **Verbosity**: balanced"));
    assert!(ctx.contains("- **Consensus Depth**: balanced"));
    assert!(!ctx.contains("default-workflow"));
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
        "/analyze why CI fails on cargo fmt",
        &[String::from("analyzer")],
        &[PromptContextMemory {
            content: String::from("Fix CI by running cargo fmt before push."),
            code_context: Some(String::from(
                "**Related Files:**\n- src/example/module.py (python)",
            )),
        }],
    )
    .expect("relevant memory is injected");
    assert!(context.contains("## Relevant Memory (agents: analyzer)"));
    assert!(context.contains("Fix CI by running cargo fmt before push."));
    assert!(!context.contains("relevance: 0.00"));
    assert!(context.contains("**Related Files:**"));
    assert!(context.contains("src/example/module.py"));
}

/// Issue #1483: words and path segments from a prompt are not agents.
#[test]
fn ordinary_words_and_paths_are_not_agent_references() {
    for prompt in [
        "look at the files in /skills /web /docs /commands /plugin and /bin today",
        "check ~/.amplihack/bin/amplihack-hook and /amplihack-recipe-runner output",
        "search amaz /amaz products",
        "reply with just: pong",
    ] {
        assert!(
            detect_agent_references(prompt).is_empty(),
            "no agent in {prompt:?}: {:?}",
            detect_agent_references(prompt)
        );
    }
    assert_eq!(
        detect_agent_references("run /analyzer on /skills and /builder here"),
        vec!["analyzer".to_string(), "builder".to_string()]
    );
}

/// Issue #1483: the stored "pong" smoke-test memory is unrelated to the
/// prompt and must not be injected, once per agent or at all.
#[test]
fn unrelated_smoke_test_memory_is_not_injected() {
    let agents = ["analyzer", "builder", "reviewer"].map(String::from);
    // Shaped as session-stop stores it: `Agent <name>: <transcript>`.
    let memories = [PromptContextMemory {
        content: String::from("Agent general: user: reply with just: pong\n\nassistant: pong"),
        code_context: None,
    }];
    for prompt in [
        "update the skills under docs and web, then rebuild bin",
        "/analyze the amplihack-hook plugin commands",
        "/analyze the builder agent output",
    ] {
        assert_eq!(
            format_agent_memory_context(prompt, &agents, &memories),
            None,
            "nothing is injected for {prompt:?}"
        );
    }
}

/// A mid-prompt slash command still names its agent.
#[test]
fn midprompt_slash_command_agents_are_detected() {
    assert_eq!(
        detect_agent_references("please /reflect on this session"),
        vec!["reflection".to_string()]
    );
    assert_eq!(
        detect_agent_references("then /ultrathink about it"),
        vec!["orchestrator".to_string()]
    );
}

/// Issue #1483 end to end: learnings stored by session-stop under several
/// agents reach the prompt once when relevant, and not at all otherwise.
#[test]
fn stored_learnings_are_injected_once_and_only_when_relevant() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let _home = EnvVarGuard::set("HOME", dir.path());
    let _backend = EnvVarGuard::set("AMPLIHACK_MEMORY_BACKEND", "sqlite");
    let session = "issue-1483-session";

    for agent in ["general", "analyzer", "builder"] {
        amplihack_memory::cli_memory::store_session_learning(
            session,
            agent,
            "user: reply with just: pong\n\nassistant: pong",
            None,
            true,
        )
        .unwrap();
        amplihack_memory::cli_memory::store_session_learning(
            session,
            agent,
            "The auth middleware rejected expired tokens before refresh",
            None,
            true,
        )
        .unwrap();
    }

    assert_eq!(
        memory::inject_memory("/analyze the builder agent output", Some(session)),
        None
    );

    let context = memory::inject_memory(
        "/analyze why the auth middleware rejected expired tokens",
        Some(session),
    )
    .expect("relevant learning is injected");
    assert_eq!(
        context
            .matches("The auth middleware rejected expired tokens before refresh")
            .count(),
        1
    );
    assert!(!context.contains("pong"));
    assert!(!context.contains("Agent "));
    assert!(!context.contains("relevance: 0.00"));
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
    // Pin the host so output shaping is deterministic under parallel tests
    // (default host resolution is cwd/env-derived and would race).
    let _agent_bin = EnvVarGuard::set("AMPLIHACK_AGENT_BINARY", "claude");
    let _session_depth = EnvVarGuard::unset("AMPLIHACK_SESSION_DEPTH");
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
    let _agent_bin = EnvVarGuard::set("AMPLIHACK_AGENT_BINARY", "claude");
    let _session_depth = EnvVarGuard::unset("AMPLIHACK_SESSION_DEPTH");
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
    let _agent_bin = EnvVarGuard::set("AMPLIHACK_AGENT_BINARY", "claude");
    let _session_depth = EnvVarGuard::unset("AMPLIHACK_SESSION_DEPTH");
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
    let _agent_bin = EnvVarGuard::set("AMPLIHACK_AGENT_BINARY", "claude");
    let _session_depth = EnvVarGuard::unset("AMPLIHACK_SESSION_DEPTH");
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

#[test]
fn workflow_active_semaphore_skips_dev_detection() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _agent_bin = EnvVarGuard::set("AMPLIHACK_AGENT_BINARY", "claude");
    let _session_depth = EnvVarGuard::unset("AMPLIHACK_SESSION_DEPTH");
    let dir = tempfile::tempdir().unwrap();
    let original = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();

    // Create workflow-active semaphore with current PID.
    let dirs = ProjectDirs::new(dir.path());
    let lock_dir = dirs.runtime.join("locks");
    fs::create_dir_all(&lock_dir).unwrap();
    fs::write(
        lock_dir.join(".workflow_active"),
        serde_json::json!({ "pid": std::process::id() }).to_string(),
    )
    .unwrap();

    let hook = UserPromptSubmitHook;
    let result = hook
        .process(HookInput::UserPromptSubmit {
            user_prompt: Some("/dev continue parity audit".to_string()),
            session_id: Some("wf-active-session".to_string()),
            extra: Value::Null,
        })
        .unwrap();

    let _ = std::env::set_current_dir(&original);

    // When workflow is active, /dev detection should be suppressed.
    let ctx = result["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or("");
    assert!(
        !ctx.contains("/dev workflow detected"),
        "dev detection should be suppressed when workflow is active"
    );
}

#[test]
fn workflow_active_via_session_depth_skips_dev_detection() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _agent_bin = EnvVarGuard::set("AMPLIHACK_AGENT_BINARY", "claude");
    let dir = tempfile::tempdir().unwrap();
    let original = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();

    let previous = std::env::var_os("AMPLIHACK_SESSION_DEPTH");
    unsafe { std::env::set_var("AMPLIHACK_SESSION_DEPTH", "1") };

    let hook = UserPromptSubmitHook;
    let result = hook
        .process(HookInput::UserPromptSubmit {
            user_prompt: Some("/dev continue parity audit".to_string()),
            session_id: Some("depth-session".to_string()),
            extra: Value::Null,
        })
        .unwrap();

    match previous {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_SESSION_DEPTH", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_SESSION_DEPTH") },
    }
    let _ = std::env::set_current_dir(&original);

    let ctx = result["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or("");
    assert!(
        !ctx.contains("/dev workflow detected"),
        "dev detection should be suppressed in nested recipe sessions"
    );
}

#[test]
fn is_workflow_active_returns_false_when_no_semaphore() {
    let dir = tempfile::tempdir().unwrap();
    let dirs = ProjectDirs::new(dir.path());
    assert!(!is_workflow_active(&dirs));
}

#[test]
fn is_workflow_active_returns_true_with_live_pid() {
    let dir = tempfile::tempdir().unwrap();
    let dirs = ProjectDirs::new(dir.path());
    let lock_dir = dirs.runtime.join("locks");
    fs::create_dir_all(&lock_dir).unwrap();
    fs::write(
        lock_dir.join(".workflow_active"),
        serde_json::json!({ "pid": std::process::id() }).to_string(),
    )
    .unwrap();
    assert!(is_workflow_active(&dirs));
}
