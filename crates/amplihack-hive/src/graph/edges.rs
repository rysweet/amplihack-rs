use std::collections::HashMap;

use crate::models::HiveEdge;

use super::HiveGraph;

impl HiveGraph {
    /// Add a directed edge between two nodes.
    pub fn add_edge(
        &mut self,
        source_id: impl Into<String>,
        target_id: impl Into<String>,
        edge_type: impl Into<String>,
        properties: HashMap<String, String>,
    ) {
        self.edges.push(HiveEdge {
            source_id: source_id.into(),
            target_id: target_id.into(),
            edge_type: edge_type.into(),
            properties,
        });
    }

    /// Get all edges where `node_id` is source or target.
    pub fn get_edges(&self, node_id: &str) -> Vec<&HiveEdge> {
        self.edges
            .iter()
            .filter(|e| e.source_id == node_id || e.target_id == node_id)
            .collect()
    }

    /// Get edges from a specific source with a specific type.
    pub fn get_edges_from(&self, source_id: &str, edge_type: &str) -> Vec<&HiveEdge> {
        self.edges
            .iter()
            .filter(|e| e.source_id == source_id && e.edge_type == edge_type)
            .collect()
    }
}
