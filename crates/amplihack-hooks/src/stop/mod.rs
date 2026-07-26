//! Stop hook: lock mode, power steering, and reflection.
//!
//! The stop hook decides whether to block session exit. It implements:
//! - Lock mode: if `.lock_active` exists, block with continuation prompt
//! - Safety valve: after N lock iterations, auto-approve
//! - Power steering: check for incomplete work
//! - Reflection: optional SDK bridge for session reflection

pub mod lock;
pub mod power_steering;
pub mod reflection;

use crate::protocol::{FailurePolicy, Hook};
use amplihack_types::{HookInput, ProjectDirs};
use serde_json::Value;

/// Default continuation prompt when lock mode is active.
const DEFAULT_CONTINUATION_PROMPT: &str =
    "Continue working on the current task. Do not stop until the task is complete.";

pub struct StopHook;

impl Hook for StopHook {
    fn name(&self) -> &'static str {
        "stop"
    }

    fn hook_event_name(&self) -> Option<&'static str> {
        Some("Stop")
    }

    fn failure_policy(&self) -> FailurePolicy {
        FailurePolicy::Open
    }

    fn process(&self, input: HookInput) -> anyhow::Result<Value> {
        let (session_id, transcript_path) = match input {
            HookInput::Stop {
                session_id,
                transcript_path,
                ..
            } => (session_id, transcript_path),
            _ => return Ok(approve()),
        };

        let session_id = session_id.unwrap_or_else(get_session_id);
        let dirs = ProjectDirs::from_cwd();

        // Check lock mode.
        if lock::is_lock_active(&dirs) {
            return lock::handle_lock_mode(&dirs, &session_id);
        }

        // Check power steering (if enabled).
        if power_steering::should_run(&dirs)
            && let Some(block) =
                power_steering::check(&dirs, &session_id, transcript_path.as_deref())?
        {
            return Ok(block);
        }

        // Run reflection (if enabled).
        if reflection::should_run(&dirs)
            && let Some(block) =
                reflection::run_reflection(&dirs, &session_id, transcript_path.as_deref())?
        {
            return Ok(block);
        }

        // NOTE: the Signal channel is intentionally NOT torn down here. This
        // hook fires at the end of *every* assistant turn (Claude Code `Stop`,
        // Copilot `agentStop`), not at session end, so tearing the channel down
        // here would kill the per-session Signal group after the first turn.
        // Teardown lives in `SessionStopHook` (the genuine session-end event).
        //
        // Full-conversation mirroring (assistant side): relay this turn's final
        // assistant message to the session's Signal group. No-op unless the
        // channel is configured.
        #[cfg(feature = "signal")]
        if let Some(assistant_text) = transcript_path
            .as_deref()
            .and_then(last_assistant_message_from_transcript)
        {
            crate::signal_integration::relay_outbound(Some(&session_id), &assistant_text);
        }

        Ok(approve())
    }
}

/// Best-effort extraction of the **last** assistant message text from a
/// transcript JSONL file, for outbound mirroring. Tolerant of the several
/// transcript shapes across hosts (Copilot `assistant.message`, Claude
/// `role: assistant`, nested `message.role`). Returns `None` on any failure so
/// mirroring never blocks session exit.
#[cfg(feature = "signal")]
fn last_assistant_message_from_transcript(transcript_path: &std::path::Path) -> Option<String> {
    let contents = std::fs::read_to_string(transcript_path).ok()?;
    contents.lines().rev().find_map(|line| {
        let line = line.trim();
        if line.is_empty() {
            return None;
        }
        let entry = serde_json::from_str::<Value>(line).ok()?;
        extract_assistant_text_from_entry(&entry)
    })
}

/// Pull assistant text out of a single transcript entry across host shapes.
#[cfg(feature = "signal")]
fn extract_assistant_text_from_entry(entry: &Value) -> Option<String> {
    let object = entry.as_object()?;

    if let Some(message) = object.get("message").and_then(Value::as_object)
        && message.get("role").and_then(Value::as_str) == Some("assistant")
    {
        return extract_entry_text(message.get("content").unwrap_or(&Value::Null));
    }

    if object.get("role").and_then(Value::as_str) == Some("assistant") {
        return extract_entry_text(
            object
                .get("content")
                .or_else(|| object.get("text"))
                .unwrap_or(&Value::Null),
        );
    }

    if object.get("type").and_then(Value::as_str) == Some("assistant.message") {
        return extract_entry_text(
            object
                .get("data")
                .and_then(|value| value.get("content"))
                .unwrap_or(&Value::Null),
        );
    }

    if object.get("type").and_then(Value::as_str) == Some("assistant") {
        return extract_entry_text(
            object
                .get("content")
                .or_else(|| object.get("message").and_then(|value| value.get("content")))
                .unwrap_or(&Value::Null),
        );
    }

    None
}

/// Flatten a transcript `content` value (string | array of parts | object)
/// into a single text string.
#[cfg(feature = "signal")]
fn extract_entry_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) if !text.trim().is_empty() => Some(text.to_string()),
        Value::String(_) => None,
        Value::Array(items) => {
            let parts = items
                .iter()
                .filter_map(extract_entry_text)
                .filter(|text| !text.trim().is_empty())
                .collect::<Vec<_>>();
            if parts.is_empty() {
                None
            } else {
                Some(parts.join("\n"))
            }
        }
        Value::Object(map) => map
            .get("text")
            .or_else(|| map.get("value"))
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .or_else(|| map.get("content").and_then(extract_entry_text)),
        _ => None,
    }
}

/// Approve (allow session to exit).
fn approve() -> Value {
    serde_json::json!({"decision": "approve"})
}

/// Get the current session ID from env or generate one.
fn get_session_id() -> String {
    if let Ok(id) = std::env::var("CLAUDE_SESSION_ID") {
        return id;
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    format!("session-{}", now.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approve_has_correct_format() {
        let result = approve();
        assert_eq!(result["decision"], "approve");
    }

    #[test]
    fn handles_unknown_events() {
        let hook = StopHook;
        let result = hook.process(HookInput::Unknown).unwrap();
        assert_eq!(result["decision"], "approve");
    }
}
