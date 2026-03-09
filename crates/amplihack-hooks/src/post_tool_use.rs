//! Post-tool-use hook: observe tool results for metrics and validation.
//!
//! Records tool invocation metrics (tool name, duration, category) to a JSONL
//! file. Performs tool-specific validation for Write/Edit/MultiEdit operations.
//! Does not block any operations — purely observational.

use crate::protocol::{FailurePolicy, Hook};
use amplihack_types::HookInput;
use amplihack_types::ProjectDirs;
use serde::Serialize;
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::time::SystemTime;

pub struct PostToolUseHook;

#[derive(Serialize)]
struct ToolMetric {
    timestamp: String,
    tool_name: String,
    category: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    warning: Option<String>,
    hook: &'static str,
}

/// Categorize a tool invocation for high-level metrics.
fn categorize_tool(name: &str) -> &'static str {
    match name {
        "Bash" | "bash" | "terminal" => "bash_commands",
        "Write" | "Edit" | "MultiEdit" | "create" | "edit" => "file_operations",
        "Read" | "View" | "view" | "glob" | "grep" | "Grep" | "Search" => "search_operations",
        _ => "other",
    }
}

/// Validate tool-specific results for Write/Edit/MultiEdit.
///
/// Returns a warning string if the tool result indicates a problem.
fn validate_tool_result(tool_name: &str, tool_result: Option<&Value>) -> Option<String> {
    let result = tool_result?;

    match tool_name {
        "Write" | "Edit" | "MultiEdit" | "create" | "edit" => {
            // Check for error indicators in the result.
            if let Some(error) = result.get("error").and_then(Value::as_str) {
                return Some(format!("{tool_name} error: {error}"));
            }
            if let Some(false) = result.get("success").and_then(Value::as_bool) {
                let msg = result
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown error");
                return Some(format!("{tool_name} failed: {msg}"));
            }
            None
        }
        _ => None,
    }
}

impl Hook for PostToolUseHook {
    fn name(&self) -> &'static str {
        "post_tool_use"
    }

    fn failure_policy(&self) -> FailurePolicy {
        FailurePolicy::Open
    }

    fn process(&self, input: HookInput) -> anyhow::Result<Value> {
        let (tool_name, tool_result, session_id) = match input {
            HookInput::PostToolUse {
                tool_name,
                tool_result,
                session_id,
                ..
            } => (tool_name, tool_result, session_id),
            _ => return Ok(Value::Object(serde_json::Map::new())),
        };

        // Tool-specific validation.
        let warning = validate_tool_result(&tool_name, tool_result.as_ref());
        if let Some(ref w) = warning {
            tracing::warn!("{}", w);
        }

        // Record the tool metric with category.
        if let Err(e) = save_tool_metric(&tool_name, session_id.as_deref(), warning.as_deref()) {
            tracing::warn!("Failed to save tool metric: {}", e);
        }

        Ok(Value::Object(serde_json::Map::new()))
    }
}

fn save_tool_metric(
    tool_name: &str,
    session_id: Option<&str>,
    warning: Option<&str>,
) -> anyhow::Result<()> {
    let dirs = ProjectDirs::from_cwd();
    fs::create_dir_all(&dirs.metrics)?;

    let metrics_file = dirs.metrics.join("post_tool_use_metrics.jsonl");

    let metric = ToolMetric {
        timestamp: now_iso8601(),
        tool_name: tool_name.to_string(),
        category: categorize_tool(tool_name),
        session_id: session_id.map(String::from),
        warning: warning.map(String::from),
        hook: "post_tool_use",
    };

    let json = serde_json::to_string(&metric)?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(metrics_file)?;
    writeln!(file, "{}", json)?;

    Ok(())
}

fn now_iso8601() -> String {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}", now.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categorizes_tools_correctly() {
        assert_eq!(categorize_tool("Bash"), "bash_commands");
        assert_eq!(categorize_tool("Write"), "file_operations");
        assert_eq!(categorize_tool("Edit"), "file_operations");
        assert_eq!(categorize_tool("grep"), "search_operations");
        assert_eq!(categorize_tool("CustomTool"), "other");
    }

    #[test]
    fn validates_edit_errors() {
        let result = serde_json::json!({"error": "file not found"});
        let warning = validate_tool_result("Edit", Some(&result));
        assert!(warning.is_some());
        assert!(warning.unwrap().contains("file not found"));
    }

    #[test]
    fn validates_success_false() {
        let result = serde_json::json!({"success": false, "message": "permission denied"});
        let warning = validate_tool_result("Write", Some(&result));
        assert!(warning.is_some());
        assert!(warning.unwrap().contains("permission denied"));
    }

    #[test]
    fn no_warning_on_success() {
        let result = serde_json::json!({"success": true});
        assert!(validate_tool_result("Edit", Some(&result)).is_none());
    }

    #[test]
    fn no_warning_for_bash() {
        let result = serde_json::json!({"error": "something"});
        assert!(validate_tool_result("Bash", Some(&result)).is_none());
    }

    #[test]
    fn allows_all_tools() {
        let hook = PostToolUseHook;
        let input = HookInput::PostToolUse {
            tool_name: "Bash".to_string(),
            tool_input: serde_json::json!({"command": "ls"}),
            tool_result: None,
            session_id: None,
        };
        let result = hook.process(input).unwrap();
        assert!(result.as_object().unwrap().is_empty());
    }

    #[test]
    fn handles_unknown_events() {
        let hook = PostToolUseHook;
        let result = hook.process(HookInput::Unknown).unwrap();
        assert!(result.as_object().unwrap().is_empty());
    }
}
