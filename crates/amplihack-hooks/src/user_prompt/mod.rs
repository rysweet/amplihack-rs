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
use crate::session_start::{is_nested_recipe_session, is_workflow_active};
use amplihack_types::HookInput;
use serde_json::Value;

pub use memory::format_agent_memory_context;
pub use preferences::{build_preference_context, extract_preferences};

pub struct UserPromptSubmitHook;

impl Hook for UserPromptSubmitHook {
    fn name(&self) -> &'static str {
        "user_prompt_submit"
    }

    fn hook_event_name(&self) -> Option<&'static str> {
        Some("UserPromptSubmit")
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

        // Full-conversation mirroring: relay the user's prompt to the session's
        // Signal group (no-op unless the channel is configured). This is the
        // "user side" of the whole-session mirror; the assistant side is
        // mirrored from the Stop hook.
        crate::signal_integration::relay_outbound(session_id.as_deref(), &prompt);

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

        // When a workflow is already active (recipe session or workflow semaphore),
        // skip framework injection and dev-prompt detection to prevent
        // classify-and-decompose recursion (ported from Python PR #3974).
        let workflow_active = is_nested_recipe_session() || is_workflow_active(&dirs);

        // Check AMPLIHACK.md injection (skip when workflow is active to avoid
        // injecting "use dev-orchestrator" instructions into agent steps).
        if !workflow_active
            && let Some(framework_context) = memory::check_framework_injection(&dirs)
            && !framework_context.is_empty()
        {
            context_parts.push(framework_context);
        }

        // Detect /dev invocations and inject workflow enforcement context
        // (skip when workflow is active to prevent false-positive detection on
        // agent step prompts that mention "dev-orchestrator").
        if !workflow_active && preferences::is_dev_invocation(&prompt) {
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

        // Signal channel: surface any queued operator instructions as advisory
        // context. They are data, never commands.
        if let Some(operator_context) =
            crate::signal_integration::drain_into_context(session_id.as_deref())
        {
            context_parts.push(operator_context);
        }

        if context_parts.is_empty() {
            return Ok(Value::Object(serde_json::Map::new()));
        }

        let additional_context = context_parts.join("\n\n");

        // Host-aware shaping: Copilot needs a top-level `additionalContext`
        // string, Claude (and others) the nested `hookSpecificOutput`. This
        // reshape is applied ONLY when the Signal channel is actually configured
        // — its sole purpose is to make operator context reach the operator's
        // host. When the channel is not configured (the default, every golden
        // test, and the non-signal build) the output stays byte-for-byte the
        // historical nested shape that the golden contract pins.
        #[cfg(feature = "signal")]
        if crate::signal_integration::is_channel_configured() {
            let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            let host = crate::signal_integration::inject_host(&cwd);
            let mut out = serde_json::Map::new();
            crate::signal_integration::merge_additional_context(
                &mut out,
                &host,
                "UserPromptSubmit",
                &additional_context,
            );
            return Ok(Value::Object(out));
        }

        Ok(serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "UserPromptSubmit",
                "additionalContext": additional_context
            }
        }))
    }
}
