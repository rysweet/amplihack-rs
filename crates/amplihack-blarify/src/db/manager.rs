//! Graph database manager trait and environment.
//!
//! Mirrors the Python `repositories/graph_db_manager/db_manager.py`.

use std::collections::HashMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::types::{NodeFoundByNameTypeDto, ReferenceSearchResultDto};
use crate::graph::node::Node;
use crate::graph::relationship::Relationship;

/// Database environment (main vs dev/branch).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DbEnvironment {
    Main,
    Dev,
}

impl std::fmt::Display for DbEnvironment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Main => write!(f, "main"),
            Self::Dev => write!(f, "dev"),
        }
    }
}

/// Query parameters for Cypher queries.
pub type QueryParams = HashMap<String, serde_json::Value>;

/// Abstract graph database manager trait.
///
/// Implementations handle Neo4j, FalkorDB, or embedded Kuzu backends.
pub trait DbManager: Send + Sync {
    /// Close the database connection.
    fn close(&self) -> Result<()>;

    /// Save a complete graph (nodes + edges) to the database.
    fn save_graph(&self, nodes: &[Node], edges: &[Relationship]) -> Result<()>;

    /// Create nodes in the database.
    fn create_nodes(&self, nodes: &[Node]) -> Result<()>;

    /// Create edges between nodes in the database.
    fn create_edges(&self, edges: &[Relationship]) -> Result<()>;

    /// Delete nodes matching the given path (and their relationships).
    fn detach_delete_nodes_with_path(&self, path: &str) -> Result<()>;

    /// Execute a Cypher query and return results.
    fn query(
        &self,
        cypher_query: &str,
        parameters: Option<&QueryParams>,
        transaction: bool,
    ) -> Result<Vec<HashMap<String, serde_json::Value>>>;

    /// Retrieve a node by its unique ID.
    fn get_node_by_id(&self, node_id: &str) -> Result<Option<ReferenceSearchResultDto>>;

    /// Retrieve nodes matching name and type.
    fn get_node_by_name_and_type(
        &self,
        name: &str,
        node_type: &str,
    ) -> Result<Vec<NodeFoundByNameTypeDto>>;

    /// Get the entity ID for this manager instance.
    fn entity_id(&self) -> &str;

    /// Get the repository ID for this manager instance.
    fn repo_id(&self) -> &str;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_environment_display() {
        assert_eq!(DbEnvironment::Main.to_string(), "main");
        assert_eq!(DbEnvironment::Dev.to_string(), "dev");
    }

    #[test]
    fn db_environment_roundtrip() {
        let json = serde_json::to_string(&DbEnvironment::Main).unwrap();
        let deser: DbEnvironment = serde_json::from_str(&json).unwrap();
        assert_eq!(deser, DbEnvironment::Main);
    }

    #[test]
    fn query_params_construction() {
        let mut params = QueryParams::new();
        params.insert("entity_id".into(), serde_json::Value::String("e1".into()));
        params.insert("repo_id".into(), serde_json::Value::String("r1".into()));
        assert_eq!(params.len(), 2);
    }

    #[test]
    fn db_environment_variants_are_distinct() {
        assert_ne!(DbEnvironment::Main, DbEnvironment::Dev);
    }
}
