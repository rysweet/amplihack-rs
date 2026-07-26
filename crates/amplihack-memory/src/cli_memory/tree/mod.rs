//! `memory tree` command implementation.

mod render;

#[cfg(test)]
use render::render_tree;
use render::render_tree_from_backend;

#[cfg(test)]
use super::backend::MemoryTreeBackend;
use super::*;
use anyhow::Result;

pub fn run_tree(
    session_id: Option<&str>,
    memory_type: Option<&str>,
    depth: Option<u32>,
    backend: &str,
) -> Result<()> {
    let resolved = resolve_memory_cli_backend(backend)?;
    if let Some(notice) = resolved.cli_notice.as_deref() {
        println!("⚠️ Compatibility mode: {notice}");
    }
    let backend = super::backend::open_tree_backend(resolved.choice)?;
    let output = render_tree_from_backend(
        backend.as_ref(),
        session_id,
        memory_type,
        depth,
        resolved.graph_notice.as_deref(),
    )?;
    println!("{output}");
    Ok(())
}

#[cfg(test)]
mod backend_tests {
    use super::*;
    use std::cell::Cell;

    struct FakeBackend {
        name: &'static str,
        session_rows: Vec<(SessionSummary, Vec<MemoryRecord>)>,
        agent_counts: Vec<(String, usize)>,
        agent_count_calls: Cell<usize>,
    }

    impl MemoryTreeBackend for FakeBackend {
        fn backend_name(&self) -> &'static str {
            self.name
        }

        fn load_session_rows(
            &self,
            _session_id: Option<&str>,
            _memory_type: Option<&str>,
        ) -> Result<Vec<(SessionSummary, Vec<MemoryRecord>)>> {
            Ok(self.session_rows.clone())
        }

        fn collect_agent_counts(&self) -> Result<Vec<(String, usize)>> {
            self.agent_count_calls
                .set(self.agent_count_calls.get().saturating_add(1));
            Ok(self.agent_counts.clone())
        }
    }

    #[test]
    fn render_tree_from_backend_uses_backend_name_and_rows() {
        let backend = FakeBackend {
            name: "ladybug-preview",
            session_rows: vec![(
                SessionSummary {
                    session_id: "sess-1".to_string(),
                    memory_count: 1,
                },
                vec![MemoryRecord {
                    memory_id: "m-1".to_string(),
                    memory_type: "learning".to_string(),
                    title: "Remember".to_string(),
                    content: "Use a backend seam".to_string(),
                    metadata: serde_json::json!({"confidence": 0.9}),
                    importance: Some(7),
                    accessed_at: None,
                    expires_at: None,
                }],
            )],
            agent_counts: vec![("claude".to_string(), 1)],
            agent_count_calls: Cell::new(0),
        };

        let rendered = render_tree_from_backend(&backend, None, None, Some(3), None).unwrap();

        assert!(rendered.contains("Backend: ladybug-preview"));
        assert!(rendered.contains("sess-1 (1 memories)"));
        assert!(rendered.contains("Learning: Remember"));
        assert_eq!(backend.agent_count_calls.get(), 1);
    }

    #[test]
    fn render_tree_from_backend_skips_agent_counts_for_shallow_depth() {
        let backend = FakeBackend {
            name: "ladybug-preview",
            session_rows: vec![(
                SessionSummary {
                    session_id: "sess-1".to_string(),
                    memory_count: 0,
                },
                Vec::new(),
            )],
            agent_counts: vec![("claude".to_string(), 2)],
            agent_count_calls: Cell::new(0),
        };

        let rendered = render_tree_from_backend(&backend, None, None, Some(2), None).unwrap();

        assert!(!rendered.contains("👥 Agents"));
        assert_eq!(backend.agent_count_calls.get(), 0);
    }

    #[test]
    fn render_tree_from_backend_includes_compatibility_notice() {
        let backend = FakeBackend {
            name: "graph-db",
            session_rows: Vec::new(),
            agent_counts: Vec::new(),
            agent_count_calls: Cell::new(0),
        };

        let rendered = render_tree_from_backend(
            &backend,
            None,
            None,
            Some(3),
            Some("using legacy `AMPLIHACK_KUZU_DB_PATH`; prefer `AMPLIHACK_GRAPH_DB_PATH`."),
        )
        .unwrap();

        assert!(rendered.contains("⚠️ Compatibility mode: using legacy `AMPLIHACK_KUZU_DB_PATH`"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::home_env_lock;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn render_tree_matches_python_shape() {
        let session = SessionSummary {
            session_id: "test_sess".to_string(),
            memory_count: 2,
        };
        let rows = vec![(
            session,
            vec![
                MemoryRecord {
                    memory_id: "m-1".to_string(),
                    memory_type: "conversation".to_string(),
                    title: "Hello".to_string(),
                    content: "world".to_string(),
                    metadata: serde_json::json!({"confidence": 0.9}),
                    importance: Some(8),
                    accessed_at: Some("2026-01-02T03:04:05".to_string()),
                    expires_at: None,
                },
                MemoryRecord {
                    memory_id: "m-2".to_string(),
                    memory_type: "context".to_string(),
                    title: "Ctx".to_string(),
                    content: "details".to_string(),
                    metadata: serde_json::json!({"usage_count": 3}),
                    importance: None,
                    accessed_at: Some("2026-01-02T03:04:05".to_string()),
                    expires_at: None,
                },
            ],
        )];
        let output = render_tree(
            SQLITE_TREE_BACKEND_NAME,
            &rows,
            &[("agent1".to_string(), 2)],
            true,
            None,
            None,
        );
        assert!(output.contains("🧠 Memory Graph (Backend: sqlite)"));
        assert!(output.contains("📝 Conversation: Hello (★★★★★★★★☆☆ 8/10) (confidence: 0.9)"));
        assert!(output.contains("🔧 Context: Ctx (used: 3x)"));
    }

    #[test]
    fn memory_graph_compatibility_notice_surfaces_legacy_env_alias() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
        let previous_kuzu = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
        unsafe {
            std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH");
            std::env::set_var("AMPLIHACK_KUZU_DB_PATH", "/tmp/legacy-memory-alias");
        }

        let notice = memory_graph_compatibility_notice(BackendChoice::GraphDb)
            .expect("legacy alias notice expected");

        match previous_graph {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
        }
        match previous_kuzu {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
        }

        assert!(notice.contains("AMPLIHACK_KUZU_DB_PATH"));
        assert!(notice.contains("AMPLIHACK_GRAPH_DB_PATH"));
    }

    #[test]
    fn memory_graph_compatibility_notice_surfaces_legacy_store() {
        let _home_guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home = tempfile::tempdir().unwrap();
        let previous_home = std::env::var_os("HOME");
        let previous_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
        let previous_kuzu = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
        let legacy_store = home.path().join(".amplihack").join("memory_kuzu.db");
        std::fs::create_dir_all(legacy_store.parent().unwrap()).unwrap();
        std::fs::write(&legacy_store, "legacy-memory").unwrap();
        unsafe {
            std::env::set_var("HOME", home.path());
            std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH");
            std::env::remove_var("AMPLIHACK_KUZU_DB_PATH");
        }

        let notice = memory_graph_compatibility_notice(BackendChoice::GraphDb)
            .expect("legacy store notice expected");

        match previous_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        match previous_graph {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
        }
        match previous_kuzu {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
        }

        assert!(notice.contains("memory_kuzu.db"));
        assert!(notice.contains("memory_graph.db"));
    }

    /// AC: memory tree must return a *non-empty structured error* (not silent
    /// empty output) when the requested backend is unavailable.
    ///
    /// This confirms the no-silent-degradation contract: a caller that cannot
    /// open the SQLite database gets an explicit `Err` with a meaningful
    /// message, not an empty result or a panic.
    #[test]
    fn run_tree_with_unavailable_sqlite_returns_structured_error() {
        let _home_guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Override HOME to a path that cannot contain a valid SQLite file so
        // that `open_sqlite_memory_db()` is guaranteed to fail.
        let tmp = tempfile::tempdir().unwrap();
        let _fake_home = tmp.path().join("no-home");
        // Intentionally do NOT create `_fake_home` — the SQLite open will fail
        // because fs::create_dir_all cannot create it inside a read-only root
        // (or, more practically, the subsequent open will fail on a bad path).
        // We use a path that is a *file* (not a directory) so that
        // `fs::create_dir_all(parent)` or `SqliteConnection::open` returns Err.
        std::fs::write(tmp.path().join("not-a-dir"), b"x").unwrap();
        let fake_home_path = tmp.path().join("not-a-dir"); // file, not dir

        let prev_home = std::env::var_os("HOME");
        // Safety: single-threaded test body; env var restored before return.
        unsafe {
            std::env::set_var("HOME", &fake_home_path);
        }

        let result = run_tree(None, None, None, "sqlite");

        match prev_home {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }

        let err = result.expect_err(
            "run_tree must return Err when SQLite is unavailable (non-silent degradation)",
        );
        let msg = format!("{err:#}");
        assert!(
            !msg.is_empty(),
            "error message from unavailable SQLite backend must be non-empty"
        );
    }
}
