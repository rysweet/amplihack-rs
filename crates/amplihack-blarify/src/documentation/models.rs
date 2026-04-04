//! Result types for the documentation layer.
//!
//! Mirrors the Python `documentation/result_models.py`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Result of a documentation creation process.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DocumentationResult {
    /// Generated documentation node dicts.
    #[serde(default)]
    pub information_nodes: Vec<HashMap<String, serde_json::Value>>,

    /// IDs of documentation nodes created.
    #[serde(default)]
    pub documentation_node_ids: Vec<String>,

    /// IDs of source code nodes processed.
    #[serde(default)]
    pub source_node_ids: Vec<String>,

    /// Analyzed code components.
    #[serde(default)]
    pub analyzed_nodes: Vec<HashMap<String, serde_json::Value>>,

    /// Total number of nodes processed.
    #[serde(default)]
    pub total_nodes_processed: usize,

    /// Total processing time in seconds.
    #[serde(default)]
    pub processing_time_seconds: f64,

    /// Error message if processing failed.
    #[serde(default)]
    pub error: Option<String>,

    /// Non-fatal warnings during processing.
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// Result of workflow discovery and analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowResult {
    /// ID of the entry point node.
    pub entry_point_id: String,

    /// Name of the entry point.
    pub entry_point_name: String,

    /// File path of the entry point.
    pub entry_point_path: String,

    /// ID of the final node in the workflow.
    #[serde(default)]
    pub end_point_id: Option<String>,

    /// Name of the final node.
    #[serde(default)]
    pub end_point_name: Option<String>,

    /// File path of the final node.
    #[serde(default)]
    pub end_point_path: Option<String>,

    /// Nodes participating in the workflow.
    #[serde(default)]
    pub workflow_nodes: Vec<HashMap<String, serde_json::Value>>,

    /// Edges representing the workflow execution flow.
    #[serde(default)]
    pub workflow_edges: Vec<HashMap<String, serde_json::Value>>,

    /// IDs of associated documentation nodes.
    #[serde(default)]
    pub documentation_node_ids: Vec<String>,

    /// Type of workflow discovered.
    #[serde(default = "default_workflow_type")]
    pub workflow_type: String,

    /// Number of execution steps.
    #[serde(default)]
    pub total_execution_steps: usize,

    /// Length of the execution path.
    #[serde(default)]
    pub path_length: usize,

    /// Method used to discover the workflow.
    #[serde(default = "default_discovered_by")]
    pub discovered_by: String,

    /// Workflow complexity score.
    #[serde(default)]
    pub complexity_score: Option<i32>,

    /// Whether the workflow contains cycles.
    #[serde(default)]
    pub has_cycles: bool,

    /// Error message if discovery failed.
    #[serde(default)]
    pub error: Option<String>,
}

fn default_workflow_type() -> String {
    "execution_trace".into()
}
fn default_discovered_by() -> String {
    "code_workflow_discovery".into()
}

/// Result of the complete workflow discovery process.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkflowDiscoveryResult {
    /// All discovered workflows.
    #[serde(default)]
    pub discovered_workflows: Vec<WorkflowResult>,

    /// Entry points used for discovery.
    #[serde(default)]
    pub entry_points: Vec<HashMap<String, serde_json::Value>>,

    /// Total number of entry points analyzed.
    #[serde(default)]
    pub total_entry_points: usize,

    /// Total number of workflows discovered.
    #[serde(default)]
    pub total_workflows: usize,

    /// Time taken for discovery in seconds.
    #[serde(default)]
    pub discovery_time_seconds: f64,

    /// Error message if discovery failed.
    #[serde(default)]
    pub error: Option<String>,

    /// Non-fatal warnings during discovery.
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// Result of framework detection analysis.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FrameworkDetectionResult {
    /// Primary framework detected.
    #[serde(default)]
    pub primary_framework: Option<String>,

    /// Version of the framework if detected.
    #[serde(default)]
    pub framework_version: Option<String>,

    /// Technologies detected in the codebase.
    #[serde(default)]
    pub technology_stack: Vec<String>,

    /// Main architectural folders.
    #[serde(default)]
    pub main_folders: Vec<String>,

    /// Configuration files found.
    #[serde(default)]
    pub config_files: Vec<String>,

    /// Confidence in the framework detection (0.0-1.0).
    #[serde(default)]
    pub confidence_score: f64,

    /// Method used for detection.
    #[serde(default = "default_analysis_method")]
    pub analysis_method: String,

    /// Raw LLM analysis output.
    #[serde(default)]
    pub raw_analysis: String,

    /// Error message if detection failed.
    #[serde(default)]
    pub error: Option<String>,
}

fn default_analysis_method() -> String {
    "llm_analysis".into()
}

/// Result of processing a single node during bottom-up batch processing.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProcessingResult {
    /// Path of the processed node.
    #[serde(default)]
    pub node_path: String,

    /// Relationships discovered for this node.
    #[serde(default)]
    pub node_relationships: Vec<HashMap<String, serde_json::Value>>,

    /// Hierarchical analysis results.
    #[serde(default)]
    pub hierarchical_analysis: HashMap<String, serde_json::Value>,

    /// Error message if processing failed.
    #[serde(default)]
    pub error: Option<String>,

    /// Total nodes processed.
    #[serde(default)]
    pub total_nodes_processed: usize,

    /// Save status information.
    #[serde(default)]
    pub save_status: Option<HashMap<String, serde_json::Value>>,

    /// Generated documentation node dicts.
    #[serde(default)]
    pub information_nodes: Vec<HashMap<String, serde_json::Value>>,

    /// IDs of documentation nodes created.
    #[serde(default)]
    pub documentation_node_ids: Vec<String>,

    /// IDs of source nodes processed.
    #[serde(default)]
    pub source_node_ids: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documentation_result_default() {
        let r = DocumentationResult::default();
        assert_eq!(r.total_nodes_processed, 0);
        assert!(r.error.is_none());
        assert!(r.warnings.is_empty());
    }

    #[test]
    fn workflow_result_roundtrip() {
        let w = WorkflowResult {
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
            total_execution_steps: 5,
            path_length: 3,
            discovered_by: "code_workflow_discovery".into(),
            complexity_score: Some(7),
            has_cycles: false,
            error: None,
        };
        let json = serde_json::to_string(&w).unwrap();
        let deser: WorkflowResult = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.entry_point_id, "ep1");
        assert_eq!(deser.total_execution_steps, 5);
    }

    #[test]
    fn framework_detection_defaults() {
        let f = FrameworkDetectionResult::default();
        assert!(f.primary_framework.is_none());
        assert_eq!(f.confidence_score, 0.0);
        assert!(f.analysis_method.is_empty());
    }

    #[test]
    fn workflow_discovery_result_default() {
        let r = WorkflowDiscoveryResult::default();
        assert!(r.discovered_workflows.is_empty());
        assert_eq!(r.total_entry_points, 0);
    }

    #[test]
    fn processing_result_default() {
        let r = ProcessingResult::default();
        assert!(r.node_path.is_empty());
        assert!(r.error.is_none());
    }
}
