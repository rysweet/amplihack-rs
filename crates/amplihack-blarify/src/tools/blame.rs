//! Blame info tool — get Git blame data for code nodes.
//!
//! Mirrors the Python `tools/get_blame_info.py`.

use std::collections::HashMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::db::manager::DbManager;
use crate::tools::dependency_graph::{DependencyGraphInput, resolve_reference_id};

/// Input for blame info retrieval.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlameInfoInput {
    #[serde(default)]
    pub reference_id: Option<String>,
    #[serde(default)]
    pub file_path: Option<String>,
    #[serde(default)]
    pub symbol_name: Option<String>,
}

/// Get blame information for a code node.
pub fn get_blame_info(db_manager: &dyn DbManager, input: &BlameInfoInput) -> Result<String> {
    let dep_input = DependencyGraphInput {
        reference_id: input.reference_id.clone(),
        file_path: input.file_path.clone(),
        symbol_name: input.symbol_name.clone(),
        depth: 1,
    };
    dep_input.validate()?;

    let node_id = resolve_reference_id(db_manager, &dep_input)?;
    let node_result = db_manager.get_node_by_id(&node_id)?;

    match node_result {
        Some(result) => Ok(format_blame_output(&result.node_name, &result.node_path)),
        None => Ok(format!("Node not found: {node_id}")),
    }
}

/// Format blame output in GitHub-style format.
fn format_blame_output(node_name: &str, node_path: &str) -> String {
    let mut lines = Vec::new();
    lines.push(format!("=== Blame Info: {node_name} ==="));
    lines.push(format!("File: {node_path}"));
    lines.push(String::new());
    lines.push("Note: Blame data requires VCS integration to be configured.".into());
    lines.push("Use the VCS controller to fetch blame data from the repository.".into());
    lines.join("\n")
}

/// Format a "time ago" string from a timestamp.
pub fn format_time_ago(timestamp: &str, ref_timestamp: Option<&str>) -> String {
    use chrono::{DateTime, Utc};

    let ts = match timestamp.parse::<DateTime<Utc>>() {
        Ok(t) => t,
        Err(_) => return timestamp.to_string(),
    };

    let now = match ref_timestamp {
        Some(r) => r.parse::<DateTime<Utc>>().unwrap_or_else(|_| Utc::now()),
        None => Utc::now(),
    };

    let diff = now.signed_duration_since(ts);
    let days = diff.num_days();

    if days == 0 {
        let hours = diff.num_hours();
        if hours == 0 {
            let minutes = diff.num_minutes();
            format!("{minutes} minutes ago")
        } else {
            format!("{hours} hours ago")
        }
    } else if days < 30 {
        format!("{days} days ago")
    } else if days < 365 {
        format!("{} months ago", days / 30)
    } else {
        format!("{} years ago", days / 365)
    }
}

/// Build a line-to-blame mapping.
pub fn build_line_blame_map(
    blame_data: &[HashMap<String, serde_json::Value>],
    start_line: i64,
    num_lines: i64,
) -> HashMap<i64, HashMap<String, serde_json::Value>> {
    let mut map = HashMap::new();

    for entry in blame_data {
        let ranges = entry
            .get("line_ranges")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        for range in ranges {
            let start = range.get("start").and_then(|v| v.as_i64()).unwrap_or(0);
            let end = range.get("end").and_then(|v| v.as_i64()).unwrap_or(0);

            for line in start..=end {
                if line >= start_line && line < start_line + num_lines {
                    map.insert(line, entry.clone());
                }
            }
        }
    }

    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_time_ago_days() {
        let result = format_time_ago("2024-01-01T00:00:00Z", Some("2024-01-15T00:00:00Z"));
        assert!(result.contains("14 days ago"));
    }

    #[test]
    fn format_time_ago_months() {
        let result = format_time_ago("2024-01-01T00:00:00Z", Some("2024-04-01T00:00:00Z"));
        assert!(result.contains("months ago"));
    }

    #[test]
    fn format_time_ago_invalid() {
        let result = format_time_ago("invalid", None);
        assert_eq!(result, "invalid");
    }

    #[test]
    fn build_blame_map_basic() {
        let mut entry = HashMap::new();
        entry.insert("sha".into(), serde_json::json!("abc123"));
        entry.insert(
            "line_ranges".into(),
            serde_json::json!([{"start": 5, "end": 10}]),
        );

        let map = build_line_blame_map(&[entry], 1, 20);
        assert!(map.contains_key(&5));
        assert!(map.contains_key(&10));
        assert!(!map.contains_key(&11));
    }

    #[test]
    fn blame_output_format() {
        let output = format_blame_output("my_func", "src/lib.rs");
        assert!(output.contains("my_func"));
        assert!(output.contains("src/lib.rs"));
    }
}
