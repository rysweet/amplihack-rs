use super::super::*;
use super::graph_helpers::{
    clear_hierarchical_agent_data, create_hierarchical_edge, get_existing_hierarchical_ids,
    with_hierarchical_graph_conn,
};
use super::trait_def::MAX_JSON_FILE_SIZE;
use crate::commands::memory::backend::graph_db::GraphDbValue;
use crate::commands::memory::ensure_parent_dir;
use anyhow::{Context as _, Result};
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use tracing;

pub(super) fn import_hierarchical_json_impl(
    agent_name: &str,
    input: &str,
    merge: bool,
    storage_path: Option<&str>,
) -> Result<ImportResult> {
    let input_path = PathBuf::from(input);

    // Guard against OOM from giant files — mirrors the 500 MiB check in the
    // SQLite backend so both backends have equivalent input validation.
    let file_meta = input_path
        .metadata()
        .with_context(|| format!("cannot stat input file {}", input_path.display()))?;
    if file_meta.len() > MAX_JSON_FILE_SIZE {
        anyhow::bail!(
            "input file exceeds maximum allowed size ({} bytes > {MAX_JSON_FILE_SIZE} bytes)",
            file_meta.len()
        );
    }

    let mut raw = String::new();
    fs::File::open(&input_path)?.read_to_string(&mut raw)?;
    let data: HierarchicalExportData = serde_json::from_str(&raw)?;
    let db_path = resolve_hierarchical_db_path(agent_name, storage_path)?;
    with_hierarchical_graph_conn(&db_path, |conn| {
        if !merge {
            clear_hierarchical_agent_data(conn, agent_name)?;
        }
        let existing_ids: std::collections::HashSet<String> = if merge {
            get_existing_hierarchical_ids(conn, agent_name)?
                .into_iter()
                .collect()
        } else {
            std::collections::HashSet::new()
        };
        let mut plan = build_hierarchical_import_plan(&data, merge, |memory_id| {
            existing_ids.contains(memory_id)
        });
        let mut stats = std::mem::take(&mut plan.stats);

        for node in plan.episodic_nodes {
            let mut prepared = conn.prepare(
            "CREATE (e:EpisodicMemory {memory_id: $memory_id, content: $content, source_label: $source_label, agent_id: $agent_id, tags: $tags, metadata: $metadata, created_at: $created_at})",
        )?;
            let ep_result = conn.execute(
                &mut prepared,
                vec![
                    ("memory_id", GraphDbValue::String(node.memory_id.clone())),
                    ("content", GraphDbValue::String(node.content.clone())),
                    (
                        "source_label",
                        GraphDbValue::String(node.source_label.clone()),
                    ),
                    ("agent_id", GraphDbValue::String(agent_name.to_string())),
                    (
                        "tags",
                        GraphDbValue::String(serde_json::to_string(&node.tags)?),
                    ),
                    (
                        "metadata",
                        GraphDbValue::String(serde_json::to_string(&node.metadata)?),
                    ),
                    ("created_at", GraphDbValue::String(node.created_at.clone())),
                ],
            );
            if ep_result.is_ok() {
                stats.episodic_nodes_imported += 1;
            } else {
                tracing::warn!(memory_id = %node.memory_id, "failed to insert episodic node into graph-db");
                stats.errors += 1;
            }
        }

        for node in plan.semantic_nodes {
            let mut prepared = conn.prepare(
            "CREATE (m:SemanticMemory {memory_id: $memory_id, concept: $concept, content: $content, confidence: $confidence, source_id: $source_id, agent_id: $agent_id, tags: $tags, metadata: $metadata, created_at: $created_at, entity_name: $entity_name})",
        )?;
            if conn
                .execute(
                    &mut prepared,
                    vec![
                        ("memory_id", GraphDbValue::String(node.memory_id.clone())),
                        ("concept", GraphDbValue::String(node.concept.clone())),
                        ("content", GraphDbValue::String(node.content.clone())),
                        ("confidence", GraphDbValue::Double(node.confidence)),
                        ("source_id", GraphDbValue::String(node.source_id.clone())),
                        ("agent_id", GraphDbValue::String(agent_name.to_string())),
                        (
                            "tags",
                            GraphDbValue::String(serde_json::to_string(&node.tags)?),
                        ),
                        (
                            "metadata",
                            GraphDbValue::String(serde_json::to_string(&node.metadata)?),
                        ),
                        ("created_at", GraphDbValue::String(node.created_at.clone())),
                        (
                            "entity_name",
                            GraphDbValue::String(node.entity_name.clone()),
                        ),
                    ],
                )
                .is_ok()
            {
                stats.semantic_nodes_imported += 1;
            } else {
                tracing::warn!(memory_id = %node.memory_id, "failed to insert semantic node into graph-db");
                stats.errors += 1;
            }
        }

        for edge in plan.similar_to_edges {
            if create_hierarchical_edge(
                conn,
                "MATCH (a:SemanticMemory {memory_id: $sid}) MATCH (b:SemanticMemory {memory_id: $tid}) CREATE (a)-[:SIMILAR_TO {weight: $weight, metadata: $metadata}]->(b)",
                vec![
                    ("sid", GraphDbValue::String(edge.source_id.clone())),
                    ("tid", GraphDbValue::String(edge.target_id.clone())),
                    ("weight", GraphDbValue::Double(edge.weight)),
                    (
                        "metadata",
                        GraphDbValue::String(serde_json::to_string(&edge.metadata)?),
                    ),
                ],
            )? {
                stats.edges_imported += 1;
            } else {
                tracing::warn!(source = %edge.source_id, target = %edge.target_id, "failed to insert SIMILAR_TO edge into graph-db");
                stats.errors += 1;
            }
        }
        for edge in plan.derives_from_edges {
            if create_hierarchical_edge(
                conn,
                "MATCH (s:SemanticMemory {memory_id: $sid}) MATCH (e:EpisodicMemory {memory_id: $tid}) CREATE (s)-[:DERIVES_FROM {extraction_method: $method, confidence: $confidence}]->(e)",
                vec![
                    ("sid", GraphDbValue::String(edge.source_id.clone())),
                    ("tid", GraphDbValue::String(edge.target_id.clone())),
                    (
                        "method",
                        GraphDbValue::String(edge.extraction_method.clone()),
                    ),
                    ("confidence", GraphDbValue::Double(edge.confidence)),
                ],
            )? {
                stats.edges_imported += 1;
            } else {
                tracing::warn!(source = %edge.source_id, target = %edge.target_id, "failed to insert DERIVES_FROM edge into graph-db");
                stats.errors += 1;
            }
        }
        for edge in plan.supersedes_edges {
            if create_hierarchical_edge(
                conn,
                "MATCH (newer:SemanticMemory {memory_id: $sid}) MATCH (older:SemanticMemory {memory_id: $tid}) CREATE (newer)-[:SUPERSEDES {reason: $reason, temporal_delta: $delta}]->(older)",
                vec![
                    ("sid", GraphDbValue::String(edge.source_id.clone())),
                    ("tid", GraphDbValue::String(edge.target_id.clone())),
                    ("reason", GraphDbValue::String(edge.reason.clone())),
                    ("delta", GraphDbValue::String(edge.temporal_delta.clone())),
                ],
            )? {
                stats.edges_imported += 1;
            } else {
                tracing::warn!(source = %edge.source_id, target = %edge.target_id, "failed to insert SUPERSEDES edge into graph-db");
                stats.errors += 1;
            }
        }
        for edge in plan.transitioned_to_edges {
            if create_hierarchical_edge(
                conn,
                "MATCH (newer:SemanticMemory {memory_id: $sid}) MATCH (older:SemanticMemory {memory_id: $tid}) CREATE (newer)-[:TRANSITIONED_TO {from_value: $from_val, to_value: $to_val, turn: $turn, transition_type: $ttype}]->(older)",
                vec![
                    ("sid", GraphDbValue::String(edge.source_id.clone())),
                    ("tid", GraphDbValue::String(edge.target_id.clone())),
                    ("from_val", GraphDbValue::String(edge.from_value.clone())),
                    ("to_val", GraphDbValue::String(edge.to_value.clone())),
                    ("turn", GraphDbValue::Int64(edge.turn)),
                    ("ttype", GraphDbValue::String(edge.transition_type.clone())),
                ],
            )? {
                stats.edges_imported += 1;
            } else {
                tracing::warn!(source = %edge.source_id, target = %edge.target_id, "failed to insert TRANSITIONED_TO edge into graph-db");
                stats.errors += 1;
            }
        }

        Ok(build_hierarchical_import_result(
            agent_name,
            data.agent_name.clone(),
            merge,
            stats,
        ))
    })
}

pub(super) fn import_hierarchical_raw_db_impl(
    agent_name: &str,
    input: &str,
    merge: bool,
    storage_path: Option<&str>,
) -> Result<ImportResult> {
    if merge {
        anyhow::bail!(
            "Merge mode is not supported for raw-db format. Use JSON format for merge imports, or set merge=False to replace the DB entirely."
        );
    }
    let input_path = PathBuf::from(input);
    if !input_path.exists() {
        anyhow::bail!("Input path does not exist: {}", input_path.display());
    }
    let target_path = resolve_hierarchical_db_path(agent_name, storage_path)?;
    ensure_parent_dir(&target_path)?;
    if target_path.exists() {
        let backup_path = target_path.with_extension("bak");
        if backup_path.exists() {
            if backup_path.is_dir() {
                fs::remove_dir_all(&backup_path)?;
            } else {
                fs::remove_file(&backup_path)?;
            }
        }
        fs::rename(&target_path, &backup_path)?;
    }
    copy_hierarchical_storage(&input_path, &target_path)?;
    Ok(ImportResult {
        agent_name: agent_name.to_string(),
        format: "raw-db".to_string(),
        source_agent: None,
        merge: false,
        statistics: vec![(
            "note".to_string(),
            "Raw graph DB replaced - restart agent to use new DB".to_string(),
        )],
    })
}
