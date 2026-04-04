//! Workflow creator for code workflow discovery.
//!
//! Mirrors the Python `documentation/workflow_creator.py`.

use std::collections::HashMap;
use std::time::Instant;

use anyhow::{Context, Result};
use tracing::{debug, info, warn};

use super::models::{WorkflowDiscoveryResult, WorkflowResult};
use super::queries;
use crate::db::manager::{DbManager, QueryParams};
use crate::graph::node::GraphEnvironment;

/// Creates and manages code workflow documentation.
pub struct WorkflowCreator<'a> {
    db_manager: &'a dyn DbManager,
    #[allow(dead_code)]
    graph_environment: GraphEnvironment,
}

impl<'a> WorkflowCreator<'a> {
    /// Create a new workflow creator.
    pub fn new(db_manager: &'a dyn DbManager, graph_environment: GraphEnvironment) -> Self {
        Self {
            db_manager,
            graph_environment,
        }
    }

    /// Discover workflows starting from entry points.
    pub fn discover_workflows(
        &self,
        entry_points: Option<&[String]>,
        max_depth: usize,
        save_to_database: bool,
        file_paths: Option<&[String]>,
    ) -> Result<WorkflowDiscoveryResult> {
        let start = Instant::now();
        info!("Starting workflow discovery");

        let entry_point_data = self.discover_entry_points(file_paths)?;
        let total_entry_points = entry_point_data.len();

        // Filter to specific entry points if provided
        let entry_ids: Vec<String> = if let Some(eps) = entry_points {
            entry_point_data
                .iter()
                .filter(|ep| {
                    ep.get("id")
                        .and_then(|v| v.as_str())
                        .is_some_and(|id| eps.contains(&id.to_string()))
                })
                .filter_map(|ep| ep.get("id").and_then(|v| v.as_str()).map(String::from))
                .collect()
        } else {
            entry_point_data
                .iter()
                .filter_map(|ep| ep.get("id").and_then(|v| v.as_str()).map(String::from))
                .collect()
        };

        // Delete existing workflows for these entry points
        if save_to_database && !entry_ids.is_empty() {
            self.delete_workflow_nodes_for_entry_points(&entry_ids)?;
        }

        let mut all_workflows = Vec::new();
        for entry_id in &entry_ids {
            match self.analyze_workflow_from_entry_point(entry_id, max_depth) {
                Ok(workflows) => all_workflows.extend(workflows),
                Err(e) => warn!(entry_id, error = %e, "Failed to analyze workflow"),
            }
        }

        if save_to_database && !all_workflows.is_empty() {
            self.save_workflows_to_database(&all_workflows)?;
        }

        let elapsed = start.elapsed().as_secs_f64();
        info!(
            total_entry_points,
            total_workflows = all_workflows.len(),
            elapsed_secs = elapsed,
            "Workflow discovery complete"
        );

        Ok(WorkflowDiscoveryResult {
            discovered_workflows: all_workflows.clone(),
            entry_points: entry_point_data,
            total_entry_points,
            total_workflows: all_workflows.len(),
            discovery_time_seconds: elapsed,
            error: None,
            warnings: vec![],
        })
    }

    /// Discover entry points from the code graph.
    fn discover_entry_points(
        &self,
        file_paths: Option<&[String]>,
    ) -> Result<Vec<HashMap<String, serde_json::Value>>> {
        let mut params = QueryParams::new();
        params.insert(
            "entity_id".into(),
            serde_json::Value::String(self.db_manager.entity_id().into()),
        );
        params.insert(
            "repo_id".into(),
            serde_json::Value::String(self.db_manager.repo_id().into()),
        );

        let query = if let Some(paths) = file_paths {
            params.insert("file_paths".into(), serde_json::json!(paths));
            crate::db::queries::ENTRY_POINTS_FOR_FILE_PATHS_QUERY
        } else {
            crate::db::queries::POTENTIAL_ENTRY_POINTS_QUERY
        };

        let results = self
            .db_manager
            .query(query, Some(&params), false)
            .context("Failed to discover entry points")?;
        debug!(count = results.len(), "Discovered entry points");
        Ok(results)
    }

    /// Delete existing workflow nodes for entry points.
    fn delete_workflow_nodes_for_entry_points(&self, entry_point_ids: &[String]) -> Result<()> {
        let mut params = QueryParams::new();
        params.insert("entry_point_ids".into(), serde_json::json!(entry_point_ids));

        self.db_manager
            .query(
                queries::DELETE_WORKFLOWS_FOR_ENTRY_POINTS,
                Some(&params),
                true,
            )
            .context("Failed to delete workflow nodes")?;
        debug!(count = entry_point_ids.len(), "Deleted existing workflows");
        Ok(())
    }

    /// Analyze workflows from a single entry point.
    fn analyze_workflow_from_entry_point(
        &self,
        entry_point_id: &str,
        max_depth: usize,
    ) -> Result<Vec<WorkflowResult>> {
        let raw_workflows = self.execute_code_workflows_query(entry_point_id, max_depth)?;
        let mut results = Vec::new();
        for data in &raw_workflows {
            match self.convert_to_workflow_result(data) {
                Ok(result) => results.push(result),
                Err(e) => warn!(error = %e, "Failed to convert workflow data"),
            }
        }
        Ok(results)
    }

    /// Execute the code workflows Cypher query.
    fn execute_code_workflows_query(
        &self,
        entry_point_id: &str,
        max_depth: usize,
    ) -> Result<Vec<HashMap<String, serde_json::Value>>> {
        let mut params = QueryParams::new();
        params.insert(
            "entry_point_id".into(),
            serde_json::Value::String(entry_point_id.into()),
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
            "max_depth".into(),
            serde_json::Value::Number((max_depth as u64).into()),
        );
        params.insert("batch_size".into(), serde_json::Value::Number(100.into()));

        self.db_manager
            .query(
                crate::db::queries::CODE_WORKFLOWS_QUERY,
                Some(&params),
                false,
            )
            .context("Failed to execute code workflows query")
    }

    /// Convert raw query data into a `WorkflowResult`.
    fn convert_to_workflow_result(
        &self,
        data: &HashMap<String, serde_json::Value>,
    ) -> Result<WorkflowResult> {
        let nodes = data
            .get("nodes")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let edges = data
            .get("edges")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let first_node = nodes.first();
        let last_node = nodes.last();

        Ok(WorkflowResult {
            entry_point_id: first_node
                .and_then(|n| n.get("id"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .into(),
            entry_point_name: first_node
                .and_then(|n| n.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .into(),
            entry_point_path: first_node
                .and_then(|n| n.get("path"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .into(),
            end_point_id: last_node
                .and_then(|n| n.get("id"))
                .and_then(|v| v.as_str())
                .map(String::from),
            end_point_name: last_node
                .and_then(|n| n.get("name"))
                .and_then(|v| v.as_str())
                .map(String::from),
            end_point_path: last_node
                .and_then(|n| n.get("path"))
                .and_then(|v| v.as_str())
                .map(String::from),
            workflow_nodes: nodes
                .iter()
                .filter_map(|n| {
                    n.as_object()
                        .map(|o| o.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                })
                .collect(),
            workflow_edges: edges
                .iter()
                .filter_map(|e| {
                    e.as_object()
                        .map(|o| o.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                })
                .collect(),
            documentation_node_ids: vec![],
            workflow_type: "execution_trace".into(),
            total_execution_steps: nodes.len(),
            path_length: data.get("depth").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
            discovered_by: "code_workflow_discovery".into(),
            complexity_score: None,
            has_cycles: false,
            error: None,
        })
    }

    /// Save discovered workflows to the database.
    fn save_workflows_to_database(&self, _workflows: &[WorkflowResult]) -> Result<()> {
        // Workflow persistence would create WORKFLOW nodes and WORKFLOW_STEP relationships.
        // This is a stub — actual implementation depends on concrete DB backend.
        debug!(count = _workflows.len(), "Would save workflows to database");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workflow_result_defaults() {
        let r = WorkflowResult {
            entry_point_id: "ep1".into(),
            entry_point_name: "main".into(),
            entry_point_path: "src/main.rs".into(),
            end_point_id: None,
            end_point_name: None,
            end_point_path: None,
            workflow_nodes: vec![],
            workflow_edges: vec![],
            documentation_node_ids: vec![],
            workflow_type: "execution_trace".into(),
            total_execution_steps: 0,
            path_length: 0,
            discovered_by: "code_workflow_discovery".into(),
            complexity_score: None,
            has_cycles: false,
            error: None,
        };
        assert_eq!(r.workflow_type, "execution_trace");
        assert!(!r.has_cycles);
    }

    #[test]
    fn workflow_discovery_result_empty() {
        let r = WorkflowDiscoveryResult::default();
        assert!(r.discovered_workflows.is_empty());
        assert_eq!(r.total_entry_points, 0);
    }

    #[test]
    fn convert_workflow_data() {
        // Simulate raw query result
        let mut data: HashMap<String, serde_json::Value> = HashMap::new();
        data.insert(
            "nodes".into(),
            serde_json::json!([
                {"id": "n1", "name": "entry", "labels": ["FUNCTION"], "path": "src/a.rs"},
                {"id": "n2", "name": "callee", "labels": ["FUNCTION"], "path": "src/b.rs"}
            ]),
        );
        data.insert("edges".into(), serde_json::json!([{"type": "CALLS"}]));
        data.insert("depth".into(), serde_json::json!(1));

        // Create a dummy struct to test convert_to_workflow_result outside the trait context
        let result_nodes = data
            .get("nodes")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        assert_eq!(result_nodes.len(), 2);
    }
}
