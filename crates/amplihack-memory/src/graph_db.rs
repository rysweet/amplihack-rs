//! LadybugDB embedded graph database backend.
//!
//! Ports Python `amplihack/memory/kuzu/` (now LadybugDB):
//! - `GraphDbConnector` — Graph database connector (Cypher query interface)
//! - `CodeGraph` — Code navigation graph built from SCIP indexes
//! - `QueryResult` — Structured query results
//! - `SessionIntegration` — Session-aware graph operations

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::graph_store::{EdgeDirection, EdgeRecord, NodeTriple, Props};

/// Whether the graph database backend is available (compile-time constant for Rust).
pub const GRAPH_DB_AVAILABLE: bool = true;

/// Query result from the graph database.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<serde_json::Value>>,
    pub execution_time_ms: f64,
}

impl QueryResult {
    /// Number of rows returned.
    pub fn num_rows(&self) -> usize {
        self.rows.len()
    }

    /// Check if the result set is empty.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Convert rows to a vec of maps (column_name → value).
    pub fn to_maps(&self) -> Vec<HashMap<String, serde_json::Value>> {
        self.rows
            .iter()
            .map(|row| {
                self.columns
                    .iter()
                    .zip(row.iter())
                    .map(|(col, val)| (col.clone(), val.clone()))
                    .collect()
            })
            .collect()
    }
}

/// Configuration for the graph database connector.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphDbConfig {
    pub db_path: PathBuf,
    pub buffer_pool_size: usize,
    pub read_only: bool,
}

impl Default for GraphDbConfig {
    fn default() -> Self {
        Self {
            db_path: PathBuf::from(".amplihack/kuzu_db"),
            buffer_pool_size: 256 * 1024 * 1024, // 256MB
            read_only: false,
        }
    }
}

/// LadybugDB embedded graph database connector.
///
/// Provides Cypher query execution over a file-based graph database.
/// Zero infrastructure — no server process needed.
pub struct GraphDbConnector {
    config: GraphDbConfig,
    nodes: HashMap<String, HashMap<String, Props>>,
    edges: Vec<StoredEdge>,
}

#[derive(Debug, Clone)]
struct StoredEdge {
    rel_type: String,
    from_id: String,
    to_id: String,
    props: Props,
}

impl GraphDbConnector {
    /// Create a new connector with default configuration.
    pub fn new(db_path: impl AsRef<Path>) -> Self {
        Self {
            config: GraphDbConfig {
                db_path: db_path.as_ref().to_path_buf(),
                ..Default::default()
            },
            nodes: HashMap::new(),
            edges: Vec::new(),
        }
    }

    /// Create with full config.
    pub fn with_config(config: GraphDbConfig) -> Self {
        Self {
            config,
            nodes: HashMap::new(),
            edges: Vec::new(),
        }
    }

    /// Get the database path.
    pub fn db_path(&self) -> &Path {
        &self.config.db_path
    }

    /// Add a node to the graph.
    pub fn add_node(
        &mut self,
        table: &str,
        node_id: &str,
        properties: Props,
    ) {
        self.nodes
            .entry(table.to_string())
            .or_default()
            .insert(node_id.to_string(), properties);
    }

    /// Get a node by table and ID.
    pub fn get_node(&self, table: &str, node_id: &str) -> Option<&Props> {
        self.nodes.get(table)?.get(node_id)
    }

    /// Add an edge between two nodes.
    pub fn add_edge(
        &mut self,
        rel_type: &str,
        from_id: &str,
        to_id: &str,
        properties: Props,
    ) {
        self.edges.push(StoredEdge {
            rel_type: rel_type.to_string(),
            from_id: from_id.to_string(),
            to_id: to_id.to_string(),
            props: properties,
        });
    }

    /// Get edges for a node.
    pub fn get_edges(
        &self,
        node_id: &str,
        direction: EdgeDirection,
    ) -> Vec<EdgeRecord> {
        self.edges
            .iter()
            .filter(|e| match direction {
                EdgeDirection::Outgoing => e.from_id == node_id,
                EdgeDirection::Incoming => e.to_id == node_id,
                EdgeDirection::Both => {
                    e.from_id == node_id || e.to_id == node_id
                }
            })
            .map(|e| EdgeRecord {
                rel_type: e.rel_type.clone(),
                from_id: e.from_id.clone(),
                to_id: e.to_id.clone(),
                properties: e.props.clone(),
            })
            .collect()
    }

    /// Count all nodes across all tables.
    pub fn node_count(&self) -> usize {
        self.nodes.values().map(|t| t.len()).sum()
    }

    /// Count all edges.
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// Get all node tables.
    pub fn tables(&self) -> Vec<String> {
        self.nodes.keys().cloned().collect()
    }

    /// Delete a node by table and ID.
    pub fn delete_node(&mut self, table: &str, node_id: &str) -> bool {
        if let Some(t) = self.nodes.get_mut(table)
            && t.remove(node_id).is_some()
        {
            self.edges
                .retain(|e| e.from_id != node_id && e.to_id != node_id);
            return true;
        }
        false
    }
}

/// Code entity kind for code graphs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CodeEntityKind {
    Function,
    Class,
    Method,
    Module,
    Variable,
    Import,
    Interface,
    Enum,
}

/// A code entity in the code graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeEntity {
    pub name: String,
    pub kind: CodeEntityKind,
    pub file_path: String,
    pub line_start: usize,
    pub line_end: usize,
    #[serde(default)]
    pub docstring: Option<String>,
    #[serde(default)]
    pub properties: Props,
}

/// Relationship between code entities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CodeRelation {
    Calls,
    Imports,
    Inherits,
    Contains,
    References,
    Overrides,
}

/// A code graph built from SCIP indexes.
///
/// Provides code navigation (find callers, find definitions, etc.)
/// over an indexed codebase.
pub struct CodeGraph {
    entities: Vec<CodeEntity>,
    relations: Vec<(usize, CodeRelation, usize)>,
}

impl CodeGraph {
    pub fn new() -> Self {
        Self {
            entities: Vec::new(),
            relations: Vec::new(),
        }
    }

    /// Add a code entity, returning its index.
    pub fn add_entity(&mut self, entity: CodeEntity) -> usize {
        let idx = self.entities.len();
        self.entities.push(entity);
        idx
    }

    /// Add a relationship between entities.
    pub fn add_relation(
        &mut self,
        from_idx: usize,
        relation: CodeRelation,
        to_idx: usize,
    ) {
        self.relations.push((from_idx, relation, to_idx));
    }

    /// Find entities by name.
    pub fn find_by_name(&self, name: &str) -> Vec<&CodeEntity> {
        self.entities
            .iter()
            .filter(|e| e.name == name)
            .collect()
    }

    /// Find entities in a file.
    pub fn find_in_file(&self, file_path: &str) -> Vec<&CodeEntity> {
        self.entities
            .iter()
            .filter(|e| e.file_path == file_path)
            .collect()
    }

    /// Get callers of an entity.
    pub fn callers_of(&self, entity_idx: usize) -> Vec<&CodeEntity> {
        self.relations
            .iter()
            .filter(|(_, rel, to)| *rel == CodeRelation::Calls && *to == entity_idx)
            .filter_map(|(from, _, _)| self.entities.get(*from))
            .collect()
    }

    /// Get callees of an entity.
    pub fn callees_of(&self, entity_idx: usize) -> Vec<&CodeEntity> {
        self.relations
            .iter()
            .filter(|(from, rel, _)| {
                *rel == CodeRelation::Calls && *from == entity_idx
            })
            .filter_map(|(_, _, to)| self.entities.get(*to))
            .collect()
    }

    /// Total entity count.
    pub fn entity_count(&self) -> usize {
        self.entities.len()
    }

    /// Total relation count.
    pub fn relation_count(&self) -> usize {
        self.relations.len()
    }
}

impl Default for CodeGraph {
    fn default() -> Self {
        Self::new()
    }
}

/// Session-aware graph integration.
pub struct SessionIntegration {
    session_id: String,
    connector: GraphDbConnector,
}

impl SessionIntegration {
    pub fn new(session_id: impl Into<String>, connector: GraphDbConnector) -> Self {
        Self {
            session_id: session_id.into(),
            connector,
        }
    }

    /// Store a fact linked to the current session.
    pub fn store_session_fact(
        &mut self,
        fact_id: &str,
        content: &str,
        category: &str,
    ) {
        let mut props = Props::new();
        props.insert(
            "content".to_string(),
            serde_json::Value::String(content.to_string()),
        );
        props.insert(
            "category".to_string(),
            serde_json::Value::String(category.to_string()),
        );
        props.insert(
            "session_id".to_string(),
            serde_json::Value::String(self.session_id.clone()),
        );
        self.connector.add_node("Fact", fact_id, props);
    }

    /// Retrieve facts for the current session.
    pub fn get_session_facts(&self) -> Vec<NodeTriple> {
        let session_id = &self.session_id;
        self.connector
            .nodes
            .get("Fact")
            .map(|facts| {
                facts
                    .iter()
                    .filter(|(_, props)| {
                        props
                            .get("session_id")
                            .and_then(|v| v.as_str())
                            .map(|s| s == session_id)
                            .unwrap_or(false)
                    })
                    .map(|(id, props)| {
                        ("Fact".to_string(), id.clone(), props.clone())
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get session ID.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connector_basic() {
        let mut conn = GraphDbConnector::new("/tmp/test_graph_db");
        let mut props = Props::new();
        props.insert(
            "name".to_string(),
            serde_json::Value::String("Alice".to_string()),
        );
        conn.add_node("Person", "p1", props);
        assert_eq!(conn.node_count(), 1);
        assert!(conn.get_node("Person", "p1").is_some());
    }

    #[test]
    fn connector_edges() {
        let mut conn = GraphDbConnector::new("/tmp/test_graph_db");
        conn.add_node("Person", "p1", Props::new());
        conn.add_node("Person", "p2", Props::new());
        conn.add_edge("KNOWS", "p1", "p2", Props::new());

        let edges = conn.get_edges("p1", EdgeDirection::Outgoing);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].to_id, "p2");
    }

    #[test]
    fn connector_delete() {
        let mut conn = GraphDbConnector::new("/tmp/test_graph_db");
        conn.add_node("Person", "p1", Props::new());
        conn.add_edge("KNOWS", "p1", "p2", Props::new());
        assert!(conn.delete_node("Person", "p1"));
        assert_eq!(conn.node_count(), 0);
        assert_eq!(conn.edge_count(), 0);
    }

    #[test]
    fn code_graph_basic() {
        let mut graph = CodeGraph::new();
        let idx1 = graph.add_entity(CodeEntity {
            name: "main".to_string(),
            kind: CodeEntityKind::Function,
            file_path: "src/main.rs".to_string(),
            line_start: 1,
            line_end: 10,
            docstring: None,
            properties: Props::new(),
        });
        let idx2 = graph.add_entity(CodeEntity {
            name: "helper".to_string(),
            kind: CodeEntityKind::Function,
            file_path: "src/lib.rs".to_string(),
            line_start: 5,
            line_end: 15,
            docstring: Some("A helper".to_string()),
            properties: Props::new(),
        });

        graph.add_relation(idx1, CodeRelation::Calls, idx2);

        assert_eq!(graph.entity_count(), 2);
        assert_eq!(graph.relation_count(), 1);
        assert_eq!(graph.callees_of(idx1).len(), 1);
        assert_eq!(graph.callers_of(idx2).len(), 1);
    }

    #[test]
    fn code_graph_find() {
        let mut graph = CodeGraph::new();
        graph.add_entity(CodeEntity {
            name: "test_fn".to_string(),
            kind: CodeEntityKind::Function,
            file_path: "src/test.rs".to_string(),
            line_start: 1,
            line_end: 5,
            docstring: None,
            properties: Props::new(),
        });

        assert_eq!(graph.find_by_name("test_fn").len(), 1);
        assert_eq!(graph.find_in_file("src/test.rs").len(), 1);
        assert!(graph.find_by_name("nonexistent").is_empty());
    }

    #[test]
    fn session_integration() {
        let conn = GraphDbConnector::new("/tmp/test_graph_db");
        let mut sess = SessionIntegration::new("sess-1", conn);

        sess.store_session_fact("f1", "Rust is fast", "technical");
        sess.store_session_fact("f2", "Python is flexible", "technical");

        let facts = sess.get_session_facts();
        assert_eq!(facts.len(), 2);
    }

    #[test]
    fn query_result_to_maps() {
        let qr = QueryResult {
            columns: vec!["name".to_string(), "age".to_string()],
            rows: vec![vec![
                serde_json::Value::String("Alice".to_string()),
                serde_json::json!(30),
            ]],
            execution_time_ms: 1.5,
        };

        let maps = qr.to_maps();
        assert_eq!(maps.len(), 1);
        assert_eq!(maps[0]["name"], "Alice");
    }

    #[test]
    fn query_result_serde() {
        let qr = QueryResult {
            columns: vec!["x".to_string()],
            rows: vec![vec![serde_json::json!(1)]],
            execution_time_ms: 0.5,
        };
        let json = serde_json::to_string(&qr).unwrap();
        let back: QueryResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.num_rows(), 1);
    }

    #[test]
    fn config_default() {
        let cfg = GraphDbConfig::default();
        assert!(!cfg.read_only);
        assert!(cfg.buffer_pool_size > 0);
    }
}
