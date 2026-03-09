//! Post-tool-use hook: observe tool results for metrics and validation.
//!
//! Records tool invocation metrics (tool name, duration) to a JSONL file.
//! Does not block any operations — purely observational.

use crate::protocol::{FailurePolicy, Hook};
use amplihack_types::HookInput;
use serde::Serialize;
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::SystemTime;

pub struct PostToolUseHook;

#[derive(Serialize)]
struct ToolMetric {
    timestamp: String,
    tool_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
    hook: &'static str,
}

impl Hook for PostToolUseHook {
    fn name(&self) -> &'static str {
        "post_tool_use"
    }

    fn failure_policy(&self) -> FailurePolicy {
        FailurePolicy::Open
    }

    fn process(&self, input: HookInput) -> anyhow::Result<Value> {
        let (tool_name, session_id) = match input {
            HookInput::PostToolUse {
                tool_name,
                session_id,
                ..
            } => (tool_name, session_id),
            _ => return Ok(Value::Object(serde_json::Map::new())),
        };

        // Record the tool metric.
        if let Err(e) = save_tool_metric(&tool_name, session_id.as_deref()) {
            tracing::warn!("Failed to save tool metric: {}", e);
        }

        Ok(Value::Object(serde_json::Map::new()))
    }
}

fn save_tool_metric(tool_name: &str, session_id: Option<&str>) -> anyhow::Result<()> {
    let metrics_dir = get_metrics_dir()?;
    fs::create_dir_all(&metrics_dir)?;

    let metrics_file = metrics_dir.join("post_tool_use_metrics.jsonl");

    let metric = ToolMetric {
        timestamp: now_iso8601(),
        tool_name: tool_name.to_string(),
        session_id: session_id.map(String::from),
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

fn get_metrics_dir() -> anyhow::Result<PathBuf> {
    let dir = std::env::current_dir()?
        .join(".claude")
        .join("runtime")
        .join("metrics");
    Ok(dir)
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
