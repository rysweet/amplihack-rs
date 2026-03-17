//! SQLite backend implementation for the memory subsystem.
//!
//! All rusqlite-specific code lives here, symmetric with the Kùzu backend in
//! `backend/kuzu.rs`. This provides the insertion point for future backends
//! (e.g. LadybugDB) without requiring surgery to the rest of the memory
//! subsystem.

use super::super::*;
use super::{MemoryRuntimeBackend, MemorySessionBackend, MemoryTreeBackend};
use anyhow::Result;
use rusqlite::{Connection as SqliteConnection, params};

pub(crate) const SQLITE_TREE_BACKEND_NAME: &str = "unknown";

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

pub(crate) fn open_sqlite_memory_db() -> Result<SqliteConnection> {
    let path = home_dir()?.join(".amplihack").join("memory.db");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = SqliteConnection::open(path)?;
    conn.execute_batch(SQLITE_SCHEMA)?;
    Ok(conn)
}

pub(crate) fn list_sqlite_sessions_from_conn(
    conn: &SqliteConnection,
) -> Result<Vec<SessionSummary>> {
    let mut stmt = conn.prepare("SELECT session_id FROM sessions ORDER BY last_accessed DESC")?;
    let mut rows = stmt.query([])?;
    let mut sessions = Vec::new();
    while let Some(row) = rows.next()? {
        let session_id: String = row.get(0)?;
        let memory_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM memory_entries WHERE session_id = ?1",
            params![session_id],
            |row| row.get(0),
        )?;
        sessions.push(SessionSummary {
            session_id,
            memory_count: memory_count as usize,
        });
    }
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

pub(crate) fn delete_sqlite_session(session_id: &str) -> Result<bool> {
    let conn = open_sqlite_memory_db()?;
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
}

pub(crate) fn store_learning_sqlite(record: &SessionLearningRecord) -> Result<Option<String>> {
    let conn = open_sqlite_memory_db()?;
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

    conn.execute(
        "INSERT OR IGNORE INTO sessions (session_id, created_at, last_accessed, metadata) VALUES (?1, ?2, ?3, '{}')",
        params![record.session_id, now, now],
    )?;
    conn.execute(
        "UPDATE sessions SET last_accessed = ?2 WHERE session_id = ?1",
        params![record.session_id, now],
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO session_agents (session_id, agent_id, first_used, last_used) VALUES (?1, ?2, ?3, ?4)",
        params![record.session_id, record.agent_id, now, now],
    )?;
    conn.execute(
        "UPDATE session_agents SET last_used = ?3 WHERE session_id = ?1 AND agent_id = ?2",
        params![record.session_id, record.agent_id, now],
    )?;
    conn.execute(
        "INSERT INTO memory_entries (id, session_id, agent_id, memory_type, title, content, metadata, importance, created_at, accessed_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
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
