use super::*;
use crate::test_support::home_env_lock;
use rusqlite::{Connection as SqliteConnection, params};
use std::fs;

// -----------------------------------------------------------------------
// SQLite tests (existing)
// -----------------------------------------------------------------------

#[test]
fn sqlite_session_listing_reads_schema() -> Result<()> {
    let conn = SqliteConnection::open_in_memory()?;
    conn.execute_batch(SQLITE_SCHEMA)?;
    conn.execute(
        "INSERT INTO sessions (session_id, created_at, last_accessed, metadata) VALUES (?1, ?2, ?3, '{}')",
        params!["test_sess", "2026-01-02T03:04:05", "2026-01-02T03:04:05"],
    )?;
    conn.execute(
        "INSERT INTO session_agents (session_id, agent_id, first_used, last_used) VALUES (?1, ?2, ?3, ?4)",
        params!["test_sess", "agent1", "2026-01-02T03:04:05", "2026-01-02T03:04:05"],
    )?;
    conn.execute(
        "INSERT INTO memory_entries (id, session_id, agent_id, memory_type, title, content, metadata, created_at, accessed_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, '{}', ?7, ?8)",
        params!["m1", "test_sess", "agent1", "conversation", "Hello", "world", "2026-01-02T03:04:05", "2026-01-02T03:04:05"],
    )?;
    let sessions = list_sqlite_sessions_from_conn(&conn)?;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].memory_count, 1);
    Ok(())
}

#[test]
fn retrieve_prompt_context_memories_reads_sqlite_backend() -> Result<()> {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir()?;
    let prev_home = std::env::var_os("HOME");
    let prev_backend = std::env::var_os("AMPLIHACK_MEMORY_BACKEND");
    unsafe {
        std::env::set_var("HOME", dir.path());
        std::env::set_var("AMPLIHACK_MEMORY_BACKEND", "sqlite");
    }

    let conn = open_sqlite_memory_db()?;
    conn.execute(
        "INSERT INTO sessions (session_id, created_at, last_accessed, metadata) VALUES (?1, ?2, ?3, '{}')",
        params!["prompt-session", "2026-01-02T03:04:05", "2026-01-02T03:04:05"],
    )?;
    conn.execute(
        "INSERT INTO session_agents (session_id, agent_id, first_used, last_used) VALUES (?1, ?2, ?3, ?4)",
        params![
            "prompt-session",
            "agent1",
            "2026-01-02T03:04:05",
            "2026-01-02T03:04:05"
        ],
    )?;
    conn.execute(
        "INSERT INTO memory_entries (id, session_id, agent_id, memory_type, title, content, metadata, importance, created_at, accessed_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            "m1",
            "prompt-session",
            "agent1",
            "learning",
            "Fix CI",
            "To fix CI, rerun cargo fmt and cargo clippy before pushing.",
            r#"{"new_memory_type":"semantic"}"#,
            8,
            "2026-01-02T03:04:05",
            "2099-01-02T03:04:05"
        ],
    )?;
    conn.execute(
        "INSERT INTO memory_entries (id, session_id, agent_id, memory_type, title, content, metadata, importance, created_at, accessed_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            "m2",
            "prompt-session",
            "agent1",
            "context",
            "Temporary note",
            "This is only temporary working memory.",
            r#"{"new_memory_type":"working"}"#,
            10,
            "2026-01-02T03:04:05",
            "2099-01-02T03:04:05"
        ],
    )?;

    let memories = retrieve_prompt_context_memories("prompt-session", "fix ci", 2000)?;

    match prev_home {
        Some(value) => unsafe { std::env::set_var("HOME", value) },
        None => unsafe { std::env::remove_var("HOME") },
    }
    match prev_backend {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_MEMORY_BACKEND", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_MEMORY_BACKEND") },
    }

    assert_eq!(memories.len(), 1);
    assert!(memories[0].content.contains("rerun cargo fmt"));
    assert_eq!(memories[0].code_context, None);
    Ok(())
}

#[test]
fn project_artifact_paths_include_root_and_artifact_index_paths() {
    let project = Path::new("/tmp/example-project");
    let paths = project_artifact_paths(project);

    assert_eq!(paths.artifact_dir, project.join(".amplihack"));
    assert_eq!(
        paths.indexes_dir,
        project.join(".amplihack").join("indexes")
    );
    assert_eq!(
        paths.blarify_json,
        project.join(".amplihack").join("blarify.json")
    );
    assert_eq!(paths.root_index_scip, project.join("index.scip"));
    assert_eq!(
        paths.index_scip,
        project.join(".amplihack").join("index.scip")
    );
    assert_eq!(
        paths.index_scip_backup,
        project.join(".amplihack").join("index.scip.backup")
    );
    assert_eq!(
        paths.indexing_pid,
        project.join(".amplihack").join("indexing.pid")
    );
}

#[test]
fn required_parent_dir_rejects_paths_without_parent_directory() {
    let err = required_parent_dir(Path::new("index.scip")).unwrap_err();

    assert!(
        err.to_string()
            .contains("path index.scip has no parent directory")
    );
}

#[test]
fn ensure_parent_dir_allows_current_directory_relative_paths() -> Result<()> {
    ensure_parent_dir(Path::new("index.scip"))?;
    Ok(())
}

#[test]
fn retrieve_prompt_context_memories_enriches_graph_db_code_context() -> Result<()> {
    let _home_guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir()?;
    let db_path = dir.path().join(".amplihack").join("graph_db");
    let prev_home = std::env::var_os("HOME");
    let prev_backend = std::env::var_os("AMPLIHACK_MEMORY_BACKEND");
    let prev_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
    let prev_kuzu = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
    unsafe {
        std::env::set_var("HOME", dir.path());
        std::env::set_var("AMPLIHACK_MEMORY_BACKEND", "graph-db");
        std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", &db_path);
        std::env::remove_var("AMPLIHACK_KUZU_DB_PATH");
    }

    let record = SessionLearningRecord {
        session_id: "prompt-session".to_string(),
        agent_id: "agent1".to_string(),
        content: "Investigated helper behavior in src/example/module.py.".to_string(),
        title: "Helper behavior".to_string(),
        metadata: serde_json::json!({
            "new_memory_type": "semantic",
            "file": "src/example/module.py"
        }),
        importance: 8,
    };
    let memory_id = store_learning_with_backend(BackendChoice::GraphDb, &record)?
        .expect("memory should be stored");

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
                {"id":"func:helper","name":"helper","file_path":"src/example/utils.py","line_number":1,"signature":"def helper()","docstring":"Helper function"}
            ],
            "imports": [],
            "relationships": [
                {"type":"CALLS","source_id":"func:Example.process","target_id":"func:helper"}
            ]
        })
        .to_string(),
    )?;
    super::code_graph::import_blarify_json(&json_path, Some(&db_path))?;

    let memories = retrieve_prompt_context_memories("prompt-session", "helper", 2000)?;

    match prev_home {
        Some(value) => unsafe { std::env::set_var("HOME", value) },
        None => unsafe { std::env::remove_var("HOME") },
    }
    match prev_backend {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_MEMORY_BACKEND", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_MEMORY_BACKEND") },
    }
    match prev_graph {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
    }
    match prev_kuzu {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
    }

    assert_eq!(memories.len(), 1);
    assert!(memories[0].content.contains("Investigated helper behavior"));
    let code_context = memories[0]
        .code_context
        .as_deref()
        .expect("graph-db prompt memory should include code context");
    assert!(code_context.contains("**Related Files:**"));
    assert!(code_context.contains("src/example/module.py"));
    assert!(code_context.contains("**Related Functions:**"));
    assert!(code_context.contains("helper"));
    assert!(!memory_id.is_empty());
    Ok(())
}

#[test]
fn retrieve_prompt_context_memories_does_not_silently_fallback_to_sqlite() -> Result<()> {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir()?;
    let prev_home = std::env::var_os("HOME");
    let prev_backend = std::env::var_os("AMPLIHACK_MEMORY_BACKEND");
    let prev_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
    let prev_kuzu = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
    let graph_parent_blocker = dir.path().join("graph-parent-blocker");
    fs::write(&graph_parent_blocker, "blocker")?;

    unsafe {
        std::env::set_var("HOME", dir.path());
        std::env::remove_var("AMPLIHACK_MEMORY_BACKEND");
        std::env::set_var(
            "AMPLIHACK_GRAPH_DB_PATH",
            graph_parent_blocker.join("graph_db"),
        );
        std::env::remove_var("AMPLIHACK_KUZU_DB_PATH");
    }

    let conn = open_sqlite_memory_db()?;
    conn.execute(
        "INSERT INTO sessions (session_id, created_at, last_accessed, metadata) VALUES (?1, ?2, ?3, '{}')",
        params!["prompt-session", "2026-01-02T03:04:05", "2026-01-02T03:04:05"],
    )?;
    conn.execute(
        "INSERT INTO session_agents (session_id, agent_id, first_used, last_used) VALUES (?1, ?2, ?3, ?4)",
        params!["prompt-session", "agent1", "2026-01-02T03:04:05", "2026-01-02T03:04:05"],
    )?;
    conn.execute(
        "INSERT INTO memory_entries (id, session_id, agent_id, memory_type, title, content, metadata, importance, created_at, accessed_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            "m1",
            "prompt-session",
            "agent1",
            "learning",
            "Fix CI",
            "SQLite memory should not be used when default graph-db setup fails.",
            r#"{"new_memory_type":"semantic"}"#,
            8,
            "2026-01-02T03:04:05",
            "2099-01-02T03:04:05"
        ],
    )?;

    let result = retrieve_prompt_context_memories("prompt-session", "fix ci", 2000);

    match prev_home {
        Some(value) => unsafe { std::env::set_var("HOME", value) },
        None => unsafe { std::env::remove_var("HOME") },
    }
    match prev_backend {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_MEMORY_BACKEND", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_MEMORY_BACKEND") },
    }
    match prev_graph {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
    }
    match prev_kuzu {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
    }

    let error = result.expect_err("default backend path should not silently fall back to sqlite");
    assert!(
        error.to_string().contains("No such file or directory")
            || error.to_string().contains("File exists")
            || error.to_string().contains("missing")
            || error.to_string().contains("failed"),
        "unexpected error: {error}"
    );
    Ok(())
}

#[test]
fn enrich_prompt_context_memories_with_code_context_surfaces_graph_open_failure() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let blocker = dir.path().join("graph-parent-blocker");
    fs::write(&blocker, "blocker")?;
    let db_path = blocker.join("graph_db");

    let result = enrich_prompt_context_memories_with_code_context_at_path(
        vec![SelectedPromptContextMemory {
            memory_id: "mem-1".to_string(),
            content: "Investigated helper behavior.".to_string(),
            code_context: None,
        }],
        &db_path,
    );

    assert!(
        result.is_err(),
        "expected graph-open failure to surface, got Ok: {:?}",
        result.ok()
    );
    let error = result.err().unwrap().to_string();
    assert!(
        error.contains("prompt memory code-context enrichment unavailable")
            || error.contains("File exists")
            || error.contains("Not a directory")
            || error.contains("os error"),
        "expected explicit graph-open failure, got: {error}"
    );
    Ok(())
}

#[test]
fn enrich_prompt_context_memories_with_code_context_surfaces_context_lookup_failure() -> Result<()>
{
    struct FailingReader;

    impl code_graph::CodeGraphReaderBackend for FailingReader {
        fn stats(&self) -> Result<code_graph::CodeGraphStats> {
            Ok(code_graph::CodeGraphStats::default())
        }

        fn context_payload(&self, _memory_id: &str) -> Result<code_graph::CodeGraphContextPayload> {
            Err(anyhow::anyhow!("synthetic code-context failure"))
        }

        fn files(&self, _pattern: Option<&str>, _limit: u32) -> Result<Vec<String>> {
            Ok(Vec::new())
        }

        fn functions(
            &self,
            _file: Option<&str>,
            _limit: u32,
        ) -> Result<Vec<code_graph::CodeGraphNamedEntry>> {
            Ok(Vec::new())
        }

        fn classes(
            &self,
            _file: Option<&str>,
            _limit: u32,
        ) -> Result<Vec<code_graph::CodeGraphNamedEntry>> {
            Ok(Vec::new())
        }

        fn search(
            &self,
            _name: &str,
            _limit: u32,
        ) -> Result<Vec<code_graph::CodeGraphSearchEntry>> {
            Ok(Vec::new())
        }

        fn callers(&self, _name: &str, _limit: u32) -> Result<Vec<code_graph::CodeGraphEdgeEntry>> {
            Ok(Vec::new())
        }

        fn callees(&self, _name: &str, _limit: u32) -> Result<Vec<code_graph::CodeGraphEdgeEntry>> {
            Ok(Vec::new())
        }
    }

    let result = enrich_prompt_context_memories_with_reader(
        vec![SelectedPromptContextMemory {
            memory_id: "mem-lookup".to_string(),
            content: "Remember helper behavior.".to_string(),
            code_context: None,
        }],
        &FailingReader,
    );

    assert!(
        result.is_err(),
        "expected context lookup failure to surface, got Ok: {:?}",
        result.ok()
    );
    let error = result.err().unwrap();
    let error_message = error.to_string();
    let error_chain = format!("{error:#}");
    assert!(
        error_message.contains("failed to load prompt memory code context for mem-lookup"),
        "expected memory-specific lookup error, got: {error_message}"
    );
    assert!(
        error_chain.contains("synthetic code-context failure"),
        "expected root-cause context lookup error, got: {error_chain}"
    );
    Ok(())
}

#[test]
fn resolve_memory_graph_db_path_prefers_env_override() -> Result<()> {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir()?;
    let override_path = dir.path().join("project-legacy-graph-alias");
    let previous_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
    let previous = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
    unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") };
    unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", &override_path) };

    let resolved = resolve_memory_graph_db_path()?;

    match previous_graph {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
    }
    match previous {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
    }

    assert_eq!(resolved, override_path);
    Ok(())
}

#[test]
fn store_session_learning_does_not_silently_fallback_to_sqlite() -> Result<()> {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir()?;
    let prev_home = std::env::var_os("HOME");
    let prev_backend = std::env::var_os("AMPLIHACK_MEMORY_BACKEND");
    let prev_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
    let prev_kuzu = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
    let graph_parent_blocker = dir.path().join("graph-parent-blocker");
    fs::write(&graph_parent_blocker, "blocker")?;

    unsafe {
        std::env::set_var("HOME", dir.path());
        std::env::remove_var("AMPLIHACK_MEMORY_BACKEND");
        std::env::set_var(
            "AMPLIHACK_GRAPH_DB_PATH",
            graph_parent_blocker.join("graph_db"),
        );
        std::env::remove_var("AMPLIHACK_KUZU_DB_PATH");
    }

    let sqlite_path = dir.path().join(".amplihack").join("memory.db");
    ensure_parent_dir(&sqlite_path)?;
    let conn = open_sqlite_memory_db()?;
    conn.execute_batch(SQLITE_SCHEMA)?;

    let result = store_session_learning(
        "prompt-session",
        "agent1",
        "This learning record is long enough to persist if sqlite fallback were still active.",
        Some("prove no fallback"),
        true,
    );

    let sqlite_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM memory_entries", [], |row| row.get(0))?;

    match prev_home {
        Some(value) => unsafe { std::env::set_var("HOME", value) },
        None => unsafe { std::env::remove_var("HOME") },
    }
    match prev_backend {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_MEMORY_BACKEND", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_MEMORY_BACKEND") },
    }
    match prev_graph {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
    }
    match prev_kuzu {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
    }

    assert_eq!(
        sqlite_count, 0,
        "sqlite fallback should not have stored anything"
    );
    let error =
        result.expect_err("default learning storage should not silently fall back to sqlite");
    assert!(
        error.to_string().contains("No such file or directory")
            || error.to_string().contains("File exists")
            || error.to_string().contains("missing")
            || error.to_string().contains("failed"),
        "unexpected error: {error}"
    );
    Ok(())
}

#[test]
fn resolve_memory_graph_db_path_prefers_backend_neutral_override() -> Result<()> {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir()?;
    let override_path = dir.path().join("project-graph");
    let previous_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
    let previous = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
    unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", &override_path) };
    unsafe {
        std::env::set_var(
            "AMPLIHACK_KUZU_DB_PATH",
            dir.path().join("project-legacy-graph-alias"),
        )
    };

    let resolved = resolve_memory_graph_db_path()?;

    match previous_graph {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
    }
    match previous {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
    }

    assert_eq!(resolved, override_path);
    Ok(())
}

#[test]
fn resolve_memory_graph_db_path_rejects_relative_graph_override() -> Result<()> {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir()?;
    let previous_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
    let previous = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
    let prev_home = std::env::var_os("HOME");
    unsafe { std::env::set_var("HOME", dir.path()) };
    unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", "relative/graph.db") };
    unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") };

    let error = resolve_memory_graph_db_path().unwrap_err();

    match prev_home {
        Some(value) => unsafe { std::env::set_var("HOME", value) },
        None => unsafe { std::env::remove_var("HOME") },
    }
    match previous_graph {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
    }
    match previous {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
    }

    let rendered = format!("{error:#}");
    assert!(rendered.contains("invalid AMPLIHACK_GRAPH_DB_PATH override"));
    assert!(rendered.contains("memory graph DB path must be absolute"));
    Ok(())
}

#[test]
fn resolve_memory_graph_db_path_rejects_proc_prefixed_graph_override() -> Result<()> {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir()?;
    let previous_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
    let previous = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
    let prev_home = std::env::var_os("HOME");
    unsafe { std::env::set_var("HOME", dir.path()) };
    unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", "/proc/1/mem") };
    unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") };

    let error = resolve_memory_graph_db_path().unwrap_err();

    match prev_home {
        Some(value) => unsafe { std::env::set_var("HOME", value) },
        None => unsafe { std::env::remove_var("HOME") },
    }
    match previous_graph {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
    }
    match previous {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
    }

    let rendered = format!("{error:#}");
    assert!(rendered.contains("invalid AMPLIHACK_GRAPH_DB_PATH override"));
    assert!(rendered.contains("blocked prefix /proc"));
    Ok(())
}

#[test]
fn resolve_memory_graph_db_path_invalid_graph_override_does_not_fall_through_to_kuzu_alias()
-> Result<()> {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir()?;
    let kuzu_override = dir.path().join("project-legacy-graph-alias");
    let previous_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
    let previous = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
    let prev_home = std::env::var_os("HOME");
    unsafe { std::env::set_var("HOME", dir.path()) };
    unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", "/tmp/../etc/shadow") };
    unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", &kuzu_override) };

    let error = resolve_memory_graph_db_path().unwrap_err();

    match prev_home {
        Some(value) => unsafe { std::env::set_var("HOME", value) },
        None => unsafe { std::env::remove_var("HOME") },
    }
    match previous_graph {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
    }
    match previous {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
    }

    let rendered = format!("{error:#}");
    assert!(rendered.contains("invalid AMPLIHACK_GRAPH_DB_PATH override"));
    assert!(rendered.contains("/tmp/../etc/shadow"));
    Ok(())
}

#[test]
fn select_prompt_context_memories_respects_token_budget() {
    let memories = vec![
        MemoryRecord {
            memory_id: "m-large".to_string(),
            memory_type: "learning".to_string(),
            title: "Large".to_string(),
            content: "x".repeat(200),
            metadata: serde_json::json!({"new_memory_type": "semantic"}),
            importance: Some(10),
            accessed_at: Some("2099-01-02T03:04:05".to_string()),
            expires_at: None,
        },
        MemoryRecord {
            memory_id: "m-small".to_string(),
            memory_type: "learning".to_string(),
            title: "Small".to_string(),
            content: "fix ci quickly".to_string(),
            metadata: serde_json::json!({"new_memory_type": "semantic"}),
            importance: Some(1),
            accessed_at: Some("2099-01-02T03:04:05".to_string()),
            expires_at: None,
        },
    ];

    let selected = select_prompt_context_memories(memories, "fix ci", 10);

    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].memory_id, "m-small");
    assert_eq!(selected[0].content, "fix ci quickly");
}

#[test]
fn build_learning_record_uses_semantic_metadata() {
    let record = build_learning_record(
        "sess-1",
        "analyzer",
        "Fixed CI by running cargo fmt and clippy locally before push.",
        Some("stabilize CI"),
        true,
    )
    .expect("record should be created");

    assert!(record.content.starts_with("Agent analyzer:"));
    assert_eq!(
        record
            .metadata
            .get("new_memory_type")
            .and_then(JsonValue::as_str),
        Some("semantic")
    );
    assert_eq!(
        record.metadata.get("task").and_then(JsonValue::as_str),
        Some("stabilize CI")
    );
}

// -----------------------------------------------------------------------
// BackendChoice / TransferFormat unit tests
// -----------------------------------------------------------------------

/// BackendChoice::parse must accept "graph-db" and "sqlite", with "kuzu"
/// retained as a compatibility alias.
///
/// These tests are purely logic-level and do not touch the kuzu C++ FFI.
/// They document the expected API contract for callers of the memory backend.
#[test]
fn backend_choice_parse_graph_db_and_kuzu_alias() {
    assert_eq!(
        BackendChoice::parse("graph-db").unwrap(),
        BackendChoice::GraphDb
    );
    assert_eq!(
        BackendChoice::parse("kuzu").unwrap(),
        BackendChoice::GraphDb
    );
}

#[test]
fn backend_choice_parse_sqlite() {
    assert_eq!(
        BackendChoice::parse("sqlite").unwrap(),
        BackendChoice::Sqlite
    );
}

#[test]
fn backend_choice_parse_invalid_returns_error() {
    assert!(
        BackendChoice::parse("postgres").is_err(),
        "Unknown backend names must be rejected"
    );
    assert!(
        BackendChoice::parse("").is_err(),
        "Empty string must be rejected"
    );
    assert!(
        BackendChoice::parse("KUZU").is_err(),
        "Case-sensitive: 'KUZU' is not 'kuzu'"
    );
}

#[test]
fn backend_cli_compatibility_notice_only_for_kuzu_alias() {
    assert_eq!(
        backend_cli_compatibility_notice("kuzu").as_deref(),
        Some("CLI value `kuzu` is a legacy compatibility alias; prefer `graph-db`.")
    );
    assert_eq!(backend_cli_compatibility_notice("graph-db"), None);
    assert_eq!(backend_cli_compatibility_notice("auto"), None);
}

#[test]
fn transfer_format_parse_json() {
    assert_eq!(TransferFormat::parse("json").unwrap(), TransferFormat::Json);
}

#[test]
fn transfer_format_parse_raw_db_and_kuzu_alias() {
    assert_eq!(
        TransferFormat::parse("raw-db").unwrap(),
        TransferFormat::RawDb
    );
    assert_eq!(
        TransferFormat::parse("kuzu").unwrap(),
        TransferFormat::RawDb
    );
}

#[test]
fn transfer_format_cli_compatibility_notice_only_for_kuzu_alias() {
    assert_eq!(
        transfer_format_cli_compatibility_notice("kuzu").as_deref(),
        Some("CLI value `kuzu` is a legacy compatibility alias; prefer `raw-db`.")
    );
    assert_eq!(transfer_format_cli_compatibility_notice("raw-db"), None);
    assert_eq!(transfer_format_cli_compatibility_notice("json"), None);
}

#[test]
fn transfer_format_parse_invalid_returns_error() {
    assert!(
        TransferFormat::parse("csv").is_err(),
        "Unsupported formats must be rejected"
    );
    assert!(
        TransferFormat::parse("").is_err(),
        "Empty string must be rejected"
    );
}

#[test]
fn resolve_memory_backend_preference_invalid_message_uses_neutral_values() {
    let previous = std::env::var("AMPLIHACK_MEMORY_BACKEND").ok();
    unsafe { std::env::set_var("AMPLIHACK_MEMORY_BACKEND", "postgres") };
    let error = resolve_memory_backend_preference().unwrap_err();
    match previous {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_MEMORY_BACKEND", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_MEMORY_BACKEND") },
    }
    let message = error.to_string();
    assert!(message.contains("Valid values: sqlite, graph-db."));
    assert!(message.contains("Legacy compatibility value: kuzu"));
    assert!(!message.contains("sqlite, kuzu, graph-db"));
}

// -----------------------------------------------------------------------
// parse_json_value unit tests
// -----------------------------------------------------------------------

#[test]
fn parse_json_value_empty_string_returns_empty_object() {
    let val = parse_json_value("").unwrap();
    assert!(
        val.is_object(),
        "Empty string must parse to empty JSON object"
    );
    assert!(val.as_object().unwrap().is_empty());
}

#[test]
fn parse_json_value_valid_json_parses_correctly() {
    let val = parse_json_value(r#"{"key": "value"}"#).unwrap();
    assert_eq!(val["key"], "value");
}

#[test]
fn parse_json_value_invalid_json_returns_error() {
    assert!(
        parse_json_value("{not valid json}").is_err(),
        "Invalid JSON must return an error"
    );
}
