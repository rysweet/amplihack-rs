//! Expanded context tool — view a node with full context.
//!
//! Mirrors the Python `tools/get_expanded_context.py`.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::db::manager::DbManager;
use crate::tools::code_analysis::format_code_with_line_numbers;
use crate::tools::dependency_graph::{DependencyGraphInput, resolve_reference_id};
use crate::tools::file_context::{FileContextInput, get_file_context};

/// Input for expanded context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpandedContextInput {
    #[serde(default)]
    pub reference_id: Option<String>,
    #[serde(default)]
    pub file_path: Option<String>,
    #[serde(default)]
    pub symbol_name: Option<String>,
}

/// Get expanded context for a code node.
///
/// Includes collapsed code view, file context, relationships, and documentation.
pub fn get_expanded_context(
    db_manager: &dyn DbManager,
    input: &ExpandedContextInput,
) -> Result<String> {
    let dep_input = DependencyGraphInput {
        reference_id: input.reference_id.clone(),
        file_path: input.file_path.clone(),
        symbol_name: input.symbol_name.clone(),
        depth: 1,
    };
    dep_input.validate()?;

    let node_id = resolve_reference_id(db_manager, &dep_input)?;
    let node_result = db_manager.get_node_by_id(&node_id)?;

    let result = match node_result {
        Some(ref res) => res,
        None => return Ok(format!("Node not found: {node_id}")),
    };

    let mut output = Vec::new();

    // Header
    output.push(format!("=== Expanded Context: {} ===", result.node_name));
    output.push(format!("Path: {}", result.node_path));
    output.push(format!("Labels: {}", result.node_labels.join(", ")));

    // Code (collapsed view)
    output.push(String::new());
    output.push("--- Code (collapsed) ---".into());
    output.push(format_code_with_line_numbers(
        &result.code,
        result.start_line,
    ));

    // File context
    if node_id.len() == 32 {
        let ctx_input = FileContextInput {
            node_id: node_id.clone(),
        };
        if let Ok(ctx) = get_file_context(db_manager, &ctx_input)
            && let Some(text) = ctx.get("text").and_then(|v| v.as_str())
            && !text.is_empty()
        {
            output.push(String::new());
            output.push("--- File Context ---".into());
            output.push(text.to_string());
        }
    }

    // Relations
    if let Some(ref outbound) = result.outbound_relations
        && !outbound.is_empty()
    {
        output.push(String::new());
        output.push("--- Outbound Relations ---".into());
        for rel in outbound {
            output.push(format!(
                "  {} -> {} ({})",
                rel.relationship_type,
                rel.node_name,
                rel.node_type.join(", ")
            ));
        }
    }

    if let Some(ref inbound) = result.inbound_relations
        && !inbound.is_empty()
    {
        output.push(String::new());
        output.push("--- Inbound Relations ---".into());
        for rel in inbound {
            output.push(format!(
                "  {} <- {} ({})",
                rel.relationship_type,
                rel.node_name,
                rel.node_type.join(", ")
            ));
        }
    }

    // Documentation
    if let Some(ref doc) = result.documentation {
        output.push(String::new());
        output.push("--- Documentation ---".into());
        output.push(doc.clone());
    }

    Ok(output.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expanded_context_input_defaults() {
        let json = r#"{}"#;
        let input: ExpandedContextInput = serde_json::from_str(json).unwrap();
        assert!(input.reference_id.is_none());
        assert!(input.file_path.is_none());
        assert!(input.symbol_name.is_none());
    }

    #[test]
    fn expanded_context_input_with_ref() {
        let input = ExpandedContextInput {
            reference_id: Some("a".repeat(32)),
            file_path: None,
            symbol_name: None,
        };
        let dep = DependencyGraphInput {
            reference_id: input.reference_id.clone(),
            file_path: None,
            symbol_name: None,
            depth: 1,
        };
        assert!(dep.validate().is_ok());
    }

    #[test]
    fn expanded_context_input_serialization() {
        let input = ExpandedContextInput {
            reference_id: None,
            file_path: Some("src/lib.rs".into()),
            symbol_name: Some("my_func".into()),
        };
        let json = serde_json::to_string(&input).unwrap();
        assert!(json.contains("src/lib.rs"));
    }
}
