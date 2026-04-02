//! Post-tool-use hook: observe tool results, validate operations, and detect
//! blarify index staleness.
//!
//! # Responsibilities
//!
//! 1. **Metrics**: Records tool invocation metrics (tool name, category,
//!    timestamp) to a JSONL file for later analysis.
//! 2. **Validation**: Performs tool-specific result validation for
//!    Write/Edit/MultiEdit operations and emits warnings on failure.
//! 3. **Blarify staleness** (parity with `blarify_staleness_hook.py`):
//!    When a code file is modified via Write/Edit/MultiEdit the hook writes a
//!    `.amplihack/blarify_stale` marker file so that the next session start (or
//!    explicit `amplihack index-code`) knows to trigger a re-index.
//!
//! None of these operations block the tool — failure policy is `Open`.

mod launcher;
mod metrics;
mod validation;
mod workflow;

#[cfg(test)]
mod tests;

use crate::protocol::{FailurePolicy, Hook};
use amplihack_types::{HookInput, ProjectDirs};
use serde_json::Value;

// Re-export for crate-level access (used by user_prompt.rs).
pub(crate) use workflow::begin_workflow_enforcement_tracking;

// Imports used directly in the Hook impl.
use metrics::save_tool_metric;
use validation::{mark_blarify_stale_if_needed, validate_tool_result};
use workflow::{TOOL_CALL_THRESHOLD, update_workflow_enforcement};

// Imports used only in tests (available via `super::*`).
#[cfg(test)]
use metrics::categorize_tool;
#[cfg(test)]
use std::fs;
#[cfg(test)]
use validation::{extract_written_paths, is_code_file};
#[cfg(test)]
use workflow::{WorkflowEnforcementState, read_workflow_state, write_workflow_state};

pub struct PostToolUseHook;

// ---------------------------------------------------------------------------
// Hook implementation
// ---------------------------------------------------------------------------

impl Hook for PostToolUseHook {
    fn name(&self) -> &'static str {
        "post_tool_use"
    }

    fn failure_policy(&self) -> FailurePolicy {
        FailurePolicy::Open
    }

    fn process(&self, input: HookInput) -> anyhow::Result<Value> {
        let (tool_name, tool_input, tool_result, session_id) = match input {
            HookInput::PostToolUse {
                tool_name,
                tool_input,
                tool_result,
                session_id,
            } => (tool_name, tool_input, tool_result, session_id),
            _ => return Ok(Value::Object(serde_json::Map::new())),
        };

        // Tool-specific validation.
        let warning = validate_tool_result(&tool_name, tool_result.as_ref());
        if let Some(ref w) = warning {
            tracing::warn!("{}", w);
        }

        // Blarify staleness detection (parity with blarify_staleness_hook.py).
        mark_blarify_stale_if_needed(&tool_name, &tool_input);

        let dirs = ProjectDirs::from_cwd();
        if let Some(message) = launcher::copilot_post_tool_use_message(&dirs, &tool_name) {
            tracing::info!("{}", message);
        }

        let workflow_warning =
            update_workflow_enforcement(&tool_name, &tool_input, session_id.as_deref());

        // Record the tool metric with category.
        if let Err(e) = save_tool_metric(&tool_name, session_id.as_deref(), warning.as_deref()) {
            tracing::warn!("Failed to save tool metric: {}", e);
        }

        let mut output = serde_json::Map::new();
        if let Some(message) = workflow_warning {
            output.insert("warnings".to_string(), serde_json::json!([message]));
            output.insert(
                "metadata".to_string(),
                serde_json::json!({
                    "workflow_enforcement": "WARNING",
                    "tool_calls_without_evidence": TOOL_CALL_THRESHOLD,
                }),
            );
        }

        Ok(Value::Object(output))
    }
}
