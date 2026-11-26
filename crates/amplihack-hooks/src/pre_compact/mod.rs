//! Pre-compact hook: exports conversation transcript before context compaction.
//!
//! When the context window is about to be compacted, this hook:
//! 1. Exports the current conversation to a JSONL transcript file
//! 2. Extracts the original request for context preservation
//! 3. Saves compaction metadata
//!
//! Everything is written under `<state root>/runtime/sessions/<key>/logs/`
//! (#1963), never the working directory. Those files are the hook's record of
//! the compaction.
//!
//! Claude Code accepts no `hookSpecificOutput` for PreCompact (#3269), so the
//! hook prints `{}` on success and when no state root resolves (it then writes
//! nothing and logs a warning). When the export or the metadata write fails it
//! prints only the common `systemMessage` field, so the person sees the
//! failure, and logs it.

mod export;
mod request;
#[cfg(test)]
mod tests;

use crate::protocol::{FailurePolicy, Hook};
use amplihack_types::HookInput;
use serde_json::Value;

/// Reported when no amplihack state root resolves (R5).
const NO_STATE_ROOT_MESSAGE: &str =
    "Pre-compact state not saved: no amplihack state root (set HOME or AMPLIHACK_HOME)";

pub struct PreCompactHook;

impl Hook for PreCompactHook {
    fn name(&self) -> &'static str {
        "pre_compact"
    }

    fn hook_event_name(&self) -> Option<&'static str> {
        Some("PreCompact")
    }

    fn failure_policy(&self) -> FailurePolicy {
        FailurePolicy::Open
    }

    fn process(&self, input: HookInput) -> anyhow::Result<Value> {
        let (session_id, transcript_path, extra) = match input {
            HookInput::PreCompact {
                session_id,
                transcript_path,
                extra,
            } => (session_id, transcript_path, extra),
            _ => return Ok(Value::Object(serde_json::Map::new())),
        };

        let session_id = session_id.unwrap_or_else(export::generate_session_id);
        let Some(session_dir) = amplihack_types::session_state_dir(Some(&session_id)) else {
            tracing::warn!("{}", NO_STATE_ROOT_MESSAGE);
            return Ok(Value::Object(serde_json::Map::new()));
        };

        let exported_path = match transcript_path.as_deref() {
            Some(path) => match export::export_transcript(&session_dir, path) {
                Ok(path) => path,
                Err(error) => {
                    return Ok(export::error_response(
                        "Failed to export transcript",
                        &error,
                    ));
                }
            },
            None => None,
        };

        let original_request_preserved = request::preserve_original_request(
            &session_dir,
            &session_id,
            transcript_path.as_deref(),
            &extra,
        );

        if let Err(error) = export::save_compaction_metadata(
            &session_dir,
            &session_id,
            exported_path.as_deref(),
            &extra,
            original_request_preserved,
        ) {
            return Ok(export::error_response(
                "Failed to save compaction metadata",
                &error,
            ));
        }

        match exported_path.as_ref() {
            Some(path) => tracing::info!(
                "Conversation exported successfully - transcript preserved at {}",
                path.display()
            ),
            None => tracing::info!("Pre-compact metadata saved successfully"),
        }

        Ok(Value::Object(serde_json::Map::new()))
    }
}
