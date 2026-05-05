//! Dependency graph tool — generate Mermaid graphs from a node.
//!
//! Mirrors the Python `tools/get_dependency_graph.py`.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::db::manager::{DbManager, QueryParams};
use crate::db::queries;

/// Input parameters for get_dependency_graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependencyGraphInput {
    #[serde(default)]
    pub reference_id: Option<String>,
    #[serde(default)]
    pub file_path: Option<String>,
    #[serde(default)]
    pub symbol_name: Option<String>,
    #[serde(default = "default_depth")]
    pub depth: usize,
}

fn default_depth() -> usize {
    2
}

impl DependencyGraphInput {
    /// Validate that either reference_id or (file_path + symbol_name) are provided.
    pub fn validate(&self) -> Result<()> {
        if let Some(ref id) = self.reference_id {
            if id.len() != 32 {
                bail!("reference_id must be 32 characters, got {}", id.len());
            }
            Ok(())
        } else if self.file_path.is_some() && self.symbol_name.is_some() {
            Ok(())
        } else {
            bail!("Provide either reference_id (32 chars) or both file_path and symbol_name")
        }
    }
}

/// Resolve a node ID from flexible input.
pub fn resolve_reference_id(
    db_manager: &dyn DbManager,
    input: &DependencyGraphInput,
) -> Result<String> {
    if let Some(ref id) = input.reference_id {
        return Ok(id.clone());
    }

    let file_path = input.file_path.as_deref().unwrap_or("");
    let symbol_name = input.symbol_name.as_deref().unwrap_or("");

    let results = db_manager.get_node_by_name_and_type(symbol_name, "FUNCTION")?;
    for result in &results {
        if result.file_path == file_path {
            return Ok(result.node_id.clone());
        }
    }

    // Try CLASS type
    let results = db_manager.get_node_by_name_and_type(symbol_name, "CLASS")?;
    for result in &results {
        if result.file_path == file_path {
            return Ok(result.node_id.clone());
        }
    }

    bail!("Could not resolve symbol '{symbol_name}' in file '{file_path}'")
}

/// Generate a Mermaid dependency graph for a node.
pub fn get_dependency_graph(
    db_manager: &dyn DbManager,
    input: &DependencyGraphInput,
) -> Result<String> {
    input.validate()?;
    let node_id = resolve_reference_id(db_manager, input)?;
    get_mermaid_graph(db_manager, &node_id)
}

/// Generate a Mermaid graph from a starting node.
pub fn get_mermaid_graph(db_manager: &dyn DbManager, node_id: &str) -> Result<String> {
    let mut params = QueryParams::new();
    params.insert("node_id".into(), serde_json::Value::String(node_id.into()));
    params.insert(
        "entity_id".into(),
        serde_json::Value::String(db_manager.entity_id().into()),
    );
    params.insert(
        "repo_id".into(),
        serde_json::Value::String(db_manager.repo_id().into()),
    );

    let results = db_manager.query(queries::MERMAID_GRAPH_QUERY, Some(&params), false)?;

    let mut lines = vec!["graph TD".to_string()];
    for row in &results {
        let source = row
            .get("source_name")
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        let source_id = row.get("source_id").and_then(|v| v.as_str()).unwrap_or("?");
        let rel_type = row.get("rel_type").and_then(|v| v.as_str());
        let target = row.get("target_name").and_then(|v| v.as_str());
        let target_id = row.get("target_id").and_then(|v| v.as_str());

        if let (Some(rel), Some(tgt), Some(tid)) = (rel_type, target, target_id) {
            lines.push(format!(
                "    {source_id}[\"{source}\"] -->|{rel}| {tid}[\"{tgt}\"]"
            ));
        }
    }

    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_with_reference_id() {
        let input = DependencyGraphInput {
            reference_id: Some("a".repeat(32)),
            file_path: None,
            symbol_name: None,
            depth: 2,
        };
        assert!(input.validate().is_ok());
    }

    #[test]
    fn validate_with_file_and_symbol() {
        let input = DependencyGraphInput {
            reference_id: None,
            file_path: Some("src/main.rs".into()),
            symbol_name: Some("main".into()),
            depth: 2,
        };
        assert!(input.validate().is_ok());
    }

    #[test]
    fn validate_without_inputs() {
        let input = DependencyGraphInput {
            reference_id: None,
            file_path: None,
            symbol_name: None,
            depth: 2,
        };
        assert!(input.validate().is_err());
    }

    #[test]
    fn validate_short_reference_id() {
        let input = DependencyGraphInput {
            reference_id: Some("short".into()),
            file_path: None,
            symbol_name: None,
            depth: 2,
        };
        assert!(input.validate().is_err());
    }

    #[test]
    fn default_depth_is_two() {
        let input: DependencyGraphInput =
            serde_json::from_str(r#"{"reference_id": null}"#).unwrap();
        assert_eq!(input.depth, 2);
    }
}
