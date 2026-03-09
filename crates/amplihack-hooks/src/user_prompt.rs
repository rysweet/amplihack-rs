//! User prompt submit hook: injects context and preferences into user prompts.
//!
//! On every user message, this hook:
//! 1. Loads cached user preferences (USER_PREFERENCES.md)
//! 2. Injects memory context via Python bridge
//! 3. Detects framework injection needs (AMPLIHACK.md vs CLAUDE.md)
//! 4. Returns modified prompt with injected context

use crate::protocol::{FailurePolicy, Hook};
use amplihack_state::PythonBridge;
use amplihack_types::HookInput;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Embedded Python bridge script for memory injection.
const MEMORY_INJECT_BRIDGE: &str = r#"
import sys
import json

try:
    input_data = json.load(sys.stdin)
    action = input_data.get("action", "inject_memory")
    session_id = input_data.get("session_id", "")
    prompt = input_data.get("prompt", "")

    try:
        from amplihack.memory.coordinator import MemoryCoordinator
        coordinator = MemoryCoordinator()
        context = coordinator.inject_memory_for_agents_sync(
            session_id=session_id,
            prompt=prompt
        )
        result = {"injected_context": context or "", "memory_keys_used": []}
    except ImportError:
        result = {"injected_context": "", "memory_keys_used": []}
    except Exception as e:
        result = {"injected_context": "", "error": str(e)}

    json.dump(result, sys.stdout)
except Exception as e:
    json.dump({"injected_context": "", "error": str(e)}, sys.stdout)
"#;

pub struct UserPromptSubmitHook;

impl Hook for UserPromptSubmitHook {
    fn name(&self) -> &'static str {
        "user_prompt_submit"
    }

    fn failure_policy(&self) -> FailurePolicy {
        FailurePolicy::Open
    }

    fn process(&self, input: HookInput) -> anyhow::Result<Value> {
        let (user_prompt, session_id) = match input {
            HookInput::UserPromptSubmit {
                user_prompt,
                session_id,
                ..
            } => (user_prompt, session_id),
            _ => return Ok(Value::Object(serde_json::Map::new())),
        };

        let prompt = user_prompt.unwrap_or_default();
        if prompt.is_empty() {
            return Ok(Value::Object(serde_json::Map::new()));
        }

        let mut context_parts: Vec<String> = Vec::new();

        // Load user preferences.
        if let Some(prefs_context) = load_user_preferences()
            && !prefs_context.is_empty()
        {
            context_parts.push(prefs_context);
        }

        // Inject memory context via bridge.
        if let Some(memory_context) = inject_memory(&prompt, session_id.as_deref())
            && !memory_context.is_empty()
        {
            context_parts.push(memory_context);
        }

        // Check AMPLIHACK.md injection.
        if let Some(framework_context) = check_framework_injection()
            && !framework_context.is_empty()
        {
            context_parts.push(framework_context);
        }

        if context_parts.is_empty() {
            return Ok(Value::Object(serde_json::Map::new()));
        }

        // Build the modified prompt with injected context.
        let additional_context = context_parts.join("\n\n");
        let new_prompt = format!("{}\n\n{}", additional_context, prompt);

        Ok(serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "UserPromptSubmit",
                "userPromptContent": new_prompt
            }
        }))
    }
}

/// Load user preferences from USER_PREFERENCES.md.
fn load_user_preferences() -> Option<String> {
    let cwd = std::env::current_dir().ok()?;

    // Search for USER_PREFERENCES.md in known locations.
    let candidates = [
        cwd.join(".claude")
            .join("context")
            .join("USER_PREFERENCES.md"),
        cwd.join("USER_PREFERENCES.md"),
    ];

    for path in &candidates {
        if path.exists() {
            match fs::read_to_string(path) {
                Ok(content) => {
                    let prefs = extract_preferences(&content);
                    if !prefs.is_empty() {
                        return Some(build_preference_context(&prefs));
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to read preferences: {}", e);
                }
            }
        }
    }

    None
}

/// Extract preference key-value pairs from markdown content.
fn extract_preferences(content: &str) -> Vec<(String, String)> {
    let mut prefs = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();
        // Look for table rows: | key | value |
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
                }
            }
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

/// Inject memory context via Python bridge.
fn inject_memory(prompt: &str, session_id: Option<&str>) -> Option<String> {
    let input = serde_json::json!({
        "action": "inject_memory",
        "session_id": session_id.unwrap_or(""),
        "prompt": prompt,
    });

    match PythonBridge::call(MEMORY_INJECT_BRIDGE, &input, Duration::from_secs(5)) {
        Ok(result) => {
            let context = result
                .get("injected_context")
                .and_then(Value::as_str)
                .unwrap_or("");
            if context.is_empty() {
                None
            } else {
                Some(context.to_string())
            }
        }
        Err(e) => {
            tracing::warn!("Memory injection failed: {}", e);
            None
        }
    }
}

/// Check if AMPLIHACK.md should be injected (differs from CLAUDE.md).
fn check_framework_injection() -> Option<String> {
    let cwd = std::env::current_dir().ok()?;

    // Find AMPLIHACK.md.
    let amplihack_path = find_amplihack_md(&cwd)?;
    let claude_path = cwd.join("CLAUDE.md");

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

fn find_amplihack_md(cwd: &Path) -> Option<PathBuf> {
    // Check CLAUDE_PLUGIN_ROOT env var first.
    if let Ok(root) = std::env::var("CLAUDE_PLUGIN_ROOT") {
        let path = PathBuf::from(root).join("AMPLIHACK.md");
        if path.exists() {
            return Some(path);
        }
    }

    // Check .claude/AMPLIHACK.md.
    let path = cwd.join(".claude").join("AMPLIHACK.md");
    if path.exists() {
        return Some(path);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn handles_unknown_events() {
        let hook = UserPromptSubmitHook;
        let result = hook.process(HookInput::Unknown).unwrap();
        assert!(result.as_object().unwrap().is_empty());
    }
}
