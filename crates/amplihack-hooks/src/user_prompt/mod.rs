//! User prompt submit hook: injects context and preferences into user prompts.
//!
//! On every user message, this hook:
//! 1. Loads cached user preferences (USER_PREFERENCES.md)
//! 2. Injects native Rust memory context for referenced agents
//! 3. Detects framework injection needs (AMPLIHACK.md vs CLAUDE.md)
//! 4. Returns modified prompt with injected context

mod memory;
mod preferences;
#[cfg(test)]
mod tests;

use crate::post_tool_use::begin_workflow_enforcement_tracking;
use crate::prompt_input::extract_user_prompt;
use crate::protocol::{FailurePolicy, Hook};
use amplihack_types::HookInput;
use serde_json::Value;

pub use memory::format_agent_memory_context;
pub use preferences::{build_preference_context, extract_preferences};

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

        let dirs = amplihack_types::ProjectDirs::from_cwd();
        let mut context_parts: Vec<String> = Vec::new();

        // Load user preferences (including learned patterns detection).
        let (prefs_context, has_learned_patterns) =
            preferences::load_user_preferences_with_patterns(&dirs);
        if let Some(ctx) = prefs_context
            && !ctx.is_empty()
        {
            context_parts.push(ctx);
        }
        if has_learned_patterns {
            context_parts.push("Has Learned Patterns: Yes".to_string());
        }

        // Inject memory context for referenced agents.
        if let Some(memory_context) = memory::inject_memory(&prompt, session_id.as_deref())
            && !memory_context.is_empty()
        {
            context_parts.push(memory_context);
        }

        // Check AMPLIHACK.md injection.
        if let Some(framework_context) = memory::check_framework_injection(&dirs)
            && !framework_context.is_empty()
        {
            context_parts.push(framework_context);
        }

        // Detect /dev invocations and inject workflow enforcement context.
        if preferences::is_dev_invocation(&prompt) {
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
