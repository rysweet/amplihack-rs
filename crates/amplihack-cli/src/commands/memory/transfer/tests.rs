use super::*;
use crate::commands::memory::{BackendChoice, TransferFormat, memory_home_paths};
use crate::test_support::{home_env_lock, restore_home, set_home};
use anyhow::Result;
use std::collections::HashSet;
use std::fs;
use tempfile::tempdir;

#[test]
fn hierarchical_json_round_trip_uses_public_transfer_flow() -> Result<()> {
    let temp = tempdir()?;
    let db_path = temp.path().join("roundtrip.graph_db");
    let input_path = temp.path().join("input.json");
    let output_path = temp.path().join("output.json");

    let payload = HierarchicalExportData {
        agent_name: "source-agent".to_string(),
        exported_at: "1704067200".to_string(),
        format_version: "1.1".to_string(),
        semantic_nodes: vec![SemanticNode {
            memory_id: "semantic-1".to_string(),
            concept: "repo".to_string(),
            content: "Remember the repo root".to_string(),
            confidence: 0.9,
            source_id: "episode-1".to_string(),
            tags: vec!["memory".to_string(), "repo".to_string()],
            metadata: serde_json::json!({"file": "README.md"}),
            created_at: "1704067200".to_string(),
            entity_name: "amplihack".to_string(),
        }],
        episodic_nodes: vec![EpisodicNode {
            memory_id: "episode-1".to_string(),
            content: "Opened the repository".to_string(),
            source_label: "session".to_string(),
            tags: vec!["session".to_string()],
            metadata: serde_json::json!({"turn": 1}),
            created_at: "1704067200".to_string(),
        }],
        similar_to_edges: vec![],
        derives_from_edges: vec![DerivesEdge {
            source_id: "semantic-1".to_string(),
            target_id: "episode-1".to_string(),
            extraction_method: "unit-test".to_string(),
            confidence: 0.75,
        }],
        supersedes_edges: vec![],
        transitioned_to_edges: vec![],
        statistics: HierarchicalStats {
            semantic_node_count: 1,
            episodic_node_count: 1,
            similar_to_edge_count: 0,
            derives_from_edge_count: 1,
            supersedes_edge_count: 0,
            transitioned_to_edge_count: 0,
        },
    };
    fs::write(&input_path, serde_json::to_string_pretty(&payload)?)?;

    let db_path_str = db_path.to_string_lossy().into_owned();
    let input_path_str = input_path.to_string_lossy().into_owned();
    let output_path_str = output_path.to_string_lossy().into_owned();

    let import = import_memory(
        "target-agent",
        &input_path_str,
        TransferFormat::Json,
        false,
        Some(&db_path_str),
        BackendChoice::GraphDb,
    )?;
    assert_eq!(import.source_agent.as_deref(), Some("source-agent"));
    assert!(
        import
            .statistics
            .iter()
            .any(|(name, value)| name == "semantic_nodes_imported" && value == "1")
    );

    let export = export_memory(
        "target-agent",
        &output_path_str,
        TransferFormat::Json,
        Some(&db_path_str),
        BackendChoice::GraphDb,
    )?;
    assert_eq!(export.format, "json");

    let exported: HierarchicalExportData = serde_json::from_str(&fs::read_to_string(output_path)?)?;
    assert_eq!(exported.agent_name, "target-agent");
    assert_eq!(exported.semantic_nodes.len(), 1);
    assert_eq!(exported.episodic_nodes.len(), 1);
    assert_eq!(exported.derives_from_edges.len(), 1);
    assert_eq!(exported.semantic_nodes[0].memory_id, "semantic-1");
    assert_eq!(exported.episodic_nodes[0].memory_id, "episode-1");

    Ok(())
}

#[test]
fn resolve_hierarchical_db_path_prefers_neutral_subdir_for_agent_root() -> Result<()> {
    let temp = tempdir()?;
    let agent_root = temp.path().join("agent-root");
    fs::create_dir_all(&agent_root)?;

    let resolved = resolve_hierarchical_db_path(
        "agent-a",
        Some(agent_root.to_str().expect("temp path should be utf-8")),
    )?;

    assert_eq!(resolved, agent_root.join("graph_db"));
    Ok(())
}

#[test]
fn resolve_hierarchical_db_path_prefers_existing_legacy_subdir() -> Result<()> {
    let temp = tempdir()?;
    let agent_root = temp.path().join("agent-root");
    let legacy = agent_root.join("kuzu_db");
    fs::create_dir_all(&legacy)?;

    let resolved = resolve_hierarchical_db_path(
        "agent-b",
        Some(agent_root.to_str().expect("temp path should be utf-8")),
    )?;

    assert_eq!(resolved, legacy);
    Ok(())
}

#[test]
fn resolve_hierarchical_db_path_prefers_existing_legacy_store_path() -> Result<()> {
    let temp = tempdir()?;
    let agent_root = temp.path().join("agent-root");
    let legacy = agent_root.join("kuzu_db");
    fs::create_dir_all(&agent_root)?;
    fs::write(&legacy, b"legacy graph store placeholder")?;

    let resolved = resolve_hierarchical_db_path(
        "agent-b",
        Some(agent_root.to_str().expect("temp path should be utf-8")),
    )?;

    assert_eq!(resolved, legacy);
    Ok(())
}

#[test]
fn resolve_hierarchical_db_path_keeps_direct_legacy_store_path() -> Result<()> {
    let temp = tempdir()?;
    let legacy = temp.path().join("kuzu_db");

    let resolved = resolve_hierarchical_db_path("agent-direct", Some(legacy.to_str().unwrap()))?;

    assert_eq!(resolved, legacy);
    Ok(())
}

// ----- Regression tests for empty-graph_db override bug -----

/// Regression: populated kuzu_db + no graph_db sibling → resolver must return kuzu_db.
/// This documents the expected behaviour for the parity-test happy path where graph_db
/// has never been auto-created.
#[test]
fn resolve_hierarchical_db_path_populated_kuzu_db_no_graph_db_returns_kuzu_db() -> Result<()> {
    let temp = tempdir()?;
    let agent_root = temp.path().join("agent-root");
    let kuzu_db = agent_root.join("kuzu_db");
    fs::create_dir_all(&kuzu_db)?;
    // Simulate a populated kuzu database directory with at least one file.
    fs::write(kuzu_db.join("data.kuzu"), b"placeholder")?;

    let resolved = resolve_hierarchical_db_path(
        "agent-c",
        Some(agent_root.to_str().expect("temp path should be utf-8")),
    )?;

    assert_eq!(resolved, kuzu_db);
    Ok(())
}

/// Regression: populated kuzu_db + EMPTY graph_db sibling → resolver must still return
/// kuzu_db.  This is the exact failure mode: open_graph_db_at_path() auto-creates an empty
/// graph_db directory on a prior mis-resolve, which then causes a second resolve to prefer
/// the empty neutral directory over the populated legacy one.
#[test]
fn resolve_hierarchical_db_path_populated_kuzu_db_empty_graph_db_returns_kuzu_db() -> Result<()> {
    let temp = tempdir()?;
    let agent_root = temp.path().join("agent-root");
    let kuzu_db = agent_root.join("kuzu_db");
    let graph_db = agent_root.join("graph_db");

    // kuzu_db is populated (non-empty).
    fs::create_dir_all(&kuzu_db)?;
    fs::write(kuzu_db.join("data.kuzu"), b"placeholder")?;

    // graph_db exists but is empty — the auto-created directory that triggers the bug.
    fs::create_dir_all(&graph_db)?;
    assert!(graph_db.exists());
    assert!(
        is_dir_empty(&graph_db),
        "graph_db must be empty for this regression test"
    );

    let resolved = resolve_hierarchical_db_path(
        "agent-c",
        Some(agent_root.to_str().expect("temp path should be utf-8")),
    )?;

    assert_eq!(
        resolved, kuzu_db,
        "resolver must prefer populated kuzu_db over auto-created empty graph_db"
    );
    Ok(())
}

/// Full export round-trip: import into kuzu_db under agent-root, then export from
/// agent-root with a pre-existing empty graph_db directory (regression scenario).
/// The export must return non-empty JSON (>0 nodes) from the kuzu_db, not from the
/// empty graph_db that was auto-created before the fix was applied.
#[test]
fn hierarchical_json_export_agent_root_kuzu_db_with_empty_graph_db_sibling() -> Result<()> {
    let temp = tempdir()?;
    let agent_root = temp.path().join("agent-root");
    let legacy_db = agent_root.join("kuzu_db");
    let neutral_db = agent_root.join("graph_db");
    let input_path = temp.path().join("regression-input.json");
    let output_path = temp.path().join("regression-output.json");

    fs::create_dir_all(&agent_root)?;

    let payload = HierarchicalExportData {
        agent_name: "source-agent".to_string(),
        exported_at: "1704067200".to_string(),
        format_version: "1.1".to_string(),
        semantic_nodes: vec![SemanticNode {
            memory_id: "semantic-regression-1".to_string(),
            concept: "security".to_string(),
            content: "Regression: kuzu_db selected over empty graph_db sibling.".to_string(),
            confidence: 0.9,
            source_id: "episode-regression-1".to_string(),
            tags: vec!["regression".to_string()],
            metadata: serde_json::json!({"kind": "regression"}),
            created_at: "1704067200".to_string(),
            entity_name: "amplihack".to_string(),
        }],
        episodic_nodes: vec![EpisodicNode {
            memory_id: "episode-regression-1".to_string(),
            content: "Imported into kuzu_db; graph_db sibling is empty.".to_string(),
            source_label: "session".to_string(),
            tags: vec!["regression".to_string()],
            metadata: serde_json::json!({"turn": 1}),
            created_at: "1704067200".to_string(),
        }],
        similar_to_edges: vec![],
        derives_from_edges: vec![DerivesEdge {
            source_id: "semantic-regression-1".to_string(),
            target_id: "episode-regression-1".to_string(),
            extraction_method: "unit-test".to_string(),
            confidence: 0.9,
        }],
        supersedes_edges: vec![],
        transitioned_to_edges: vec![],
        statistics: HierarchicalStats {
            semantic_node_count: 1,
            episodic_node_count: 1,
            similar_to_edge_count: 0,
            derives_from_edge_count: 1,
            supersedes_edge_count: 0,
            transitioned_to_edge_count: 0,
        },
    };
    fs::write(&input_path, serde_json::to_string_pretty(&payload)?)?;

    let legacy_db_str = legacy_db.to_string_lossy().into_owned();
    let input_path_str = input_path.to_string_lossy().into_owned();
    let agent_root_str = agent_root.to_string_lossy().into_owned();
    let output_path_str = output_path.to_string_lossy().into_owned();

    // Import directly into a kuzu_db path. The underlying Kuzu library materializes
    // the store; the important regression is that later export from the agent root
    // resolves back to this legacy path instead of an empty graph_db sibling.
    import_memory(
        "target-agent",
        &input_path_str,
        TransferFormat::Json,
        false,
        Some(&legacy_db_str),
        BackendChoice::GraphDb,
    )?;

    // Simulate the auto-creation race: create an empty graph_db sibling.
    fs::create_dir_all(&neutral_db)?;
    assert!(
        is_dir_empty(&neutral_db),
        "graph_db must be empty to reproduce the regression"
    );

    // Export from agent-root — must resolve to kuzu_db, not the empty graph_db.
    let export = export_memory(
        "target-agent",
        &output_path_str,
        TransferFormat::Json,
        Some(&agent_root_str),
        BackendChoice::GraphDb,
    )?;
    assert_eq!(export.format, "json");

    let exported: HierarchicalExportData = serde_json::from_str(&fs::read_to_string(output_path)?)?;
    assert_eq!(
        exported.semantic_nodes.len(),
        1,
        "export must return >0 nodes from kuzu_db, not empty result from graph_db"
    );
    assert_eq!(
        exported.episodic_nodes.len(),
        1,
        "export must return >0 episodic nodes from kuzu_db"
    );
    assert_eq!(
        exported.semantic_nodes[0].memory_id,
        "semantic-regression-1"
    );
    Ok(())
}

#[test]
fn hierarchical_json_export_from_agent_root_uses_populated_legacy_store() -> Result<()> {
    let temp = tempdir()?;
    let agent_root = temp.path().join("agent-root");
    let legacy_db = agent_root.join("kuzu_db");
    let neutral_db = agent_root.join("graph_db");
    let input_path = temp.path().join("legacy-input.json");
    let output_path = temp.path().join("legacy-output.json");

    fs::create_dir_all(&agent_root)?;
    let payload = HierarchicalExportData {
        agent_name: "source-agent".to_string(),
        exported_at: "1704067200".to_string(),
        format_version: "1.1".to_string(),
        semantic_nodes: vec![SemanticNode {
            memory_id: "semantic-legacy-1".to_string(),
            concept: "auth".to_string(),
            content: "Legacy kuzu_db should be exported from the agent root.".to_string(),
            confidence: 0.95,
            source_id: "episode-legacy-1".to_string(),
            tags: vec!["security".to_string()],
            metadata: serde_json::json!({"kind": "fact"}),
            created_at: "1704067200".to_string(),
            entity_name: "JWT".to_string(),
        }],
        episodic_nodes: vec![EpisodicNode {
            memory_id: "episode-legacy-1".to_string(),
            content: "Imported into the legacy graph store.".to_string(),
            source_label: "session".to_string(),
            tags: vec!["session".to_string()],
            metadata: serde_json::json!({"turn": 1}),
            created_at: "1704067200".to_string(),
        }],
        similar_to_edges: vec![],
        derives_from_edges: vec![DerivesEdge {
            source_id: "semantic-legacy-1".to_string(),
            target_id: "episode-legacy-1".to_string(),
            extraction_method: "unit-test".to_string(),
            confidence: 0.8,
        }],
        supersedes_edges: vec![],
        transitioned_to_edges: vec![],
        statistics: HierarchicalStats {
            semantic_node_count: 1,
            episodic_node_count: 1,
            similar_to_edge_count: 0,
            derives_from_edge_count: 1,
            supersedes_edge_count: 0,
            transitioned_to_edge_count: 0,
        },
    };
    fs::write(&input_path, serde_json::to_string_pretty(&payload)?)?;

    let legacy_db_str = legacy_db.to_string_lossy().into_owned();
    let input_path_str = input_path.to_string_lossy().into_owned();
    let agent_root_str = agent_root.to_string_lossy().into_owned();
    let output_path_str = output_path.to_string_lossy().into_owned();

    import_memory(
        "target-agent",
        &input_path_str,
        TransferFormat::Json,
        false,
        Some(&legacy_db_str),
        BackendChoice::GraphDb,
    )?;

    assert!(
        legacy_db.exists(),
        "legacy kuzu_db should exist after import"
    );
    assert!(
        !neutral_db.exists(),
        "agent-root regression requires kuzu_db without graph_db"
    );

    let export = export_memory(
        "target-agent",
        &output_path_str,
        TransferFormat::Json,
        Some(&agent_root_str),
        BackendChoice::GraphDb,
    )?;
    assert_eq!(export.format, "json");
    assert!(
        !neutral_db.exists(),
        "export should not create an empty graph_db sibling when kuzu_db is populated"
    );

    let exported: HierarchicalExportData = serde_json::from_str(&fs::read_to_string(output_path)?)?;
    assert_eq!(exported.semantic_nodes.len(), 1);
    assert_eq!(exported.episodic_nodes.len(), 1);
    assert_eq!(exported.derives_from_edges.len(), 1);
    assert_eq!(exported.semantic_nodes[0].memory_id, "semantic-legacy-1");
    Ok(())
}

#[test]
fn resolve_hierarchical_memory_paths_defaults_to_shared_home_root() -> Result<()> {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempdir()?;
    let previous_home = set_home(temp.path());

    let paths = resolve_hierarchical_memory_paths("agent-a", None)?;
    let expected_root = memory_home_paths()?.hierarchical_memory_dir;

    restore_home(previous_home);
    assert_eq!(paths.graph_base, expected_root.join("agent-a"));
    assert_eq!(paths.sqlite_db, expected_root.join("agent-a.db"));
    Ok(())
}

#[test]
fn resolve_hierarchical_memory_paths_uses_storage_override_for_both_backends() -> Result<()> {
    let temp = tempdir()?;
    let override_root = temp.path().join("override");
    fs::create_dir_all(&override_root)?;
    let override_str = override_root.to_string_lossy().into_owned();

    let paths = resolve_hierarchical_memory_paths("agent-a", Some(&override_str))?;

    assert_eq!(paths.graph_base, override_root);
    assert_eq!(paths.sqlite_db, override_root.join("agent-a.db"));
    Ok(())
}

#[test]
fn hierarchical_import_plan_applies_shared_validation_and_merge_policy() {
    let data = HierarchicalExportData {
        agent_name: "source-agent".to_string(),
        exported_at: "1704067200".to_string(),
        format_version: "1.1".to_string(),
        semantic_nodes: vec![
            SemanticNode {
                memory_id: "semantic-keep".to_string(),
                concept: "repo".to_string(),
                content: "Keep this semantic node".to_string(),
                confidence: 0.9,
                source_id: "episode-keep".to_string(),
                tags: vec![],
                metadata: serde_json::json!({}),
                created_at: "1704067200".to_string(),
                entity_name: "amplihack".to_string(),
            },
            SemanticNode {
                memory_id: "semantic-skip".to_string(),
                concept: "repo".to_string(),
                content: "Skip on merge".to_string(),
                confidence: 0.8,
                source_id: "episode-keep".to_string(),
                tags: vec![],
                metadata: serde_json::json!({}),
                created_at: "1704067200".to_string(),
                entity_name: "amplihack".to_string(),
            },
            SemanticNode {
                memory_id: String::new(),
                concept: "repo".to_string(),
                content: "Invalid".to_string(),
                confidence: 0.7,
                source_id: "episode-keep".to_string(),
                tags: vec![],
                metadata: serde_json::json!({}),
                created_at: "1704067200".to_string(),
                entity_name: "amplihack".to_string(),
            },
        ],
        episodic_nodes: vec![
            EpisodicNode {
                memory_id: "episode-keep".to_string(),
                content: "Keep this episodic node".to_string(),
                source_label: "session".to_string(),
                tags: vec![],
                metadata: serde_json::json!({}),
                created_at: "1704067200".to_string(),
            },
            EpisodicNode {
                memory_id: "episode-skip".to_string(),
                content: "Skip on merge".to_string(),
                source_label: "session".to_string(),
                tags: vec![],
                metadata: serde_json::json!({}),
                created_at: "1704067200".to_string(),
            },
            EpisodicNode {
                memory_id: String::new(),
                content: "Invalid".to_string(),
                source_label: "session".to_string(),
                tags: vec![],
                metadata: serde_json::json!({}),
                created_at: "1704067200".to_string(),
            },
        ],
        similar_to_edges: vec![SimilarEdge {
            source_id: "semantic-keep".to_string(),
            target_id: "semantic-skip".to_string(),
            weight: 0.4,
            metadata: serde_json::json!({}),
        }],
        derives_from_edges: vec![DerivesEdge {
            source_id: "semantic-keep".to_string(),
            target_id: "episode-keep".to_string(),
            extraction_method: "unit-test".to_string(),
            confidence: 0.7,
        }],
        supersedes_edges: vec![SupersedesEdge {
            source_id: "semantic-keep".to_string(),
            target_id: "semantic-skip".to_string(),
            reason: "new".to_string(),
            temporal_delta: "1".to_string(),
        }],
        transitioned_to_edges: vec![TransitionEdge {
            source_id: "semantic-keep".to_string(),
            target_id: "semantic-skip".to_string(),
            from_value: "old".to_string(),
            to_value: "new".to_string(),
            turn: 2,
            transition_type: "status".to_string(),
        }],
        statistics: HierarchicalStats {
            semantic_node_count: 3,
            episodic_node_count: 3,
            similar_to_edge_count: 1,
            derives_from_edge_count: 1,
            supersedes_edge_count: 1,
            transitioned_to_edge_count: 1,
        },
    };

    let existing_ids: HashSet<String> = ["semantic-skip", "episode-skip"]
        .into_iter()
        .map(str::to_string)
        .collect();

    let plan =
        build_hierarchical_import_plan(&data, true, |memory_id| existing_ids.contains(memory_id));

    assert_eq!(plan.episodic_nodes.len(), 1);
    assert_eq!(plan.semantic_nodes.len(), 1);
    assert_eq!(plan.episodic_nodes[0].memory_id, "episode-keep");
    assert_eq!(plan.semantic_nodes[0].memory_id, "semantic-keep");
    assert_eq!(plan.similar_to_edges.len(), 1);
    assert_eq!(plan.derives_from_edges.len(), 1);
    assert_eq!(plan.supersedes_edges.len(), 1);
    assert_eq!(plan.transitioned_to_edges.len(), 1);
    assert_eq!(plan.stats.skipped, 2);
    assert_eq!(plan.stats.errors, 2);
    assert_eq!(plan.stats.episodic_nodes_imported, 0);
    assert_eq!(plan.stats.semantic_nodes_imported, 0);
    assert_eq!(plan.stats.edges_imported, 0);
}
