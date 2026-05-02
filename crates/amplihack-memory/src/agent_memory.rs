//! Agent-facing key/value memory adapter.
//!
//! Native Rust port of `amplifier-bundle/tools/amplihack/memory/interface.py`
//! and `core.py`. Provides a small, session-scoped store backed directly by
//! the same SQLite schema the Python implementation used, so existing
//! databases remain readable.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, anyhow};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

/// Storage representation of a memory value (controls (de)serialization).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryType {
    Markdown,
    Json,
    Yaml,
    Text,
}

impl MemoryType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Markdown => "markdown",
            Self::Json => "json",
            Self::Yaml => "yaml",
            Self::Text => "text",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "json" => Self::Json,
            "yaml" => Self::Yaml,
            "text" => Self::Text,
            _ => Self::Markdown,
        }
    }
}

/// Single memory entry as returned from `list()`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRecord {
    pub key: String,
    pub memory_type: MemoryType,
}

/// Agent-scoped memory handle. One handle == one session.
pub struct AgentMemory {
    agent_name: String,
    session_id: String,
    enabled: bool,
    backend: Option<Mutex<Connection>>,
}

impl AgentMemory {
    pub fn builder() -> AgentMemoryBuilder {
        AgentMemoryBuilder::default()
    }

    pub fn agent_name(&self) -> &str {
        &self.agent_name
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled && self.backend.is_some()
    }

    /// Store a string value. Returns `Ok(false)` when memory is disabled.
    pub fn store(&self, key: &str, value: &str, memory_type: MemoryType) -> anyhow::Result<bool> {
        if key.is_empty() {
            return Err(anyhow!("Key cannot be empty"));
        }
        if !self.is_enabled() {
            return Ok(false);
        }
        let conn = self.backend.as_ref().expect("enabled").lock().unwrap();
        Self::store_raw(&conn, &self.session_id, key, value, memory_type)
    }

    /// Store a JSON value. The value is serialized with `serde_json` and
    /// always stored with `MemoryType::Json` regardless of `memory_type`'s
    /// label, to keep the round-trip lossless.
    pub fn store_json(
        &self,
        key: &str,
        value: &serde_json::Value,
        _memory_type: MemoryType,
    ) -> anyhow::Result<bool> {
        if key.is_empty() {
            return Err(anyhow!("Key cannot be empty"));
        }
        if !self.is_enabled() {
            return Ok(false);
        }
        let conn = self.backend.as_ref().expect("enabled").lock().unwrap();
        let payload = serde_json::to_string(value).context("serialize JSON value")?;
        Self::store_raw(&conn, &self.session_id, key, &payload, MemoryType::Json)
    }

    fn store_raw(
        conn: &Connection,
        session_id: &str,
        key: &str,
        value: &str,
        memory_type: MemoryType,
    ) -> anyhow::Result<bool> {
        conn.execute(
            "INSERT INTO agent_memories \
             (session_id, memory_key, memory_value, memory_type, created_at, accessed_count) \
             VALUES (?1, ?2, ?3, ?4, \
                COALESCE((SELECT created_at FROM agent_memories \
                          WHERE session_id = ?1 AND memory_key = ?2), CURRENT_TIMESTAMP), \
                COALESCE((SELECT accessed_count FROM agent_memories \
                          WHERE session_id = ?1 AND memory_key = ?2), 0)) \
             ON CONFLICT(session_id, memory_key) \
             DO UPDATE SET memory_value = excluded.memory_value, \
                           memory_type = excluded.memory_type",
            params![session_id, key, value, memory_type.as_str()],
        )?;
        Ok(true)
    }

    /// Retrieve a value as a JSON value. String values are wrapped, JSON
    /// values are decoded.
    pub fn retrieve(&self, key: &str) -> anyhow::Result<Option<serde_json::Value>> {
        if !self.is_enabled() {
            return Ok(None);
        }
        let conn = self.backend.as_ref().expect("enabled").lock().unwrap();
        let row: Option<(String, String)> = conn
            .query_row(
                "SELECT memory_value, memory_type FROM agent_memories \
                 WHERE session_id = ?1 AND memory_key = ?2",
                params![self.session_id, key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((value, ty)) = row else {
            return Ok(None);
        };
        // best-effort access counter bump
        let _ = conn.execute(
            "UPDATE agent_memories SET accessed_count = accessed_count + 1 \
             WHERE session_id = ?1 AND memory_key = ?2",
            params![self.session_id, key],
        );
        if ty == "json" {
            Ok(Some(
                serde_json::from_str(&value).unwrap_or(serde_json::Value::String(value)),
            ))
        } else {
            Ok(Some(serde_json::Value::String(value)))
        }
    }

    /// List all entries for the current session.
    pub fn list(&self) -> anyhow::Result<Vec<MemoryRecord>> {
        if !self.is_enabled() {
            return Ok(vec![]);
        }
        let conn = self.backend.as_ref().expect("enabled").lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT memory_key, memory_type FROM agent_memories \
             WHERE session_id = ?1 ORDER BY memory_key",
        )?;
        let rows = stmt.query_map(params![self.session_id], |r| {
            let key: String = r.get(0)?;
            let ty: String = r.get(1)?;
            Ok(MemoryRecord {
                key,
                memory_type: MemoryType::parse(&ty),
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Delete a key. Returns true if a row was deleted, false otherwise
    /// (including when memory is disabled).
    pub fn delete(&self, key: &str) -> anyhow::Result<bool> {
        if !self.is_enabled() {
            return Ok(false);
        }
        let conn = self.backend.as_ref().expect("enabled").lock().unwrap();
        let n = conn.execute(
            "DELETE FROM agent_memories WHERE session_id = ?1 AND memory_key = ?2",
            params![self.session_id, key],
        )?;
        Ok(n > 0)
    }

    pub fn clear_session(&self) -> anyhow::Result<bool> {
        if !self.is_enabled() {
            return Ok(false);
        }
        let conn = self.backend.as_ref().expect("enabled").lock().unwrap();
        conn.execute(
            "DELETE FROM agent_memories WHERE session_id = ?1",
            params![self.session_id],
        )?;
        Ok(true)
    }
}

/// Builder for `AgentMemory`.
#[derive(Default)]
pub struct AgentMemoryBuilder {
    agent_name: Option<String>,
    session_id: Option<String>,
    db_path: Option<PathBuf>,
    enabled: Option<bool>,
}

impl AgentMemoryBuilder {
    pub fn agent_name(mut self, name: impl Into<String>) -> Self {
        self.agent_name = Some(name.into());
        self
    }

    pub fn session_id(mut self, sid: impl Into<String>) -> Self {
        self.session_id = Some(sid.into());
        self
    }

    pub fn db_path(mut self, p: impl Into<PathBuf>) -> Self {
        self.db_path = Some(p.into());
        self
    }

    pub fn enabled(mut self, e: bool) -> Self {
        self.enabled = Some(e);
        self
    }

    pub fn build(self) -> anyhow::Result<AgentMemory> {
        let agent = self
            .agent_name
            .ok_or_else(|| anyhow!("agent_name is required"))?;
        let session_id = self
            .session_id
            .unwrap_or_else(|| generate_session_id(&agent));
        let enabled = self.enabled.unwrap_or(true);
        let db_path = self
            .db_path
            .unwrap_or_else(|| PathBuf::from(".claude/runtime/memory.db"));
        let backend = if enabled {
            Some(Mutex::new(open_database(&db_path, &session_id, &agent)?))
        } else {
            None
        };
        Ok(AgentMemory {
            agent_name: agent,
            session_id,
            enabled,
            backend,
        })
    }
}

fn open_database(path: &Path, session_id: &str, agent: &str) -> anyhow::Result<Connection> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).context("create db parent dir")?;
    }
    let conn = Connection::open(path).context("open sqlite db")?;
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;\n\
         CREATE TABLE IF NOT EXISTS agent_sessions (\
            id TEXT PRIMARY KEY,\
            agent_name TEXT NOT NULL,\
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,\
            last_accessed TIMESTAMP DEFAULT CURRENT_TIMESTAMP,\
            metadata TEXT);\n\
         CREATE TABLE IF NOT EXISTS agent_memories (\
            id INTEGER PRIMARY KEY AUTOINCREMENT,\
            session_id TEXT REFERENCES agent_sessions(id) ON DELETE CASCADE,\
            memory_key TEXT NOT NULL,\
            memory_value TEXT NOT NULL,\
            memory_type TEXT DEFAULT 'markdown',\
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,\
            accessed_count INTEGER DEFAULT 0,\
            UNIQUE(session_id, memory_key));\n\
         CREATE INDEX IF NOT EXISTS idx_memories_session ON agent_memories(session_id);\n\
         CREATE INDEX IF NOT EXISTS idx_memories_key ON agent_memories(memory_key);\n\
         CREATE INDEX IF NOT EXISTS idx_sessions_agent ON agent_sessions(agent_name);",
    )?;
    conn.execute(
        "INSERT INTO agent_sessions (id, agent_name) VALUES (?1, ?2) \
         ON CONFLICT(id) DO UPDATE SET last_accessed = CURRENT_TIMESTAMP",
        params![session_id, agent],
    )?;
    Ok(conn)
}

fn generate_session_id(agent: &str) -> String {
    let ts = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let unique = uuid::Uuid::new_v4().simple().to_string();
    format!("{agent}_{ts}_{}", &unique[..8])
}
