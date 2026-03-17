//! Parity tests for `HierarchicalTransferBackend` (S-2d).
//!
//! 14 functions exercising both `SqliteHierarchicalTransferBackend` and
//! `GraphDbHierarchicalTransferBackend` to confirm identical semantics across
//! export/import/raw-db flows.
//!
//! The `open_hierarchical_transfer_backend` factory and
//! `SqliteHierarchicalTransferBackend` do NOT exist yet. These tests will
//! fail to compile until S-2a/S-2b/S-2c are implemented.

use crate::commands::memory::BackendChoice;
use crate::commands::memory::transfer::backend::open_hierarchical_transfer_backend_for;
use crate::commands::memory::transfer::{HierarchicalExportData, HierarchicalStats, ImportResult};
use crate::test_support::home_env_lock;
use std::fs;

// ---------------------------------------------------------------------------
// EnvGuard
// ---------------------------------------------------------------------------

struct EnvGuard {
    prev_home: Option<std::ffi::OsString>,
    prev_backend: Option<std::ffi::OsString>,
    #[allow(dead_code)]
    lock: std::sync::MutexGuard<'static, ()>,
}

impl EnvGuard {
    fn setup(home: &std::path::Path, backend: &str) -> Self {
        let lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
        let prev_home = std::env::var_os("HOME");
        let prev_backend = std::env::var_os("AMPLIHACK_MEMORY_BACKEND");
        unsafe {
            std::env::set_var("HOME", home);
            std::env::set_var("AMPLIHACK_MEMORY_BACKEND", backend);
        }
        Self {
            prev_home,
            prev_backend,
            lock,
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe {
            match self.prev_home.take() {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            match self.prev_backend.take() {
                Some(v) => std::env::set_var("AMPLIHACK_MEMORY_BACKEND", v),
                None => std::env::remove_var("AMPLIHACK_MEMORY_BACKEND"),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn minimal_export_data(agent_name: &str) -> HierarchicalExportData {
    HierarchicalExportData {
        agent_name: agent_name.to_string(),
        exported_at: "1704067200".to_string(),
        format_version: "1.1".to_string(),
        semantic_nodes: vec![],
        episodic_nodes: vec![],
        similar_to_edges: vec![],
        derives_from_edges: vec![],
        supersedes_edges: vec![],
        transitioned_to_edges: vec![],
        statistics: HierarchicalStats::default(),
    }
}

fn write_export_file(path: &std::path::Path, data: &HierarchicalExportData) {
    fs::write(path, serde_json::to_string_pretty(data).expect("serialize"))
        .expect("write export file");
}

fn assert_stat(result: &ImportResult, key: &str, value: &str) {
    assert!(
        result
            .statistics
            .iter()
            .any(|(k, v)| k == key && v == value),
        "expected statistic '{key}={value}' in import result, got: {:?}",
        result.statistics
    );
}

// ---------------------------------------------------------------------------
// export_empty_agent – both backends
// ---------------------------------------------------------------------------

/// Exporting an empty agent via the SQLite transfer backend must succeed and
/// report 0 semantic + episodic nodes.
#[test]
fn export_empty_agent_sqlite() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let _guard = EnvGuard::setup(dir.path(), "sqlite");
    let db_path = dir.path().join("agent.sqlite");
    let out_path = dir.path().join("empty_export.json");

    let backend = open_hierarchical_transfer_backend_for(BackendChoice::Sqlite);
    let result = backend.export_hierarchical_json(
        "empty-agent",
        out_path.to_str().unwrap(),
        Some(db_path.to_str().unwrap()),
    )?;

    assert_eq!(result.format, "json", "format must be 'json'");
    let exported: HierarchicalExportData = serde_json::from_str(&fs::read_to_string(&out_path)?)?;
    assert_eq!(exported.semantic_nodes.len(), 0, "no semantic nodes");
    assert_eq!(exported.episodic_nodes.len(), 0, "no episodic nodes");
    Ok(())
}

/// Exporting an empty agent via the graph-db transfer backend must succeed.
#[test]
fn export_empty_agent_graph_db() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let _guard = EnvGuard::setup(dir.path(), "graph-db");
    let db_path = dir.path().join("agent.graph_db");
    let out_path = dir.path().join("empty_export.json");

    let backend = open_hierarchical_transfer_backend_for(BackendChoice::GraphDb);
    let result = backend.export_hierarchical_json(
        "empty-agent",
        out_path.to_str().unwrap(),
        Some(db_path.to_str().unwrap()),
    )?;

    assert_eq!(result.format, "json");
    let exported: HierarchicalExportData = serde_json::from_str(&fs::read_to_string(&out_path)?)?;
    assert_eq!(exported.semantic_nodes.len(), 0);
    assert_eq!(exported.episodic_nodes.len(), 0);
    Ok(())
}

// ---------------------------------------------------------------------------
// import_json_round_trip – both backends
// ---------------------------------------------------------------------------

/// Import data into SQLite, then export it back and verify round-trip fidelity.
#[test]
fn import_json_round_trip_sqlite() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let _guard = EnvGuard::setup(dir.path(), "sqlite");
    let db_path = dir.path().join("rt.sqlite");
    let input_path = dir.path().join("input.json");
    let output_path = dir.path().join("output.json");

    let data = HierarchicalExportData {
        agent_name: "source-agent".to_string(),
        exported_at: "1704067200".to_string(),
        format_version: "1.1".to_string(),
        semantic_nodes: vec![crate::commands::memory::transfer::SemanticNode {
            memory_id: "sem-rt-1".to_string(),
            concept: "round-trip".to_string(),
            content: "SQLite round-trip content".to_string(),
            confidence: 0.9,
            source_id: "".to_string(),
            tags: vec![],
            metadata: serde_json::json!({}),
            created_at: "1704067200".to_string(),
            entity_name: "test".to_string(),
        }],
        episodic_nodes: vec![],
        similar_to_edges: vec![],
        derives_from_edges: vec![],
        supersedes_edges: vec![],
        transitioned_to_edges: vec![],
        statistics: HierarchicalStats {
            semantic_node_count: 1,
            ..Default::default()
        },
    };
    write_export_file(&input_path, &data);

    let backend = open_hierarchical_transfer_backend_for(BackendChoice::Sqlite);
    let import = backend.import_hierarchical_json(
        "target-agent",
        input_path.to_str().unwrap(),
        false,
        Some(db_path.to_str().unwrap()),
    )?;
    assert_stat(&import, "semantic_nodes_imported", "1");

    let export = backend.export_hierarchical_json(
        "target-agent",
        output_path.to_str().unwrap(),
        Some(db_path.to_str().unwrap()),
    )?;
    assert_eq!(export.format, "json");

    let exported: HierarchicalExportData =
        serde_json::from_str(&fs::read_to_string(&output_path)?)?;
    assert_eq!(
        exported.semantic_nodes.len(),
        1,
        "round-trip must preserve 1 semantic node"
    );
    assert_eq!(exported.semantic_nodes[0].memory_id, "sem-rt-1");
    Ok(())
}

/// Import + export round-trip via graph-db.
#[test]
fn import_json_round_trip_graph_db() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let _guard = EnvGuard::setup(dir.path(), "graph-db");
    let db_path = dir.path().join("rt.graph_db");
    let input_path = dir.path().join("input.json");
    let output_path = dir.path().join("output.json");

    let data = HierarchicalExportData {
        agent_name: "source-agent".to_string(),
        exported_at: "1704067200".to_string(),
        format_version: "1.1".to_string(),
        semantic_nodes: vec![crate::commands::memory::transfer::SemanticNode {
            memory_id: "sem-rt-k1".to_string(),
            concept: "round-trip-graph-db".to_string(),
            content: "Graph DB round-trip content".to_string(),
            confidence: 0.85,
            source_id: "".to_string(),
            tags: vec![],
            metadata: serde_json::json!({}),
            created_at: "1704067200".to_string(),
            entity_name: "test".to_string(),
        }],
        episodic_nodes: vec![],
        similar_to_edges: vec![],
        derives_from_edges: vec![],
        supersedes_edges: vec![],
        transitioned_to_edges: vec![],
        statistics: HierarchicalStats {
            semantic_node_count: 1,
            ..Default::default()
        },
    };
    write_export_file(&input_path, &data);

    let backend = open_hierarchical_transfer_backend_for(BackendChoice::GraphDb);
    let import = backend.import_hierarchical_json(
        "target-agent",
        input_path.to_str().unwrap(),
        false,
        Some(db_path.to_str().unwrap()),
    )?;
    assert_stat(&import, "semantic_nodes_imported", "1");

    let export = backend.export_hierarchical_json(
        "target-agent",
        output_path.to_str().unwrap(),
        Some(db_path.to_str().unwrap()),
    )?;
    assert_eq!(export.format, "json");

    let exported: HierarchicalExportData =
        serde_json::from_str(&fs::read_to_string(&output_path)?)?;
    assert_eq!(exported.semantic_nodes.len(), 1);
    assert_eq!(exported.semantic_nodes[0].memory_id, "sem-rt-k1");
    Ok(())
}

// ---------------------------------------------------------------------------
// import_merge_skips_existing – both backends
// ---------------------------------------------------------------------------

fn assert_merge_skips_existing(choice: BackendChoice) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let backend_str = match choice {
        BackendChoice::Sqlite => "sqlite",
        BackendChoice::GraphDb => "graph-db",
    };
    let _guard = EnvGuard::setup(dir.path(), backend_str);
    let db_path = dir.path().join("merge.db");
    let input_path = dir.path().join("input.json");

    let data = HierarchicalExportData {
        agent_name: "source".to_string(),
        exported_at: "1704067200".to_string(),
        format_version: "1.1".to_string(),
        semantic_nodes: vec![crate::commands::memory::transfer::SemanticNode {
            memory_id: "sem-merge-1".to_string(),
            concept: "merge-test".to_string(),
            content: "merge content".to_string(),
            confidence: 0.7,
            source_id: "".to_string(),
            tags: vec![],
            metadata: serde_json::json!({}),
            created_at: "1704067200".to_string(),
            entity_name: "".to_string(),
        }],
        episodic_nodes: vec![],
        similar_to_edges: vec![],
        derives_from_edges: vec![],
        supersedes_edges: vec![],
        transitioned_to_edges: vec![],
        statistics: HierarchicalStats::default(),
    };
    write_export_file(&input_path, &data);

    let backend = open_hierarchical_transfer_backend_for(choice);
    let db_str = db_path.to_str().unwrap();
    let input_str = input_path.to_str().unwrap();

    // First import: no merge.
    backend.import_hierarchical_json("agent", input_str, false, Some(db_str))?;

    // Second import: merge=true. The existing ID must be skipped.
    let result = backend.import_hierarchical_json("agent", input_str, true, Some(db_str))?;
    assert_stat(&result, "skipped", "1");
    Ok(())
}

#[test]
fn import_merge_skips_existing_sqlite() -> anyhow::Result<()> {
    assert_merge_skips_existing(BackendChoice::Sqlite)
}

#[test]
fn import_merge_skips_existing_graph_db() -> anyhow::Result<()> {
    assert_merge_skips_existing(BackendChoice::GraphDb)
}

// ---------------------------------------------------------------------------
// import_no_merge_clears – both backends
// ---------------------------------------------------------------------------

fn assert_no_merge_clears(choice: BackendChoice) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let backend_str = match choice {
        BackendChoice::Sqlite => "sqlite",
        BackendChoice::GraphDb => "graph-db",
    };
    let _guard = EnvGuard::setup(dir.path(), backend_str);
    let db_path = dir.path().join("replace.db");
    let first_input = dir.path().join("first.json");
    let second_input = dir.path().join("second.json");
    let output_path = dir.path().join("out.json");

    let first = HierarchicalExportData {
        agent_name: "src".to_string(),
        exported_at: "1704067200".to_string(),
        format_version: "1.1".to_string(),
        semantic_nodes: vec![crate::commands::memory::transfer::SemanticNode {
            memory_id: "sem-first".to_string(),
            concept: "first".to_string(),
            content: "first content".to_string(),
            confidence: 0.9,
            source_id: "".to_string(),
            tags: vec![],
            metadata: serde_json::json!({}),
            created_at: "1704067200".to_string(),
            entity_name: "".to_string(),
        }],
        episodic_nodes: vec![],
        similar_to_edges: vec![],
        derives_from_edges: vec![],
        supersedes_edges: vec![],
        transitioned_to_edges: vec![],
        statistics: HierarchicalStats::default(),
    };
    let second = minimal_export_data("src"); // no nodes

    write_export_file(&first_input, &first);
    write_export_file(&second_input, &second);

    let backend = open_hierarchical_transfer_backend_for(choice);
    let db_str = db_path.to_str().unwrap();

    backend.import_hierarchical_json(
        "agent",
        first_input.to_str().unwrap(),
        false,
        Some(db_str),
    )?;
    // Second import with merge=false must clear the existing data.
    backend.import_hierarchical_json(
        "agent",
        second_input.to_str().unwrap(),
        false,
        Some(db_str),
    )?;

    let export =
        backend.export_hierarchical_json("agent", output_path.to_str().unwrap(), Some(db_str))?;
    assert_eq!(export.format, "json");

    let exported: HierarchicalExportData =
        serde_json::from_str(&fs::read_to_string(&output_path)?)?;
    assert_eq!(
        exported.semantic_nodes.len(),
        0,
        "no-merge import must clear previous data; expected 0 nodes"
    );
    Ok(())
}

#[test]
fn import_no_merge_clears_sqlite() -> anyhow::Result<()> {
    assert_no_merge_clears(BackendChoice::Sqlite)
}

#[test]
fn import_no_merge_clears_graph_db() -> anyhow::Result<()> {
    assert_no_merge_clears(BackendChoice::GraphDb)
}

// ---------------------------------------------------------------------------
// derives_from_edge_preserved – both backends
// ---------------------------------------------------------------------------

fn assert_derives_from_edge_preserved(choice: BackendChoice) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let backend_str = match choice {
        BackendChoice::Sqlite => "sqlite",
        BackendChoice::GraphDb => "graph-db",
    };
    let _guard = EnvGuard::setup(dir.path(), backend_str);
    let db_path = dir.path().join("derives.db");
    let input_path = dir.path().join("input.json");
    let output_path = dir.path().join("output.json");

    let data = HierarchicalExportData {
        agent_name: "src".to_string(),
        exported_at: "1704067200".to_string(),
        format_version: "1.1".to_string(),
        semantic_nodes: vec![crate::commands::memory::transfer::SemanticNode {
            memory_id: "sem-d1".to_string(),
            concept: "derived".to_string(),
            content: "derived content".to_string(),
            confidence: 0.8,
            source_id: "ep-d1".to_string(),
            tags: vec![],
            metadata: serde_json::json!({}),
            created_at: "1704067200".to_string(),
            entity_name: "".to_string(),
        }],
        episodic_nodes: vec![crate::commands::memory::transfer::EpisodicNode {
            memory_id: "ep-d1".to_string(),
            content: "episodic source".to_string(),
            source_label: "session".to_string(),
            tags: vec![],
            metadata: serde_json::json!({}),
            created_at: "1704067200".to_string(),
        }],
        similar_to_edges: vec![],
        derives_from_edges: vec![crate::commands::memory::transfer::DerivesEdge {
            source_id: "sem-d1".to_string(),
            target_id: "ep-d1".to_string(),
            extraction_method: "test".to_string(),
            confidence: 0.75,
        }],
        supersedes_edges: vec![],
        transitioned_to_edges: vec![],
        statistics: HierarchicalStats::default(),
    };
    write_export_file(&input_path, &data);

    let backend = open_hierarchical_transfer_backend_for(choice);
    let db_str = db_path.to_str().unwrap();
    let import = backend.import_hierarchical_json(
        "agent",
        input_path.to_str().unwrap(),
        false,
        Some(db_str),
    )?;
    assert_stat(&import, "edges_imported", "1");

    let export =
        backend.export_hierarchical_json("agent", output_path.to_str().unwrap(), Some(db_str))?;
    assert_eq!(export.format, "json");

    let exported: HierarchicalExportData =
        serde_json::from_str(&fs::read_to_string(&output_path)?)?;
    assert_eq!(
        exported.derives_from_edges.len(),
        1,
        "DERIVES_FROM edge must be preserved after round-trip"
    );
    assert_eq!(exported.derives_from_edges[0].source_id, "sem-d1");
    assert_eq!(exported.derives_from_edges[0].target_id, "ep-d1");
    Ok(())
}

#[test]
fn derives_from_edge_preserved_sqlite() -> anyhow::Result<()> {
    assert_derives_from_edge_preserved(BackendChoice::Sqlite)
}

#[test]
fn derives_from_edge_preserved_graph_db() -> anyhow::Result<()> {
    assert_derives_from_edge_preserved(BackendChoice::GraphDb)
}

// ---------------------------------------------------------------------------
// raw_db_replace – both backends
// ---------------------------------------------------------------------------

fn assert_raw_db_replace(choice: BackendChoice) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let backend_str = match choice {
        BackendChoice::Sqlite => "sqlite",
        BackendChoice::GraphDb => "graph-db",
    };
    let _guard = EnvGuard::setup(dir.path(), backend_str);
    let src_db = dir.path().join("source.db");
    let dst_db = dir.path().join("target.db");
    let raw_export_path = dir.path().join("raw_export");

    // Create a minimal source database.
    let backend = open_hierarchical_transfer_backend_for(choice);

    // Initialize source by exporting (creates an empty DB if needed).
    let json_init = dir.path().join("init.json");
    write_export_file(&json_init, &minimal_export_data("src"));
    backend.import_hierarchical_json(
        "agent",
        json_init.to_str().unwrap(),
        false,
        Some(src_db.to_str().unwrap()),
    )?;

    // Export raw-db from source.
    let raw_result = backend.export_hierarchical_raw_db(
        "agent",
        raw_export_path.to_str().unwrap(),
        Some(src_db.to_str().unwrap()),
    )?;
    assert_eq!(raw_result.format, "raw-db");

    // Import raw-db into target (replace, merge=false).
    let import = backend.import_hierarchical_raw_db(
        "agent",
        raw_export_path.to_str().unwrap(),
        false,
        Some(dst_db.to_str().unwrap()),
    )?;
    assert!(!import.merge, "raw-db import must always be merge=false");
    Ok(())
}

#[test]
fn raw_db_replace_sqlite() -> anyhow::Result<()> {
    assert_raw_db_replace(BackendChoice::Sqlite)
}

#[test]
fn raw_db_replace_graph_db() -> anyhow::Result<()> {
    assert_raw_db_replace(BackendChoice::GraphDb)
}

// ---------------------------------------------------------------------------
// raw_db_merge_rejected – both backends
// ---------------------------------------------------------------------------

fn assert_raw_db_merge_rejected(choice: BackendChoice) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let backend_str = match choice {
        BackendChoice::Sqlite => "sqlite",
        BackendChoice::GraphDb => "graph-db",
    };
    let _guard = EnvGuard::setup(dir.path(), backend_str);
    let db_path = dir.path().join("any.db");
    let raw_input = dir.path().join("raw_input");
    // Create a placeholder raw input so path existence is not the reason for failure.
    fs::create_dir_all(&raw_input).expect("create raw_input dir");

    let backend = open_hierarchical_transfer_backend_for(choice);
    let result = backend.import_hierarchical_raw_db(
        "agent",
        raw_input.to_str().unwrap(),
        true, // merge=true: must be rejected
        Some(db_path.to_str().unwrap()),
    );
    assert!(
        result.is_err(),
        "import_hierarchical_raw_db with merge=true must return Err for {:?} backend",
        choice
    );
    Ok(())
}

#[test]
fn raw_db_merge_rejected_sqlite() -> anyhow::Result<()> {
    assert_raw_db_merge_rejected(BackendChoice::Sqlite)
}

#[test]
fn raw_db_merge_rejected_graph_db() -> anyhow::Result<()> {
    assert_raw_db_merge_rejected(BackendChoice::GraphDb)
}
