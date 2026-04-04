//! Edge management for the hive knowledge graph.

use std::collections::HashMap;

use crate::models::HiveEdge;

use super::HiveGraph;

impl HiveGraph {
    pub fn add_edge(
        &mut self,
        source_id: &str,
        target_id: &str,
        edge_type: &str,
        properties: HashMap<String, String>,
    ) {
        self.edges.push(HiveEdge {
            source_id: source_id.to_string(),
            target_id: target_id.to_string(),
            edge_type: edge_type.to_string(),
            properties,
        });
    }

    pub fn get_edges(&self, node_id: &str) -> Vec<&HiveEdge> {
        self.edges
            .iter()
            .filter(|e| e.source_id == node_id || e.target_id == node_id)
            .collect()
    }

    pub fn get_edges_from(&self, source_id: &str, edge_type: &str) -> Vec<&HiveEdge> {
        self.edges
            .iter()
            .filter(|e| e.source_id == source_id && e.edge_type == edge_type)
            .collect()
    }
}
