use super::*;
use crate::commands::memory::BackendChoice;
use crate::commands::memory::backend::kuzu::{kuzu_f64, kuzu_i64, kuzu_rows, kuzu_string};
use kuzu::{
    Connection as KuzuConnection, Database as KuzuDatabase, SystemConfig, Value as KuzuValue,
};
use serde_json::Value as JsonValue;
use std::fs;
use std::io::Read;

pub(super) trait HierarchicalTransferBackend {
    fn export_hierarchical_json(
        &self,
        agent_name: &str,
        output: &str,
        storage_path: Option<&str>,
    ) -> Result<ExportResult>;
    fn import_hierarchical_json(
        &self,
        agent_name: &str,
        input: &str,
        merge: bool,
        storage_path: Option<&str>,
    ) -> Result<ImportResult>;
    fn export_hierarchical_raw_db(
        &self,
        agent_name: &str,
        output: &str,
        storage_path: Option<&str>,
    ) -> Result<ExportResult>;
    fn import_hierarchical_raw_db(
        &self,
        agent_name: &str,
        input: &str,
        merge: bool,
        storage_path: Option<&str>,
    ) -> Result<ImportResult>;
}

pub(super) struct GraphDbHierarchicalTransferBackend;

pub(super) fn open_hierarchical_transfer_backend_for(
    choice: BackendChoice,
) -> Box<dyn HierarchicalTransferBackend> {
    match choice {
        BackendChoice::Sqlite => Box::new(super::sqlite_backend::SqliteHierarchicalTransferBackend),
        BackendChoice::GraphDb => Box::new(GraphDbHierarchicalTransferBackend),
    }
}

pub(super) fn export_hierarchical_json(
    agent_name: &str,
    output: &str,
    storage_path: Option<&str>,
    choice: BackendChoice,
) -> Result<ExportResult> {
    open_hierarchical_transfer_backend_for(choice).export_hierarchical_json(
        agent_name,
        output,
        storage_path,
    )
}

pub(super) fn import_hierarchical_json(
    agent_name: &str,
    input: &str,
    merge: bool,
    storage_path: Option<&str>,
    choice: BackendChoice,
) -> Result<ImportResult> {
    open_hierarchical_transfer_backend_for(choice).import_hierarchical_json(
        agent_name,
        input,
        merge,
        storage_path,
    )
}

pub(super) fn export_hierarchical_raw_db(
    agent_name: &str,
    output: &str,
    storage_path: Option<&str>,
    choice: BackendChoice,
) -> Result<ExportResult> {
    open_hierarchical_transfer_backend_for(choice).export_hierarchical_raw_db(
        agent_name,
        output,
        storage_path,
    )
}

pub(super) fn import_hierarchical_raw_db(
    agent_name: &str,
    input: &str,
    merge: bool,
    storage_path: Option<&str>,
    choice: BackendChoice,
) -> Result<ImportResult> {
    open_hierarchical_transfer_backend_for(choice).import_hierarchical_raw_db(
        agent_name,
        input,
        merge,
        storage_path,
    )
}

impl HierarchicalTransferBackend for GraphDbHierarchicalTransferBackend {
    fn export_hierarchical_json(
        &self,
        agent_name: &str,
        output: &str,
        storage_path: Option<&str>,
    ) -> Result<ExportResult> {
        export_hierarchical_json_impl(agent_name, output, storage_path)
    }

    fn import_hierarchical_json(
        &self,
        agent_name: &str,
        input: &str,
        merge: bool,
        storage_path: Option<&str>,
    ) -> Result<ImportResult> {
        import_hierarchical_json_impl(agent_name, input, merge, storage_path)
    }

    fn export_hierarchical_raw_db(
        &self,
        agent_name: &str,
        output: &str,
        storage_path: Option<&str>,
    ) -> Result<ExportResult> {
        export_hierarchical_raw_db_impl(agent_name, output, storage_path)
    }

    fn import_hierarchical_raw_db(
        &self,
        agent_name: &str,
        input: &str,
        merge: bool,
        storage_path: Option<&str>,
    ) -> Result<ImportResult> {
        import_hierarchical_raw_db_impl(agent_name, input, merge, storage_path)
    }
}

fn export_hierarchical_json_impl(
    agent_name: &str,
    output: &str,
    storage_path: Option<&str>,
) -> Result<ExportResult> {
    let db_path = resolve_hierarchical_db_path(agent_name, storage_path)?;
    let db = KuzuDatabase::new(&db_path, SystemConfig::default())?;
    let conn = KuzuConnection::new(&db)?;
    init_hierarchical_schema(&conn)?;

    let semantic_nodes = kuzu_rows(
        &conn,
        "MATCH (m:SemanticMemory) WHERE m.agent_id = $agent_id RETURN m.memory_id, m.concept, m.content, m.confidence, m.source_id, m.tags, m.metadata, m.created_at, m.entity_name ORDER BY m.created_at ASC",
        vec![("agent_id", KuzuValue::String(agent_name.to_string()))],
    )?
    .into_iter()
    .map(|row| -> Result<SemanticNode> {
        Ok(SemanticNode {
            memory_id: kuzu_string(row.first())?,
            concept: kuzu_string(row.get(1))?,
            content: kuzu_string(row.get(2))?,
            confidence: kuzu_f64(row.get(3))?,
            source_id: kuzu_string(row.get(4))?,
            tags: parse_json_array_of_strings(&kuzu_string(row.get(5))?)?,
            metadata: parse_json_value(&kuzu_string(row.get(6))?)
                .unwrap_or(JsonValue::Object(Default::default())),
            created_at: kuzu_string(row.get(7))?,
            entity_name: kuzu_string(row.get(8))?,
        })
    })
    .collect::<Result<Vec<_>>>()?;

    let episodic_nodes = kuzu_rows(
        &conn,
        "MATCH (e:EpisodicMemory) WHERE e.agent_id = $agent_id RETURN e.memory_id, e.content, e.source_label, e.tags, e.metadata, e.created_at ORDER BY e.created_at ASC",
        vec![("agent_id", KuzuValue::String(agent_name.to_string()))],
    )?
    .into_iter()
    .map(|row| -> Result<EpisodicNode> {
        Ok(EpisodicNode {
            memory_id: kuzu_string(row.first())?,
            content: kuzu_string(row.get(1))?,
            source_label: kuzu_string(row.get(2))?,
            tags: parse_json_array_of_strings(&kuzu_string(row.get(3))?)?,
            metadata: parse_json_value(&kuzu_string(row.get(4))?)
                .unwrap_or(JsonValue::Object(Default::default())),
            created_at: kuzu_string(row.get(5))?,
        })
    })
    .collect::<Result<Vec<_>>>()?;

    let similar_to_edges = kuzu_rows(
        &conn,
        "MATCH (a:SemanticMemory)-[r:SIMILAR_TO]->(b:SemanticMemory) WHERE a.agent_id = $agent_id RETURN a.memory_id, b.memory_id, r.weight, r.metadata",
        vec![("agent_id", KuzuValue::String(agent_name.to_string()))],
    )?
    .into_iter()
    .map(|row| -> Result<SimilarEdge> {
        Ok(SimilarEdge {
            source_id: kuzu_string(row.first())?,
            target_id: kuzu_string(row.get(1))?,
            weight: kuzu_f64(row.get(2))?,
            metadata: parse_json_value(&kuzu_string(row.get(3))?)
                .unwrap_or(JsonValue::Object(Default::default())),
        })
    })
    .collect::<Result<Vec<_>>>()?;

    let derives_from_edges = kuzu_rows(
        &conn,
        "MATCH (s:SemanticMemory)-[r:DERIVES_FROM]->(e:EpisodicMemory) WHERE s.agent_id = $agent_id RETURN s.memory_id, e.memory_id, r.extraction_method, r.confidence",
        vec![("agent_id", KuzuValue::String(agent_name.to_string()))],
    )?
    .into_iter()
    .map(|row| -> Result<DerivesEdge> {
        Ok(DerivesEdge {
            source_id: kuzu_string(row.first())?,
            target_id: kuzu_string(row.get(1))?,
            extraction_method: kuzu_string(row.get(2))?,
            confidence: kuzu_f64(row.get(3))?,
        })
    })
    .collect::<Result<Vec<_>>>()?;

    let supersedes_edges = kuzu_rows(
        &conn,
        "MATCH (newer:SemanticMemory)-[r:SUPERSEDES]->(older:SemanticMemory) WHERE newer.agent_id = $agent_id RETURN newer.memory_id, older.memory_id, r.reason, r.temporal_delta",
        vec![("agent_id", KuzuValue::String(agent_name.to_string()))],
    )?
    .into_iter()
    .map(|row| -> Result<SupersedesEdge> {
        Ok(SupersedesEdge {
            source_id: kuzu_string(row.first())?,
            target_id: kuzu_string(row.get(1))?,
            reason: kuzu_string(row.get(2))?,
            temporal_delta: kuzu_string(row.get(3))?,
        })
    })
    .collect::<Result<Vec<_>>>()?;

    let transitioned_to_edges = kuzu_rows(
        &conn,
        "MATCH (newer:SemanticMemory)-[r:TRANSITIONED_TO]->(older:SemanticMemory) WHERE newer.agent_id = $agent_id RETURN newer.memory_id, older.memory_id, r.from_value, r.to_value, r.turn, r.transition_type",
        vec![("agent_id", KuzuValue::String(agent_name.to_string()))],
    )?
    .into_iter()
    .map(|row| -> Result<TransitionEdge> {
        Ok(TransitionEdge {
            source_id: kuzu_string(row.first())?,
            target_id: kuzu_string(row.get(1))?,
            from_value: kuzu_string(row.get(2))?,
            to_value: kuzu_string(row.get(3))?,
            turn: kuzu_i64(row.get(4))?,
            transition_type: kuzu_string(row.get(5))?,
        })
    })
    .collect::<Result<Vec<_>>>()?;

    let export = HierarchicalExportData {
        agent_name: agent_name.to_string(),
        exported_at: kuzu_export_timestamp(),
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
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let serialized = serde_json::to_string_pretty(&export)?;
    fs::write(&output_path, serialized)?;
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
}

fn import_hierarchical_json_impl(
    agent_name: &str,
    input: &str,
    merge: bool,
    storage_path: Option<&str>,
) -> Result<ImportResult> {
    let input_path = PathBuf::from(input);
    let mut raw = String::new();
    fs::File::open(&input_path)?.read_to_string(&mut raw)?;
    let data: HierarchicalExportData = serde_json::from_str(&raw)?;
    let db_path = resolve_hierarchical_db_path(agent_name, storage_path)?;
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let db = KuzuDatabase::new(&db_path, SystemConfig::default())?;
    let conn = KuzuConnection::new(&db)?;
    init_hierarchical_schema(&conn)?;
    if !merge {
        clear_hierarchical_agent_data(&conn, agent_name)?;
    }
    let existing_ids = if merge {
        get_existing_hierarchical_ids(&conn, agent_name)?
    } else {
        Vec::new()
    };
    let mut stats = ImportStats::default();

    for node in &data.episodic_nodes {
        if node.memory_id.is_empty() {
            stats.errors += 1;
            continue;
        }
        if merge && existing_ids.contains(&node.memory_id) {
            stats.skipped += 1;
            continue;
        }
        let mut prepared = conn.prepare(
            "CREATE (e:EpisodicMemory {memory_id: $memory_id, content: $content, source_label: $source_label, agent_id: $agent_id, tags: $tags, metadata: $metadata, created_at: $created_at})",
        )?;
        if conn
            .execute(
                &mut prepared,
                vec![
                    ("memory_id", KuzuValue::String(node.memory_id.clone())),
                    ("content", KuzuValue::String(node.content.clone())),
                    ("source_label", KuzuValue::String(node.source_label.clone())),
                    ("agent_id", KuzuValue::String(agent_name.to_string())),
                    (
                        "tags",
                        KuzuValue::String(serde_json::to_string(&node.tags)?),
                    ),
                    (
                        "metadata",
                        KuzuValue::String(serde_json::to_string(&node.metadata)?),
                    ),
                    ("created_at", KuzuValue::String(node.created_at.clone())),
                ],
            )
            .is_ok()
        {
            stats.episodic_nodes_imported += 1;
        } else {
            stats.errors += 1;
        }
    }

    for node in &data.semantic_nodes {
        if node.memory_id.is_empty() {
            stats.errors += 1;
            continue;
        }
        if merge && existing_ids.contains(&node.memory_id) {
            stats.skipped += 1;
            continue;
        }
        let mut prepared = conn.prepare(
            "CREATE (m:SemanticMemory {memory_id: $memory_id, concept: $concept, content: $content, confidence: $confidence, source_id: $source_id, agent_id: $agent_id, tags: $tags, metadata: $metadata, created_at: $created_at, entity_name: $entity_name})",
        )?;
        if conn
            .execute(
                &mut prepared,
                vec![
                    ("memory_id", KuzuValue::String(node.memory_id.clone())),
                    ("concept", KuzuValue::String(node.concept.clone())),
                    ("content", KuzuValue::String(node.content.clone())),
                    ("confidence", KuzuValue::Double(node.confidence)),
                    ("source_id", KuzuValue::String(node.source_id.clone())),
                    ("agent_id", KuzuValue::String(agent_name.to_string())),
                    (
                        "tags",
                        KuzuValue::String(serde_json::to_string(&node.tags)?),
                    ),
                    (
                        "metadata",
                        KuzuValue::String(serde_json::to_string(&node.metadata)?),
                    ),
                    ("created_at", KuzuValue::String(node.created_at.clone())),
                    ("entity_name", KuzuValue::String(node.entity_name.clone())),
                ],
            )
            .is_ok()
        {
            stats.semantic_nodes_imported += 1;
        } else {
            stats.errors += 1;
        }
    }

    for edge in &data.similar_to_edges {
        if create_hierarchical_edge(
            &conn,
            "MATCH (a:SemanticMemory {memory_id: $sid}) MATCH (b:SemanticMemory {memory_id: $tid}) CREATE (a)-[:SIMILAR_TO {weight: $weight, metadata: $metadata}]->(b)",
            vec![
                ("sid", KuzuValue::String(edge.source_id.clone())),
                ("tid", KuzuValue::String(edge.target_id.clone())),
                ("weight", KuzuValue::Double(edge.weight)),
                (
                    "metadata",
                    KuzuValue::String(serde_json::to_string(&edge.metadata)?),
                ),
            ],
        )? {
            stats.edges_imported += 1;
        } else {
            stats.errors += 1;
        }
    }
    for edge in &data.derives_from_edges {
        if create_hierarchical_edge(
            &conn,
            "MATCH (s:SemanticMemory {memory_id: $sid}) MATCH (e:EpisodicMemory {memory_id: $tid}) CREATE (s)-[:DERIVES_FROM {extraction_method: $method, confidence: $confidence}]->(e)",
            vec![
                ("sid", KuzuValue::String(edge.source_id.clone())),
                ("tid", KuzuValue::String(edge.target_id.clone())),
                ("method", KuzuValue::String(edge.extraction_method.clone())),
                ("confidence", KuzuValue::Double(edge.confidence)),
            ],
        )? {
            stats.edges_imported += 1;
        } else {
            stats.errors += 1;
        }
    }
    for edge in &data.supersedes_edges {
        if create_hierarchical_edge(
            &conn,
            "MATCH (newer:SemanticMemory {memory_id: $sid}) MATCH (older:SemanticMemory {memory_id: $tid}) CREATE (newer)-[:SUPERSEDES {reason: $reason, temporal_delta: $delta}]->(older)",
            vec![
                ("sid", KuzuValue::String(edge.source_id.clone())),
                ("tid", KuzuValue::String(edge.target_id.clone())),
                ("reason", KuzuValue::String(edge.reason.clone())),
                ("delta", KuzuValue::String(edge.temporal_delta.clone())),
            ],
        )? {
            stats.edges_imported += 1;
        } else {
            stats.errors += 1;
        }
    }
    for edge in &data.transitioned_to_edges {
        if create_hierarchical_edge(
            &conn,
            "MATCH (newer:SemanticMemory {memory_id: $sid}) MATCH (older:SemanticMemory {memory_id: $tid}) CREATE (newer)-[:TRANSITIONED_TO {from_value: $from_val, to_value: $to_val, turn: $turn, transition_type: $ttype}]->(older)",
            vec![
                ("sid", KuzuValue::String(edge.source_id.clone())),
                ("tid", KuzuValue::String(edge.target_id.clone())),
                ("from_val", KuzuValue::String(edge.from_value.clone())),
                ("to_val", KuzuValue::String(edge.to_value.clone())),
                ("turn", KuzuValue::Int64(edge.turn)),
                ("ttype", KuzuValue::String(edge.transition_type.clone())),
            ],
        )? {
            stats.edges_imported += 1;
        } else {
            stats.errors += 1;
        }
    }

    Ok(ImportResult {
        agent_name: agent_name.to_string(),
        format: "json".to_string(),
        source_agent: Some(data.agent_name),
        merge,
        statistics: vec![
            (
                "semantic_nodes_imported".to_string(),
                stats.semantic_nodes_imported.to_string(),
            ),
            (
                "episodic_nodes_imported".to_string(),
                stats.episodic_nodes_imported.to_string(),
            ),
            (
                "edges_imported".to_string(),
                stats.edges_imported.to_string(),
            ),
            ("skipped".to_string(), stats.skipped.to_string()),
            ("errors".to_string(), stats.errors.to_string()),
        ],
    })
}

fn export_hierarchical_raw_db_impl(
    agent_name: &str,
    output: &str,
    storage_path: Option<&str>,
) -> Result<ExportResult> {
    let db_path = resolve_hierarchical_db_path(agent_name, storage_path)?;
    let output_path = PathBuf::from(output);
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }
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
            "Raw graph DB copy (compat alias: kuzu) - use JSON format for node/edge counts"
                .to_string(),
        )],
    })
}

fn import_hierarchical_raw_db_impl(
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
    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent)?;
    }
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
            "Raw graph DB replaced (compat alias: kuzu) - restart agent to use new DB".to_string(),
        )],
    })
}

fn init_hierarchical_schema(conn: &KuzuConnection<'_>) -> Result<()> {
    for statement in HIERARCHICAL_SCHEMA {
        conn.query(statement)?;
    }
    Ok(())
}

fn clear_hierarchical_agent_data(conn: &KuzuConnection<'_>, agent_name: &str) -> Result<()> {
    for query in [
        "MATCH (a:SemanticMemory {agent_id: $aid})-[r:SIMILAR_TO]->() DELETE r",
        "MATCH ()-[r:SIMILAR_TO]->(b:SemanticMemory {agent_id: $aid}) DELETE r",
        "MATCH (s:SemanticMemory {agent_id: $aid})-[r:DERIVES_FROM]->() DELETE r",
        "MATCH (n:SemanticMemory {agent_id: $aid})-[r:SUPERSEDES]->() DELETE r",
        "MATCH ()-[r:SUPERSEDES]->(o:SemanticMemory {agent_id: $aid}) DELETE r",
        "MATCH (n:SemanticMemory {agent_id: $aid})-[r:TRANSITIONED_TO]->() DELETE r",
        "MATCH ()-[r:TRANSITIONED_TO]->(o:SemanticMemory {agent_id: $aid}) DELETE r",
        "MATCH (m:SemanticMemory {agent_id: $aid}) DELETE m",
        "MATCH (e:EpisodicMemory {agent_id: $aid}) DELETE e",
    ] {
        kuzu_rows(
            conn,
            query,
            vec![("aid", KuzuValue::String(agent_name.to_string()))],
        )?;
    }
    Ok(())
}

fn get_existing_hierarchical_ids(
    conn: &KuzuConnection<'_>,
    agent_name: &str,
) -> Result<Vec<String>> {
    let mut ids = Vec::new();
    for query in [
        "MATCH (m:SemanticMemory {agent_id: $aid}) RETURN m.memory_id",
        "MATCH (e:EpisodicMemory {agent_id: $aid}) RETURN e.memory_id",
    ] {
        let rows = kuzu_rows(
            conn,
            query,
            vec![("aid", KuzuValue::String(agent_name.to_string()))],
        )?;
        for row in rows {
            ids.push(kuzu_string(row.first())?);
        }
    }
    Ok(ids)
}

fn create_hierarchical_edge(
    conn: &KuzuConnection<'_>,
    query: &str,
    params: Vec<(&str, KuzuValue)>,
) -> Result<bool> {
    let mut prepared = conn.prepare(query)?;
    Ok(conn.execute(&mut prepared, params).is_ok())
}
