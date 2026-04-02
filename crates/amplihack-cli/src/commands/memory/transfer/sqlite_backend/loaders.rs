//! Functions that load nodes and edges from a hierarchical SQLite database.

use super::super::{
    DerivesEdge, EpisodicNode, SemanticNode, SimilarEdge, SupersedesEdge, TransitionEdge,
};
use crate::commands::memory::parse_json_value;
use anyhow::{Context, Result};
use rusqlite::{Connection as SqliteConnection, params};
use serde_json::Value as JsonValue;

pub(super) fn load_semantic_nodes(
    conn: &SqliteConnection,
    agent_name: &str,
) -> Result<Vec<SemanticNode>> {
    let mut stmt = conn.prepare(
        "SELECT memory_id, concept, content, confidence, source_id, tags, metadata, created_at, entity_name FROM semantic_memories WHERE agent_id = ?1 ORDER BY created_at ASC",
    )?;
    let rows = stmt
        .query_map(params![agent_name], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, f64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("failed to load semantic nodes")?;

    rows.into_iter()
        .map(
            |(
                memory_id,
                concept,
                content,
                confidence,
                source_id,
                tags_raw,
                metadata_raw,
                created_at,
                entity_name,
            )| {
                Ok(SemanticNode {
                    memory_id,
                    concept,
                    content,
                    confidence,
                    source_id,
                    tags: serde_json::from_str(&tags_raw).unwrap_or_default(),
                    metadata: parse_json_value(&metadata_raw)
                        .unwrap_or(JsonValue::Object(Default::default())),
                    created_at,
                    entity_name,
                })
            },
        )
        .collect()
}

pub(super) fn load_episodic_nodes(
    conn: &SqliteConnection,
    agent_name: &str,
) -> Result<Vec<EpisodicNode>> {
    let mut stmt = conn.prepare(
        "SELECT memory_id, content, source_label, tags, metadata, created_at FROM episodic_memories WHERE agent_id = ?1 ORDER BY created_at ASC",
    )?;
    let rows = stmt
        .query_map(params![agent_name], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("failed to load episodic nodes")?;

    rows.into_iter()
        .map(
            |(memory_id, content, source_label, tags_raw, metadata_raw, created_at)| {
                Ok(EpisodicNode {
                    memory_id,
                    content,
                    source_label,
                    tags: serde_json::from_str(&tags_raw).unwrap_or_default(),
                    metadata: parse_json_value(&metadata_raw)
                        .unwrap_or(JsonValue::Object(Default::default())),
                    created_at,
                })
            },
        )
        .collect()
}

pub(super) fn load_similar_to_edges(
    conn: &SqliteConnection,
    agent_name: &str,
) -> Result<Vec<SimilarEdge>> {
    let mut stmt = conn.prepare(
        "SELECT s.source_id, s.target_id, s.weight, s.metadata FROM similar_to_edges s JOIN semantic_memories sm ON s.source_id = sm.memory_id WHERE sm.agent_id = ?1",
    )?;
    let rows = stmt
        .query_map(params![agent_name], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, f64>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("failed to load similar_to edges")?;

    rows.into_iter()
        .map(|(source_id, target_id, weight, metadata_raw)| {
            Ok(SimilarEdge {
                source_id,
                target_id,
                weight,
                metadata: parse_json_value(&metadata_raw)
                    .unwrap_or(JsonValue::Object(Default::default())),
            })
        })
        .collect()
}

pub(super) fn load_derives_from_edges(
    conn: &SqliteConnection,
    agent_name: &str,
) -> Result<Vec<DerivesEdge>> {
    let mut stmt = conn.prepare(
        "SELECT d.source_id, d.target_id, d.extraction_method, d.confidence FROM derives_from_edges d JOIN semantic_memories sm ON d.source_id = sm.memory_id WHERE sm.agent_id = ?1",
    )?;
    let rows = stmt
        .query_map(params![agent_name], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, f64>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("failed to load derives_from edges")?;

    rows.into_iter()
        .map(|(source_id, target_id, extraction_method, confidence)| {
            Ok(DerivesEdge {
                source_id,
                target_id,
                extraction_method,
                confidence,
            })
        })
        .collect()
}

pub(super) fn load_supersedes_edges(
    conn: &SqliteConnection,
    agent_name: &str,
) -> Result<Vec<SupersedesEdge>> {
    let mut stmt = conn.prepare(
        "SELECT s.source_id, s.target_id, s.reason, s.temporal_delta FROM supersedes_edges s JOIN semantic_memories sm ON s.source_id = sm.memory_id WHERE sm.agent_id = ?1",
    )?;
    let rows = stmt
        .query_map(params![agent_name], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("failed to load supersedes edges")?;

    rows.into_iter()
        .map(|(source_id, target_id, reason, temporal_delta)| {
            Ok(SupersedesEdge {
                source_id,
                target_id,
                reason,
                temporal_delta,
            })
        })
        .collect()
}

pub(super) fn load_transitioned_to_edges(
    conn: &SqliteConnection,
    agent_name: &str,
) -> Result<Vec<TransitionEdge>> {
    let mut stmt = conn.prepare(
        "SELECT t.source_id, t.target_id, t.from_value, t.to_value, t.turn, t.transition_type FROM transitioned_to_edges t JOIN semantic_memories sm ON t.source_id = sm.memory_id WHERE sm.agent_id = ?1",
    )?;
    let rows = stmt
        .query_map(params![agent_name], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("failed to load transitioned_to edges")?;

    rows.into_iter()
        .map(
            |(source_id, target_id, from_value, to_value, turn, transition_type)| {
                Ok(TransitionEdge {
                    source_id,
                    target_id,
                    from_value,
                    to_value,
                    turn,
                    transition_type,
                })
            },
        )
        .collect()
}
