//! SQLite backend implementation for the memory subsystem.
//!
//! All rusqlite-specific code lives here, symmetric with the graph-db backend in
//! `backend/graph_db.rs`. This provides the insertion point for future backends
//! (e.g. LadybugDB) without requiring surgery to the rest of the memory
//! subsystem.

use super::super::*;
use super::{MemoryRuntimeBackend, MemorySessionBackend, MemoryTreeBackend};
use anyhow::Result;
use rusqlite::{Connection as SqliteConnection, params};

pub(crate) const SQLITE_TREE_BACKEND_NAME: &str = "sqlite";

pub(crate) const SQLITE_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS memory_entries (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    memory_type TEXT NOT NULL,
    title TEXT NOT NULL,
    content TEXT NOT NULL,
    content_hash TEXT,
    metadata TEXT NOT NULL DEFAULT '{}',
    tags TEXT DEFAULT NULL,
    importance INTEGER DEFAULT NULL,
    created_at TEXT NOT NULL,
    accessed_at TEXT NOT NULL,
    expires_at TEXT DEFAULT NULL,
    parent_id TEXT DEFAULT NULL
);
CREATE TABLE IF NOT EXISTS sessions (
    session_id TEXT PRIMARY KEY,
    created_at TEXT NOT NULL,
    last_accessed TEXT NOT NULL,
    metadata TEXT NOT NULL DEFAULT '{}'
);
CREATE TABLE IF NOT EXISTS session_agents (
    session_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    first_used TEXT NOT NULL,
    last_used TEXT NOT NULL,
    PRIMARY KEY (session_id, agent_id)
);
"#;

pub(crate) struct SqliteBackend {
    conn: SqliteConnection,
}

impl SqliteBackend {
    pub(crate) fn open() -> Result<Self> {
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
        delete_sqlite_session_with_conn(&self.conn, session_id)
    }
}

impl MemoryRuntimeBackend for SqliteBackend {
    fn load_prompt_context_memories(&self, session_id: &str) -> Result<Vec<MemoryRecord>> {
        query_sqlite_memories_for_session(&self.conn, session_id, None)
    }

    fn store_session_learning(&self, record: &SessionLearningRecord) -> Result<Option<String>> {
        store_learning_sqlite_with_conn(&self.conn, record)
    }
}

pub(crate) fn open_sqlite_memory_db() -> Result<SqliteConnection> {
    let path = memory_home_paths()?.sqlite_db;
    ensure_parent_dir(&path)?;
    let conn = SqliteConnection::open(path)?;
    conn.execute_batch(SQLITE_SCHEMA)?;
    Ok(conn)
}

pub(crate) fn list_sqlite_sessions_from_conn(
    conn: &SqliteConnection,
) -> Result<Vec<SessionSummary>> {
    // Single JOIN eliminates the N+1 query pattern (one COUNT per session).
    let mut stmt = conn.prepare(
        "SELECT s.session_id, COUNT(me.id) \
         FROM sessions s \
         LEFT JOIN memory_entries me ON s.session_id = me.session_id \
         GROUP BY s.session_id \
         ORDER BY s.last_accessed DESC",
    )?;
    let sessions = stmt
        .query_map([], |row| {
            Ok(SessionSummary {
                session_id: row.get(0)?,
                memory_count: row.get::<_, i64>(1)? as usize,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(sessions)
}

pub(crate) fn query_sqlite_memories_for_session(
    conn: &SqliteConnection,
    session_id: &str,
    memory_type: Option<&str>,
) -> Result<Vec<MemoryRecord>> {
    let mut sql = String::from(
        "SELECT id, memory_type, title, content, metadata, importance, accessed_at, expires_at FROM memory_entries WHERE session_id = ?1 AND (expires_at IS NULL OR expires_at > datetime('now'))",
    );
    if memory_type.is_some() {
        sql.push_str(" AND memory_type = ?2");
    }
    sql.push_str(" ORDER BY accessed_at DESC, importance DESC");
    let mut stmt = conn.prepare(&sql)?;
    let mapper = |row: &rusqlite::Row<'_>| -> rusqlite::Result<MemoryRecord> {
        let metadata_raw: Option<String> = row.get(4)?;
        Ok(MemoryRecord {
            memory_id: row.get(0)?,
            memory_type: row.get(1)?,
            title: row.get(2)?,
            content: row.get(3)?,
            metadata: metadata_raw
                .as_deref()
                .map(parse_json_value)
                .transpose()
                .map_err(to_sqlite_err)?
                .unwrap_or(serde_json::Value::Object(Default::default())),
            importance: row.get(5)?,
            accessed_at: row.get(6)?,
            expires_at: row.get(7)?,
        })
    };
    let rows = if let Some(memory_type) = memory_type {
        stmt.query_map(params![session_id, memory_type], mapper)?
            .collect::<rusqlite::Result<Vec<_>>>()?
    } else {
        stmt.query_map(params![session_id], mapper)?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    Ok(rows)
}

pub(crate) fn collect_sqlite_agent_counts(conn: &SqliteConnection) -> Result<Vec<(String, usize)>> {
    let mut stmt = conn.prepare(
        "SELECT agent_id, COUNT(*) FROM memory_entries WHERE expires_at IS NULL OR expires_at > datetime('now') GROUP BY agent_id ORDER BY agent_id ASC",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Delete a session using a pre-existing connection (avoids opening a new one).
///
/// All three DELETEs (memory_entries, session_agents, sessions) are wrapped in
/// a single `BEGIN IMMEDIATE` transaction so a crash or error between statements
/// cannot leave orphaned rows.  We use explicit SQL transaction control rather
/// than `rusqlite::Transaction` because this function takes `&SqliteConnection`
/// (shared reference), which does not allow `conn.transaction()`.
pub(crate) fn delete_sqlite_session_with_conn(
    conn: &SqliteConnection,
    session_id: &str,
) -> Result<bool> {
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let outcome = (|| -> Result<bool> {
        conn.execute(
            "DELETE FROM memory_entries WHERE session_id = ?1",
            params![session_id],
        )?;
        conn.execute(
            "DELETE FROM session_agents WHERE session_id = ?1",
            params![session_id],
        )?;
        let deleted = conn.execute(
            "DELETE FROM sessions WHERE session_id = ?1",
            params![session_id],
        )?;
        Ok(deleted > 0)
    })();
    match outcome {
        Ok(deleted) => {
            conn.execute_batch("COMMIT")?;
            Ok(deleted)
        }
        Err(e) => {
            // Best-effort rollback; ignore secondary errors.
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

/// Store a learning record using a pre-existing connection (avoids opening a new one).
///
/// Uses `ON CONFLICT DO UPDATE` (SQLite 3.24+) to collapse the previous
/// INSERT-OR-IGNORE + UPDATE pair into a single statement each for `sessions`
/// and `session_agents`, halving the number of writes on the hot path.
pub(crate) fn store_learning_sqlite_with_conn(
    conn: &SqliteConnection,
    record: &SessionLearningRecord,
) -> Result<Option<String>> {
    let now = chrono::Utc::now().to_rfc3339();
    let memory_id = build_memory_id(record, &now);

    let duplicate_exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM memory_entries WHERE session_id = ?1 AND agent_id = ?2 AND content = ?3",
        params![record.session_id, record.agent_id, record.content],
        |row| row.get(0),
    )?;
    if duplicate_exists > 0 {
        return Ok(None);
    }

    // Single UPSERT per table instead of INSERT-OR-IGNORE + UPDATE.
    conn.execute(
        "INSERT INTO sessions (session_id, created_at, last_accessed, metadata) \
         VALUES (?1, ?2, ?3, '{}') \
         ON CONFLICT(session_id) DO UPDATE SET last_accessed = excluded.last_accessed",
        params![record.session_id, now, now],
    )?;
    conn.execute(
        "INSERT INTO session_agents (session_id, agent_id, first_used, last_used) \
         VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(session_id, agent_id) DO UPDATE SET last_used = excluded.last_used",
        params![record.session_id, record.agent_id, now, now],
    )?;
    conn.execute(
        "INSERT INTO memory_entries \
         (id, session_id, agent_id, memory_type, title, content, metadata, importance, created_at, accessed_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            memory_id,
            record.session_id,
            record.agent_id,
            "learning",
            record.title,
            record.content,
            serde_json::to_string(&record.metadata)?,
            record.importance,
            now,
            now,
        ],
    )?;
    Ok(Some(memory_id))
}

pub(crate) fn to_sqlite_err(error: anyhow::Error) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            error.to_string(),
        )),
    )
}
