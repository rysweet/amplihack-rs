use super::super::super::*;
use super::super::{MemoryRuntimeBackend, MemorySessionBackend, MemoryTreeBackend};
use super::learning::store_learning_graph_with_conn;
use super::queries::{
    collect_graph_db_agent_counts, delete_graph_session_with_conn, list_graph_sessions_from_conn,
    query_graph_memories_for_session,
};
use super::resolve::resolve_memory_graph_db_path;
use super::schema::init_graph_backend_schema;
use super::{GraphDbConnection, GraphDbDatabase, GraphDbSystemConfig};
use anyhow::{Context, Result};
use std::path::Path;

pub(crate) struct GraphDbHandle {
    db: GraphDbDatabase,
}

impl GraphDbHandle {
    pub(crate) fn open_memory_db() -> Result<Self> {
        let path = resolve_memory_graph_db_path()?;
        Self::open_at_path(&path)
    }

    pub(crate) fn open_at_path(path: &Path) -> Result<Self> {
        Ok(Self {
            db: open_graph_db_at_path(path)?,
        })
    }

    pub(crate) fn with_conn<T>(
        &self,
        f: impl FnOnce(&GraphDbConnection<'_>) -> Result<T>,
    ) -> Result<T> {
        let conn = connect_graph_db(&self.db)?;
        f(&conn)
    }

    pub(crate) fn initialize(
        &self,
        init: impl FnOnce(&GraphDbConnection<'_>) -> Result<()>,
    ) -> Result<()> {
        self.with_conn(init)
    }

    pub(crate) fn with_initialized_conn<T>(
        &self,
        init: impl FnOnce(&GraphDbConnection<'_>) -> Result<()>,
        f: impl FnOnce(&GraphDbConnection<'_>) -> Result<T>,
    ) -> Result<T> {
        self.with_conn(|conn| {
            init(conn)?;
            f(conn)
        })
    }
}

pub(crate) struct GraphDbBackend {
    handle: GraphDbHandle,
}

impl GraphDbBackend {
    pub(crate) fn open() -> Result<Self> {
        let handle = GraphDbHandle::open_memory_db()?;
        let backend = Self { handle };
        backend.handle.initialize(init_graph_backend_schema)?;
        Ok(backend)
    }

    fn with_conn<T>(&self, f: impl FnOnce(&GraphDbConnection<'_>) -> Result<T>) -> Result<T> {
        self.handle.with_conn(f)
    }
}

impl MemoryTreeBackend for GraphDbBackend {
    fn backend_name(&self) -> &'static str {
        GRAPH_DB_TREE_BACKEND_NAME
    }

    fn load_session_rows(
        &self,
        session_id: Option<&str>,
        memory_type: Option<&str>,
    ) -> Result<Vec<(SessionSummary, Vec<MemoryRecord>)>> {
        self.with_conn(|conn| {
            let mut sessions = list_graph_sessions_from_conn(conn)?;
            if let Some(session_id) = session_id {
                sessions.retain(|session| session.session_id == session_id);
            }

            let mut session_rows = Vec::new();
            for session in sessions {
                let memories =
                    query_graph_memories_for_session(conn, &session.session_id, memory_type)?;
                let memory_count = memories.len();
                let mut session = session;
                session.memory_count = memory_count;
                session_rows.push((session, memories));
            }
            Ok(session_rows)
        })
    }

    fn collect_agent_counts(&self) -> Result<Vec<(String, usize)>> {
        self.with_conn(collect_graph_db_agent_counts)
    }
}

impl MemorySessionBackend for GraphDbBackend {
    fn list_sessions(&self) -> Result<Vec<SessionSummary>> {
        self.with_conn(list_graph_sessions_from_conn)
    }

    fn delete_session(&self, session_id: &str) -> Result<bool> {
        self.with_conn(|conn| delete_graph_session_with_conn(conn, session_id))
    }
}

impl MemoryRuntimeBackend for GraphDbBackend {
    fn load_prompt_context_memories(&self, session_id: &str) -> Result<Vec<MemoryRecord>> {
        self.with_conn(|conn| query_graph_memories_for_session(conn, session_id, None))
    }

    fn store_session_learning(&self, record: &SessionLearningRecord) -> Result<Option<String>> {
        self.with_conn(|conn| store_learning_graph_with_conn(conn, record))
    }
}

pub(crate) fn open_graph_db_at_path(path: &Path) -> Result<GraphDbDatabase> {
    ensure_parent_dir(path)?;
    Ok(GraphDbDatabase::new(path, GraphDbSystemConfig::default())?)
}

pub(crate) fn connect_graph_db<'a>(db: &'a GraphDbDatabase) -> Result<GraphDbConnection<'a>> {
    GraphDbConnection::new(db).context("failed to connect to graph-backed memory DB")
}
