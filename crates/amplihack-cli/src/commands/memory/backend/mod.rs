//! Narrow backend seam for `memory tree`.
//!
//! This is intentionally scoped to the current consumer instead of introducing
//! a speculative database-wide abstraction. It gives future backend migrations
//! (for example LadybugDB) a real insertion point without rewriting the rest of
//! the memory subsystem up front.

pub(crate) mod kuzu;
pub(crate) mod sqlite;

#[cfg(test)]
mod memory_backend_parity_test;

use self::kuzu::KuzuBackend;
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
