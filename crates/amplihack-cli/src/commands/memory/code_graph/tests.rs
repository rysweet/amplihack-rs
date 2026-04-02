use super::import::{import_blarify_json, import_scip_file, summarize_code_graph};
use super::paths::{
    default_code_graph_db_path, default_code_graph_db_path_for_project,
    resolve_code_graph_db_path_for_project, resolve_project_code_graph_paths,
};
use super::scip::{
    SCIP_KIND_CLASS, SCIP_KIND_FUNCTION, SCIP_SYMBOL_ROLE_DEFINITION, ScipDocument, ScipIndex,
    ScipOccurrence, ScipSymbolInformation,
};
use super::types::{CodeGraphImportCounts, CodeGraphSummary};
use super::validation::{enforce_db_permissions, validate_blarify_json_size, validate_index_path};

use crate::commands::memory::backend::graph_db::{
    GraphDbValue, graph_i64, graph_rows, init_graph_backend_schema,
};
use crate::commands::memory::code_graph::backend::{
    initialize_test_code_graph_db, with_test_code_graph_conn,
};
use crate::test_support::{cwd_env_lock, home_env_lock, restore_cwd, set_cwd};

use anyhow::Result;
use prost::Message;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use time::OffsetDateTime;

use super::types::BlarifyOutput;

fn sample_blarify_output() -> BlarifyOutput {
    serde_json::from_value(serde_json::json!({
        "files": [
            {
                "path": "src/example/module.py",
                "language": "python",
                "lines_of_code": 100,
                "last_modified": "2025-01-01T00:00:00Z"
            },
            {
                "path": "src/example/utils.py",
                "language": "python",
                "lines_of_code": 50,
                "last_modified": "2025-01-01T00:00:00Z"
            }
        ],
        "classes": [{
            "id": "class:Example",
            "name": "Example",
            "file_path": "src/example/module.py",
            "line_number": 10,
            "docstring": "Example class for testing.",
            "is_abstract": false
        }],
        "functions": [
            {
                "id": "func:Example.process",
                "name": "process",
                "file_path": "src/example/module.py",
                "line_number": 20,
                "docstring": "Process data.",
                "parameters": ["self", "data"],
                "return_type": "str",
                "is_async": false,
                "complexity": 3,
                "class_id": "class:Example"
            },
            {
                "id": "func:helper",
                "name": "helper",
                "file_path": "src/example/utils.py",
                "line_number": 5,
                "docstring": "Helper function.",
                "parameters": ["x"],
                "return_type": "int",
                "is_async": false,
                "complexity": 1,
                "class_id": null
            }
        ],
        "imports": [{
            "source_file": "src/example/module.py",
            "target_file": "src/example/utils.py",
            "symbol": "helper",
            "alias": null
        }],
        "relationships": [{
            "type": "CALLS",
            "source_id": "func:Example.process",
            "target_id": "func:helper"
        }]
    }))
    .unwrap()
}

fn temp_code_graph_db() -> Result<(TempDir, PathBuf)> {
    let dir = TempDir::new().map_err(|e| anyhow::anyhow!("tempdir: {e}"))?;
    let db_path = dir.path().join("code-graph.graph_db");
    Ok((dir, db_path))
}

#[test]
fn import_blarify_json_populates_graph_db_code_graph() {
    let (_dir, db_path) = temp_code_graph_db().unwrap();
    let json_dir = TempDir::new().unwrap();
    let json_path = json_dir.path().join("blarify.json");
    fs::write(
        &json_path,
        serde_json::to_string_pretty(&sample_blarify_output()).unwrap(),
    )
    .unwrap();

    let counts = import_blarify_json(&json_path, Some(&db_path)).unwrap();

    assert_eq!(
        counts,
        CodeGraphImportCounts {
            files: 2,
            classes: 1,
            functions: 2,
            imports: 1,
            relationships: 1,
        }
    );

    with_test_code_graph_conn(Some(&db_path), |conn| {
        let rows = graph_rows(conn, "MATCH (cf:CodeFile) RETURN COUNT(cf)", vec![])?;
        assert_eq!(graph_i64(rows[0].first()).unwrap(), 2);
        let rows = graph_rows(
            conn,
            "MATCH (source:CodeFunction {function_id: $source_id})-[r:CALLS]->(target:CodeFunction {function_id: $target_id}) RETURN COUNT(r)",
            vec![
                (
                    "source_id",
                    GraphDbValue::String("func:Example.process".to_string()),
                ),
                ("target_id", GraphDbValue::String("func:helper".to_string())),
            ],
        )?;
        assert_eq!(graph_i64(rows[0].first()).unwrap(), 1);
        Ok(())
    })
    .unwrap();
}

#[test]
fn import_blarify_json_updates_without_duplicates() {
    let (_dir, db_path) = temp_code_graph_db().unwrap();
    let json_dir = TempDir::new().unwrap();
    let json_path = json_dir.path().join("blarify.json");
    fs::write(
        &json_path,
        serde_json::to_string_pretty(&sample_blarify_output()).unwrap(),
    )
    .unwrap();

    let first = import_blarify_json(&json_path, Some(&db_path)).unwrap();
    let second = import_blarify_json(&json_path, Some(&db_path)).unwrap();

    assert_eq!(first.files, 2);
    assert_eq!(second.files, 2);

    with_test_code_graph_conn(Some(&db_path), |conn| {
        let rows = graph_rows(conn, "MATCH (cf:CodeFile) RETURN COUNT(cf)", vec![])?;
        assert_eq!(graph_i64(rows[0].first()).unwrap(), 2);
        Ok(())
    })
    .unwrap();
}

#[test]
fn import_blarify_json_links_semantic_memory_by_metadata_file() {
    let (_dir, db_path) = temp_code_graph_db().unwrap();
    with_test_code_graph_conn(Some(&db_path), |conn| {
        init_graph_backend_schema(conn)?;
        let now = OffsetDateTime::now_utc();

        let mut create_memory = conn.prepare(
            "CREATE (m:SemanticMemory {memory_id: $memory_id, concept: $concept, content: $content, category: $category, confidence_score: $confidence_score, last_updated: $last_updated, version: $version, title: $title, metadata: $metadata, tags: $tags, created_at: $created_at, accessed_at: $accessed_at, agent_id: $agent_id})",
        )?;
        conn.execute(
            &mut create_memory,
            vec![
                ("memory_id", GraphDbValue::String("mem-1".to_string())),
                ("concept", GraphDbValue::String("Example memory".to_string())),
                (
                    "content",
                    GraphDbValue::String("Remember module.py".to_string()),
                ),
                ("category", GraphDbValue::String("session_end".to_string())),
                ("confidence_score", GraphDbValue::Double(1.0)),
                ("last_updated", GraphDbValue::Timestamp(now)),
                ("version", GraphDbValue::Int64(1)),
                ("title", GraphDbValue::String("Example".to_string())),
                (
                    "metadata",
                    GraphDbValue::String(r#"{"file":"src/example/module.py"}"#.to_string()),
                ),
                ("tags", GraphDbValue::String(r#"["learning"]"#.to_string())),
                ("created_at", GraphDbValue::Timestamp(now)),
                ("accessed_at", GraphDbValue::Timestamp(now)),
                ("agent_id", GraphDbValue::String("agent-1".to_string())),
            ],
        )?;
        Ok(())
    })
    .unwrap();

    let json_dir = TempDir::new().unwrap();
    let json_path = json_dir.path().join("blarify.json");
    fs::write(
        &json_path,
        serde_json::to_string_pretty(&sample_blarify_output()).unwrap(),
    )
    .unwrap();

    import_blarify_json(&json_path, Some(&db_path)).unwrap();
    import_blarify_json(&json_path, Some(&db_path)).unwrap();

    with_test_code_graph_conn(Some(&db_path), |conn| {
        let rows = graph_rows(
            conn,
            "MATCH (m:SemanticMemory {memory_id: $memory_id})-[r:RELATES_TO_FILE_SEMANTIC]->(cf:CodeFile {file_id: $file_id}) RETURN COUNT(r)",
            vec![
                ("memory_id", GraphDbValue::String("mem-1".to_string())),
                (
                    "file_id",
                    GraphDbValue::String("src/example/module.py".to_string()),
                ),
            ],
        )?;
        assert_eq!(graph_i64(rows[0].first()).unwrap(), 1);
        Ok(())
    })
    .unwrap();
}

#[test]
fn import_blarify_json_links_semantic_memory_by_function_name() {
    let (_dir, db_path) = temp_code_graph_db().unwrap();
    with_test_code_graph_conn(Some(&db_path), |conn| {
        init_graph_backend_schema(conn)?;
        let now = OffsetDateTime::now_utc();

        let mut create_memory = conn.prepare(
            "CREATE (m:SemanticMemory {memory_id: $memory_id, concept: $concept, content: $content, category: $category, confidence_score: $confidence_score, last_updated: $last_updated, version: $version, title: $title, metadata: $metadata, tags: $tags, created_at: $created_at, accessed_at: $accessed_at, agent_id: $agent_id})",
        )?;
        conn.execute(
            &mut create_memory,
            vec![
                ("memory_id", GraphDbValue::String("mem-func".to_string())),
                ("concept", GraphDbValue::String("Helper memory".to_string())),
                (
                    "content",
                    GraphDbValue::String(
                        "Remember to call helper before returning.".to_string(),
                    ),
                ),
                ("category", GraphDbValue::String("session_end".to_string())),
                ("confidence_score", GraphDbValue::Double(1.0)),
                ("last_updated", GraphDbValue::Timestamp(now)),
                ("version", GraphDbValue::Int64(1)),
                ("title", GraphDbValue::String("Helper".to_string())),
                ("metadata", GraphDbValue::String("{}".to_string())),
                ("tags", GraphDbValue::String(r#"["learning"]"#.to_string())),
                ("created_at", GraphDbValue::Timestamp(now)),
                ("accessed_at", GraphDbValue::Timestamp(now)),
                ("agent_id", GraphDbValue::String("agent-1".to_string())),
            ],
        )?;
        Ok(())
    })
    .unwrap();

    let json_dir = TempDir::new().unwrap();
    let json_path = json_dir.path().join("blarify.json");
    fs::write(
        &json_path,
        serde_json::to_string_pretty(&sample_blarify_output()).unwrap(),
    )
    .unwrap();

    import_blarify_json(&json_path, Some(&db_path)).unwrap();
    import_blarify_json(&json_path, Some(&db_path)).unwrap();

    with_test_code_graph_conn(Some(&db_path), |conn| {
        let rows = graph_rows(
            conn,
            "MATCH (m:SemanticMemory {memory_id: $memory_id})-[r:RELATES_TO_FUNCTION_SEMANTIC]->(f:CodeFunction {function_id: $function_id}) RETURN COUNT(r)",
            vec![
                ("memory_id", GraphDbValue::String("mem-func".to_string())),
                ("function_id", GraphDbValue::String("func:helper".to_string())),
            ],
        )?;
        assert_eq!(graph_i64(rows[0].first()).unwrap(), 1);
        Ok(())
    })
    .unwrap();
}

#[test]
fn default_code_graph_db_path_uses_project_local_store() {
    let _home_guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = TempDir::new().unwrap();
    let prev_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
    let prev_kuzu = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
    unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") };
    unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") };
    let previous = set_cwd(dir.path()).unwrap();

    let path = default_code_graph_db_path().unwrap();

    restore_cwd(&previous).unwrap();
    match prev_graph {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
    }
    match prev_kuzu {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
    }
    assert_eq!(path, dir.path().join(".amplihack").join("graph_db"));
}

#[test]
fn default_code_graph_db_path_prefers_existing_legacy_project_store() {
    let _home_guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = TempDir::new().unwrap();
    let previous = set_cwd(dir.path()).unwrap();
    let prev_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
    let prev_kuzu = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
    unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") };
    unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") };
    let legacy = dir.path().join(".amplihack").join("kuzu_db");
    fs::create_dir_all(&legacy).unwrap();

    let path = default_code_graph_db_path().unwrap();

    restore_cwd(&previous).unwrap();
    match prev_graph {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
    }
    match prev_kuzu {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
    }
    assert_eq!(path, legacy);
}

#[test]
fn default_code_graph_db_path_prefers_backend_neutral_override() {
    let _home_guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = TempDir::new().unwrap();
    let previous = set_cwd(dir.path()).unwrap();
    let prev_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
    let prev_kuzu = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
    unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", "/tmp/graph-override") };
    unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", "/tmp/legacy-graph-alias-override") };

    let path = default_code_graph_db_path().unwrap();

    restore_cwd(&previous).unwrap();
    match prev_graph {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
    }
    match prev_kuzu {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
    }
    assert_eq!(path, PathBuf::from("/tmp/graph-override"));
}

#[test]
fn summarize_code_graph_reads_imported_counts() {
    let (_dir, db_path) = temp_code_graph_db().unwrap();
    let json_dir = TempDir::new().unwrap();
    let json_path = json_dir.path().join("blarify.json");
    fs::write(
        &json_path,
        serde_json::to_string_pretty(&sample_blarify_output()).unwrap(),
    )
    .unwrap();

    import_blarify_json(&json_path, Some(&db_path)).unwrap();
    let summary = summarize_code_graph(Some(&db_path))
        .unwrap()
        .expect("summary should exist");

    assert_eq!(
        summary,
        CodeGraphSummary {
            files: 2,
            classes: 1,
            functions: 2,
        }
    );
}

fn sample_scip_index() -> ScipIndex {
    ScipIndex {
        documents: vec![ScipDocument {
            language: "python".to_string(),
            relative_path: "src/example/module.py".to_string(),
            text: "class Example:\n    pass\n\ndef helper():\n    return 1\n".to_string(),
            occurrences: vec![
                ScipOccurrence {
                    range: vec![0, 6, 0, 13],
                    symbol: "scip-python python pkg src/example/module.py/Example.".to_string(),
                    symbol_roles: SCIP_SYMBOL_ROLE_DEFINITION,
                },
                ScipOccurrence {
                    range: vec![3, 4, 3, 10],
                    symbol: "scip-python python pkg src/example/module.py/helper().".to_string(),
                    symbol_roles: SCIP_SYMBOL_ROLE_DEFINITION,
                },
            ],
            symbols: vec![
                ScipSymbolInformation {
                    symbol: "scip-python python pkg src/example/module.py/Example.".to_string(),
                    documentation: vec!["Example class".to_string()],
                    kind: SCIP_KIND_CLASS,
                    display_name: "Example".to_string(),
                    enclosing_symbol: String::new(),
                },
                ScipSymbolInformation {
                    symbol: "scip-python python pkg src/example/module.py/helper().".to_string(),
                    documentation: vec!["Helper".to_string()],
                    kind: SCIP_KIND_FUNCTION,
                    display_name: "helper".to_string(),
                    enclosing_symbol: String::new(),
                },
            ],
        }],
    }
}

#[test]
fn import_scip_file_populates_graph_db_code_graph() {
    let (_dir, db_path) = temp_code_graph_db().unwrap();
    let project_dir = TempDir::new().unwrap();
    let src_dir = project_dir.path().join("src/example");
    fs::create_dir_all(&src_dir).unwrap();
    fs::write(src_dir.join("module.py"), "class Example:\n    pass\n").unwrap();

    let scip_dir = TempDir::new().unwrap();
    let scip_path = scip_dir.path().join("index.scip");
    fs::write(&scip_path, sample_scip_index().encode_to_vec()).unwrap();

    let counts = import_scip_file(
        &scip_path,
        project_dir.path(),
        Some("python"),
        Some(&db_path),
    )
    .unwrap();

    assert_eq!(counts.files, 1);
    assert_eq!(counts.classes, 1);
    assert_eq!(counts.functions, 1);

    with_test_code_graph_conn(Some(&db_path), |conn| {
        let rows = graph_rows(conn, "MATCH (cf:CodeFile) RETURN COUNT(cf)", vec![])?;
        assert_eq!(graph_i64(rows[0].first()).unwrap(), 1);
        let rows = graph_rows(conn, "MATCH (f:CodeFunction) RETURN COUNT(f)", vec![])?;
        assert_eq!(graph_i64(rows[0].first()).unwrap(), 1);
        let rows = graph_rows(conn, "MATCH (c:CodeClass) RETURN COUNT(c)", vec![])?;
        assert_eq!(graph_i64(rows[0].first()).unwrap(), 1);
        Ok(())
    })
    .unwrap();
}

// ── Issue #77 security & validation tests ─────────────────────────────
//
// These tests verify the security and validation behaviour implemented for
// Issue #77.  All four groups pass with the current implementation:
//
//   1. `import_blarify_json_absent_returns_error` — PASS:
//      absent blarify.json now returns an explicit error.
//
//   2. `enforce_db_permissions_sets_restrictive_unix_modes` — PASSES:
//      `enforce_db_permissions()` sets 0o700/0o600 on DB paths (Unix).
//
//   3. `validate_index_path_*` — PASS: path canonicalization + blocklist
//      for /proc, /sys, /dev is implemented and working.
//
//   4. `validate_blarify_json_size_*` — PASS: size guard rejects files
//      exceeding BLARIFY_JSON_MAX_BYTES before deserialization.

// ── (1) Missing blarify JSON must fail explicitly ──────────────────────

/// AC7 / R5 hardening: when blarify.json does not exist, direct import must
/// return an error instead of a success-shaped empty result. Missing input
/// is a real failure that callers must surface or handle deliberately.
#[test]
fn import_blarify_json_absent_returns_error() {
    let (_dir, db_path) = temp_code_graph_db().unwrap();
    // Use a path that is guaranteed not to exist.
    let missing = PathBuf::from("/tmp/__amplihack_tdd_absent_blarify_i77__.json");
    let _ = std::fs::remove_file(&missing); // ensure it really is absent

    let result = import_blarify_json(&missing, Some(&db_path));

    assert!(
        result.is_err(),
        "Expected Err when blarify.json is absent, but got Ok: {:?}",
        result.ok()
    );
    let error = result.err().unwrap();
    assert!(
        error.to_string().contains("blarify JSON not found"),
        "missing-file error should be explicit, got: {error}"
    );
}

// ── (2) DB permissions enforcement ────────────────────────────────────

/// P1-PERM: After the graph backend initialises the database the parent directory must
/// be mode 0o700 and the DB path itself 0o600 (or 0o700 if the backend creates a
/// directory rather than a flat file).
///
/// P1-PERM: DB parent directory must be 0o700; DB file/dir must be 0o600/0o700.
#[test]
#[cfg(unix)]
fn enforce_db_permissions_sets_restrictive_unix_modes() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("secured.graph_db");

    // Initialise the DB so the path exists on disk.
    initialize_test_code_graph_db(Some(&db_path)).unwrap();

    // Call the enforcement function under test.
    enforce_db_permissions(&db_path).expect("enforce_db_permissions should succeed");

    // The parent directory must be 0o700.
    let parent_meta = fs::metadata(dir.path()).unwrap();
    let parent_mode = parent_meta.permissions().mode() & 0o777;
    assert_eq!(
        parent_mode, 0o700,
        "parent directory should be mode 0o700, got 0o{parent_mode:o}"
    );

    // The DB itself (file or directory the backend creates) must be 0o600 / 0o700.
    if db_path.exists() {
        let db_meta = fs::metadata(&db_path).unwrap();
        let db_mode = db_meta.permissions().mode() & 0o777;
        assert!(
            db_mode == 0o600 || db_mode == 0o700,
            "DB path should be mode 0o600 or 0o700, got 0o{db_mode:o}"
        );
    }
}

// ── (3) Path validation ───────────────────────────────────────────────

/// P2-PATH: `/proc` subtrees must be rejected.
#[test]
fn validate_index_path_blocks_proc_prefix() {
    let result = validate_index_path(Path::new("/proc/1/mem"));
    assert!(
        result.is_err(),
        "Expected Err for /proc path, got Ok({:?})",
        result.ok()
    );
    let msg = format!("{}", result.unwrap_err());
    assert!(
        msg.contains("/proc") || msg.to_lowercase().contains("blocked"),
        "Error message should mention the blocked prefix, got: {msg}"
    );
}

/// P2-PATH: `/sys` subtrees must be rejected.
#[test]
fn validate_index_path_blocks_sys_prefix() {
    let result = validate_index_path(Path::new("/sys/kernel/config"));
    assert!(
        result.is_err(),
        "Expected Err for /sys path, got Ok({:?})",
        result.ok()
    );
    let msg = format!("{}", result.unwrap_err());
    assert!(
        msg.contains("/sys") || msg.to_lowercase().contains("blocked"),
        "Error message should mention the blocked prefix, got: {msg}"
    );
}

/// P2-PATH: `/dev` subtrees must be rejected.
#[test]
fn validate_index_path_blocks_dev_prefix() {
    let result = validate_index_path(Path::new("/dev/null"));
    assert!(
        result.is_err(),
        "Expected Err for /dev path, got Ok({:?})",
        result.ok()
    );
    let msg = format!("{}", result.unwrap_err());
    assert!(
        msg.contains("/dev") || msg.to_lowercase().contains("blocked"),
        "Error message should mention the blocked prefix, got: {msg}"
    );
}

/// P2-PATH: Normal temp paths must be allowed through.
#[test]
fn validate_index_path_allows_normal_temp_path() {
    let dir = TempDir::new().unwrap();
    // Create a real subdirectory so canonicalize() can resolve it.
    let project_dir = dir.path().join("my_project");
    fs::create_dir_all(&project_dir).unwrap();

    let result = validate_index_path(&project_dir);
    assert!(
        result.is_ok(),
        "Expected Ok for a normal temp directory, got Err: {:?}",
        result.err()
    );
    // The returned path must be the canonicalized form.
    let canonical = result.unwrap();
    assert!(
        canonical.is_absolute(),
        "validate_index_path must return an absolute canonical path"
    );
}

/// P2-PATH: Paths that *look* like blocked prefixes but are not (e.g.
/// `/proc_data`) must be allowed.
#[test]
fn validate_index_path_allows_path_with_proc_in_name_not_prefix() {
    let dir = TempDir::new().unwrap();
    // e.g. /tmp/abc/proc_data — should NOT be blocked
    let allowed = dir.path().join("proc_data");
    fs::create_dir_all(&allowed).unwrap();

    let result = validate_index_path(&allowed);
    assert!(
        result.is_ok(),
        "Path containing 'proc' as a *directory name* (not prefix) should be \
         allowed, got Err: {:?}",
        result.err()
    );
}

// ── (4) Blarify JSON size guard ───────────────────────────────────────

/// P2-SIZE: A file that exceeds the configured byte limit must be rejected
/// BEFORE serde_json deserialization to prevent memory exhaustion.
#[test]
fn validate_blarify_json_size_rejects_file_exceeding_limit() {
    let dir = TempDir::new().unwrap();
    let json_path = dir.path().join("blarify.json");
    // Write 100 bytes of valid-ish JSON-like content.
    fs::write(
        &json_path,
        b"{\"files\":[],\"classes\":[],\"functions\":[],\"imports\":[],\"relationships\":[]}",
    )
    .unwrap();

    // With a 0-byte limit, ANY non-empty file must be rejected.
    let result = validate_blarify_json_size(&json_path, 0);
    assert!(
        result.is_err(),
        "Expected Err when file exceeds the 0-byte limit, got Ok"
    );
    let msg = format!("{}", result.unwrap_err());
    assert!(
        msg.to_lowercase().contains("size")
            || msg.to_lowercase().contains("large")
            || msg.to_lowercase().contains("exceed")
            || msg.to_lowercase().contains("limit"),
        "Error message should explain why the file was rejected, got: {msg}"
    );
}

/// P2-SIZE: A file that is WITHIN the configured limit must be accepted.
#[test]
fn validate_blarify_json_size_accepts_file_within_limit() {
    let dir = TempDir::new().unwrap();
    let json_path = dir.path().join("blarify.json");
    let content =
        b"{\"files\":[],\"classes\":[],\"functions\":[],\"imports\":[],\"relationships\":[]}";
    fs::write(&json_path, content).unwrap();

    // 500 MiB limit — content is ~80 bytes, well within bounds.
    let max: u64 = 500 * 1024 * 1024;
    let result = validate_blarify_json_size(&json_path, max);
    assert!(
        result.is_ok(),
        "Expected Ok when file is within the size limit, got Err: {:?}",
        result.err()
    );
}

/// P2-SIZE: A missing file must also be rejected (not silently pass the
/// size guard to then crash in the reader).
#[test]
fn validate_blarify_json_size_rejects_missing_file() {
    let missing = PathBuf::from("/tmp/__amplihack_tdd_absent_size_check_i77__.json");
    let _ = std::fs::remove_file(&missing);

    let result = validate_blarify_json_size(&missing, 500 * 1024 * 1024);
    assert!(
        result.is_err(),
        "Expected Err for a missing file in validate_blarify_json_size, got Ok"
    );
}

// ── (5) resolve_code_graph_db_path_for_project — I77 path resolution ────────
//
// These tests define the full behavior contract for the two public path
// resolution functions introduced in Issue #77:
//
//   default_code_graph_db_path_for_project()  — returns .amplihack/graph_db
//   resolve_code_graph_db_path_for_project()  — 4-level precedence resolver
//
// These tests lock the hardening contract for graph DB path resolution:
// unsafe env overrides are rejected, and the legacy disk shim must remain
// contained within the project root before it can activate.

/// I77-DEFAULT: default_code_graph_db_path_for_project() must return
/// `.amplihack/graph_db` regardless of env vars — it is a pure default query
/// with no env-var override semantics.
#[test]
fn default_code_graph_db_path_for_project_returns_graph_db() {
    let dir = TempDir::new().unwrap();
    let result = default_code_graph_db_path_for_project(dir.path()).unwrap();
    assert_eq!(
        result,
        dir.path().join(".amplihack").join("graph_db"),
        "default_code_graph_db_path_for_project must return .amplihack/graph_db (not kuzu_db)"
    );
}

#[test]
fn resolve_project_code_graph_paths_prefers_valid_legacy_shim_when_neutral_missing() {
    let dir = TempDir::new().unwrap();
    let amplihack_dir = dir.path().join(".amplihack");
    fs::create_dir_all(amplihack_dir.join("kuzu_db")).unwrap();

    let paths = resolve_project_code_graph_paths(dir.path()).unwrap();

    assert_eq!(
        paths.neutral,
        dir.path().join(".amplihack").join("graph_db")
    );
    assert_eq!(paths.legacy, dir.path().join(".amplihack").join("kuzu_db"));
    assert_eq!(paths.resolved, paths.legacy);
}

#[test]
fn resolve_project_code_graph_paths_prefers_neutral_when_both_paths_exist() {
    let dir = TempDir::new().unwrap();
    let amplihack_dir = dir.path().join(".amplihack");
    fs::create_dir_all(amplihack_dir.join("graph_db")).unwrap();
    fs::create_dir_all(amplihack_dir.join("kuzu_db")).unwrap();

    let paths = resolve_project_code_graph_paths(dir.path()).unwrap();

    assert_eq!(
        paths.neutral,
        dir.path().join(".amplihack").join("graph_db")
    );
    assert_eq!(paths.legacy, dir.path().join(".amplihack").join("kuzu_db"));
    assert_eq!(paths.resolved, paths.neutral);
}

/// I77-KUZU-ENV: When only AMPLIHACK_KUZU_DB_PATH is set (the legacy alias)
/// and AMPLIHACK_GRAPH_DB_PATH is absent, resolve_code_graph_db_path_for_project
/// must accept the legacy env var as the active path.
#[test]
fn resolve_code_graph_db_path_for_project_uses_kuzu_env_as_legacy_alias() {
    let _home_guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = TempDir::new().unwrap();
    let prev_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
    let prev_kuzu = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
    unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") };
    unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", "/tmp/legacy-graph-alias") };

    let path = resolve_code_graph_db_path_for_project(dir.path()).unwrap();

    match prev_graph {
        Some(v) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", v) },
        None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
    }
    match prev_kuzu {
        Some(v) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", v) },
        None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
    }

    assert_eq!(
        path,
        PathBuf::from("/tmp/legacy-graph-alias"),
        "AMPLIHACK_KUZU_DB_PATH must be used as a legacy alias when AMPLIHACK_GRAPH_DB_PATH \
         is unset"
    );
}

/// I77-SEC-TRAVERSE: An env var whose value contains a path-traversal component
/// (`..`) must be REJECTED. resolve_code_graph_db_path_for_project() must
/// surface an error instead of silently falling through to the default path.
///
/// Security reference: design spec validate_graph_db_env_path() requirement —
/// "must not contain '..' components".
///
#[test]
fn resolve_code_graph_db_path_for_project_env_var_traversal_rejected() {
    let _home_guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = TempDir::new().unwrap();
    let prev_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
    let prev_kuzu = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
    // Env var value contains a ".." path traversal component.
    unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", "/tmp/../etc/shadow") };
    unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") };

    let error = resolve_code_graph_db_path_for_project(dir.path()).unwrap_err();

    match prev_graph {
        Some(v) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", v) },
        None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
    }
    match prev_kuzu {
        Some(v) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", v) },
        None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
    }

    let rendered = format!("{error:#}");
    assert!(rendered.contains("invalid AMPLIHACK_GRAPH_DB_PATH override"));
    assert!(rendered.contains("graph DB path must not contain parent traversal"));
}

/// I77-SEC-RELATIVE: A non-absolute path in AMPLIHACK_GRAPH_DB_PATH must be
/// rejected. resolve_code_graph_db_path_for_project() must surface an error
/// instead of silently falling through to the default path.
///
/// Security reference: design spec validate_graph_db_env_path() requirement —
/// "must be absolute".
///
#[test]
fn resolve_code_graph_db_path_for_project_env_var_relative_path_rejected() {
    let _home_guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = TempDir::new().unwrap();
    let prev_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
    let prev_kuzu = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
    // Relative (non-absolute) path — should be rejected.
    unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", "relative/path/to/graph_db") };
    unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") };

    let error = resolve_code_graph_db_path_for_project(dir.path()).unwrap_err();

    match prev_graph {
        Some(v) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", v) },
        None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
    }
    match prev_kuzu {
        Some(v) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", v) },
        None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
    }

    let rendered = format!("{error:#}");
    assert!(rendered.contains("invalid AMPLIHACK_GRAPH_DB_PATH override"));
    assert!(rendered.contains("graph DB path must be absolute"));
}

/// I77-SEC-PROC: A `/proc`-prefixed path in AMPLIHACK_GRAPH_DB_PATH must be
/// rejected. resolve_code_graph_db_path_for_project() must surface an error
/// instead of silently falling through to the default path.
///
/// Security reference: design spec validate_graph_db_env_path() requirement —
/// "must not start with /proc, /sys, or /dev".
///
#[test]
fn resolve_code_graph_db_path_for_project_env_var_proc_prefix_rejected() {
    let _home_guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = TempDir::new().unwrap();
    let prev_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
    let prev_kuzu = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
    unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", "/proc/1/mem") };
    unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") };

    let error = resolve_code_graph_db_path_for_project(dir.path()).unwrap_err();

    match prev_graph {
        Some(v) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", v) },
        None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
    }
    match prev_kuzu {
        Some(v) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", v) },
        None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
    }

    let rendered = format!("{error:#}");
    assert!(rendered.contains("invalid AMPLIHACK_GRAPH_DB_PATH override"));
    assert!(rendered.contains("blocked unsafe path prefix"));
}

/// I77-SEC-SYMLINK: The legacy `kuzu_db` disk-shim must NOT activate when the
/// `kuzu_db` path is a symbolic link whose canonical target resolves outside the
/// project root. The shim must surface an error instead of silently falling
/// through to the default path.
///
/// Security reference: design spec — "Symlink attack on disk probe: legacy
/// kuzu_db path canonicalized and verified to start_with(project_root) before
/// the shim activates".
///
#[test]
#[cfg(unix)]
fn resolve_code_graph_db_path_for_project_disk_shim_blocks_escaping_symlink() {
    let _home_guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    let prev_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
    let prev_kuzu = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
    unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") };
    unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") };

    // Create .amplihack/ inside the project root.
    let amplihack_dir = dir.path().join(".amplihack");
    fs::create_dir_all(&amplihack_dir).unwrap();

    // Create a symlink: <project>/.amplihack/kuzu_db → <outside tempdir>
    // The symlink resolves OUTSIDE the project root, simulating a symlink
    // escape / TOCTOU attack.
    let kuzu_symlink = amplihack_dir.join("kuzu_db");
    std::os::unix::fs::symlink(outside.path(), &kuzu_symlink).unwrap();

    let error = resolve_code_graph_db_path_for_project(dir.path()).unwrap_err();

    match prev_graph {
        Some(v) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", v) },
        None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
    }
    match prev_kuzu {
        Some(v) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", v) },
        None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
    }

    let rendered = format!("{error:#}");
    assert!(rendered.contains("legacy graph DB shim escapes project root"));
    assert!(rendered.contains(kuzu_symlink.to_string_lossy().as_ref()));
}
