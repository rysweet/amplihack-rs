//! Workflows tool — discover and display workflows for a node.
//!
//! Mirrors the Python `tools/get_node_workflows_tool.py`.

use std::collections::HashMap;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::db::manager::{DbManager, QueryParams};

/// Input for node workflow discovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeWorkflowsInput {
    pub node_id: String,
}

impl NodeWorkflowsInput {
    pub fn validate(&self) -> Result<()> {
        if self.node_id.len() != 32 {
            bail!("node_id must be 32 characters, got {}", self.node_id.len());
        }
        Ok(())
    }
}

/// Get workflows that a node participates in.
pub fn get_node_workflows(
    db_manager: &dyn DbManager,
    input: &NodeWorkflowsInput,
) -> Result<String> {
    input.validate()?;

    let node_info = get_node_info(db_manager, &input.node_id)?;
    let node_name = match &node_info {
        Some(info) => info
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string(),
        None => return Ok(format!("Node not found: {}", input.node_id)),
    };

    let workflows = get_workflows_with_chains(db_manager, &input.node_id)?;

    if workflows.is_empty() {
        return Ok(format!(
            "No workflows found for '{node_name}' ({})",
            input.node_id
        ));
    }

    let mut output = Vec::new();
    output.push(format!("=== Workflows for '{node_name}' ==="));
    output.push(format!("Total workflows: {}", workflows.len()));

    for (i, workflow) in workflows.iter().enumerate() {
        output.push(String::new());
        output.push(format_workflow_section(workflow, i + 1));
    }

    output.push(String::new());
    output.push(format_summary(&workflows));

    Ok(output.join("\n"))
}

/// Get node info from the database.
fn get_node_info(
    db_manager: &dyn DbManager,
    node_id: &str,
) -> Result<Option<HashMap<String, serde_json::Value>>> {
    let result = db_manager.get_node_by_id(node_id)?;
    Ok(result.map(|r| {
        let mut map = HashMap::new();
        map.insert("name".into(), serde_json::json!(r.node_name));
        map.insert("path".into(), serde_json::json!(r.node_path));
        map.insert("labels".into(), serde_json::json!(r.node_labels));
        map
    }))
}

/// Get workflows with their execution chains.
fn get_workflows_with_chains(
    db_manager: &dyn DbManager,
    node_id: &str,
) -> Result<Vec<HashMap<String, serde_json::Value>>> {
    let query = r#"
        MATCH (n:NODE {node_id: $node_id})-[:BELONGS_TO_WORKFLOW]->(w:NODE {layer: 'workflows'})
        OPTIONAL MATCH (entry:NODE {node_id: w.entry_point_id})
        RETURN w.node_id AS workflow_id, w.name AS workflow_name,
               w.entry_point_id AS entry_point_id,
               entry.name AS entry_point_name,
               entry.path AS entry_point_path
    "#;

    let mut params = QueryParams::new();
    params.insert("node_id".into(), serde_json::Value::String(node_id.into()));

    db_manager.query(query, Some(&params), false)
}

/// Format a single workflow section.
fn format_workflow_section(workflow: &HashMap<String, serde_json::Value>, index: usize) -> String {
    let name = workflow
        .get("workflow_name")
        .and_then(|v| v.as_str())
        .unwrap_or("unnamed");
    let entry_name = workflow
        .get("entry_point_name")
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    let entry_path = workflow
        .get("entry_point_path")
        .and_then(|v| v.as_str())
        .unwrap_or("?");

    let mut lines = Vec::new();
    lines.push(format!("--- Workflow {index}: {name} ---"));
    lines.push(format!("Entry point: {entry_name} ({entry_path})"));
    lines.join("\n")
}

/// Format a summary of all workflows.
fn format_summary(workflows: &[HashMap<String, serde_json::Value>]) -> String {
    format!(
        "Summary: Node participates in {} workflow(s)",
        workflows.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_valid_node_id() {
        let input = NodeWorkflowsInput {
            node_id: "a".repeat(32),
        };
        assert!(input.validate().is_ok());
    }

    #[test]
    fn validate_invalid_node_id() {
        let input = NodeWorkflowsInput {
            node_id: "short".into(),
        };
        assert!(input.validate().is_err());
    }

    #[test]
    fn format_workflow_section_basic() {
        let mut workflow = HashMap::new();
        workflow.insert("workflow_name".into(), serde_json::json!("auth_flow"));
        workflow.insert("entry_point_name".into(), serde_json::json!("login"));
        workflow.insert("entry_point_path".into(), serde_json::json!("src/auth.rs"));

        let output = format_workflow_section(&workflow, 1);
        assert!(output.contains("auth_flow"));
        assert!(output.contains("login"));
    }

    #[test]
    fn format_summary_count() {
        let workflows = vec![HashMap::new(), HashMap::new()];
        let summary = format_summary(&workflows);
        assert!(summary.contains("2 workflow(s)"));
    }
}
