//! Session start hook: initializes session state and injects context.
//!
//! On session start, this hook:
//! 1. Checks for version mismatches
//! 2. Migrates global hooks if needed
//! 3. Captures original request
//! 4. Injects project context, learnings, and preferences
//! 5. Returns additional context for the session

mod blarify;
mod context_loaders;
mod migration;
#[cfg(test)]
mod tests;

use crate::original_request::{capture_original_request, format_original_request_context};
use crate::protocol::{FailurePolicy, Hook};
use amplihack_types::{HookInput, ProjectDirs};
use serde_json::Value;

use blarify::{setup_blarify_indexing, BlarifySetupResult};
use context_loaders::{
    check_version, load_code_graph_context, load_discoveries, load_project_context,
    load_user_preferences, load_workflow_context,
};
use migration::{
    code_graph_compatibility_notice, memory_graph_compatibility_notice, migrate_global_hooks,
};

pub struct SessionStartHook;

impl Hook for SessionStartHook {
    fn name(&self) -> &'static str {
        "session_start"
    }

    fn failure_policy(&self) -> FailurePolicy {
        FailurePolicy::Open
    }

    fn process(&self, input: HookInput) -> anyhow::Result<Value> {
        let (session_id, extra) = match input {
            HookInput::SessionStart {
                session_id, extra, ..
            } => (session_id, extra),
            _ => return Ok(Value::Object(serde_json::Map::new())),
        };

        let dirs = ProjectDirs::from_cwd();
        let mut context_parts: Vec<String> = Vec::new();
        // Warnings accumulate structured failures for the HookOutput `warnings` field.
        // These surface errors that did not block session start (fail-open) but that
        // the host should be aware of — e.g. code-graph setup failures.
        let mut warnings: Vec<String> = Vec::new();

        if let Some(original_request_context) =
            maybe_capture_original_request(&dirs, session_id.as_deref(), &extra)?
        {
            context_parts.push(original_request_context);
        }

        // Load project context (PROJECT.md).
        if let Some(ctx) = load_project_context(&dirs) {
            context_parts.push(ctx);
        }

        // Load recent learnings/discoveries.
        if let Some(learnings) = load_discoveries(&dirs) {
            context_parts.push(learnings);
        }

        // Load user preferences.
        if let Some(prefs) = load_user_preferences(&dirs) {
            context_parts.push(prefs);
        }

        context_parts.push(load_workflow_context(&dirs));

        // Check for version mismatch natively.
        if let Some(version_notice) = check_version(&dirs) {
            context_parts.push(version_notice);
        }

        // Migrate global hooks if needed.
        if let Some(migration_notice) = migrate_global_hooks() {
            context_parts.push(migration_notice);
        }

        // Run blarify / code-graph indexing setup and track the lifecycle status.
        //
        // indexing_status values (parity with amploxy SessionStart):
        //   "started"       — background indexing was triggered or was already running
        //   "complete"      — index is up-to-date; no new indexing triggered
        //   "error:<reason>" — setup failed; session continues (fail-open)
        let (indexing_status, blarify_setup) = match setup_blarify_indexing(&dirs) {
            Ok(result) => {
                let status = if result.indexing_active {
                    "started".to_string()
                } else {
                    "complete".to_string()
                };
                (status, result)
            }
            Err(err) => {
                let error_msg = format!("Code-graph setup failed: {err}");
                tracing::warn!("Blarify setup failed (non-critical): {}", err);
                // Surface the failure as a structured warning — not just buried in text.
                warnings.push(error_msg.clone());
                let notice = BlarifySetupResult::with_notice(
                    false,
                    format_code_graph_status(format!(
                        "{error_msg}. Continuing without automatic refresh."
                    )),
                );
                (format!("error:{err}"), notice)
            }
        };

        if let Some(status_context) = blarify_setup.status_context {
            context_parts.push(status_context);
        }
        if let Some(compatibility_notice) = code_graph_compatibility_notice(&dirs)? {
            context_parts.push(compatibility_notice);
        }
        if let Some(memory_notice) = memory_graph_compatibility_notice() {
            context_parts.push(memory_notice);
        }

        if !blarify_setup.indexing_active {
            match load_code_graph_context(&dirs) {
                Ok(Some(code_graph_context)) => context_parts.push(code_graph_context),
                Ok(None) => {}
                Err(err) => {
                    let error_msg = format!("Code-graph context unavailable: {err}");
                    warnings.push(error_msg.clone());
                    context_parts.push(format_code_graph_status(error_msg));
                }
            }
        }

        let additional_context = context_parts.join("\n\n");

        // Always emit hookSpecificOutput so that `indexing_status` is never absent.
        // When there is no additionalContext, we still report the indexing lifecycle.
        let mut hook_specific = serde_json::json!({
            "hookEventName": "SessionStart",
            "indexing_status": indexing_status,
        });
        if !additional_context.is_empty() {
            hook_specific["additionalContext"] = Value::String(additional_context);
        }

        let mut output = serde_json::json!({
            "hookSpecificOutput": hook_specific,
        });
        if !warnings.is_empty() {
            output["warnings"] = serde_json::json!(warnings);
        }

        Ok(output)
    }
}

fn maybe_capture_original_request(
    dirs: &ProjectDirs,
    session_id: Option<&str>,
    extra: &Value,
) -> anyhow::Result<Option<String>> {
    let Some(prompt) = extra.get("prompt").and_then(Value::as_str).map(str::trim) else {
        return Ok(None);
    };

    Ok(capture_original_request(dirs, session_id, prompt)?
        .as_ref()
        .map(format_original_request_context))
}

fn format_code_graph_status(body: String) -> String {
    format!("## Code Graph Status\n\n{body}")
}

fn format_memory_status(body: String) -> String {
    format!("## Memory Store Status\n\n{body}")
}
