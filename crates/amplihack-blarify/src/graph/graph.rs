use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use super::node::{Node, NodeLabel};
use super::relationship::{ExternalRelationshipStore, Relationship};

// ---------------------------------------------------------------------------
// Graph
// ---------------------------------------------------------------------------

/// The main graph container holding nodes indexed by multiple keys
/// and accumulating relationships.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Graph {
    /// All nodes keyed by their full ID.
    nodes: HashMap<String, Node>,
    /// Path → set of node IDs at that path.
    nodes_by_path: HashMap<String, HashSet<String>>,
    /// Label → set of node IDs with that label.
    nodes_by_label: HashMap<NodeLabel, HashSet<String>>,
    /// Relative ID → node ID (for quick lookup by relative path).
    #[serde(default)]
    nodes_by_relative_id: HashMap<String, String>,
    /// Explicitly-added reference relationships (e.g. CALLS, IMPORTS).
    reference_relationships: Vec<Relationship>,
}

impl Graph {
    pub fn new() -> Self {
        Self::default()
    }

    // -- Mutation --

    /// Add a single node, indexing it by ID, path, and label.
    pub fn add_node(&mut self, node: Node) {
        let id = node.id();
        let path = node.path().to_owned();
        let label = node.label();

        self.nodes_by_path
            .entry(path)
            .or_default()
            .insert(id.clone());

        self.nodes_by_label
            .entry(label)
            .or_default()
            .insert(id.clone());

        self.nodes.insert(id, node);
    }

    /// Add multiple nodes.
    pub fn add_nodes(&mut self, nodes: impl IntoIterator<Item = Node>) {
        for node in nodes {
            self.add_node(node);
        }
    }

    /// Append externally-computed reference relationships.
    pub fn add_reference_relationships(&mut self, rels: Vec<Relationship>) {
        self.reference_relationships.extend(rels);
    }

    /// Remove a node by ID, cleaning up all indexes.
    pub fn remove_node(&mut self, id: &str) -> Option<Node> {
        if let Some(node) = self.nodes.remove(id) {
            let path = node.path().to_owned();
            let label = node.label();

            if let Some(set) = self.nodes_by_path.get_mut(&path) {
                set.remove(id);
                if set.is_empty() {
                    self.nodes_by_path.remove(&path);
                }
            }
            if let Some(set) = self.nodes_by_label.get_mut(&label) {
                set.remove(id);
                if set.is_empty() {
                    self.nodes_by_label.remove(&label);
                }
            }
            self.nodes_by_relative_id.retain(|_, v| v != id);
            Some(node)
        } else {
            None
        }
    }

    // -- Queries --

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn get_node_by_id(&self, id: &str) -> Option<&Node> {
        self.nodes.get(id)
    }

    pub fn get_nodes_by_path(&self, path: &str) -> Vec<&Node> {
        self.nodes_by_path
            .get(path)
            .map(|ids| ids.iter().filter_map(|id| self.nodes.get(id)).collect())
            .unwrap_or_default()
    }

    pub fn get_file_node_by_path(&self, path: &str) -> Option<&Node> {
        self.get_nodes_by_path(path)
            .into_iter()
            .find(|n| n.label() == NodeLabel::File)
    }

    pub fn get_folder_node_by_path(&self, path: &str) -> Option<&Node> {
        self.get_nodes_by_path(path)
            .into_iter()
            .find(|n| n.label() == NodeLabel::Folder)
    }

    pub fn has_folder_node_with_path(&self, path: &str) -> bool {
        self.get_folder_node_by_path(path).is_some()
    }

    pub fn get_nodes_by_label(&self, label: NodeLabel) -> Vec<&Node> {
        self.nodes_by_label
            .get(&label)
            .map(|ids| ids.iter().filter_map(|id| self.nodes.get(id)).collect())
            .unwrap_or_default()
    }

    /// All nodes in the graph.
    pub fn all_nodes(&self) -> impl Iterator<Item = &Node> {
        self.nodes.values()
    }

    // -- Relationship export --

    /// Collect all relationships: structural (from nodes) + reference relationships.
    pub fn get_all_relationships(&self) -> Vec<Relationship> {
        let mut rels: Vec<Relationship> = self
            .nodes
            .values()
            .flat_map(|n| n.get_relationships())
            .collect();
        rels.extend(self.reference_relationships.iter().cloned());
        rels
    }

    /// Serialize all relationships to JSON objects.
    pub fn get_relationships_as_objects(&self) -> Vec<serde_json::Value> {
        self.get_all_relationships()
            .iter()
            .map(|r| r.as_object())
            .collect()
    }

    /// Serialize all nodes to JSON objects.
    pub fn get_nodes_as_objects(&self) -> Vec<serde_json::Value> {
        self.nodes.values().map(|n| n.as_object()).collect()
    }

    // -- Filtering --

    /// Create a new graph containing only nodes whose path is in `paths_to_keep`.
    pub fn filtered_by_paths(&self, paths_to_keep: &HashSet<String>) -> Graph {
        let mut new_graph = Graph::new();

        for node in self.nodes.values() {
            if paths_to_keep.contains(node.path()) {
                new_graph.add_node(node.clone());
            }
        }

        // Keep reference relationships where either endpoint path is retained
        for rel in &self.reference_relationships {
            let source_ok = self
                .nodes
                .get(&rel.source_id)
                .is_some_and(|n| paths_to_keep.contains(n.path()));
            let target_ok = self
                .nodes
                .get(&rel.target_id)
                .is_some_and(|n| paths_to_keep.contains(n.path()));
            if source_ok || target_ok {
                new_graph.reference_relationships.push(rel.clone());
            }
        }

        new_graph
    }
}

impl std::fmt::Display for Graph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Graph(nodes={}, relationships={})",
            self.nodes.len(),
            self.get_all_relationships().len()
        )
    }
}

// ---------------------------------------------------------------------------
// GraphUpdate
// ---------------------------------------------------------------------------

/// A graph snapshot bundled with external relationships for export.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphUpdate {
    pub graph: Graph,
    pub external_store: ExternalRelationshipStore,
}

impl GraphUpdate {
    pub fn new(graph: Graph, external_store: ExternalRelationshipStore) -> Self {
        Self {
            graph,
            external_store,
        }
    }

    pub fn get_nodes_as_objects(&self) -> Vec<serde_json::Value> {
        self.graph.get_nodes_as_objects()
    }

    pub fn get_relationships_as_objects(&self) -> Vec<serde_json::Value> {
        let mut rels = self.graph.get_relationships_as_objects();
        rels.extend(self.external_store.as_objects());
        rels
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::node::*;

    fn make_file(path: &str, name: &str) -> Node {
        Node::File(FileNode::new(path, name, 1, None, None, "", None, None))
    }

    fn make_folder(path: &str, name: &str) -> Node {
        Node::Folder(FolderNode::new(path, name, 0, None, None))
    }

    #[test]
    fn add_and_retrieve_node() {
        let mut g = Graph::new();
        g.add_node(make_file("file:///repo/a.py", "a.py"));
        assert_eq!(g.node_count(), 1);
        assert!(!g.get_nodes_by_path("file:///repo/a.py").is_empty());
    }

    #[test]
    fn get_file_node_by_path() {
        let mut g = Graph::new();
        g.add_node(make_file("file:///repo/b.py", "b.py"));
        let n = g.get_file_node_by_path("file:///repo/b.py");
        assert!(n.is_some());
        assert_eq!(n.unwrap().label(), NodeLabel::File);
    }

    #[test]
    fn get_folder_node_by_path() {
        let mut g = Graph::new();
        g.add_node(make_folder("file:///repo/src", "src"));
        assert!(g.has_folder_node_with_path("file:///repo/src"));
    }

    #[test]
    fn get_nodes_by_label() {
        let mut g = Graph::new();
        g.add_node(make_file("file:///a.py", "a.py"));
        g.add_node(make_file("file:///b.py", "b.py"));
        g.add_node(make_folder("file:///src", "src"));
        assert_eq!(g.get_nodes_by_label(NodeLabel::File).len(), 2);
        assert_eq!(g.get_nodes_by_label(NodeLabel::Folder).len(), 1);
    }

    #[test]
    fn remove_node() {
        let mut g = Graph::new();
        let node = make_file("file:///repo/x.py", "x.py");
        let id = node.id();
        g.add_node(node);
        assert_eq!(g.node_count(), 1);
        let removed = g.remove_node(&id);
        assert!(removed.is_some());
        assert_eq!(g.node_count(), 0);
        assert!(g.get_nodes_by_path("file:///repo/x.py").is_empty());
    }

    #[test]
    fn relationships_from_folder_node() {
        let mut folder =
            super::super::node::FolderNode::new("file:///repo/src", "src", 0, None, None);
        folder.add_child("child-1".into());
        let mut g = Graph::new();
        g.add_node(Node::Folder(folder));
        let rels = g.get_all_relationships();
        assert_eq!(rels.len(), 1);
    }

    #[test]
    fn add_reference_relationships() {
        let mut g = Graph::new();
        g.add_reference_relationships(vec![Relationship::new(
            "a",
            "b",
            super::super::relationship::RelationshipType::Calls,
        )]);
        assert_eq!(g.get_all_relationships().len(), 1);
    }

    #[test]
    fn filtered_by_paths() {
        let mut g = Graph::new();
        g.add_node(make_file("file:///repo/keep.py", "keep.py"));
        g.add_node(make_file("file:///repo/drop.py", "drop.py"));
        let keep: HashSet<String> = ["file:///repo/keep.py".to_string()].into();
        let filtered = g.filtered_by_paths(&keep);
        assert_eq!(filtered.node_count(), 1);
        assert!(
            filtered
                .get_file_node_by_path("file:///repo/keep.py")
                .is_some()
        );
    }

    #[test]
    fn graph_display() {
        let g = Graph::new();
        assert_eq!(g.to_string(), "Graph(nodes=0, relationships=0)");
    }

    #[test]
    fn graph_update_combines_relationships() {
        let mut g = Graph::new();
        g.add_reference_relationships(vec![Relationship::new(
            "a",
            "b",
            super::super::relationship::RelationshipType::Uses,
        )]);
        let mut ext = ExternalRelationshipStore::new();
        ext.create_and_add(
            "c",
            "d",
            super::super::relationship::RelationshipType::Imports,
        );
        let update = GraphUpdate::new(g, ext);
        let rels = update.get_relationships_as_objects();
        assert_eq!(rels.len(), 2);
    }
}
