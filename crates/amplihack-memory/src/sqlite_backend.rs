//! SQLite implementation of MemoryBackend.
//!
//! Uses rusqlite with bundled SQLite. Creates per-MemoryType tables,
//! WAL mode for concurrency, and FTS5 for full-text search.

use crate::backend::{BackendHealth, MemoryBackend};
use crate::models::{MemoryEntry, MemoryQuery, MemoryType, SessionInfo};
use std::path::PathBuf;

/// SQLite-backed persistent memory store.
///
/// Connection is wrapped in Mutex because rusqlite::Connection is not Sync.
pub struct SqliteBackend {
    path: PathBuf,
    #[cfg(feature = "sqlite")]
    conn: Option<std::sync::Mutex<rusqlite::Connection>>,
}

impl SqliteBackend {
    /// Open or create a SQLite database at the given path.
    pub fn open(path: impl Into<PathBuf>) -> anyhow::Result<Self> {
        let path = path.into();
        #[cfg(feature = "sqlite")]
        {
            let conn = rusqlite::Connection::open(&path)?;
            conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
            let mut backend = Self {
                path,
                conn: Some(std::sync::Mutex::new(conn)),
            };
            backend.ensure_tables()?;
            Ok(backend)
        }
        #[cfg(not(feature = "sqlite"))]
        {
            anyhow::bail!("sqlite feature not enabled")
        }
    }

    /// Open an in-memory SQLite database (for testing).
    pub fn open_in_memory() -> anyhow::Result<Self> {
        #[cfg(feature = "sqlite")]
        {
            let conn = rusqlite::Connection::open_in_memory()?;
            conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
            let mut backend = Self {
                path: PathBuf::from(":memory:"),
                conn: Some(std::sync::Mutex::new(conn)),
            };
            backend.ensure_tables()?;
            Ok(backend)
        }
        #[cfg(not(feature = "sqlite"))]
        {
            anyhow::bail!("sqlite feature not enabled")
        }
    }

    /// Create required tables for all memory types.
    fn ensure_tables(&mut self) -> anyhow::Result<()> {
        #[cfg(feature = "sqlite")]
        {
            let conn = self
                .conn
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("SQLite connection not initialized"))?
                .lock()
                .map_err(|e| anyhow::anyhow!("Mutex poisoned: {e}"))?;
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS memories (
                    id TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    agent_id TEXT NOT NULL,
                    memory_type TEXT NOT NULL,
                    title TEXT NOT NULL DEFAULT '',
                    content TEXT NOT NULL,
                    metadata TEXT NOT NULL DEFAULT '{}',
                    created_at REAL NOT NULL,
                    accessed_at REAL NOT NULL,
                    tags TEXT NOT NULL DEFAULT '[]',
                    importance REAL NOT NULL DEFAULT 0.5
                );
                CREATE INDEX IF NOT EXISTS idx_memories_session ON memories(session_id);
                CREATE INDEX IF NOT EXISTS idx_memories_type ON memories(memory_type);
                CREATE INDEX IF NOT EXISTS idx_memories_created ON memories(created_at);

                CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
                    id UNINDEXED, title, content, tags,
                    content=memories, content_rowid=rowid
                );

                CREATE TRIGGER IF NOT EXISTS memories_ai AFTER INSERT ON memories BEGIN
                    INSERT INTO memories_fts(rowid, id, title, content, tags)
                    VALUES (new.rowid, new.id, new.title, new.content, new.tags);
                END;

                CREATE TRIGGER IF NOT EXISTS memories_ad AFTER DELETE ON memories BEGIN
                    INSERT INTO memories_fts(memories_fts, rowid, id, title, content, tags)
                    VALUES ('delete', old.rowid, old.id, old.title, old.content, old.tags);
                END;",
            )?;
            Ok(())
        }
        #[cfg(not(feature = "sqlite"))]
        {
            Ok(())
        }
    }

    /// Check if WAL mode is active.
    pub fn is_wal_mode(&self) -> anyhow::Result<bool> {
        #[cfg(feature = "sqlite")]
        {
            let conn = self
                .conn
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("SQLite connection not initialized"))?
                .lock()
                .map_err(|e| anyhow::anyhow!("Mutex poisoned: {e}"))?;
            let mode: String = conn.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
            Ok(mode.to_lowercase() == "wal")
        }
        #[cfg(not(feature = "sqlite"))]
        {
            anyhow::bail!("sqlite feature not enabled")
        }
    }

    /// Full-text search across memory content.
    pub fn full_text_search(&self, query: &str, limit: usize) -> anyhow::Result<Vec<MemoryEntry>> {
        #[cfg(feature = "sqlite")]
        {
            let conn = self
                .conn
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("SQLite connection not initialized"))?
                .lock()
                .map_err(|e| anyhow::anyhow!("Mutex poisoned: {e}"))?;
            let mut stmt = conn.prepare(
                "SELECT m.id, m.session_id, m.agent_id, m.memory_type, m.title,
                        m.content, m.metadata, m.created_at, m.accessed_at,
                        m.tags, m.importance
                 FROM memories_fts fts
                 JOIN memories m ON fts.id = m.id
                 WHERE memories_fts MATCH ?1
                 LIMIT ?2",
            )?;
            let entries = stmt
                .query_map(rusqlite::params![query, limit as i64], |row| {
                    Self::row_to_entry(row)
                })?
                .filter_map(|r| r.ok())
                .collect();
            Ok(entries)
        }
        #[cfg(not(feature = "sqlite"))]
        {
            let _ = (query, limit);
            anyhow::bail!("sqlite feature not enabled")
        }
    }

    #[cfg(feature = "sqlite")]
    fn row_to_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryEntry> {
        let metadata_str: String = row.get(6)?;
        let tags_str: String = row.get(9)?;
        let memory_type_str: String = row.get(3)?;
        Ok(MemoryEntry {
            id: row.get(0)?,
            session_id: row.get(1)?,
            agent_id: row.get(2)?,
            memory_type: serde_json::from_str(&format!("\"{}\"", memory_type_str))
                .unwrap_or(MemoryType::Semantic),
            title: row.get(4)?,
            content: row.get(5)?,
            metadata: serde_json::from_str(&metadata_str).unwrap_or_default(),
            created_at: row.get(7)?,
            accessed_at: row.get(8)?,
            tags: serde_json::from_str(&tags_str).unwrap_or_default(),
            importance: row.get(10)?,
        })
    }
}

#[cfg(feature = "sqlite")]
impl MemoryBackend for SqliteBackend {
    fn store(&mut self, entry: &MemoryEntry) -> anyhow::Result<String> {
        let conn = self
            .conn
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("SQLite connection not initialized"))?
            .lock()
            .map_err(|e| anyhow::anyhow!("Mutex poisoned: {e}"))?;
        let metadata = serde_json::to_string(&entry.metadata)?;
        let tags = serde_json::to_string(&entry.tags)?;
        let mem_type = entry.memory_type.as_str();
        conn.execute(
            "INSERT OR REPLACE INTO memories
             (id, session_id, agent_id, memory_type, title, content,
              metadata, created_at, accessed_at, tags, importance)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            rusqlite::params![
                entry.id,
                entry.session_id,
                entry.agent_id,
                mem_type,
                entry.title,
                entry.content,
                metadata,
                entry.created_at,
                entry.accessed_at,
                tags,
                entry.importance,
            ],
        )?;
        Ok(entry.id.clone())
    }

    fn retrieve(&self, query: &MemoryQuery) -> anyhow::Result<Vec<MemoryEntry>> {
        let conn = self
            .conn
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("SQLite connection not initialized"))?
            .lock()
            .map_err(|e| anyhow::anyhow!("Mutex poisoned: {e}"))?;
        let mut sql = String::from(
            "SELECT id, session_id, agent_id, memory_type, title, content,
                    metadata, created_at, accessed_at, tags, importance
             FROM memories WHERE 1=1",
        );
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        let mut idx = 1;

        if let Some(ref sid) = query.session_id {
            sql.push_str(&format!(" AND session_id = ?{idx}"));
            params.push(Box::new(sid.clone()));
            idx += 1;
        }
        if let Some(ref aid) = query.agent_id {
            sql.push_str(&format!(" AND agent_id = ?{idx}"));
            params.push(Box::new(aid.clone()));
            idx += 1;
        }
        if !query.memory_types.is_empty() {
            let placeholders: Vec<String> = query
                .memory_types
                .iter()
                .map(|t| {
                    let p = format!("?{idx}");
                    params.push(Box::new(t.as_str().to_string()));
                    idx += 1;
                    p
                })
                .collect();
            sql.push_str(&format!(
                " AND memory_type IN ({})",
                placeholders.join(", ")
            ));
        }

        let limit = if query.limit > 0 { query.limit } else { 20 };
        let limit = limit.min(10_000);
        sql.push_str(&format!(" ORDER BY created_at DESC LIMIT {limit}"));

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        let mut stmt = conn.prepare(&sql)?;
        let entries = stmt
            .query_map(param_refs.as_slice(), Self::row_to_entry)?
            .filter_map(|r| r.ok())
            .collect();
        Ok(entries)
    }

    fn delete(&mut self, entry_id: &str) -> anyhow::Result<bool> {
        let conn = self
            .conn
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("SQLite connection not initialized"))?
            .lock()
            .map_err(|e| anyhow::anyhow!("Mutex poisoned: {e}"))?;
        let changed = conn.execute(
            "DELETE FROM memories WHERE id = ?1",
            rusqlite::params![entry_id],
        )?;
        Ok(changed > 0)
    }

    fn list_sessions(&self) -> anyhow::Result<Vec<SessionInfo>> {
        let conn = self
            .conn
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("SQLite connection not initialized"))?
            .lock()
            .map_err(|e| anyhow::anyhow!("Mutex poisoned: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT session_id, COUNT(*) as cnt,
                    MIN(created_at) as first, MAX(accessed_at) as last
             FROM memories GROUP BY session_id",
        )?;
        let sessions = stmt
            .query_map([], |row| {
                Ok(SessionInfo {
                    session_id: row.get(0)?,
                    agent_ids: Vec::new(),
                    memory_count: row.get::<_, i64>(1)? as usize,
                    created_at: row.get(2)?,
                    last_accessed: row.get(3)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(sessions)
    }

    fn health_check(&self) -> anyhow::Result<BackendHealth> {
        let conn = self
            .conn
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("SQLite connection not initialized"))?
            .lock()
            .map_err(|e| anyhow::anyhow!("Mutex poisoned: {e}"))?;
        let start = std::time::Instant::now();
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM memories", [], |row| row.get(0))?;
        let latency = start.elapsed().as_secs_f64() * 1000.0;
        Ok(BackendHealth {
            healthy: true,
            backend_name: "sqlite".to_string(),
            latency_ms: latency,
            entry_count: count as usize,
            details: format!("path={}", self.path.display()),
        })
    }

    fn backend_name(&self) -> &str {
        "sqlite"
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn sqlite_backend_struct_exists() {
        let _ = super::SqliteBackend {
            path: std::path::PathBuf::from(":memory:"),
            #[cfg(feature = "sqlite")]
            conn: None,
        };
    }
}
