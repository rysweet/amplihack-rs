//! Narrow backend seam for `memory tree`.
//!
//! This is intentionally scoped to the current consumer instead of introducing
//! a speculative database-wide abstraction. It gives future backend migrations
//! (for example LadybugDB) a real insertion point without rewriting the rest of
//! the memory subsystem up front.

pub(crate) mod graph_db;
pub(crate) mod sqlite;

#[cfg(test)]
mod memory_backend_parity_test;

#[cfg(test)]
mod sqlite_tree_backend_name_test;

use self::graph_db::GraphDbBackend;
use self::sqlite::SqliteBackend;
use super::*;
use anyhow::Result;

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

fn open_backend_with<T, FSqlite, FGraph>(
    choice: BackendChoice,
    sqlite: FSqlite,
    graph_db: FGraph,
) -> Result<T>
where
    FSqlite: FnOnce(SqliteBackend) -> T,
    FGraph: FnOnce(GraphDbBackend) -> T,
{
    match choice {
        BackendChoice::Sqlite => Ok(sqlite(SqliteBackend::open()?)),
        BackendChoice::GraphDb => Ok(graph_db(GraphDbBackend::open()?)),
    }
}

pub(crate) fn open_tree_backend(choice: BackendChoice) -> Result<Box<dyn MemoryTreeBackend>> {
    open_backend_with(
        choice,
        |backend| -> Box<dyn MemoryTreeBackend> { Box::new(backend) },
        |backend| -> Box<dyn MemoryTreeBackend> { Box::new(backend) },
    )
}

pub(crate) fn open_cleanup_backend(choice: BackendChoice) -> Result<Box<dyn MemorySessionBackend>> {
    open_backend_with(
        choice,
        |backend| -> Box<dyn MemorySessionBackend> { Box::new(backend) },
        |backend| -> Box<dyn MemorySessionBackend> { Box::new(backend) },
    )
}

pub(crate) fn open_runtime_backend(choice: BackendChoice) -> Result<Box<dyn MemoryRuntimeBackend>> {
    open_backend_with(
        choice,
        |backend| -> Box<dyn MemoryRuntimeBackend> { Box::new(backend) },
        |backend| -> Box<dyn MemoryRuntimeBackend> { Box::new(backend) },
    )
}
