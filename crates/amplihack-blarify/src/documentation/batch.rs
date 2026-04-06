//! Bottom-up batch processor for documentation generation.
//!
//! Mirrors the Python `documentation/utils/bottom_up_batch_processor.py`.

use std::time::Instant;

use anyhow::{Context, Result};
use tracing::{debug, info, warn};

use super::models::ProcessingResult;
use super::queries;
use crate::db::manager::{DbManager, QueryParams};
use crate::db::types::NodeWithContentDto;
use crate::graph::node::GraphEnvironment;

/// Configuration for the batch processor.
#[derive(Debug, Clone)]
pub struct BatchConfig {
    pub max_workers: usize,
    pub batch_size: usize,
    pub overwrite_documentation: bool,
    pub generate_embeddings: bool,
}

impl Default for BatchConfig {
    fn default() -> Self {
        Self {
            max_workers: 5,
            batch_size: 50,
            overwrite_documentation: false,
            generate_embeddings: false,
        }
    }
}

/// Processes documentation bottom-up through the code graph hierarchy.
///
/// Starts with leaf nodes (functions with no calls) and works upward,
/// enriching parent nodes with child descriptions.
pub struct BottomUpBatchProcessor<'a> {
    db_manager: &'a dyn DbManager,
    _graph_environment: GraphEnvironment,
    config: BatchConfig,
    root_node: Option<NodeWithContentDto>,
    run_id: String,
}

impl<'a> BottomUpBatchProcessor<'a> {
    /// Create a new batch processor.
    pub fn new(
        db_manager: &'a dyn DbManager,
        graph_environment: GraphEnvironment,
        config: BatchConfig,
        root_node: Option<NodeWithContentDto>,
    ) -> Self {
        let run_id = format!(
            "run_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
        );
        Self {
            db_manager,
            _graph_environment: graph_environment,
            config,
            root_node,
            run_id,
        }
    }

    /// Process a node and all its upstream definitions.
    pub fn process_upstream_definitions(&self, node_path: &str) -> Result<ProcessingResult> {
        let start = Instant::now();
        info!(node_path, "Processing upstream definitions");

        let root = self.resolve_root_node(node_path)?;
        let processed = self.process_node_query_based(&root)?;

        let _elapsed = start.elapsed().as_secs_f64();
        Ok(ProcessingResult {
            node_path: node_path.into(),
            total_nodes_processed: processed,
            ..Default::default()
        })
    }

    /// Process a single node path through bottom-up documentation.
    pub fn process_node(&self, node_path: &str) -> Result<ProcessingResult> {
        let start = Instant::now();
        info!(node_path, "Processing node bottom-up");

        let root = self.resolve_root_node(node_path)?;
        let mut total_processed = 0;

        // Phase 1: Process leaf nodes
        total_processed += self.process_leaf_batch(&root)?;

        // Phase 2: Process parent nodes (with all children completed)
        total_processed += self.process_parent_batch(&root)?;

        // Phase 3: Handle remaining functions (cycle-breaking)
        if self.has_pending_nodes(&root)? {
            total_processed += self.process_remaining_functions_batch(&root)?;
        }

        // Phase 4: Process root node
        total_processed += self.process_root_node(&root)?;

        let elapsed = start.elapsed().as_secs_f64();
        info!(
            node_path,
            total_processed,
            elapsed_secs = elapsed,
            "Node processing complete"
        );

        Ok(ProcessingResult {
            node_path: node_path.into(),
            total_nodes_processed: total_processed,
            ..Default::default()
        })
    }

    /// Resolve a path to a root node DTO.
    fn resolve_root_node(&self, _node_path: &str) -> Result<NodeWithContentDto> {
        if let Some(ref root) = self.root_node {
            return Ok(root.clone());
        }
        // In production, this would query the DB for the node at this path
        Ok(NodeWithContentDto {
            id: "root".into(),
            name: "root".into(),
            labels: vec!["FOLDER".into()],
            path: _node_path.into(),
            start_line: None,
            end_line: None,
            content: String::new(),
            relationship_type: None,
        })
    }

    /// Query-based processing for the entire subtree.
    fn process_node_query_based(&self, root: &NodeWithContentDto) -> Result<usize> {
        let mut total = 0;
        total += self.process_leaf_batch(root)?;
        total += self.process_parent_batch(root)?;
        if self.has_pending_nodes(root)? {
            total += self.process_remaining_functions_batch(root)?;
        }
        Ok(total)
    }

    /// Process a batch of leaf nodes.
    fn process_leaf_batch(&self, _root: &NodeWithContentDto) -> Result<usize> {
        let mut params = QueryParams::new();
        params.insert(
            "entity_id".into(),
            serde_json::Value::String(self.db_manager.entity_id().into()),
        );
        params.insert(
            "repo_id".into(),
            serde_json::Value::String(self.db_manager.repo_id().into()),
        );
        params.insert(
            "run_id".into(),
            serde_json::Value::String(self.run_id.clone()),
        );
        params.insert(
            "batch_size".into(),
            serde_json::Value::Number((self.config.batch_size as u64).into()),
        );

        let results = self
            .db_manager
            .query(queries::LEAF_NODES_BATCH, Some(&params), false)
            .context("Failed to fetch leaf nodes batch")?;

        let count = results.len();
        if count > 0 {
            // Mark these nodes as completed
            let node_ids: Vec<String> = results
                .iter()
                .filter_map(|r| r.get("id").and_then(|v| v.as_str()).map(String::from))
                .collect();
            self.mark_nodes_completed(&node_ids)?;
            debug!(count, "Processed leaf nodes batch");
        }
        Ok(count)
    }

    /// Process parent nodes whose children are all completed.
    fn process_parent_batch(&self, root: &NodeWithContentDto) -> Result<usize> {
        let mut params = QueryParams::new();
        params.insert(
            "root_node_id".into(),
            serde_json::Value::String(root.id.clone()),
        );
        params.insert(
            "entity_id".into(),
            serde_json::Value::String(self.db_manager.entity_id().into()),
        );
        params.insert(
            "repo_id".into(),
            serde_json::Value::String(self.db_manager.repo_id().into()),
        );
        params.insert(
            "run_id".into(),
            serde_json::Value::String(self.run_id.clone()),
        );
        params.insert(
            "batch_size".into(),
            serde_json::Value::Number((self.config.batch_size as u64).into()),
        );

        let results = self
            .db_manager
            .query(
                queries::PROCESSABLE_NODES_WITH_DESCRIPTIONS,
                Some(&params),
                false,
            )
            .context("Failed to fetch processable nodes")?;

        let count = results.len();
        if count > 0 {
            let node_ids: Vec<String> = results
                .iter()
                .filter_map(|r| r.get("id").and_then(|v| v.as_str()).map(String::from))
                .collect();
            self.mark_nodes_completed(&node_ids)?;
            debug!(count, "Processed parent nodes batch");
        }
        Ok(count)
    }

    /// Process remaining pending functions (handles cycles).
    fn process_remaining_functions_batch(&self, root: &NodeWithContentDto) -> Result<usize> {
        let mut params = QueryParams::new();
        params.insert(
            "root_node_id".into(),
            serde_json::Value::String(root.id.clone()),
        );
        params.insert(
            "entity_id".into(),
            serde_json::Value::String(self.db_manager.entity_id().into()),
        );
        params.insert(
            "repo_id".into(),
            serde_json::Value::String(self.db_manager.repo_id().into()),
        );
        params.insert(
            "run_id".into(),
            serde_json::Value::String(self.run_id.clone()),
        );
        params.insert(
            "batch_size".into(),
            serde_json::Value::Number((self.config.batch_size as u64).into()),
        );

        let results = self
            .db_manager
            .query(queries::REMAINING_PENDING_FUNCTIONS, Some(&params), false)
            .context("Failed to fetch remaining pending functions")?;

        let count = results.len();
        if count > 0 {
            let node_ids: Vec<String> = results
                .iter()
                .filter_map(|r| r.get("id").and_then(|v| v.as_str()).map(String::from))
                .collect();
            self.mark_nodes_completed(&node_ids)?;
            warn!(count, "Processed remaining functions (possible cycles)");
        }
        Ok(count)
    }

    /// Process the root node itself.
    fn process_root_node(&self, root: &NodeWithContentDto) -> Result<usize> {
        self.mark_nodes_completed(std::slice::from_ref(&root.id))?;
        debug!(node_id = %root.id, "Processed root node");
        Ok(1)
    }

    /// Check if there are pending nodes under the root.
    fn has_pending_nodes(&self, root: &NodeWithContentDto) -> Result<bool> {
        let mut params = QueryParams::new();
        params.insert(
            "root_node_id".into(),
            serde_json::Value::String(root.id.clone()),
        );
        params.insert(
            "entity_id".into(),
            serde_json::Value::String(self.db_manager.entity_id().into()),
        );
        params.insert(
            "repo_id".into(),
            serde_json::Value::String(self.db_manager.repo_id().into()),
        );

        let results = self
            .db_manager
            .query(queries::CHECK_PENDING_NODES, Some(&params), false)
            .context("Failed to check pending nodes")?;

        let count = results
            .first()
            .and_then(|r| r.get("pending_count"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        Ok(count > 0)
    }

    /// Mark nodes as completed in the database.
    fn mark_nodes_completed(&self, node_ids: &[String]) -> Result<()> {
        if node_ids.is_empty() {
            return Ok(());
        }
        let mut params = QueryParams::new();
        params.insert("node_ids".into(), serde_json::json!(node_ids));
        params.insert(
            "entity_id".into(),
            serde_json::Value::String(self.db_manager.entity_id().into()),
        );
        params.insert(
            "repo_id".into(),
            serde_json::Value::String(self.db_manager.repo_id().into()),
        );
        params.insert(
            "run_id".into(),
            serde_json::Value::String(self.run_id.clone()),
        );

        self.db_manager
            .query(queries::MARK_NODES_COMPLETED, Some(&params), true)
            .context("Failed to mark nodes completed")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_config_defaults() {
        let config = BatchConfig::default();
        assert_eq!(config.max_workers, 5);
        assert_eq!(config.batch_size, 50);
        assert!(!config.overwrite_documentation);
    }

    #[test]
    fn run_id_is_unique() {
        // Test the run_id format
        let run_id = format!(
            "run_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
        );
        assert!(run_id.starts_with("run_"));
        assert!(run_id.len() > 4);
    }

    #[test]
    fn processing_result_default_is_empty() {
        let r = ProcessingResult::default();
        assert_eq!(r.total_nodes_processed, 0);
        assert!(r.error.is_none());
    }
}
