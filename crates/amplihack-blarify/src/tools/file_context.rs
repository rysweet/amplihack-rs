//! File context tool — view a node in its file context.
//!
//! Mirrors the Python `tools/get_file_context_tool.py`.

use std::collections::{HashMap, HashSet};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::db::manager::{DbManager, QueryParams};
use crate::db::queries;

/// Input for file context retrieval.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileContextInput {
    pub node_id: String,
}

impl FileContextInput {
    pub fn validate(&self) -> Result<()> {
        if self.node_id.len() != 32 {
            bail!("node_id must be 32 characters, got {}", self.node_id.len());
        }
        Ok(())
    }
}

/// Recursively inject code for placeholder references.
///
/// Replaces `# Code replaced for brevity, see node: <id>` with actual code,
/// preserving indentation.
pub fn recursively_inject_code(
    code: &str,
    node_map: &HashMap<String, String>,
    visited: &mut HashSet<String>,
) -> String {
    let mut result = String::new();
    for line in code.lines() {
        if let Some(pos) = line.find("# Code replaced for brevity, see node: ") {
            let id_start = pos + "# Code replaced for brevity, see node: ".len();
            let node_id = line[id_start..].trim();
            if !visited.contains(node_id) {
                visited.insert(node_id.to_string());
                if let Some(child_code) = node_map.get(node_id) {
                    let indent = &line[..pos];
                    let injected = recursively_inject_code(child_code, node_map, visited);
                    for (i, injected_line) in injected.lines().enumerate() {
                        if i == 0 {
                            result.push_str(indent);
                            result.push_str(injected_line);
                        } else {
                            result.push('\n');
                            result.push_str(indent);
                            result.push_str(injected_line);
                        }
                    }
                    result.push('\n');
                    continue;
                }
            }
        }
        result.push_str(line);
        result.push('\n');
    }
    result
}

/// Assemble source from a parent chain.
pub fn assemble_source_from_chain(chain: &[(String, String)]) -> String {
    if chain.is_empty() {
        return String::new();
    }

    let mut node_map = HashMap::new();
    for (id, code) in chain.iter().rev().skip(1) {
        node_map.insert(id.clone(), code.clone());
    }

    let parent_code = &chain.last().unwrap().1;
    let mut visited = HashSet::new();
    recursively_inject_code(parent_code, &node_map, &mut visited)
}

/// Get file context for a node by ID.
pub fn get_file_context(
    db_manager: &dyn DbManager,
    input: &FileContextInput,
) -> Result<serde_json::Value> {
    input.validate()?;

    let mut params = QueryParams::new();
    params.insert(
        "node_id".into(),
        serde_json::Value::String(input.node_id.clone()),
    );
    params.insert(
        "entity_id".into(),
        serde_json::Value::String(db_manager.entity_id().into()),
    );
    params.insert(
        "repo_id".into(),
        serde_json::Value::String(db_manager.repo_id().into()),
    );

    let results = db_manager.query(queries::FILE_CONTEXT_BY_ID_QUERY, Some(&params), false)?;

    let chain: Vec<(String, String)> = results
        .iter()
        .filter_map(|r| {
            let id = r.get("id").and_then(|v| v.as_str())?.to_string();
            let content = r.get("content").and_then(|v| v.as_str())?.to_string();
            Some((id, content))
        })
        .collect();

    let assembled = assemble_source_from_chain(&chain);
    Ok(serde_json::json!({"text": assembled}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inject_simple_placeholder() {
        let code = "def main():\n    # Code replaced for brevity, see node: abc123\n    pass";
        let mut map = HashMap::new();
        map.insert("abc123".into(), "print('hello')".into());
        let mut visited = HashSet::new();
        let result = recursively_inject_code(code, &map, &mut visited);
        assert!(result.contains("print('hello')"));
        assert!(!result.contains("# Code replaced for brevity"));
    }

    #[test]
    fn inject_avoids_cycles() {
        let code = "# Code replaced for brevity, see node: self_ref";
        let mut map = HashMap::new();
        map.insert(
            "self_ref".into(),
            "# Code replaced for brevity, see node: self_ref".into(),
        );
        let mut visited = HashSet::new();
        let result = recursively_inject_code(code, &map, &mut visited);
        // Should not infinitely recurse
        assert!(!result.is_empty());
    }

    #[test]
    fn assemble_empty_chain() {
        let result = assemble_source_from_chain(&[]);
        assert!(result.is_empty());
    }

    #[test]
    fn validate_node_id_length() {
        let input = FileContextInput {
            node_id: "short".into(),
        };
        assert!(input.validate().is_err());

        let input = FileContextInput {
            node_id: "a".repeat(32),
        };
        assert!(input.validate().is_ok());
    }
}
