//! Pre-compact hook: exports conversation transcript before context compaction.
//!
//! When the context window is about to be compacted, this hook:
//! 1. Exports the current conversation to a JSONL transcript file
//! 2. Extracts the original request for context preservation
//! 3. Saves compaction metadata

mod export;
mod request;
#[cfg(test)]
mod tests;

use crate::protocol::{FailurePolicy, Hook};
use amplihack_types::{HookInput, ProjectDirs};
use serde_json::Value;

pub struct PreCompactHook;

impl Hook for PreCompactHook {
    fn name(&self) -> &'static str {
        "pre_compact"
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
        let dirs = ProjectDirs::from_cwd();

        let exported_path = match transcript_path.as_deref() {
            Some(path) => match export::export_transcript(path, &session_id) {
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

        let original_request_preserved = match request::preserve_original_request(
            &dirs,
            &session_id,
            transcript_path.as_deref(),
            &extra,
        ) {
            Ok(preserved) => preserved,
            Err(error) => {
                tracing::warn!(
                    session_id,
                    error = %error,
                    "failed to preserve original request during pre-compact"
                );
                false
            }
        };

        let metadata = match export::save_compaction_metadata(
            &session_id,
            exported_path.as_deref(),
            &extra,
            original_request_preserved,
        ) {
            Ok(metadata) => metadata,
            Err(error) => {
                return Ok(export::error_response(
                    "Failed to save compaction metadata",
                    &error,
                ));
            }
        };

        let message = match exported_path.as_ref() {
            Some(path) => format!(
                "Conversation exported successfully - transcript preserved at {}",
                path.display()
            ),
            None => "Pre-compact metadata saved successfully".to_string(),
        };

        Ok(serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PreCompact",
                "status": "success",
                "message": message,
                "transcript_path": exported_path.map(|path| path.display().to_string()),
                "metadata": metadata,
            }
        }))
    }
}
