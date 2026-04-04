//! DTOs for graph database operations.
//!
//! Mirrors the Python `repositories/graph_db_manager/dtos/` types.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A relationship edge between two graph nodes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EdgeDto {
    pub relationship_type: String,
    pub node_id: String,
    pub node_name: String,
    pub node_type: Vec<String>,
}

/// A code graph node with full metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeNodeDto {
    pub id: String,
    pub name: String,
    pub label: String,
    pub path: String,
    pub start_line: i64,
    pub end_line: i64,
}

/// Full search result for a node reference, including relations and documentation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReferenceSearchResultDto {
    pub node_id: String,
    pub node_name: String,
    pub node_labels: Vec<String>,
    pub node_path: String,
    pub code: String,
    #[serde(default)]
    pub start_line: Option<i64>,
    #[serde(default)]
    pub end_line: Option<i64>,
    #[serde(default)]
    pub file_path: Option<String>,
    #[serde(default)]
    pub inbound_relations: Option<Vec<EdgeDto>>,
    #[serde(default)]
    pub outbound_relations: Option<Vec<EdgeDto>>,
    #[serde(default)]
    pub documentation: Option<String>,
    #[serde(default)]
    pub workflows: Option<Vec<HashMap<String, serde_json::Value>>>,
}

/// Node with its source content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeWithContentDto {
    pub id: String,
    pub name: String,
    pub labels: Vec<String>,
    pub path: String,
    pub start_line: Option<i64>,
    pub end_line: Option<i64>,
    pub content: String,
    #[serde(default)]
    pub relationship_type: Option<String>,
}

/// A leaf node in the graph (no children).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeafNodeDto {
    pub id: String,
    pub name: String,
    pub labels: Vec<String>,
    pub path: String,
    pub start_line: Option<i64>,
    pub end_line: Option<i64>,
    pub content: String,
}

/// Node found by name and type search.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeFoundByNameTypeDto {
    pub node_id: String,
    pub node_name: String,
    pub node_type: Vec<String>,
    pub file_path: String,
    #[serde(default)]
    pub code: Option<String>,
}

/// Node found by path search.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeFoundByPathDto {
    pub node_id: String,
    pub name: String,
    pub label: String,
    pub node_path: String,
}

/// Node found by text content search.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeFoundByTextDto {
    pub id: String,
    pub name: String,
    pub label: String,
    pub diff_text: String,
    pub relevant_snippet: String,
    pub node_path: String,
}

/// Documentation search result with similarity score.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentationSearchResultDto {
    pub node_id: String,
    pub title: String,
    pub content: String,
    pub similarity_score: f64,
    pub source_path: String,
    pub source_labels: Vec<String>,
    pub info_type: String,
    #[serde(default)]
    pub enhanced_content: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edge_dto_roundtrip() {
        let edge = EdgeDto {
            relationship_type: "CALLS".into(),
            node_id: "abc123".into(),
            node_name: "my_func".into(),
            node_type: vec!["FUNCTION".into()],
        };
        let json = serde_json::to_string(&edge).unwrap();
        let deser: EdgeDto = serde_json::from_str(&json).unwrap();
        assert_eq!(edge, deser);
    }

    #[test]
    fn code_node_dto_roundtrip() {
        let node = CodeNodeDto {
            id: "n1".into(),
            name: "main".into(),
            label: "FUNCTION".into(),
            path: "src/main.rs".into(),
            start_line: 1,
            end_line: 50,
        };
        let json = serde_json::to_string(&node).unwrap();
        let deser: CodeNodeDto = serde_json::from_str(&json).unwrap();
        assert_eq!(node, deser);
    }

    #[test]
    fn reference_search_result_optional_fields() {
        let json = r#"{
            "node_id": "x",
            "node_name": "foo",
            "node_labels": ["FUNCTION"],
            "node_path": "src/foo.rs",
            "code": "fn foo() {}"
        }"#;
        let result: ReferenceSearchResultDto = serde_json::from_str(json).unwrap();
        assert_eq!(result.node_id, "x");
        assert!(result.start_line.is_none());
        assert!(result.inbound_relations.is_none());
        assert!(result.documentation.is_none());
    }

    #[test]
    fn leaf_node_dto_serialization() {
        let leaf = LeafNodeDto {
            id: "leaf1".into(),
            name: "helper".into(),
            labels: vec!["FUNCTION".into()],
            path: "src/util.rs".into(),
            start_line: Some(10),
            end_line: Some(20),
            content: "fn helper() {}".into(),
        };
        let json = serde_json::to_string(&leaf).unwrap();
        assert!(json.contains("\"helper\""));
    }

    #[test]
    fn documentation_search_result_with_enhanced() {
        let dto = DocumentationSearchResultDto {
            node_id: "doc1".into(),
            title: "Auth module".into(),
            content: "Handles authentication".into(),
            similarity_score: 0.95,
            source_path: "src/auth.rs".into(),
            source_labels: vec!["FILE".into()],
            info_type: "documentation".into(),
            enhanced_content: Some("Extended description".into()),
        };
        assert!(dto.similarity_score > 0.9);
        assert!(dto.enhanced_content.is_some());
    }
}
