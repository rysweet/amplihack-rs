use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct HierarchicalExportData {
    pub(crate) agent_name: String,
    pub(crate) exported_at: String,
    pub(crate) format_version: String,
    pub(crate) semantic_nodes: Vec<SemanticNode>,
    pub(crate) episodic_nodes: Vec<EpisodicNode>,
    pub(crate) similar_to_edges: Vec<SimilarEdge>,
    pub(crate) derives_from_edges: Vec<DerivesEdge>,
    pub(crate) supersedes_edges: Vec<SupersedesEdge>,
    pub(crate) transitioned_to_edges: Vec<TransitionEdge>,
    #[serde(default)]
    pub(crate) statistics: HierarchicalStats,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct SemanticNode {
    pub(crate) memory_id: String,
    pub(crate) concept: String,
    pub(crate) content: String,
    pub(crate) confidence: f64,
    pub(crate) source_id: String,
    pub(crate) tags: Vec<String>,
    pub(crate) metadata: JsonValue,
    pub(crate) created_at: String,
    pub(crate) entity_name: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct EpisodicNode {
    pub(crate) memory_id: String,
    pub(crate) content: String,
    pub(crate) source_label: String,
    pub(crate) tags: Vec<String>,
    pub(crate) metadata: JsonValue,
    pub(crate) created_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct SimilarEdge {
    pub(crate) source_id: String,
    pub(crate) target_id: String,
    pub(crate) weight: f64,
    pub(crate) metadata: JsonValue,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct DerivesEdge {
    pub(crate) source_id: String,
    pub(crate) target_id: String,
    pub(crate) extraction_method: String,
    pub(crate) confidence: f64,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct SupersedesEdge {
    pub(crate) source_id: String,
    pub(crate) target_id: String,
    pub(crate) reason: String,
    pub(crate) temporal_delta: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct TransitionEdge {
    pub(crate) source_id: String,
    pub(crate) target_id: String,
    pub(crate) from_value: String,
    pub(crate) to_value: String,
    pub(crate) turn: i64,
    pub(crate) transition_type: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct HierarchicalStats {
    pub(crate) semantic_node_count: usize,
    pub(crate) episodic_node_count: usize,
    pub(crate) similar_to_edge_count: usize,
    pub(crate) derives_from_edge_count: usize,
    pub(crate) supersedes_edge_count: usize,
    pub(crate) transitioned_to_edge_count: usize,
}

#[derive(Debug, Default)]
pub(crate) struct ImportStats {
    pub(crate) semantic_nodes_imported: usize,
    pub(crate) episodic_nodes_imported: usize,
    pub(crate) edges_imported: usize,
    pub(crate) skipped: usize,
    pub(crate) errors: usize,
}

pub(crate) struct HierarchicalImportPlan<'a> {
    pub(crate) episodic_nodes: Vec<&'a EpisodicNode>,
    pub(crate) semantic_nodes: Vec<&'a SemanticNode>,
    pub(crate) similar_to_edges: &'a [SimilarEdge],
    pub(crate) derives_from_edges: &'a [DerivesEdge],
    pub(crate) supersedes_edges: &'a [SupersedesEdge],
    pub(crate) transitioned_to_edges: &'a [TransitionEdge],
    pub(crate) stats: ImportStats,
}

#[derive(Debug)]
pub(crate) struct ExportResult {
    pub(crate) agent_name: String,
    pub(crate) format: String,
    pub(crate) output_path: String,
    pub(crate) file_size_bytes: Option<u64>,
    pub(crate) statistics: Vec<(String, String)>,
}

impl ExportResult {
    pub(crate) fn statistics_lines(&self) -> Vec<(String, String)> {
        self.statistics.clone()
    }
}

#[derive(Debug)]
pub(crate) struct ImportResult {
    pub(crate) agent_name: String,
    pub(crate) format: String,
    pub(crate) source_agent: Option<String>,
    pub(crate) merge: bool,
    pub(crate) statistics: Vec<(String, String)>,
}

impl ImportResult {
    pub(crate) fn statistics_lines(&self) -> Vec<(String, String)> {
        self.statistics.clone()
    }
}
