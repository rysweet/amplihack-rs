use crate::QueryCodeCommands;
use crate::commands::memory::code_graph::{
    CodeGraphEdgeEntry, CodeGraphNamedEntry, CodeGraphReaderBackend,
    code_graph_compatibility_notice_for_project, open_code_graph_reader,
};
use anyhow::Result;
use std::path::Path;

pub fn run_query_code(
    command: QueryCodeCommands,
    db_path: Option<&Path>,
    json_output: bool,
    limit: u32,
) -> Result<()> {
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

fn run_stats(backend: &dyn CodeGraphReaderBackend, json_output: bool) -> Result<()> {
    let stats = backend.stats()?;

    if json_output {
        println!("{}", serde_json::to_string_pretty(&stats)?);
    } else {
        println!("Code Graph Statistics:");
        println!("  Files:     {}", stats.files);
        println!("  Classes:   {}", stats.classes);
        println!("  Functions: {}", stats.functions);
        println!("  Memory→File links:     {}", stats.memory_file_links);
        println!("  Memory→Function links: {}", stats.memory_function_links);
    }

    Ok(())
}

fn run_context(
    backend: &dyn CodeGraphReaderBackend,
    memory_id: &str,
    json_output: bool,
) -> Result<()> {
    let payload = backend.context_payload(memory_id)?;

    if json_output {
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    println!("Code context for memory '{memory_id}':");

    if payload.files.is_empty() {
        println!("  Files: none");
    } else {
        println!("  Files:");
        for file in payload.files {
            println!(
                "    - {} [{}] ({} bytes)",
                file.path, file.language, file.size_bytes
            );
        }
    }

    if payload.functions.is_empty() {
        println!("  Functions: none");
    } else {
        println!("  Functions:");
        for function in payload.functions {
            println!("    - {} :: {}", function.name, function.signature);
        }
    }

    if payload.classes.is_empty() {
        println!("  Classes: none");
    } else {
        println!("  Classes:");
        for class in payload.classes {
            println!("    - {} ({})", class.name, class.fully_qualified_name);
        }
    }

    Ok(())
}

fn run_files(
    backend: &dyn CodeGraphReaderBackend,
    pattern: Option<&str>,
    json_output: bool,
    limit: u32,
) -> Result<()> {
    print_string_list(backend.files(pattern, limit)?, json_output, limit)
}

fn run_functions(
    backend: &dyn CodeGraphReaderBackend,
    file: Option<&str>,
    json_output: bool,
    limit: u32,
) -> Result<()> {
    print_named_entries(backend.functions(file, limit)?, json_output, limit)
}

fn run_classes(
    backend: &dyn CodeGraphReaderBackend,
    file: Option<&str>,
    json_output: bool,
    limit: u32,
) -> Result<()> {
    print_named_entries(backend.classes(file, limit)?, json_output, limit)
}

fn run_search(
    backend: &dyn CodeGraphReaderBackend,
    name: &str,
    json_output: bool,
    limit: u32,
) -> Result<()> {
    let payload = backend.search(name, limit)?;

    if json_output {
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else if payload.is_empty() {
        println!("No results for '{name}'");
    } else {
        for item in payload {
            println!("  [{}] {}", item.kind, item.name);
        }
    }

    Ok(())
}

fn run_callers(
    backend: &dyn CodeGraphReaderBackend,
    name: &str,
    json_output: bool,
    limit: u32,
) -> Result<()> {
    run_edge_entries(
        backend.callers(name, limit)?,
        json_output,
        &format!("Functions calling '{name}':"),
        &format!("No callers found for '{name}'"),
    )
}

fn run_callees(
    backend: &dyn CodeGraphReaderBackend,
    name: &str,
    json_output: bool,
    limit: u32,
) -> Result<()> {
    run_edge_entries(
        backend.callees(name, limit)?,
        json_output,
        &format!("Functions called by '{name}':"),
        &format!("No callees found for '{name}'"),
    )
}

fn print_named_entries(
    entries: Vec<CodeGraphNamedEntry>,
    json_output: bool,
    limit: u32,
) -> Result<()> {
    if json_output {
        println!("{}", serde_json::to_string_pretty(&entries)?);
    } else {
        for entry in &entries {
            if let Some(file) = &entry.file {
                println!("  {file}::{}", entry.name);
            } else {
                println!("  {}", entry.name);
            }
        }
        print_limit_hint(entries.len(), limit);
    }
    Ok(())
}

fn run_edge_entries(
    entries: Vec<CodeGraphEdgeEntry>,
    json_output: bool,
    heading: &str,
    empty_message: &str,
) -> Result<()> {
    if json_output {
        println!("{}", serde_json::to_string_pretty(&entries)?);
    } else if entries.is_empty() {
        println!("{empty_message}");
    } else {
        println!("{heading}");
        for entry in entries {
            println!("  {} -> {}", entry.caller, entry.callee);
        }
    }

    Ok(())
}

fn print_string_list(values: Vec<String>, json_output: bool, limit: u32) -> Result<()> {
    if json_output {
        println!("{}", serde_json::to_string_pretty(&values)?);
    } else {
        for value in &values {
            println!("{value}");
        }
        print_limit_hint(values.len(), limit);
    }
    Ok(())
}

fn print_limit_hint(actual_len: usize, limit: u32) {
    if actual_len == limit as usize {
        println!("... (showing first {limit}, use --limit to see more)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::memory::{
        backend::graph_db::{KuzuValue, init_graph_backend_schema},
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
                    ("memory_id", KuzuValue::String("mem-query".to_string())),
                    ("concept", KuzuValue::String("Query context".to_string())),
                    (
                        "content",
                        KuzuValue::String("helper is relevant here".to_string()),
                    ),
                    ("category", KuzuValue::String("session_end".to_string())),
                    ("confidence_score", KuzuValue::Double(1.0)),
                    ("last_updated", KuzuValue::Timestamp(now)),
                    ("version", KuzuValue::Int64(1)),
                    ("title", KuzuValue::String("Helper context".to_string())),
                    (
                        "metadata",
                        KuzuValue::String(r#"{"file":"src/example/module.py"}"#.to_string()),
                    ),
                    ("tags", KuzuValue::String(r#"["learning"]"#.to_string())),
                    ("created_at", KuzuValue::Timestamp(now)),
                    ("accessed_at", KuzuValue::Timestamp(now)),
                    ("agent_id", KuzuValue::String("agent-1".to_string())),
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
