//! User prompt submit hook: injects context and preferences into user prompts.
//!
//! On every user message, this hook:
//! 1. Loads cached user preferences (USER_PREFERENCES.md)
//! 2. Injects native Rust memory context for referenced agents
//! 3. Detects framework injection needs (AMPLIHACK.md vs CLAUDE.md)
//! 4. Returns modified prompt with injected context

use crate::agent_memory::{detect_agent_references, detect_slash_command_agent};
use crate::post_tool_use::begin_workflow_enforcement_tracking;
use crate::prompt_input::extract_user_prompt;
use crate::protocol::{FailurePolicy, Hook};
use amplihack_cli::memory::{PromptContextMemory, retrieve_prompt_context_memories};
use amplihack_types::{HookInput, ProjectDirs};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

pub struct UserPromptSubmitHook;

impl Hook for UserPromptSubmitHook {
    fn name(&self) -> &'static str {
        "user_prompt_submit"
    }

    fn failure_policy(&self) -> FailurePolicy {
        FailurePolicy::Open
    }

    fn process(&self, input: HookInput) -> anyhow::Result<Value> {
        let (user_prompt, session_id, extra) = match input {
            HookInput::UserPromptSubmit {
                user_prompt,
                session_id,
                extra,
            } => (user_prompt, session_id, extra),
            _ => return Ok(Value::Object(serde_json::Map::new())),
        };

        let prompt = extract_user_prompt(user_prompt.as_deref(), &extra);
        if prompt.is_empty() {
            return Ok(Value::Object(serde_json::Map::new()));
        }

        let dirs = ProjectDirs::from_cwd();
        let mut context_parts: Vec<String> = Vec::new();

        // Load user preferences (including learned patterns detection).
        let (prefs_context, has_learned_patterns) = load_user_preferences_with_patterns(&dirs);
        if let Some(ctx) = prefs_context
            && !ctx.is_empty()
        {
            context_parts.push(ctx);
        }
        if has_learned_patterns {
            context_parts.push("Has Learned Patterns: Yes".to_string());
        }

        // Inject memory context for referenced agents.
        if let Some(memory_context) = inject_memory(&prompt, session_id.as_deref())
            && !memory_context.is_empty()
        {
            context_parts.push(memory_context);
        }

        // Check AMPLIHACK.md injection.
        if let Some(framework_context) = check_framework_injection(&dirs)
            && !framework_context.is_empty()
        {
            context_parts.push(framework_context);
        }

        // Detect /dev invocations and inject workflow enforcement context.
        if is_dev_invocation(&prompt) {
            if let Err(error) = begin_workflow_enforcement_tracking(session_id.as_deref()) {
                tracing::warn!(
                    "workflow enforcement: failed to initialize state from user prompt: {}",
                    error
                );
            }
            context_parts.push(
                "🔧 /dev workflow detected. Follow DEFAULT_WORKFLOW steps. \
                 Track progress with TodoWrite."
                    .to_string(),
            );
        }

        if context_parts.is_empty() {
            return Ok(Value::Object(serde_json::Map::new()));
        }

        let additional_context = context_parts.join("\n\n");

        Ok(serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "UserPromptSubmit",
                "additionalContext": additional_context
            }
        }))
    }
}

/// Load user preferences from USER_PREFERENCES.md.
/// Also detects `## Learned Patterns` section.
fn load_user_preferences_with_patterns(dirs: &ProjectDirs) -> (Option<String>, bool) {
    let mut candidates = Vec::new();
    if let Some(path) = dirs.resolve_preferences_file() {
        candidates.push(path);
    }
    candidates.push(dirs.root.join("USER_PREFERENCES.md"));

    for path in &candidates {
        if path.exists() {
            match fs::read_to_string(path) {
                Ok(content) => {
                    let has_learned = content.contains("## Learned Patterns")
                        && content
                            .split("## Learned Patterns")
                            .nth(1)
                            .map(|s| {
                                s.lines()
                                    .any(|l| !l.trim().is_empty() && !l.starts_with('#'))
                            })
                            .unwrap_or(false);
                    let prefs = extract_preferences(&content);
                    if !prefs.is_empty() {
                        return (Some(build_preference_context(&prefs)), has_learned);
                    }
                    return (None, has_learned);
                }
                Err(e) => {
                    tracing::warn!("Failed to read preferences: {}", e);
                }
            }
        }
    }

    (None, false)
}

/// Check if the user prompt is a /dev invocation.
fn is_dev_invocation(prompt: &str) -> bool {
    let lowered = prompt.trim().to_ascii_lowercase();
    lowered == "/dev"
        || lowered.starts_with("/dev ")
        || lowered.starts_with("/dev\n")
        || lowered.contains("\n/dev ")
        || lowered.contains("\n/dev\n")
        || lowered.contains("dev-orchestrator")
        || lowered.starts_with("/amplihack:dev")
        || lowered.starts_with("/.claude:amplihack:dev")
}

/// Extract preference key-value pairs from markdown content.
///
/// Supports both formats for Python parity:
/// - Table format: `| key | value |`
/// - Header format: `### Key\nvalue`
fn extract_preferences(content: &str) -> Vec<(String, String)> {
    let mut prefs = Vec::new();

    // Try table format first.
    let mut found_table = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('|') && trimmed.ends_with('|') {
            let parts: Vec<&str> = trimmed.split('|').map(str::trim).collect();
            if parts.len() >= 3 {
                let key = parts[1].trim();
                let value = parts[2].trim();
                if !key.is_empty()
                    && !value.is_empty()
                    && key != "Setting"
                    && key != "---"
                    && !key.starts_with('-')
                {
                    prefs.push((key.to_string(), value.to_string()));
                    found_table = true;
                }
            }
        }
    }

    if found_table {
        return prefs;
    }

    // Fall back to header format: ### Key\nvalue
    let lines: Vec<&str> = content.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim();
        if let Some(header) = trimmed.strip_prefix("### ") {
            let key = header.trim().to_string();
            // Collect value lines until next header or end.
            let mut value_lines = Vec::new();
            i += 1;
            while i < lines.len() {
                let next = lines[i].trim();
                if next.starts_with("### ") || next.starts_with("## ") || next.starts_with("# ") {
                    break;
                }
                if !next.is_empty() {
                    value_lines.push(next);
                }
                i += 1;
            }
            if !key.is_empty() && !value_lines.is_empty() {
                prefs.push((key, value_lines.join(" ")));
            }
        } else {
            i += 1;
        }
    }

    prefs
}

/// Build a context string from preferences.
fn build_preference_context(prefs: &[(String, String)]) -> String {
    let mut parts = vec!["## User Preferences".to_string()];
    for (key, value) in prefs {
        parts.push(format!("- **{}**: {}", key, value));
    }
    parts.join("\n")
}

fn inject_memory(prompt: &str, session_id: Option<&str>) -> Option<String> {
    let mut agent_types = detect_agent_references(prompt);
    if let Some(agent) = detect_slash_command_agent(prompt)
        && !agent_types.iter().any(|existing| existing == agent)
    {
        agent_types.push(agent.to_string());
    }

    if agent_types.is_empty() {
        return None;
    }

    let session_id = session_id
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("hook_session");
    let query_text = prompt.chars().take(500).collect::<String>();

    match retrieve_prompt_context_memories(session_id, &query_text, 2000) {
        Ok(memories) if !memories.is_empty() => {
            Some(format_agent_memory_context(&agent_types, &memories))
        }
        Ok(_) => None,
        Err(error) => {
            tracing::warn!("Memory injection failed: {}", error);
            None
        }
    }
}

fn format_agent_memory_context(agent_types: &[String], memories: &[PromptContextMemory]) -> String {
    agent_types
        .iter()
        .map(|agent_type| {
            let mut lines = vec![format!("\n## Memory for {} Agent\n", agent_type)];
            for memory in memories {
                lines.push(format!("- {} (relevance: 0.00)", memory.content));
                if let Some(code_context) = memory.code_context.as_deref()
                    && !code_context.trim().is_empty()
                {
                    lines.push(code_context.to_string());
                }
            }
            lines.join("\n")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Check if AMPLIHACK.md should be injected (differs from CLAUDE.md).
fn check_framework_injection(dirs: &ProjectDirs) -> Option<String> {
    let amplihack_path = find_amplihack_md(dirs)?;
    let claude_path = dirs.claude_md();

    let amplihack_content = fs::read_to_string(&amplihack_path).ok()?;
    let claude_content = fs::read_to_string(&claude_path).ok().unwrap_or_default();

    // Normalize whitespace for comparison.
    let norm_amplihack: String = amplihack_content.split_whitespace().collect();
    let norm_claude: String = claude_content.split_whitespace().collect();

    if norm_amplihack == norm_claude {
        return None; // Already identical.
    }

    Some(amplihack_content)
}

fn find_amplihack_md(dirs: &ProjectDirs) -> Option<PathBuf> {
    // Check CLAUDE_PLUGIN_ROOT env var first.
    if let Ok(root) = std::env::var("CLAUDE_PLUGIN_ROOT") {
        let path = PathBuf::from(root).join("AMPLIHACK.md");
        if path.exists() {
            return Some(path);
        }
    }

    // Check .claude/AMPLIHACK.md.
    let path = dirs.amplihack_md();
    if path.exists() {
        return Some(path);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_memory::{detect_agent_references, detect_slash_command_agent};
    use crate::post_tool_use::PostToolUseHook;
    use crate::test_support::env_lock;

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
        assert!(is_dev_invocation("/DEV implement caching"));
        assert!(is_dev_invocation("/Dev add logging middleware"));
        assert!(is_dev_invocation("/amplihack:dev continue parity"));
        assert!(is_dev_invocation("Please use dev-orchestrator for this"));
        assert!(!is_dev_invocation("/review this change"));
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
            load_user_preferences_with_patterns(&ProjectDirs::new(project.path()));

        match previous {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_ROOT", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_ROOT") },
        }

        assert!(!has_learned_patterns);
        assert!(context.unwrap().contains("**Verbosity**: concise"));
    }
}
