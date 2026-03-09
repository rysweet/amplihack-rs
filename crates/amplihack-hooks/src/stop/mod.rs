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

        Ok(approve())
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
