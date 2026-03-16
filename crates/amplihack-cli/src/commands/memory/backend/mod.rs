//! Narrow backend seam for `memory tree`.
//!
//! This is intentionally scoped to the current consumer instead of introducing
//! a speculative database-wide abstraction. It gives future backend migrations
//! (for example LadybugDB) a real insertion point without rewriting the rest of
//! the memory subsystem up front.

use super::*;
use anyhow::{Context, Result};
use kuzu::Connection as KuzuConnection;

pub(crate) trait MemoryTreeBackend {
    fn backend_name(&self) -> &'static str;
    fn load_session_rows(
        &self,
        session_id: Option<&str>,
        memory_type: Option<&str>,
    ) -> Result<Vec<(SessionSummary, Vec<MemoryRecord>)>>;
    fn collect_agent_counts(&self) -> Result<Vec<(String, usize)>>;
}

pub(crate) trait MemorySessionBackend {
    fn list_sessions(&self) -> Result<Vec<SessionSummary>>;
    fn delete_session(&self, session_id: &str) -> Result<bool>;
}

pub(crate) trait MemoryRuntimeBackend {
    fn load_prompt_context_memories(&self, session_id: &str) -> Result<Vec<MemoryRecord>>;
    fn store_session_learning(&self, record: &SessionLearningRecord) -> Result<Option<String>>;
}

pub(crate) fn open_tree_backend(choice: BackendChoice) -> Result<Box<dyn MemoryTreeBackend>> {
    match choice {
        BackendChoice::Sqlite => Ok(Box::new(SqliteBackend::open()?)),
        BackendChoice::Kuzu => Ok(Box::new(KuzuBackend::open()?)),
    }
}

pub(crate) fn open_cleanup_backend(choice: BackendChoice) -> Result<Box<dyn MemorySessionBackend>> {
    match choice {
        BackendChoice::Sqlite => Ok(Box::new(SqliteBackend::open()?)),
        BackendChoice::Kuzu => Ok(Box::new(KuzuBackend::open()?)),
    }
}

pub(crate) fn open_runtime_backend(choice: BackendChoice) -> Result<Box<dyn MemoryRuntimeBackend>> {
    match choice {
        BackendChoice::Sqlite => Ok(Box::new(SqliteBackend::open()?)),
        BackendChoice::Kuzu => Ok(Box::new(KuzuBackend::open()?)),
    }
}

struct SqliteBackend {
    conn: SqliteConnection,
}

impl SqliteBackend {
    fn open() -> Result<Self> {
        Ok(Self {
            conn: open_sqlite_memory_db()?,
        })
    }
}

impl MemoryTreeBackend for SqliteBackend {
    fn backend_name(&self) -> &'static str {
        SQLITE_TREE_BACKEND_NAME
    }

    fn load_session_rows(
        &self,
        session_id: Option<&str>,
        memory_type: Option<&str>,
    ) -> Result<Vec<(SessionSummary, Vec<MemoryRecord>)>> {
        let mut sessions = list_sqlite_sessions_from_conn(&self.conn)?;
        if let Some(session_id) = session_id {
            sessions.retain(|session| session.session_id == session_id);
        }

        let mut session_rows = Vec::new();
        for session in sessions {
            let memories =
                query_sqlite_memories_for_session(&self.conn, &session.session_id, memory_type)?;
            session_rows.push((session, memories));
        }
        Ok(session_rows)
    }

    fn collect_agent_counts(&self) -> Result<Vec<(String, usize)>> {
        collect_sqlite_agent_counts(&self.conn)
    }
}

impl MemorySessionBackend for SqliteBackend {
    fn list_sessions(&self) -> Result<Vec<SessionSummary>> {
        list_sqlite_sessions_from_conn(&self.conn)
    }

    fn delete_session(&self, session_id: &str) -> Result<bool> {
        delete_sqlite_session(session_id)
    }
}

impl MemoryRuntimeBackend for SqliteBackend {
    fn load_prompt_context_memories(&self, session_id: &str) -> Result<Vec<MemoryRecord>> {
        query_sqlite_memories_for_session(&self.conn, session_id, None)
    }

    fn store_session_learning(&self, record: &SessionLearningRecord) -> Result<Option<String>> {
        store_learning_sqlite(record)
    }
}

struct KuzuBackend {
    db: KuzuDatabase,
}

impl KuzuBackend {
    fn open() -> Result<Self> {
        let db = open_kuzu_memory_db()?;
        let backend = Self { db };
        backend.with_conn(init_kuzu_backend_schema)?;
        Ok(backend)
    }

    fn with_conn<T>(&self, f: impl FnOnce(&KuzuConnection<'_>) -> Result<T>) -> Result<T> {
        let conn = KuzuConnection::new(&self.db).context("failed to connect to Kùzu memory DB")?;
        f(&conn)
    }
}

impl MemoryTreeBackend for KuzuBackend {
    fn backend_name(&self) -> &'static str {
        KUZU_TREE_BACKEND_NAME
    }

    fn load_session_rows(
        &self,
        session_id: Option<&str>,
        _memory_type: Option<&str>,
    ) -> Result<Vec<(SessionSummary, Vec<MemoryRecord>)>> {
        self.with_conn(|conn| {
            let mut sessions = list_kuzu_sessions_from_conn(conn)?;
            if let Some(session_id) = session_id {
                sessions.retain(|session| session.session_id == session_id);
            }

            let mut session_rows = Vec::new();
            for session in sessions {
                let memories = query_kuzu_memories_for_session(conn, &session.session_id)?;
                let memory_count = memories.len();
                let mut session = session;
                session.memory_count = memory_count;
                session_rows.push((session, memories));
            }
            Ok(session_rows)
        })
    }

    fn collect_agent_counts(&self) -> Result<Vec<(String, usize)>> {
        self.with_conn(collect_kuzu_agent_counts)
    }
}

impl MemorySessionBackend for KuzuBackend {
    fn list_sessions(&self) -> Result<Vec<SessionSummary>> {
        self.with_conn(list_kuzu_sessions_from_conn)
    }

    fn delete_session(&self, session_id: &str) -> Result<bool> {
        delete_kuzu_session(session_id)
    }
}

impl MemoryRuntimeBackend for KuzuBackend {
    fn load_prompt_context_memories(&self, session_id: &str) -> Result<Vec<MemoryRecord>> {
        self.with_conn(|conn| query_kuzu_memories_for_session(conn, session_id))
    }

    fn store_session_learning(&self, record: &SessionLearningRecord) -> Result<Option<String>> {
        store_learning_kuzu(record)
    }
}
