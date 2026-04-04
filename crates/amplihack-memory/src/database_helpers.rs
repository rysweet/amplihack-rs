//! Private helpers for the database module.

use crate::models::MemoryType;
use std::collections::HashMap;

#[cfg(feature = "sqlite")]
use crate::models::MemoryEntry;

pub(crate) const CREATE_TABLES_SQL: &str = "\
    CREATE TABLE IF NOT EXISTS memory_entries (
        id TEXT PRIMARY KEY,
        session_id TEXT NOT NULL,
        agent_id TEXT NOT NULL,
        memory_type TEXT NOT NULL,
        title TEXT NOT NULL,
        content TEXT NOT NULL,
        content_hash TEXT NOT NULL,
        metadata TEXT NOT NULL DEFAULT '{}',
        tags TEXT DEFAULT NULL,
        importance REAL DEFAULT NULL,
        created_at TEXT NOT NULL,
        accessed_at TEXT NOT NULL,
        expires_at TEXT DEFAULT NULL,
        parent_id TEXT DEFAULT NULL,
        FOREIGN KEY (parent_id)
            REFERENCES memory_entries(id) ON DELETE SET NULL
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
        PRIMARY KEY (session_id, agent_id),
        FOREIGN KEY (session_id)
            REFERENCES sessions(session_id) ON DELETE CASCADE
    );";

pub(crate) const CREATE_INDEXES_SQL: &str = "\
    CREATE INDEX IF NOT EXISTS idx_mem_session_agent
        ON memory_entries(session_id, agent_id);
    CREATE INDEX IF NOT EXISTS idx_mem_type
        ON memory_entries(memory_type);
    CREATE INDEX IF NOT EXISTS idx_mem_created
        ON memory_entries(created_at);
    CREATE INDEX IF NOT EXISTS idx_mem_accessed
        ON memory_entries(accessed_at);
    CREATE INDEX IF NOT EXISTS idx_mem_expires
        ON memory_entries(expires_at);
    CREATE INDEX IF NOT EXISTS idx_mem_importance
        ON memory_entries(importance);
    CREATE INDEX IF NOT EXISTS idx_mem_content_hash
        ON memory_entries(session_id, content_hash);
    CREATE INDEX IF NOT EXISTS idx_mem_parent
        ON memory_entries(parent_id);
    CREATE INDEX IF NOT EXISTS idx_sessions_accessed
        ON sessions(last_accessed);
    CREATE INDEX IF NOT EXISTS idx_sagents_used
        ON session_agents(last_used);";

pub(crate) fn now_iso() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let (y, m, d) = days_to_ymd(days);
    let h = time_of_day / 3600;
    let min = (time_of_day % 3600) / 60;
    let s = time_of_day % 60;
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{min:02}:{s:02}Z")
}

fn days_to_ymd(mut days: u64) -> (u64, u64, u64) {
    days += 719_468;
    let era = days / 146_097;
    let doe = days % 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

pub(crate) fn iso_to_epoch(iso: &str) -> f64 {
    let parts: Vec<&str> = iso.split('T').collect();
    if parts.len() < 2 {
        return 0.0;
    }
    let date_parts: Vec<u64> = parts[0].split('-').filter_map(|s| s.parse().ok()).collect();
    let time_str = parts[1].trim_end_matches('Z');
    let time_parts: Vec<u64> = time_str
        .split(':')
        .filter_map(|s| s.split('.').next().and_then(|t| t.parse().ok()))
        .collect();
    if date_parts.len() < 3 || time_parts.len() < 3 {
        return 0.0;
    }
    let (y, m, d) = (date_parts[0], date_parts[1], date_parts[2]);
    let (h, min, s) = (time_parts[0], time_parts[1], time_parts[2]);
    let m_adj = if m <= 2 { m + 9 } else { m - 3 };
    let y_adj = if m <= 2 { y - 1 } else { y };
    let days =
        y_adj * 365 + y_adj / 4 - y_adj / 100 + y_adj / 400 + (m_adj * 153 + 2) / 5 + d - 719_469;
    (days * 86400 + h * 3600 + min * 60 + s) as f64
}

#[cfg(feature = "sqlite")]
pub(crate) fn row_to_entry(row: &rusqlite::Row<'_>) -> Option<MemoryEntry> {
    let id: String = row.get(0).ok()?;
    let session_id: String = row.get(1).ok()?;
    let agent_id: String = row.get(2).ok()?;
    let mt_str: String = row.get(3).ok()?;
    let title: String = row.get(4).ok()?;
    let content: String = row.get(5).ok()?;
    let meta_json: String = row.get(6).ok()?;
    let tags_json: Option<String> = row.get(7).ok()?;
    let importance: Option<f64> = row.get(8).ok()?;
    let created_str: String = row.get(9).ok()?;
    let accessed_str: String = row.get(10).ok()?;

    let memory_type = str_to_memory_type(&mt_str)?;
    let metadata: HashMap<String, serde_json::Value> =
        serde_json::from_str(&meta_json).unwrap_or_default();
    let tags: Vec<String> = tags_json
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();

    Some(MemoryEntry {
        id,
        session_id,
        agent_id,
        memory_type,
        title,
        content,
        metadata,
        created_at: iso_to_epoch(&created_str),
        accessed_at: iso_to_epoch(&accessed_str),
        tags,
        importance: importance.unwrap_or(0.5),
    })
}

pub(crate) fn str_to_memory_type(s: &str) -> Option<MemoryType> {
    match s {
        "episodic" => Some(MemoryType::Episodic),
        "semantic" => Some(MemoryType::Semantic),
        "procedural" => Some(MemoryType::Procedural),
        "prospective" => Some(MemoryType::Prospective),
        "working" => Some(MemoryType::Working),
        "strategic" => Some(MemoryType::Strategic),
        "code_context" => Some(MemoryType::CodeContext),
        "project_structure" => Some(MemoryType::ProjectStructure),
        "user_preference" => Some(MemoryType::UserPreference),
        "error_pattern" => Some(MemoryType::ErrorPattern),
        "conversation" => Some(MemoryType::Conversation),
        "task" => Some(MemoryType::Task),
        _ => None,
    }
}
