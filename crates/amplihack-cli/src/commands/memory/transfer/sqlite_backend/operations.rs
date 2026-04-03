//! `HierarchicalTransferBackend` implementation for the SQLite backend.

use super::super::backend::HierarchicalTransferBackend;
use super::super::{
    ExportResult, HierarchicalExportData, HierarchicalStats, ImportResult,
    build_hierarchical_import_result, graph_export_timestamp,
};
use super::import_helpers::{
    clear_agent_data, copy_dir, get_existing_ids, insert_nodes_and_edges,
    with_retry_immediate_transaction,
};
use super::loaders::{
    load_derives_from_edges, load_episodic_nodes, load_semantic_nodes, load_similar_to_edges,
    load_supersedes_edges, load_transitioned_to_edges,
};
use super::schema::{MAX_JSON_FILE_SIZE, init_hierarchical_sqlite_schema};
use super::validation::{
    SqliteHierarchicalTransferBackend, open_hierarchical_sqlite_conn,
    resolve_hierarchical_sqlite_path,
};
use crate::commands::memory::ensure_parent_dir;
use anyhow::{Context, Result};
use rusqlite::Connection as SqliteConnection;
use std::fs;
use std::io::Read;
use std::path::PathBuf;

impl HierarchicalTransferBackend for SqliteHierarchicalTransferBackend {
    fn export_hierarchical_json(
        &self,
        agent_name: &str,
        output: &str,
        storage_path: Option<&str>,
    ) -> Result<ExportResult> {
        let db_path = resolve_hierarchical_sqlite_path(agent_name, storage_path)?;
        let conn = open_hierarchical_sqlite_conn(&db_path)?;

        let semantic_nodes = load_semantic_nodes(&conn, agent_name)?;
        let episodic_nodes = load_episodic_nodes(&conn, agent_name)?;
        let similar_to_edges = load_similar_to_edges(&conn, agent_name)?;
        let derives_from_edges = load_derives_from_edges(&conn, agent_name)?;
        let supersedes_edges = load_supersedes_edges(&conn, agent_name)?;
        let transitioned_to_edges = load_transitioned_to_edges(&conn, agent_name)?;

        let statistics = HierarchicalStats {
            semantic_node_count: semantic_nodes.len(),
            episodic_node_count: episodic_nodes.len(),
            similar_to_edge_count: similar_to_edges.len(),
            derives_from_edge_count: derives_from_edges.len(),
            supersedes_edge_count: supersedes_edges.len(),
            transitioned_to_edge_count: transitioned_to_edges.len(),
        };

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
            statistics,
        };

        let output_path = PathBuf::from(output);
        ensure_parent_dir(&output_path)?;

        // Write to a .tmp file, then atomically rename.
        let tmp_path = output_path.with_extension("json.tmp");
        let serialized = serde_json::to_string_pretty(&export)
            .context("failed to serialize hierarchical export data")?;
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
    }

    fn import_hierarchical_json(
        &self,
        agent_name: &str,
        input: &str,
        merge: bool,
        storage_path: Option<&str>,
    ) -> Result<ImportResult> {
        let input_path = PathBuf::from(input);

        // Validate file size before deserializing.
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
        fs::File::open(&input_path)
            .with_context(|| format!("cannot open {}", input_path.display()))?
            .read_to_string(&mut raw)?;
        let data: HierarchicalExportData =
            serde_json::from_str(&raw).context("failed to deserialize hierarchical export JSON")?;

        let db_path = resolve_hierarchical_sqlite_path(agent_name, storage_path)?;
        // `mut` required to start a rusqlite Transaction for the non-merge path.
        let mut conn = open_hierarchical_sqlite_conn(&db_path)?;

        let stats = if !merge {
            // Wrap clear_agent_data + all inserts in a single IMMEDIATE
            // transaction with retry on SQLITE_BUSY.  If the process dies
            // between the clear and the first insert – or any fatal error
            // occurs – the transaction rolls back automatically on drop,
            // leaving original data intact.
            with_retry_immediate_transaction(&mut conn, |tx| {
                clear_agent_data(tx, agent_name)?;
                insert_nodes_and_edges(
                    tx,
                    agent_name,
                    &data,
                    false,
                    &std::collections::HashSet::new(),
                )
            })?
        } else {
            let existing_ids: std::collections::HashSet<String> =
                get_existing_ids(&conn, agent_name)?.into_iter().collect();
            insert_nodes_and_edges(&conn, agent_name, &data, true, &existing_ids)?
        };

        Ok(build_hierarchical_import_result(
            agent_name,
            data.agent_name,
            merge,
            stats,
        ))
    }

    fn export_hierarchical_raw_db(
        &self,
        agent_name: &str,
        output: &str,
        storage_path: Option<&str>,
    ) -> Result<ExportResult> {
        let db_path = resolve_hierarchical_sqlite_path(agent_name, storage_path)?;
        let output_path = PathBuf::from(output);

        ensure_parent_dir(&output_path)?;

        // Remove existing output.
        if output_path.symlink_metadata().is_ok() {
            if output_path.is_dir() {
                fs::remove_dir_all(&output_path)?;
            } else {
                fs::remove_file(&output_path)?;
            }
        }

        // If the source DB exists, copy it; otherwise create an empty output.
        if db_path.symlink_metadata().is_ok() {
            if db_path.is_symlink() {
                anyhow::bail!(
                    "source database is a symlink, refusing to copy: {}",
                    db_path.display()
                );
            }
            fs::copy(&db_path, &output_path).with_context(|| {
                format!(
                    "failed to copy {} to {}",
                    db_path.display(),
                    output_path.display()
                )
            })?;
        } else {
            // Create a new empty DB at the output location.
            let conn = SqliteConnection::open(&output_path)?;
            init_hierarchical_sqlite_schema(&conn)?;
        }

        let file_size = output_path.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(ExportResult {
            agent_name: agent_name.to_string(),
            format: "raw-db".to_string(),
            output_path: output_path.canonicalize()?.display().to_string(),
            file_size_bytes: Some(file_size),
            statistics: vec![(
                "note".to_string(),
                "Raw SQLite DB copy - use JSON format for node/edge counts".to_string(),
            )],
        })
    }

    fn import_hierarchical_raw_db(
        &self,
        agent_name: &str,
        input: &str,
        merge: bool,
        storage_path: Option<&str>,
    ) -> Result<ImportResult> {
        if merge {
            anyhow::bail!(
                "Merge mode is not supported for raw-db format. Use JSON format for merge imports, or set merge=false to replace the DB entirely."
            );
        }

        let input_path = PathBuf::from(input);
        if input_path.symlink_metadata().is_err() {
            anyhow::bail!("input path does not exist: {}", input_path.display());
        }

        let target_path = resolve_hierarchical_sqlite_path(agent_name, storage_path)?;
        ensure_parent_dir(&target_path)?;

        // Backup existing target if present.
        if target_path.symlink_metadata().is_ok() {
            let backup_path = target_path.with_extension("db.bak");
            if backup_path.symlink_metadata().is_ok() {
                if backup_path.is_dir() {
                    fs::remove_dir_all(&backup_path)?;
                } else {
                    fs::remove_file(&backup_path)?;
                }
            }
            fs::rename(&target_path, &backup_path)?;
        }

        if input_path.is_dir() {
            // If input is a directory (unlikely for sqlite but handle gracefully),
            // just copy the directory.
            copy_dir(&input_path, &target_path)?;
        } else {
            fs::copy(&input_path, &target_path).with_context(|| {
                format!(
                    "failed to copy {} to {}",
                    input_path.display(),
                    target_path.display()
                )
            })?;
        }

        Ok(ImportResult {
            agent_name: agent_name.to_string(),
            format: "raw-db".to_string(),
            source_agent: None,
            merge: false,
            statistics: vec![(
                "note".to_string(),
                "Raw SQLite DB replaced - restart agent to use new DB".to_string(),
            )],
        })
    }
}
