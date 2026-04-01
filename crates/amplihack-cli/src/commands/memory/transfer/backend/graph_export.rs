use super::super::*;
use super::graph_helpers::with_hierarchical_graph_conn;
use crate::commands::memory::backend::graph_db::{
    GraphDbValue, graph_f64, graph_i64, graph_rows, graph_string,
};
use crate::commands::memory::{ensure_parent_dir, parse_json_value};
use anyhow::{Context as _, Result};
use serde_json::Value as JsonValue;
use std::fs;
use std::path::PathBuf;

pub(super) fn export_hierarchical_json_impl(
    agent_name: &str,
    output: &str,
    storage_path: Option<&str>,
) -> Result<ExportResult> {
    let db_path = resolve_hierarchical_db_path(agent_name, storage_path)?;
    with_hierarchical_graph_conn(&db_path, |conn| {
        let semantic_nodes = graph_rows(
        conn,
        "MATCH (m:SemanticMemory) WHERE m.agent_id = $agent_id RETURN m.memory_id, m.concept, m.content, m.confidence, m.source_id, m.tags, m.metadata, m.created_at, m.entity_name ORDER BY m.created_at ASC",
        vec![("agent_id", GraphDbValue::String(agent_name.to_string()))],
    )?
    .into_iter()
    .map(|row| -> Result<SemanticNode> {
        Ok(SemanticNode {
            memory_id: graph_string(row.first())?,
            concept: graph_string(row.get(1))?,
            content: graph_string(row.get(2))?,
            confidence: graph_f64(row.get(3))?,
            source_id: graph_string(row.get(4))?,
            tags: parse_json_array_of_strings(&graph_string(row.get(5))?)?,
            metadata: parse_json_value(&graph_string(row.get(6))?)
                .unwrap_or(JsonValue::Object(Default::default())),
            created_at: graph_string(row.get(7))?,
            entity_name: graph_string(row.get(8))?,
        })
    })
    .collect::<Result<Vec<_>>>()?;

        let episodic_nodes = graph_rows(
        conn,
        "MATCH (e:EpisodicMemory) WHERE e.agent_id = $agent_id RETURN e.memory_id, e.content, e.source_label, e.tags, e.metadata, e.created_at ORDER BY e.created_at ASC",
        vec![("agent_id", GraphDbValue::String(agent_name.to_string()))],
    )?
    .into_iter()
    .map(|row| -> Result<EpisodicNode> {
        Ok(EpisodicNode {
            memory_id: graph_string(row.first())?,
            content: graph_string(row.get(1))?,
            source_label: graph_string(row.get(2))?,
            tags: parse_json_array_of_strings(&graph_string(row.get(3))?)?,
            metadata: parse_json_value(&graph_string(row.get(4))?)
                .unwrap_or(JsonValue::Object(Default::default())),
            created_at: graph_string(row.get(5))?,
        })
    })
    .collect::<Result<Vec<_>>>()?;

        let similar_to_edges = graph_rows(
        conn,
        "MATCH (a:SemanticMemory)-[r:SIMILAR_TO]->(b:SemanticMemory) WHERE a.agent_id = $agent_id RETURN a.memory_id, b.memory_id, r.weight, r.metadata",
        vec![("agent_id", GraphDbValue::String(agent_name.to_string()))],
    )?
    .into_iter()
    .map(|row| -> Result<SimilarEdge> {
        Ok(SimilarEdge {
            source_id: graph_string(row.first())?,
            target_id: graph_string(row.get(1))?,
            weight: graph_f64(row.get(2))?,
            metadata: parse_json_value(&graph_string(row.get(3))?)
                .unwrap_or(JsonValue::Object(Default::default())),
        })
    })
    .collect::<Result<Vec<_>>>()?;

        let derives_from_edges = graph_rows(
        conn,
        "MATCH (s:SemanticMemory)-[r:DERIVES_FROM]->(e:EpisodicMemory) WHERE s.agent_id = $agent_id RETURN s.memory_id, e.memory_id, r.extraction_method, r.confidence",
        vec![("agent_id", GraphDbValue::String(agent_name.to_string()))],
    )?
    .into_iter()
    .map(|row| -> Result<DerivesEdge> {
        Ok(DerivesEdge {
            source_id: graph_string(row.first())?,
            target_id: graph_string(row.get(1))?,
            extraction_method: graph_string(row.get(2))?,
            confidence: graph_f64(row.get(3))?,
        })
    })
    .collect::<Result<Vec<_>>>()?;

        let supersedes_edges = graph_rows(
        conn,
        "MATCH (newer:SemanticMemory)-[r:SUPERSEDES]->(older:SemanticMemory) WHERE newer.agent_id = $agent_id RETURN newer.memory_id, older.memory_id, r.reason, r.temporal_delta",
        vec![("agent_id", GraphDbValue::String(agent_name.to_string()))],
    )?
    .into_iter()
    .map(|row| -> Result<SupersedesEdge> {
        Ok(SupersedesEdge {
            source_id: graph_string(row.first())?,
            target_id: graph_string(row.get(1))?,
            reason: graph_string(row.get(2))?,
            temporal_delta: graph_string(row.get(3))?,
        })
    })
    .collect::<Result<Vec<_>>>()?;

        let transitioned_to_edges = graph_rows(
        conn,
        "MATCH (newer:SemanticMemory)-[r:TRANSITIONED_TO]->(older:SemanticMemory) WHERE newer.agent_id = $agent_id RETURN newer.memory_id, older.memory_id, r.from_value, r.to_value, r.turn, r.transition_type",
        vec![("agent_id", GraphDbValue::String(agent_name.to_string()))],
    )?
    .into_iter()
    .map(|row| -> Result<TransitionEdge> {
        Ok(TransitionEdge {
            source_id: graph_string(row.first())?,
            target_id: graph_string(row.get(1))?,
            from_value: graph_string(row.get(2))?,
            to_value: graph_string(row.get(3))?,
            turn: graph_i64(row.get(4))?,
            transition_type: graph_string(row.get(5))?,
        })
    })
    .collect::<Result<Vec<_>>>()?;

        let export = HierarchicalExportData {
            agent_name: agent_name.to_string(),
            exported_at: graph_export_timestamp(),
            format_version: "1.1".to_string(),
            semantic_nodes,
            episodic_nodes,
            similar_to_edges,
            derives_from_edges,
            supersedes_edges,
            transitioned_to_edges,
            statistics: HierarchicalStats::default(),
        };
        let mut export = export;
        export.statistics = HierarchicalStats {
            semantic_node_count: export.semantic_nodes.len(),
            episodic_node_count: export.episodic_nodes.len(),
            similar_to_edge_count: export.similar_to_edges.len(),
            derives_from_edge_count: export.derives_from_edges.len(),
            supersedes_edge_count: export.supersedes_edges.len(),
            transitioned_to_edge_count: export.transitioned_to_edges.len(),
        };

        let output_path = PathBuf::from(output);
        ensure_parent_dir(&output_path)?;
        // Write to a .tmp file first, then atomically rename — mirrors the SQLite
        // backend's behaviour so both backends provide the same crash-safety guarantee.
        let tmp_path = output_path.with_extension("json.tmp");
        let serialized = serde_json::to_string_pretty(&export)?;
        fs::write(&tmp_path, &serialized)
            .with_context(|| format!("failed to write tmp file {}", tmp_path.display()))?;
        fs::rename(&tmp_path, &output_path)
            .with_context(|| format!("failed to rename tmp to {}", output_path.display()))?;
        let file_size = output_path.metadata()?.len();
        Ok(ExportResult {
            agent_name: agent_name.to_string(),
            format: "json".to_string(),
            output_path: output_path.canonicalize()?.display().to_string(),
            file_size_bytes: Some(file_size),
            statistics: vec![
                (
                    "semantic_node_count".to_string(),
                    export.statistics.semantic_node_count.to_string(),
                ),
                (
                    "episodic_node_count".to_string(),
                    export.statistics.episodic_node_count.to_string(),
                ),
                (
                    "similar_to_edge_count".to_string(),
                    export.statistics.similar_to_edge_count.to_string(),
                ),
                (
                    "derives_from_edge_count".to_string(),
                    export.statistics.derives_from_edge_count.to_string(),
                ),
                (
                    "supersedes_edge_count".to_string(),
                    export.statistics.supersedes_edge_count.to_string(),
                ),
                (
                    "transitioned_to_edge_count".to_string(),
                    export.statistics.transitioned_to_edge_count.to_string(),
                ),
            ],
        })
    })
}

pub(super) fn export_hierarchical_raw_db_impl(
    agent_name: &str,
    output: &str,
    storage_path: Option<&str>,
) -> Result<ExportResult> {
    let db_path = resolve_hierarchical_db_path(agent_name, storage_path)?;
    let output_path = PathBuf::from(output);
    ensure_parent_dir(&output_path)?;
    if output_path.exists() {
        if output_path.is_dir() {
            fs::remove_dir_all(&output_path)?;
        } else {
            fs::remove_file(&output_path)?;
        }
    }
    copy_hierarchical_storage(&db_path, &output_path)?;
    let size = compute_path_size(&output_path)?;
    Ok(ExportResult {
        agent_name: agent_name.to_string(),
        format: "raw-db".to_string(),
        output_path: output_path.canonicalize()?.display().to_string(),
        file_size_bytes: Some(size),
        statistics: vec![(
            "note".to_string(),
            "Raw graph DB copy - use JSON format for node/edge counts".to_string(),
        )],
    })
}
