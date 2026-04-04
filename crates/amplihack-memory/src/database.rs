//! Database abstraction layer for the memory system.
//!
//! Port of Python `amplihack/memory/database.py`.
//! Provides thread-safe SQLite storage with WAL mode, content hashing,
//! session tracking, and memory CRUD operations.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

#[cfg(feature = "sqlite")]
use rusqlite::{Connection, params};
#[cfg(feature = "sqlite")]
use std::path::{Path, PathBuf};
#[cfg(feature = "sqlite")]
use std::sync::Mutex;

use crate::models::MemoryType;
#[cfg(feature = "sqlite")]
use crate::models::{MemoryEntry, SessionInfo};

#[cfg(feature = "sqlite")]
use crate::database_helpers::{CREATE_INDEXES_SQL, CREATE_TABLES_SQL, row_to_entry};
pub(crate) use crate::database_helpers::{iso_to_epoch, now_iso};

/// Database query parameters (simplified from Python MemoryQuery).
#[derive(Debug, Clone, Default)]
pub struct DbQuery {
    pub session_id: Option<String>,
    pub agent_id: Option<String>,
    pub memory_type: Option<MemoryType>,
    pub tags: Vec<String>,
    pub content_search: Option<String>,
    pub min_importance: Option<f64>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    pub include_expired: bool,
}

/// Database statistics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DbStats {
    pub total_memories: usize,
    pub total_sessions: usize,
    pub memory_types: HashMap<String, usize>,
    pub top_agents: HashMap<String, usize>,
    pub db_size_bytes: u64,
}

/// Compute SHA-256 content hash for dedup.
pub fn content_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Thread-safe SQLite database for agent memory storage.
///
/// Feature-gated behind `sqlite`. Falls back to a no-op stub otherwise.
#[cfg(feature = "sqlite")]
pub struct MemoryDatabase {
    db_path: PathBuf,
    conn: Mutex<Connection>,
}

#[cfg(feature = "sqlite")]
impl MemoryDatabase {
    /// Open or create the database at `db_path`.
    pub fn open(db_path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let db_path = db_path.as_ref().to_path_buf();
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(&db_path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA temp_store=MEMORY;
             PRAGMA foreign_keys=ON;",
        )?;
        let db = Self {
            db_path,
            conn: Mutex::new(conn),
        };
        db.create_tables()?;
        db.create_indexes()?;
        Ok(db)
    }

    /// Open an in-memory database (for tests).
    pub fn open_in_memory() -> anyhow::Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        let db = Self {
            db_path: PathBuf::from(":memory:"),
            conn: Mutex::new(conn),
        };
        db.create_tables()?;
        db.create_indexes()?;
        Ok(db)
    }

    fn create_tables(&self) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(CREATE_TABLES_SQL)?;
        Ok(())
    }

    fn create_indexes(&self) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(CREATE_INDEXES_SQL)?;
        Ok(())
    }

    /// Store a memory entry (upsert).
    pub fn store_memory(&self, entry: &MemoryEntry) -> anyhow::Result<bool> {
        let conn = self.conn.lock().unwrap();
        let now = now_iso();
        let hash = content_hash(&entry.content);
        let tags_json: Option<String> = if entry.tags.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&entry.tags)?)
        };
        let meta_json = serde_json::to_string(&entry.metadata)?;
        let created = &now;
        let accessed = &now;

        // Upsert session
        conn.execute(
            "INSERT INTO sessions (session_id, created_at, last_accessed, metadata)
             VALUES (?1, ?2, ?2, '{}')
             ON CONFLICT(session_id)
             DO UPDATE SET last_accessed = ?2",
            params![entry.session_id, now],
        )?;

        // Upsert session-agent
        conn.execute(
            "INSERT INTO session_agents (session_id, agent_id, first_used, last_used)
             VALUES (?1, ?2, ?3, ?3)
             ON CONFLICT(session_id, agent_id)
             DO UPDATE SET last_used = ?3",
            params![entry.session_id, entry.agent_id, now],
        )?;

        conn.execute(
            "INSERT OR REPLACE INTO memory_entries
             (id, session_id, agent_id, memory_type, title, content,
              content_hash, metadata, tags, importance,
              created_at, accessed_at, expires_at, parent_id)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
            params![
                entry.id,
                entry.session_id,
                entry.agent_id,
                entry.memory_type.as_str(),
                entry.title,
                entry.content,
                hash,
                meta_json,
                tags_json,
                entry.importance,
                created,
                accessed,
                Option::<String>::None,
                Option::<String>::None,
            ],
        )?;
        Ok(true)
    }

    /// Retrieve memories matching a query.
    pub fn retrieve_memories(&self, query: &DbQuery) -> anyhow::Result<Vec<MemoryEntry>> {
        const COLS: &str = "id,session_id,agent_id,memory_type,title,content,metadata,tags,importance,created_at,accessed_at";
        let conn = self.conn.lock().unwrap();
        let mut sql = format!("SELECT {COLS} FROM memory_entries WHERE 1=1");
        let mut p: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(ref sid) = query.session_id {
            sql.push_str(" AND session_id=?");
            p.push(Box::new(sid.clone()));
        }
        if let Some(ref aid) = query.agent_id {
            sql.push_str(" AND agent_id=?");
            p.push(Box::new(aid.clone()));
        }
        if let Some(ref mt) = query.memory_type {
            sql.push_str(" AND memory_type=?");
            p.push(Box::new(mt.as_str().to_string()));
        }
        if let Some(ref search) = query.content_search {
            sql.push_str(" AND (content LIKE ? OR title LIKE ?)");
            let pat = format!("%{search}%");
            p.push(Box::new(pat.clone()));
            p.push(Box::new(pat));
        }
        if let Some(min_imp) = query.min_importance {
            sql.push_str(" AND importance>=?");
            p.push(Box::new(min_imp));
        }
        if !query.include_expired {
            sql.push_str(" AND (expires_at IS NULL OR expires_at>datetime('now'))");
        }
        sql.push_str(" ORDER BY accessed_at DESC,importance DESC");
        if let Some(limit) = query.limit {
            sql.push_str(&format!(" LIMIT {limit}"));
            if let Some(offset) = query.offset {
                sql.push_str(&format!(" OFFSET {offset}"));
            }
        }
        let refs: Vec<&dyn rusqlite::types::ToSql> = p.iter().map(|b| b.as_ref()).collect();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(refs.as_slice(), |row| Ok(row_to_entry(row)))?;
        Ok(rows.filter_map(|r| r.ok().flatten()).collect())
    }

    /// Get a single memory by ID.
    pub fn get_by_id(&self, memory_id: &str) -> anyhow::Result<Option<MemoryEntry>> {
        const COLS: &str = "id,session_id,agent_id,memory_type,title,content,metadata,tags,importance,created_at,accessed_at";
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!("SELECT {COLS} FROM memory_entries WHERE id=?1"))?;
        let mut rows = stmt.query(params![memory_id])?;
        match rows.next()? {
            Some(row) => Ok(row_to_entry(row)),
            None => Ok(None),
        }
    }

    /// Delete a memory by ID. Returns true if it existed.
    pub fn delete_memory(&self, memory_id: &str) -> anyhow::Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "DELETE FROM memory_entries WHERE id = ?1",
            params![memory_id],
        )?;
        Ok(changed > 0)
    }

    /// Remove expired entries, return count.
    pub fn cleanup_expired(&self) -> anyhow::Result<usize> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "DELETE FROM memory_entries
             WHERE expires_at IS NOT NULL AND expires_at < datetime('now')",
            [],
        )?;
        Ok(changed)
    }

    /// Delete a session and all its memories.
    pub fn delete_session(&self, session_id: &str) -> anyhow::Result<bool> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM memory_entries WHERE session_id = ?1",
            params![session_id],
        )?;
        conn.execute(
            "DELETE FROM session_agents WHERE session_id = ?1",
            params![session_id],
        )?;
        let changed = conn.execute(
            "DELETE FROM sessions WHERE session_id = ?1",
            params![session_id],
        )?;
        Ok(changed > 0)
    }

    /// Get session info.
    pub fn get_session_info(&self, session_id: &str) -> anyhow::Result<Option<SessionInfo>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT session_id, created_at, last_accessed FROM sessions WHERE session_id = ?1",
        )?;
        let mut rows = stmt.query(params![session_id])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        let sid: String = row.get(0)?;
        let created: String = row.get(1)?;
        let accessed: String = row.get(2)?;

        let agents = self.session_agents(&conn, &sid)?;
        let count = self.session_memory_count(&conn, &sid)?;
        Ok(Some(SessionInfo {
            session_id: sid,
            agent_ids: agents,
            memory_count: count,
            created_at: iso_to_epoch(&created),
            last_accessed: iso_to_epoch(&accessed),
        }))
    }

    /// List sessions ordered by last accessed (descending).
    pub fn list_sessions(&self, limit: Option<usize>) -> anyhow::Result<Vec<SessionInfo>> {
        let conn = self.conn.lock().unwrap();
        let sql = match limit {
            Some(n) => format!("SELECT session_id, created_at, last_accessed FROM sessions ORDER BY last_accessed DESC LIMIT {n}"),
            None => "SELECT session_id, created_at, last_accessed FROM sessions ORDER BY last_accessed DESC".into(),
        };
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;

        let mut sessions = Vec::new();
        for r in rows {
            let (sid, created, accessed) = r?;
            let agents = self.session_agents(&conn, &sid)?;
            let count = self.session_memory_count(&conn, &sid)?;
            sessions.push(SessionInfo {
                session_id: sid,
                agent_ids: agents,
                memory_count: count,
                created_at: iso_to_epoch(&created),
                last_accessed: iso_to_epoch(&accessed),
            });
        }
        Ok(sessions)
    }

    /// Aggregate statistics.
    pub fn get_stats(&self) -> anyhow::Result<DbStats> {
        let conn = self.conn.lock().unwrap();
        let total_memories: i64 =
            conn.query_row("SELECT COUNT(*) FROM memory_entries", [], |r| r.get(0))?;
        let total_sessions: i64 =
            conn.query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))?;

        let mut mt_stmt =
            conn.prepare("SELECT memory_type, COUNT(*) FROM memory_entries GROUP BY memory_type")?;
        let memory_types: HashMap<String, usize> = mt_stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
            .filter_map(|r| r.ok())
            .map(|(k, v)| (k, v as usize))
            .collect();

        let mut ag_stmt = conn.prepare(
            "SELECT agent_id, COUNT(*) FROM memory_entries
             GROUP BY agent_id ORDER BY COUNT(*) DESC LIMIT 10",
        )?;
        let top_agents: HashMap<String, usize> = ag_stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
            .filter_map(|r| r.ok())
            .map(|(k, v)| (k, v as usize))
            .collect();

        let db_size_bytes = std::fs::metadata(&self.db_path)
            .map(|m| m.len())
            .unwrap_or(0);

        Ok(DbStats {
            total_memories: total_memories as usize,
            total_sessions: total_sessions as usize,
            memory_types,
            top_agents,
            db_size_bytes,
        })
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// Run VACUUM.
    pub fn vacuum(&self) -> anyhow::Result<()> {
        self.conn.lock().unwrap().execute_batch("VACUUM")?;
        Ok(())
    }

    /// Run ANALYZE + PRAGMA optimize.
    pub fn optimize(&self) -> anyhow::Result<()> {
        self.conn
            .lock()
            .unwrap()
            .execute_batch("ANALYZE; PRAGMA optimize;")?;
        Ok(())
    }

    fn session_agents(&self, conn: &Connection, session_id: &str) -> anyhow::Result<Vec<String>> {
        let mut stmt = conn.prepare(
            "SELECT agent_id FROM session_agents WHERE session_id = ?1 ORDER BY last_used DESC",
        )?;
        Ok(stmt
            .query_map(params![session_id], |r| r.get(0))?
            .filter_map(|r| r.ok())
            .collect())
    }

    fn session_memory_count(&self, conn: &Connection, session_id: &str) -> anyhow::Result<usize> {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM memory_entries WHERE session_id = ?1",
            params![session_id],
            |r| r.get(0),
        )?;
        Ok(count as usize)
    }
}

#[cfg(test)]
#[path = "tests/database_tests.rs"]
mod tests;
