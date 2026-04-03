//! MemoryBackend trait — abstract storage interface.
//!
//! Defines the contract that all persistent backends (SQLite, Redis, Kuzu)
//! must implement. This is the high-level memory API, distinct from the
//! lower-level GraphStore trait which operates on raw nodes/edges.

use crate::models::{MemoryEntry, MemoryQuery, SessionInfo};
use serde::{Deserialize, Serialize};

/// Health status for a backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendHealth {
    pub healthy: bool,
    pub backend_name: String,
    pub latency_ms: f64,
    pub entry_count: usize,
    pub details: String,
}

impl BackendHealth {
    pub fn ok(name: &str) -> Self {
        Self {
            healthy: true,
            backend_name: name.to_string(),
            latency_ms: 0.0,
            entry_count: 0,
            details: String::new(),
        }
    }

    pub fn degraded(name: &str, reason: &str) -> Self {
        Self {
            healthy: false,
            backend_name: name.to_string(),
            latency_ms: 0.0,
            entry_count: 0,
            details: reason.to_string(),
        }
    }
}

/// Abstract memory backend trait.
///
/// Each backend provides persistent storage for MemoryEntry values,
/// supporting store, retrieve, delete, session listing, and health checks.
pub trait MemoryBackend: Send + Sync {
    /// Store a memory entry. Returns the entry ID on success.
    fn store(&mut self, entry: &MemoryEntry) -> anyhow::Result<String>;

    /// Retrieve entries matching a query.
    fn retrieve(&self, query: &MemoryQuery) -> anyhow::Result<Vec<MemoryEntry>>;

    /// Delete an entry by ID. Returns true if the entry existed.
    fn delete(&mut self, entry_id: &str) -> anyhow::Result<bool>;

    /// List all known sessions.
    fn list_sessions(&self) -> anyhow::Result<Vec<SessionInfo>>;

    /// Check backend health.
    fn health_check(&self) -> anyhow::Result<BackendHealth>;

    /// Name of this backend (e.g. "sqlite", "redis", "in_memory").
    fn backend_name(&self) -> &str;
}

/// In-memory backend for testing — no persistence.
pub struct InMemoryBackend {
    entries: Vec<MemoryEntry>,
}

impl InMemoryBackend {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }
}

impl Default for InMemoryBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryBackend for InMemoryBackend {
    fn store(&mut self, entry: &MemoryEntry) -> anyhow::Result<String> {
        let id = entry.id.clone();
        self.entries.push(entry.clone());
        Ok(id)
    }

    fn retrieve(&self, query: &MemoryQuery) -> anyhow::Result<Vec<MemoryEntry>> {
        let query_lower = query.query_text.to_lowercase();
        let results = self
            .entries
            .iter()
            .filter(|e| {
                if !query_lower.is_empty()
                    && !e.content.to_lowercase().contains(&query_lower)
                {
                    return false;
                }
                if let Some(ref sid) = query.session_id
                    && e.session_id != *sid {
                        return false;
                    }
                if !query.memory_types.is_empty()
                    && !query.memory_types.contains(&e.memory_type)
                {
                    return false;
                }
                true
            })
            .take(if query.limit > 0 { query.limit } else { 20 })
            .cloned()
            .collect();
        Ok(results)
    }

    fn delete(&mut self, entry_id: &str) -> anyhow::Result<bool> {
        let before = self.entries.len();
        self.entries.retain(|e| e.id != entry_id);
        Ok(self.entries.len() < before)
    }

    fn list_sessions(&self) -> anyhow::Result<Vec<SessionInfo>> {
        use std::collections::{HashMap, HashSet};
        let mut sessions: HashMap<String, (SessionInfo, HashSet<String>)> = HashMap::new();
        for e in &self.entries {
            let (info, seen_agents) = sessions
                .entry(e.session_id.clone())
                .or_insert_with(|| {
                    (
                        SessionInfo {
                            session_id: e.session_id.clone(),
                            agent_ids: Vec::new(),
                            memory_count: 0,
                            created_at: e.created_at,
                            last_accessed: e.accessed_at,
                        },
                        HashSet::new(),
                    )
                });
            info.memory_count += 1;
            if seen_agents.insert(e.agent_id.clone()) {
                info.agent_ids.push(e.agent_id.clone());
            }
            if e.accessed_at > info.last_accessed {
                info.last_accessed = e.accessed_at;
            }
        }
        Ok(sessions.into_values().map(|(info, _)| info).collect())
    }

    fn health_check(&self) -> anyhow::Result<BackendHealth> {
        Ok(BackendHealth {
            healthy: true,
            backend_name: "in_memory".to_string(),
            latency_ms: 0.0,
            entry_count: self.entries.len(),
            details: "in-memory store is always healthy".to_string(),
        })
    }

    fn backend_name(&self) -> &str {
        "in_memory"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{MemoryEntry, MemoryQuery, MemoryType};

    #[test]
    fn in_memory_backend_store_and_retrieve() {
        let mut backend = InMemoryBackend::new();
        let entry = MemoryEntry::new("s1", "a1", MemoryType::Semantic, "test content");
        let id = backend.store(&entry).unwrap();
        assert!(!id.is_empty());
        let results = backend.retrieve(&MemoryQuery::new("test")).unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn in_memory_backend_delete() {
        let mut backend = InMemoryBackend::new();
        let entry = MemoryEntry::new("s1", "a1", MemoryType::Semantic, "delete me");
        let id = backend.store(&entry).unwrap();
        assert!(backend.delete(&id).unwrap());
        assert!(!backend.delete(&id).unwrap());
    }

    #[test]
    fn in_memory_backend_health() {
        let backend = InMemoryBackend::new();
        let health = backend.health_check().unwrap();
        assert!(health.healthy);
        assert_eq!(health.backend_name, "in_memory");
    }

    #[test]
    fn in_memory_backend_name() {
        let backend = InMemoryBackend::new();
        assert_eq!(backend.backend_name(), "in_memory");
    }
}
