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
mod state_paths;

#[cfg(test)]
mod issue_2053_tests;

use crate::protocol::{FailurePolicy, Hook};
use amplihack_types::{HookInput, ProjectDirs, session_state_dir};
use serde_json::Value;
use state_paths::StopStatePaths;

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

        let Some(session_dir) = session_state_dir(session_id.as_deref()) else {
            tracing::warn!("Stop state unavailable: no amplihack state root, approving");
            return Ok(approve());
        };
        let state = StopStatePaths::new(session_dir);
        let dirs = ProjectDirs::from_cwd();

        // Check lock mode.
        if lock::is_lock_active(&dirs) {
            return lock::handle_lock_mode_with_state(&dirs, &state);
        }

        // Check power steering (if enabled).
        if power_steering::should_run(&dirs)
            && let Some(block) =
                power_steering::check_with_state(&dirs, &state, transcript_path.as_deref())?
        {
            return Ok(block);
        }

        // Run reflection (if enabled).
        if reflection::should_run(&dirs)
            && let Some(block) =
                reflection::run_reflection_with_state(&dirs, &state, transcript_path.as_deref())?
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
