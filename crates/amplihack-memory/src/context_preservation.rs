//! Context preservation for Claude agent workflows.
//!
//! Native Rust port of
//! `amplifier-bundle/tools/amplihack/memory/context_preservation.py`.
//! Persists conversation context, workflow state, and decision history to
//! the agent-memory SQLite store.

use std::path::Path;
use std::sync::Mutex;

use anyhow::Context;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

const TABLE: &str = "context_records";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationContext {
    pub agent_id: String,
    pub topic: String,
    #[serde(default)]
    pub messages: Vec<String>,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowState {
    pub workflow_name: String,
    pub step: u64,
    #[serde(default)]
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDecision {
    pub agent_id: String,
    pub decision: String,
    #[serde(default)]
    pub rationale: String,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

pub struct ContextPreserver {
    session_id: String,
    conn: Mutex<Connection>,
}

impl ContextPreserver {
    pub fn with_db(
        session_id: impl Into<String>,
        db_path: impl AsRef<Path>,
    ) -> anyhow::Result<Self> {
        let path = db_path.as_ref();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).context("create db parent dir")?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS context_records (\
                id INTEGER PRIMARY KEY AUTOINCREMENT,\
                session_id TEXT NOT NULL,\
                kind TEXT NOT NULL,\
                key TEXT NOT NULL,\
                payload TEXT NOT NULL,\
                created_at INTEGER NOT NULL\
             );\n\
             CREATE INDEX IF NOT EXISTS idx_ctx_session ON context_records(session_id);\n\
             CREATE INDEX IF NOT EXISTS idx_ctx_kind ON context_records(kind);\n\
             CREATE INDEX IF NOT EXISTS idx_ctx_key ON context_records(key);",
        )?;
        Ok(Self {
            session_id: session_id.into(),
            conn: Mutex::new(conn),
        })
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    fn now() -> i64 {
        chrono::Utc::now().timestamp()
    }

    fn insert(&self, kind: &str, key: &str, payload: &str) -> anyhow::Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO context_records (session_id, kind, key, payload, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![self.session_id, kind, key, payload, Self::now()],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn preserve_conversation_context(&self, ctx: &ConversationContext) -> anyhow::Result<i64> {
        let payload = serde_json::to_string(ctx)?;
        self.insert("conversation", &ctx.agent_id, &payload)
    }

    pub fn restore_conversation_context(
        &self,
        agent_id: Option<&str>,
    ) -> anyhow::Result<Option<ConversationContext>> {
        let conn = self.conn.lock().unwrap();
        let row: Option<String> = match agent_id {
            Some(a) => conn
                .query_row(
                    &format!(
                        "SELECT payload FROM {TABLE} \
                         WHERE session_id = ?1 AND kind = 'conversation' AND key = ?2 \
                         ORDER BY id DESC LIMIT 1"
                    ),
                    params![self.session_id, a],
                    |r| r.get(0),
                )
                .optional()?,
            None => conn
                .query_row(
                    &format!(
                        "SELECT payload FROM {TABLE} \
                         WHERE session_id = ?1 AND kind = 'conversation' \
                         ORDER BY id DESC LIMIT 1"
                    ),
                    params![self.session_id],
                    |r| r.get(0),
                )
                .optional()?,
        };
        match row {
            Some(p) => Ok(Some(serde_json::from_str(&p)?)),
            None => Ok(None),
        }
    }

    pub fn preserve_workflow_state(&self, w: &WorkflowState) -> anyhow::Result<i64> {
        let payload = serde_json::to_string(w)?;
        self.insert("workflow", &w.workflow_name, &payload)
    }

    pub fn restore_workflow_state(
        &self,
        workflow_name: &str,
    ) -> anyhow::Result<Option<WorkflowState>> {
        let conn = self.conn.lock().unwrap();
        let row: Option<String> = conn
            .query_row(
                &format!(
                    "SELECT payload FROM {TABLE} \
                     WHERE session_id = ?1 AND kind = 'workflow' AND key = ?2 \
                     ORDER BY id DESC LIMIT 1"
                ),
                params![self.session_id, workflow_name],
                |r| r.get(0),
            )
            .optional()?;
        match row {
            Some(p) => Ok(Some(serde_json::from_str(&p)?)),
            None => Ok(None),
        }
    }

    pub fn preserve_agent_decision(&self, d: &AgentDecision) -> anyhow::Result<i64> {
        let payload = serde_json::to_string(d)?;
        self.insert("decision", &d.agent_id, &payload)
    }

    pub fn get_decision_history(
        &self,
        agent_id: Option<&str>,
        limit: Option<usize>,
    ) -> anyhow::Result<Vec<AgentDecision>> {
        let conn = self.conn.lock().unwrap();
        let cap = limit.unwrap_or(usize::MAX);
        let rows: Vec<String> = match agent_id {
            Some(a) => {
                let mut stmt = conn.prepare(&format!(
                    "SELECT payload FROM {TABLE} \
                     WHERE session_id = ?1 AND kind = 'decision' AND key = ?2 \
                     ORDER BY id ASC"
                ))?;
                stmt.query_map(params![self.session_id, a], |r| r.get::<_, String>(0))?
                    .filter_map(|r| r.ok())
                    .take(cap)
                    .collect()
            }
            None => {
                let mut stmt = conn.prepare(&format!(
                    "SELECT payload FROM {TABLE} \
                     WHERE session_id = ?1 AND kind = 'decision' \
                     ORDER BY id ASC"
                ))?;
                stmt.query_map(params![self.session_id], |r| r.get::<_, String>(0))?
                    .filter_map(|r| r.ok())
                    .take(cap)
                    .collect()
            }
        };
        let mut out = Vec::with_capacity(rows.len());
        for raw in rows {
            out.push(serde_json::from_str(&raw)?);
        }
        Ok(out)
    }

    /// Delete records older than `older_than_days` for the current session.
    /// Returns the number of rows removed.
    pub fn cleanup_old_context(&self, older_than_days: i64) -> anyhow::Result<usize> {
        let cutoff = chrono::Utc::now().timestamp() - older_than_days * 86_400;
        let conn = self.conn.lock().unwrap();
        let n = conn.execute(
            &format!("DELETE FROM {TABLE} WHERE session_id = ?1 AND created_at < ?2"),
            params![self.session_id, cutoff],
        )?;
        Ok(n)
    }
}
