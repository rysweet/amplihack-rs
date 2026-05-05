//! Tool invocation metrics recording.
//!
//! Writes one JSONL line per tool invocation to `post_tool_use_metrics.jsonl`
//! under the project metrics directory.

use amplihack_types::ProjectDirs;
use serde::Serialize;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::time::SystemTime;

// ---------------------------------------------------------------------------
// Tool categorisation
// ---------------------------------------------------------------------------

/// Categorize a tool invocation for high-level metrics.
pub(super) fn categorize_tool(name: &str) -> &'static str {
    match name {
        "Bash" | "bash" | "terminal" => "bash_commands",
        "Write" | "Edit" | "MultiEdit" | "create" | "edit" => "file_operations",
        "Read" | "View" | "view" | "glob" | "grep" | "Grep" | "Search" => "search_operations",
        _ => "other",
    }
}

// ---------------------------------------------------------------------------
// Metric record
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

pub(super) fn save_tool_metric(
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
    writeln!(file, "{json}")?;

    Ok(())
}

fn now_iso8601() -> String {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}", now.as_secs())
}
