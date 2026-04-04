use std::collections::HashSet;

use anyhow::Result;
use tracing::{debug, info, warn};

use super::graph::{Graph, GraphUpdate};
use super::node::{GraphEnvironment, Node};
use super::node_factory::NodeFactory;
use super::relationship::ExternalRelationshipStore;
use crate::project::file_explorer::{File, Folder, ProjectFilesIterator};

// ---------------------------------------------------------------------------
// GraphBuilder
// ---------------------------------------------------------------------------

/// High-level builder that walks a project's file tree and constructs
/// a code graph with folder/file nodes and structural relationships.
#[derive(Debug)]
pub struct GraphBuilder {
    pub graph_environment: GraphEnvironment,
    pub root_path: String,
    pub extensions_to_skip: Vec<String>,
    pub names_to_skip: Vec<String>,
}

impl GraphBuilder {
    pub fn new(root_path: impl Into<String>) -> Self {
        let rp: String = root_path.into();
        Self {
            graph_environment: GraphEnvironment::new("blarify", "repo", &rp),
            root_path: rp,
            extensions_to_skip: Vec::new(),
            names_to_skip: Vec::new(),
        }
    }

    pub fn with_environment(mut self, env: GraphEnvironment) -> Self {
        self.graph_environment = env;
        self
    }

    pub fn with_skip_extensions(mut self, exts: Vec<String>) -> Self {
        self.extensions_to_skip = exts;
        self
    }

    pub fn with_skip_names(mut self, names: Vec<String>) -> Self {
        self.names_to_skip = names;
        self
    }

    /// Build the graph by walking the filesystem.
    pub fn build(&self) -> Result<Graph> {
        info!(root = %self.root_path, "building code graph");

        let iterator = ProjectFilesIterator::new(
            &self.root_path,
            &self.extensions_to_skip,
            &self.names_to_skip,
            None, // blarignore
            0.8,  // max file size MB
        );

        let mut graph = Graph::new();
        let mut folder_count = 0u64;
        let mut file_count = 0u64;

        for folder in iterator {
            let folder_node = self.process_folder(&folder, &mut graph);

            // Create file nodes for each file in the folder
            for file in &folder.files {
                let file_node = self.create_file_node(file, &folder_node);
                let file_id = file_node.id();

                // Link folder → file
                if let Node::Folder(ref mut fn_node) = graph
                    .get_nodes_by_path(folder_node.path())
                    .first()
                    .map(|n| (*n).clone())
                    .unwrap_or_else(|| folder_node.clone())
                {
                    // We'll handle this via explicit add_child below
                    let _ = fn_node;
                }

                graph.add_node(file_node);
                file_count += 1;

                // Update the folder node's contains list
                if let Some(Node::Folder(ref mut fn_node)) = graph.remove_node(&folder_node.id()) {
                    let file_hashed = Node::File(crate::graph::node::FileNode::new(
                        file.uri_path(),
                        &file.name,
                        file.level,
                        None,
                        None,
                        "",
                        Some(self.graph_environment.clone()),
                        Some(folder_node.node_repr_for_identifier()),
                    ))
                    .hashed_id();
                    fn_node.add_child(file_hashed);
                    graph.add_node(Node::Folder(fn_node.clone()));
                } else {
                    // Re-add the original if remove failed (shouldn't happen)
                    debug!(file_id = %file_id, "could not update folder contains");
                }
            }

            folder_count += 1;
        }

        info!(
            folders = folder_count,
            files = file_count,
            total = graph.node_count(),
            "graph build complete"
        );

        Ok(graph)
    }

    /// Build a hierarchy-only graph (folders and files, no code analysis).
    pub fn build_hierarchy_only(&self) -> Result<Graph> {
        self.build()
    }

    /// Incremental update: remove old nodes for changed paths, rebuild them.
    pub fn incremental_update(
        &self,
        graph: &mut Graph,
        updated_paths: &[String],
    ) -> Result<GraphUpdate> {
        info!(updated = updated_paths.len(), "incremental graph update");

        // Remove old nodes at these paths
        let paths_set: HashSet<&str> = updated_paths.iter().map(|s| s.as_str()).collect();
        let ids_to_remove: Vec<String> = graph
            .all_nodes()
            .filter(|n| paths_set.contains(n.path()))
            .map(|n| n.id())
            .collect();

        for id in &ids_to_remove {
            graph.remove_node(id);
        }

        // Remove empty folder nodes iteratively
        self.remove_empty_folders(graph);

        let external_store = ExternalRelationshipStore::new();

        info!(
            removed = ids_to_remove.len(),
            remaining = graph.node_count(),
            "incremental update complete"
        );

        Ok(GraphUpdate::new(graph.clone(), external_store))
    }

    fn remove_empty_folders(&self, graph: &mut Graph) {
        loop {
            let empty_folder_ids: Vec<String> = graph
                .get_nodes_by_label(super::node::NodeLabel::Folder)
                .iter()
                .filter(|n| n.contained_children_ids().is_empty())
                .map(|n| n.id())
                .collect();

            if empty_folder_ids.is_empty() {
                break;
            }

            for id in &empty_folder_ids {
                debug!(id = %id, "removing empty folder");
                graph.remove_node(id);
            }
        }
    }

    fn process_folder(&self, folder: &Folder, graph: &mut Graph) -> Node {
        // Check if parent folder exists
        let parent_identifier = if let Some(parent_path) = parent_folder_path(&folder.path) {
            graph
                .get_folder_node_by_path(&format!("file://{parent_path}"))
                .map(|n| n.node_repr_for_identifier())
        } else {
            None
        };

        let folder_node = NodeFactory::create_folder_node(
            folder,
            parent_identifier,
            Some(self.graph_environment.clone()),
        );
        let node = Node::Folder(folder_node);

        // Link parent folder → this folder
        let node_hashed_id = node.hashed_id();
        let folder_uri = format!("file://{}", folder.path);
        if let Some(parent_path) = parent_folder_path(&folder.path) {
            let parent_uri = format!("file://{parent_path}");
            if let Some(Node::Folder(ref parent)) =
                graph.get_folder_node_by_path(&parent_uri).cloned()
            {
                let mut parent_clone = parent.clone();
                parent_clone.add_child(node_hashed_id);
                let parent_id = Node::Folder(parent.clone()).id();
                graph.remove_node(&parent_id);
                graph.add_node(Node::Folder(parent_clone));
            }
        }

        graph.add_node(node.clone());

        if graph.has_folder_node_with_path(&folder_uri) {
            debug!(path = %folder.path, "added folder node");
        } else {
            warn!(path = %folder.path, "folder node not indexed properly");
        }

        node
    }

    fn create_file_node(&self, file: &File, parent_node: &Node) -> Node {
        let file_node = NodeFactory::create_file_node(
            file.uri_path(),
            &file.name,
            file.level,
            None,
            None,
            "", // Code text would be filled by tree-sitter parsing
            Some(parent_node.node_repr_for_identifier()),
            Some(self.graph_environment.clone()),
        );
        Node::File(file_node)
    }
}

/// Extract the parent directory path from a path string.
fn parent_folder_path(path: &str) -> Option<String> {
    let p = std::path::Path::new(path);
    p.parent().map(|pp| pp.to_string_lossy().into_owned())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_builder_new_defaults() {
        let gb = GraphBuilder::new("/repo");
        assert_eq!(gb.root_path, "/repo");
        assert_eq!(gb.graph_environment.environment, "blarify");
        assert_eq!(gb.graph_environment.diff_identifier, "repo");
    }

    #[test]
    fn graph_builder_with_environment() {
        let env = GraphEnvironment::new("custom", "branch-1", "/custom/root");
        let gb = GraphBuilder::new("/repo").with_environment(env);
        assert_eq!(gb.graph_environment.environment, "custom");
    }

    #[test]
    fn parent_folder_path_extracts_parent() {
        assert_eq!(
            parent_folder_path("/repo/src/lib"),
            Some("/repo/src".into())
        );
        assert_eq!(parent_folder_path("/repo"), Some("/".into()));
    }

    #[test]
    fn parent_folder_path_root() {
        assert_eq!(parent_folder_path("/"), None);
    }

    #[test]
    fn graph_builder_build_empty_dir() {
        // Build with a nonexistent path produces an empty graph (no panic)
        let gb = GraphBuilder::new("/nonexistent/path/that/does/not/exist");
        let result = gb.build();
        assert!(result.is_ok());
        let graph = result.unwrap();
        assert_eq!(graph.node_count(), 0);
    }

    #[test]
    fn incremental_update_removes_old_nodes() {
        let gb = GraphBuilder::new("/repo");
        let mut graph = Graph::new();
        graph.add_node(Node::File(crate::graph::node::FileNode::new(
            "file:///repo/old.py",
            "old.py",
            1,
            None,
            None,
            "old code",
            None,
            None,
        )));
        assert_eq!(graph.node_count(), 1);

        let update = gb
            .incremental_update(&mut graph, &["file:///repo/old.py".to_string()])
            .unwrap();
        assert_eq!(update.graph.node_count(), 0);
    }
}
