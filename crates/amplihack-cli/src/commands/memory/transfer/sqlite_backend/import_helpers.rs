//! Helpers for importing data into a hierarchical SQLite database.

use super::super::{HierarchicalExportData, ImportStats, build_hierarchical_import_plan};
use anyhow::Result;
use rusqlite::{Connection as SqliteConnection, params};
use std::fs;
use tracing;

/// Insert all nodes and edges from `data` into `conn`, respecting merge semantics.
///
/// Accepts any `&rusqlite::Connection` (including `&*Transaction` via Deref) so
/// callers can wrap this in an explicit transaction without changing the signature.
pub(super) fn insert_nodes_and_edges(
    conn: &SqliteConnection,
    agent_name: &str,
    data: &HierarchicalExportData,
    merge: bool,
    existing_ids: &std::collections::HashSet<String>,
) -> Result<ImportStats> {
    let mut plan =
        build_hierarchical_import_plan(data, merge, |memory_id| existing_ids.contains(memory_id));
    let mut stats = std::mem::take(&mut plan.stats);

    for node in plan.episodic_nodes {
        let result = conn.execute(
            "INSERT OR IGNORE INTO episodic_memories (memory_id, agent_id, content, source_label, tags, metadata, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                node.memory_id,
                agent_name,
                node.content,
                node.source_label,
                serde_json::to_string(&node.tags)?,
                serde_json::to_string(&node.metadata)?,
                node.created_at,
            ],
        );
        match result {
            Ok(_) => stats.episodic_nodes_imported += 1,
            Err(e) => {
                tracing::warn!(memory_id = %node.memory_id, error = %e, "failed to insert episodic node");
                stats.errors += 1;
            }
        }
    }

    for node in plan.semantic_nodes {
        let result = conn.execute(
            "INSERT OR IGNORE INTO semantic_memories (memory_id, agent_id, concept, content, confidence, source_id, tags, metadata, created_at, entity_name) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                node.memory_id,
                agent_name,
                node.concept,
                node.content,
                node.confidence,
                node.source_id,
                serde_json::to_string(&node.tags)?,
                serde_json::to_string(&node.metadata)?,
                node.created_at,
                node.entity_name,
            ],
        );
        match result {
            Ok(_) => stats.semantic_nodes_imported += 1,
            Err(e) => {
                tracing::warn!(memory_id = %node.memory_id, error = %e, "failed to insert semantic node");
                stats.errors += 1;
            }
        }
    }

    for edge in plan.similar_to_edges {
        let result = conn.execute(
            "INSERT INTO similar_to_edges (source_id, target_id, weight, metadata) VALUES (?1, ?2, ?3, ?4)",
            params![
                edge.source_id,
                edge.target_id,
                edge.weight,
                serde_json::to_string(&edge.metadata)?,
            ],
        );
        match result {
            Ok(_) => stats.edges_imported += 1,
            Err(e) => {
                tracing::warn!(source = %edge.source_id, target = %edge.target_id, error = %e, "failed to insert similar_to edge");
                stats.errors += 1;
            }
        }
    }

    for edge in plan.derives_from_edges {
        let result = conn.execute(
            "INSERT INTO derives_from_edges (source_id, target_id, extraction_method, confidence) VALUES (?1, ?2, ?3, ?4)",
            params![
                edge.source_id,
                edge.target_id,
                edge.extraction_method,
                edge.confidence,
            ],
        );
        match result {
            Ok(_) => stats.edges_imported += 1,
            Err(e) => {
                tracing::warn!(source = %edge.source_id, target = %edge.target_id, error = %e, "failed to insert derives_from edge");
                stats.errors += 1;
            }
        }
    }

    for edge in plan.supersedes_edges {
        let result = conn.execute(
            "INSERT INTO supersedes_edges (source_id, target_id, reason, temporal_delta) VALUES (?1, ?2, ?3, ?4)",
            params![
                edge.source_id,
                edge.target_id,
                edge.reason,
                edge.temporal_delta,
            ],
        );
        match result {
            Ok(_) => stats.edges_imported += 1,
            Err(e) => {
                tracing::warn!(source = %edge.source_id, target = %edge.target_id, error = %e, "failed to insert supersedes edge");
                stats.errors += 1;
            }
        }
    }

    for edge in plan.transitioned_to_edges {
        let result = conn.execute(
            "INSERT INTO transitioned_to_edges (source_id, target_id, from_value, to_value, turn, transition_type) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                edge.source_id,
                edge.target_id,
                edge.from_value,
                edge.to_value,
                edge.turn,
                edge.transition_type,
            ],
        );
        match result {
            Ok(_) => stats.edges_imported += 1,
            Err(e) => {
                tracing::warn!(source = %edge.source_id, target = %edge.target_id, error = %e, "failed to insert transitioned_to edge");
                stats.errors += 1;
            }
        }
    }

    Ok(stats)
}

pub(super) fn clear_agent_data(conn: &SqliteConnection, agent_name: &str) -> Result<()> {
    // Delete edges whose source node belongs to this agent.
    conn.execute(
        "DELETE FROM similar_to_edges WHERE source_id IN (SELECT memory_id FROM semantic_memories WHERE agent_id = ?1)",
        params![agent_name],
    )?;
    conn.execute(
        "DELETE FROM derives_from_edges WHERE source_id IN (SELECT memory_id FROM semantic_memories WHERE agent_id = ?1)",
        params![agent_name],
    )?;
    conn.execute(
        "DELETE FROM supersedes_edges WHERE source_id IN (SELECT memory_id FROM semantic_memories WHERE agent_id = ?1)",
        params![agent_name],
    )?;
    conn.execute(
        "DELETE FROM transitioned_to_edges WHERE source_id IN (SELECT memory_id FROM semantic_memories WHERE agent_id = ?1)",
        params![agent_name],
    )?;
    conn.execute(
        "DELETE FROM semantic_memories WHERE agent_id = ?1",
        params![agent_name],
    )?;
    conn.execute(
        "DELETE FROM episodic_memories WHERE agent_id = ?1",
        params![agent_name],
    )?;
    Ok(())
}

pub(super) fn get_existing_ids(conn: &SqliteConnection, agent_name: &str) -> Result<Vec<String>> {
    let mut ids = Vec::new();
    let mut stmt = conn.prepare("SELECT memory_id FROM semantic_memories WHERE agent_id = ?1")?;
    let rows = stmt
        .query_map(params![agent_name], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ids.extend(rows);

    let mut stmt2 = conn.prepare("SELECT memory_id FROM episodic_memories WHERE agent_id = ?1")?;
    let rows2 = stmt2
        .query_map(params![agent_name], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ids.extend(rows2);
    Ok(ids)
}

pub(super) fn copy_dir(src: &std::path::Path, dst: &std::path::Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let file_type = entry.file_type()?;
        // Skip symlinks to prevent directory-traversal attacks, mirroring the
        // behaviour of copy_dir_recursive_inner in the graph-db backend.
        if file_type.is_symlink() {
            tracing::warn!("Skipping symlink during copy: {}", from.display());
            continue;
        }
        if file_type.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}
