//! Code analysis tool — detailed analysis of a code node.
//!
//! Mirrors the Python `tools/get_code_analysis.py`.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::db::manager::DbManager;
use crate::db::types::{EdgeDto, ReferenceSearchResultDto};
use crate::tools::dependency_graph::{DependencyGraphInput, resolve_reference_id};

/// Code-generated relationship types (not user-defined).
const CODE_GENERATED_RELATIONSHIPS: &[&str] = &[
    "CALLS",
    "REFERENCES",
    "IMPORTS",
    "FUNCTION_DEFINITION",
    "CLASS_DEFINITION",
    "CONTAINS",
];

/// Input for code analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeAnalysisInput {
    #[serde(default)]
    pub reference_id: Option<String>,
    #[serde(default)]
    pub file_path: Option<String>,
    #[serde(default)]
    pub symbol_name: Option<String>,
}

/// Format code with line numbers.
pub fn format_code_with_line_numbers(code: &str, start_line: Option<i64>) -> String {
    let base = start_line.unwrap_or(1);
    code.lines()
        .enumerate()
        .map(|(i, line)| format!("{:>4} | {}", base + i as i64, line))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Format relationships as a readable string.
pub fn format_relations(node_name: &str, relations: &[EdgeDto], direction: &str) -> String {
    if relations.is_empty() {
        return String::new();
    }

    let mut lines = vec![format!("{direction} relationships for '{node_name}':")];
    for rel in relations {
        if is_code_generated_relationship(&rel.relationship_type) {
            lines.push(format!(
                "  - [{}] {} ({})",
                rel.relationship_type,
                rel.node_name,
                rel.node_type.join(", ")
            ));
        }
    }
    lines.join("\n")
}

/// Check if a relationship type is code-generated.
pub fn is_code_generated_relationship(rel_type: &str) -> bool {
    CODE_GENERATED_RELATIONSHIPS.contains(&rel_type)
}

/// Get detailed code analysis for a node.
pub fn get_code_analysis(db_manager: &dyn DbManager, input: &CodeAnalysisInput) -> Result<String> {
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
        Some(result) => Ok(format_analysis_result(&result)),
        None => Ok(format!("Node not found: {node_id}")),
    }
}

/// Format a full analysis result as readable text.
fn format_analysis_result(result: &ReferenceSearchResultDto) -> String {
    let mut output = Vec::new();

    output.push(format!("=== Code Analysis: {} ===", result.node_name));
    output.push(format!("Path: {}", result.node_path));
    output.push(format!("Labels: {}", result.node_labels.join(", ")));

    if let (Some(start), Some(end)) = (result.start_line, result.end_line) {
        output.push(format!("Lines: {start}-{end}"));
    }

    output.push(String::new());
    output.push("--- Code ---".into());
    output.push(format_code_with_line_numbers(
        &result.code,
        result.start_line,
    ));

    if let Some(ref inbound) = result.inbound_relations {
        let formatted = format_relations(&result.node_name, inbound, "Inbound");
        if !formatted.is_empty() {
            output.push(String::new());
            output.push(formatted);
        }
    }

    if let Some(ref outbound) = result.outbound_relations {
        let formatted = format_relations(&result.node_name, outbound, "Outbound");
        if !formatted.is_empty() {
            output.push(String::new());
            output.push(formatted);
        }
    }

    if let Some(ref doc) = result.documentation {
        output.push(String::new());
        output.push("--- Documentation ---".into());
        output.push(doc.clone());
    }

    output.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_with_line_numbers() {
        let code = "fn main() {\n    println!(\"hello\");\n}";
        let result = format_code_with_line_numbers(code, Some(10));
        assert!(result.contains("  10 |"));
        assert!(result.contains("  11 |"));
        assert!(result.contains("  12 |"));
    }

    #[test]
    fn format_with_default_start() {
        let code = "line1\nline2";
        let result = format_code_with_line_numbers(code, None);
        assert!(result.starts_with("   1 |"));
    }

    #[test]
    fn code_generated_relationship_check() {
        assert!(is_code_generated_relationship("CALLS"));
        assert!(is_code_generated_relationship("IMPORTS"));
        assert!(!is_code_generated_relationship("CUSTOM_REL"));
    }

    #[test]
    fn format_empty_relations() {
        let result = format_relations("test", &[], "Inbound");
        assert!(result.is_empty());
    }

    #[test]
    fn format_analysis_output() {
        let result = ReferenceSearchResultDto {
            node_id: "abc".into(),
            node_name: "my_func".into(),
            node_labels: vec!["FUNCTION".into()],
            node_path: "src/lib.rs".into(),
            code: "fn my_func() {}".into(),
            start_line: Some(1),
            end_line: Some(1),
            file_path: None,
            inbound_relations: None,
            outbound_relations: None,
            documentation: Some("A test function".into()),
            workflows: None,
        };
        let output = format_analysis_result(&result);
        assert!(output.contains("my_func"));
        assert!(output.contains("FUNCTION"));
        assert!(output.contains("A test function"));
    }
}
