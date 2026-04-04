//! Commit tool — look up commit details by SHA.
//!
//! Mirrors the Python `tools/get_commit_by_id_tool.py`.

use std::collections::HashMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::db::manager::{DbManager, QueryParams};

/// Input for commit lookup.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitInput {
    pub commit_sha: String,
}

/// Get commit information by SHA.
pub fn get_commit_by_id(db_manager: &dyn DbManager, input: &CommitInput) -> Result<String> {
    let info = get_commit_info(db_manager, &input.commit_sha)?;
    match info {
        Some(data) => Ok(format_commit_output(&data)),
        None => Ok(format!("Commit not found: {}", input.commit_sha)),
    }
}

/// Query commit info from the graph database.
fn get_commit_info(
    db_manager: &dyn DbManager,
    commit_sha: &str,
) -> Result<Option<HashMap<String, serde_json::Value>>> {
    let query = r#"
        MATCH (c:COMMIT {sha: $sha})
        OPTIONAL MATCH (c)-[:PART_OF]->(pr:PR)
        OPTIONAL MATCH (c)-[:MODIFIES]->(n:NODE)
        RETURN c.sha AS sha, c.message AS message, c.author AS author,
               c.timestamp AS timestamp, c.url AS url,
               c.additions AS additions, c.deletions AS deletions,
               c.patch AS patch,
               pr.number AS pr_number, pr.title AS pr_title, pr.url AS pr_url,
               collect(DISTINCT {id: n.node_id, name: n.name, path: n.path}) AS affected_nodes
    "#;

    let mut params = QueryParams::new();
    params.insert("sha".into(), serde_json::Value::String(commit_sha.into()));

    let results = db_manager.query(query, Some(&params), false)?;
    Ok(results.into_iter().next())
}

/// Format commit data as readable output.
fn format_commit_output(data: &HashMap<String, serde_json::Value>) -> String {
    let mut lines = Vec::new();

    let sha = data
        .get("sha")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let message = data.get("message").and_then(|v| v.as_str()).unwrap_or("");
    let author = data
        .get("author")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let timestamp = data.get("timestamp").and_then(|v| v.as_str()).unwrap_or("");

    lines.push(format!("=== Commit {sha} ==="));
    lines.push(format!("Author: {author}"));
    lines.push(format!("Date: {timestamp}"));
    lines.push(format!("Message: {message}"));

    // PR information
    if let Some(pr_number) = data.get("pr_number").and_then(|v| v.as_i64()) {
        let pr_title = data.get("pr_title").and_then(|v| v.as_str()).unwrap_or("");
        let pr_url = data.get("pr_url").and_then(|v| v.as_str()).unwrap_or("");
        lines.push(String::new());
        lines.push(format!("PR #{pr_number}: {pr_title}"));
        lines.push(format!("PR URL: {pr_url}"));
    }

    // Stats
    let additions = data.get("additions").and_then(|v| v.as_i64()).unwrap_or(0);
    let deletions = data.get("deletions").and_then(|v| v.as_i64()).unwrap_or(0);
    lines.push(String::new());
    lines.push(format!("Stats: +{additions} -{deletions}"));

    // Affected nodes
    if let Some(nodes) = data.get("affected_nodes").and_then(|v| v.as_array())
        && !nodes.is_empty()
    {
        lines.push(String::new());
        lines.push("Affected code nodes:".into());
        for node in nodes {
            let name = node.get("name").and_then(|v| v.as_str()).unwrap_or("?");
            let path = node.get("path").and_then(|v| v.as_str()).unwrap_or("?");
            lines.push(format!("  - {name} ({path})"));
        }
    }

    // Patch
    if let Some(patch) = data.get("patch").and_then(|v| v.as_str())
        && !patch.is_empty()
    {
        lines.push(String::new());
        lines.push("--- Diff ---".into());
        lines.push(patch.to_string());
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_basic_commit() {
        let mut data = HashMap::new();
        data.insert("sha".into(), serde_json::json!("abc123"));
        data.insert("message".into(), serde_json::json!("Fix bug"));
        data.insert("author".into(), serde_json::json!("dev"));
        data.insert("timestamp".into(), serde_json::json!("2024-01-01"));
        data.insert("additions".into(), serde_json::json!(10));
        data.insert("deletions".into(), serde_json::json!(5));

        let output = format_commit_output(&data);
        assert!(output.contains("abc123"));
        assert!(output.contains("Fix bug"));
        assert!(output.contains("+10 -5"));
    }

    #[test]
    fn format_commit_with_pr() {
        let mut data = HashMap::new();
        data.insert("sha".into(), serde_json::json!("def456"));
        data.insert("message".into(), serde_json::json!("Add feature"));
        data.insert("author".into(), serde_json::json!("dev"));
        data.insert("timestamp".into(), serde_json::json!("2024-01-01"));
        data.insert("pr_number".into(), serde_json::json!(42));
        data.insert("pr_title".into(), serde_json::json!("Feature PR"));
        data.insert(
            "pr_url".into(),
            serde_json::json!("https://github.com/r/p/pull/42"),
        );

        let output = format_commit_output(&data);
        assert!(output.contains("PR #42"));
        assert!(output.contains("Feature PR"));
    }

    #[test]
    fn commit_input_serialization() {
        let input = CommitInput {
            commit_sha: "abc123def456".into(),
        };
        let json = serde_json::to_string(&input).unwrap();
        assert!(json.contains("abc123def456"));
    }
}
