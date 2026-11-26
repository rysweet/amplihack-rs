//! Tool invocation metrics recording.
//!
//! Writes one JSONL line per tool invocation to
//! `<session state dir>/metrics/post_tool_use_metrics.jsonl` under the
//! amplihack state root (#1963), never the working directory.

use crate::state_dir::create_private_dir_all;
use serde::Serialize;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
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

/// Append one metric line to `<session_dir>/metrics/post_tool_use_metrics.jsonl`.
pub(super) fn save_tool_metric(
    session_dir: &Path,
    tool_name: &str,
    session_id: Option<&str>,
    warning: Option<&str>,
) -> anyhow::Result<()> {
    let metrics_dir = session_dir.join("metrics");
    create_private_dir_all(&metrics_dir)?;

    let metrics_file = metrics_dir.join("post_tool_use_metrics.jsonl");

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
