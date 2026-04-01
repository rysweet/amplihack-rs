mod runners;

use crate::QueryCodeCommands;
use crate::commands::memory::code_graph::{
    code_graph_compatibility_notice_for_project, open_code_graph_reader,
};
use anyhow::Result;
use std::path::Path;

use runners::{
    run_callees, run_callers, run_classes, run_context, run_files, run_functions, run_search,
    run_stats,
};

pub fn run_query_code(
    command: QueryCodeCommands,
    db_path: Option<&Path>,
    legacy_kuzu_path_used: bool,
    json_output: bool,
    limit: u32,
) -> Result<()> {
    if legacy_kuzu_path_used {
        eprintln!(
            "⚠️ Compatibility mode: CLI flag `--kuzu-path` is a legacy compatibility alias; prefer `--db-path`."
        );
    }
    let compatibility_notice = if json_output {
        None
    } else {
        code_graph_compatibility_notice_for_project(&std::env::current_dir()?, db_path)?
    };
    let backend = open_code_graph_reader(db_path)?;
    if let Some(notice) = compatibility_notice {
        println!("⚠️ Compatibility mode: {notice}");
    }

    match command {
        QueryCodeCommands::Stats => run_stats(backend.as_ref(), json_output),
        QueryCodeCommands::Context { memory_id } => {
            run_context(backend.as_ref(), &memory_id, json_output)
        }
        QueryCodeCommands::Files { pattern } => {
            run_files(backend.as_ref(), pattern.as_deref(), json_output, limit)
        }
        QueryCodeCommands::Functions { file } => {
            run_functions(backend.as_ref(), file.as_deref(), json_output, limit)
        }
        QueryCodeCommands::Classes { file } => {
            run_classes(backend.as_ref(), file.as_deref(), json_output, limit)
        }
        QueryCodeCommands::Search { name } => {
            run_search(backend.as_ref(), &name, json_output, limit)
        }
        QueryCodeCommands::Callers { name } => {
            run_callers(backend.as_ref(), &name, json_output, limit)
        }
        QueryCodeCommands::Callees { name } => {
            run_callees(backend.as_ref(), &name, json_output, limit)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::memory::{
        backend::graph_db::{GraphDbValue, init_graph_backend_schema},
        code_graph::{
            CodeGraphContextPayload, CodeGraphImportCounts, CodeGraphSearchEntry, CodeGraphStats,
            backend::with_test_code_graph_conn, import_blarify_json,
        },
    };
    use std::fs;
    use std::sync::{Mutex, OnceLock};
    use time::OffsetDateTime;

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn seed_code_graph(db_path: &Path) {
        let dir = tempfile::tempdir().unwrap();
        let json_path = dir.path().join("blarify.json");
        fs::write(
            &json_path,
            serde_json::json!({
                "files": [
                    {"path":"src/example/module.py","language":"python","lines_of_code":10},
                    {"path":"src/example/utils.py","language":"python","lines_of_code":5}
                ],
                "classes": [
                    {"id":"class:Example","name":"Example","file_path":"src/example/module.py","line_number":1}
                ],
                "functions": [
                    {"id":"func:Example.process","name":"process","file_path":"src/example/module.py","line_number":2,"class_id":"class:Example"},
                    {"id":"func:helper","name":"helper","file_path":"src/example/utils.py","line_number":1}
                ],
                "imports": [],
                "relationships": [
                    {"type":"CALLS","source_id":"func:Example.process","target_id":"func:helper"}
                ]
            })
            .to_string(),
        )
        .unwrap();
        let counts = import_blarify_json(&json_path, Some(db_path)).unwrap();
        assert_eq!(
            counts,
            CodeGraphImportCounts {
                files: 2,
                classes: 1,
                functions: 2,
                imports: 0,
                relationships: 1
            }
        );
    }

    #[test]
    fn query_code_stats_reads_seeded_graph() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("code-graph.graph_db");
        seed_code_graph(&db_path);

        let backend = open_code_graph_reader(Some(&db_path)).unwrap();
        let stats: CodeGraphStats = backend.stats().unwrap();

        assert_eq!((stats.files, stats.classes, stats.functions), (2, 1, 2));
        assert_eq!(
            (stats.memory_file_links, stats.memory_function_links),
            (0, 0)
        );
    }

    #[test]
    fn query_code_search_finds_seeded_symbols() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("code-graph.graph_db");
        seed_code_graph(&db_path);

        let backend = open_code_graph_reader(Some(&db_path)).unwrap();
        let results: Vec<CodeGraphSearchEntry> = backend.search("helper", 10).unwrap();

        assert!(
            results
                .iter()
                .any(|item| item.kind == "function" && item.name == "helper")
        );
    }

    #[test]
    fn query_code_context_returns_linked_code_entities() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("code-graph.graph_db");
        seed_code_graph(&db_path);
        with_test_code_graph_conn(Some(&db_path), |conn| {
            init_graph_backend_schema(conn)?;
            let now = OffsetDateTime::now_utc();

            let mut create_memory = conn.prepare(
                "CREATE (m:SemanticMemory {memory_id: $memory_id, concept: $concept, content: $content, category: $category, confidence_score: $confidence_score, last_updated: $last_updated, version: $version, title: $title, metadata: $metadata, tags: $tags, created_at: $created_at, accessed_at: $accessed_at, agent_id: $agent_id})",
            )?;
            conn.execute(
                &mut create_memory,
                vec![
                    ("memory_id", GraphDbValue::String("mem-query".to_string())),
                    ("concept", GraphDbValue::String("Query context".to_string())),
                    (
                        "content",
                        GraphDbValue::String("helper is relevant here".to_string()),
                    ),
                    ("category", GraphDbValue::String("session_end".to_string())),
                    ("confidence_score", GraphDbValue::Double(1.0)),
                    ("last_updated", GraphDbValue::Timestamp(now)),
                    ("version", GraphDbValue::Int64(1)),
                    ("title", GraphDbValue::String("Helper context".to_string())),
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

        let import_dir = tempfile::tempdir().unwrap();
        let json_path = import_dir.path().join("blarify.json");
        fs::write(
            &json_path,
            serde_json::json!({
                "files": [
                    {"path":"src/example/module.py","language":"python","lines_of_code":10},
                    {"path":"src/example/utils.py","language":"python","lines_of_code":5}
                ],
                "classes": [
                    {"id":"class:Example","name":"Example","file_path":"src/example/module.py","line_number":1}
                ],
                "functions": [
                    {"id":"func:Example.process","name":"process","file_path":"src/example/module.py","line_number":2,"class_id":"class:Example"},
                    {"id":"func:helper","name":"helper","file_path":"src/example/utils.py","line_number":1}
                ],
                "imports": [],
                "relationships": []
            }).to_string(),
        ).unwrap();
        import_blarify_json(&json_path, Some(&db_path)).unwrap();

        let backend = open_code_graph_reader(Some(&db_path)).unwrap();
        let payload: CodeGraphContextPayload = backend.context_payload("mem-query").unwrap();
        assert_eq!(payload.memory_id, "mem-query");
        assert_eq!(payload.files.len(), 1);
        assert_eq!(payload.functions.len(), 1);
        assert_eq!(payload.functions[0].name, "helper");
        assert_eq!(payload.classes.len(), 0);
    }

    #[test]
    fn query_code_context_returns_empty_for_unknown_memory() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("code-graph.graph_db");
        seed_code_graph(&db_path);

        let backend = open_code_graph_reader(Some(&db_path)).unwrap();
        let payload: CodeGraphContextPayload = backend.context_payload("missing-memory").unwrap();
        assert_eq!(payload.memory_id, "missing-memory");
        assert!(payload.files.is_empty());
        assert!(payload.functions.is_empty());
        assert!(payload.classes.is_empty());
    }

    #[test]
    fn code_graph_compatibility_notice_surfaces_legacy_env_alias() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
        let previous_kuzu = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
        unsafe {
            std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH");
            std::env::set_var("AMPLIHACK_KUZU_DB_PATH", "/tmp/legacy-code-graph");
        }

        let notice = code_graph_compatibility_notice_for_project(
            &std::env::current_dir().expect("current_dir must succeed"),
            None,
        )
        .expect("legacy alias notice lookup must work");

        match previous_graph {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
        }
        match previous_kuzu {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
        }

        let notice = notice.expect("legacy alias notice expected");
        assert!(notice.contains("AMPLIHACK_KUZU_DB_PATH"));
        assert!(notice.contains("AMPLIHACK_GRAPH_DB_PATH"));
    }

    #[test]
    fn code_graph_compatibility_notice_surfaces_legacy_store() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let previous_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
        let previous_kuzu = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
        let original_cwd = std::env::current_dir().unwrap();
        let legacy_store = dir.path().join(".amplihack").join("kuzu_db");
        std::fs::create_dir_all(&legacy_store).unwrap();
        std::env::set_current_dir(dir.path()).unwrap();
        unsafe {
            std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH");
            std::env::remove_var("AMPLIHACK_KUZU_DB_PATH");
        }

        let notice = code_graph_compatibility_notice_for_project(dir.path(), None)
            .expect("legacy store notice lookup must work");

        std::env::set_current_dir(original_cwd).unwrap();
        match previous_graph {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
        }
        match previous_kuzu {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
        }

        let notice = notice.expect("legacy store notice expected");
        assert!(notice.contains(".amplihack/kuzu_db"));
        assert!(notice.contains("graph_db"));
    }
}
