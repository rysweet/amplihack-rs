//! Memory manager — high-level lifecycle interface for agents.
//!
//! Port of Python `amplihack/memory/manager.py`.
//! Provides store/retrieve/update/delete with automatic session management,
//! batch operations, and convenience query methods.

use crate::backend::MemoryBackend;
use crate::models::{MemoryEntry, MemoryQuery, MemoryType, SessionInfo};
use std::collections::HashMap;
use tracing::warn;

/// High-level memory manager with session affinity.
///
/// Wraps a `MemoryBackend` and pins all operations to a single session.
pub struct MemoryManager {
    backend: Box<dyn MemoryBackend>,
    session_id: String,
}

impl MemoryManager {
    /// Create a manager bound to a given session.
    pub fn new(backend: Box<dyn MemoryBackend>, session_id: impl Into<String>) -> Self {
        Self {
            backend,
            session_id: session_id.into(),
        }
    }

    /// Create a manager with an auto-generated session ID.
    pub fn with_auto_session(backend: Box<dyn MemoryBackend>) -> Self {
        Self {
            backend,
            session_id: generate_session_id(),
        }
    }

    /// The active session ID.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Store a memory entry. Returns the entry ID.
    pub fn store(&mut self, request: StoreRequest) -> anyhow::Result<String> {
        let mut entry = MemoryEntry::new(
            &self.session_id,
            &request.agent_id,
            request.memory_type,
            &request.content,
        );
        entry.title = request.title;
        entry.tags = request.tags;
        if let Some(imp) = request.importance {
            entry.importance = imp;
        }
        entry.metadata = request.metadata;
        self.backend.store(&entry)
    }

    /// Retrieve memories matching criteria.
    pub fn retrieve(&self, criteria: RetrieveCriteria) -> anyhow::Result<Vec<MemoryEntry>> {
        let query = MemoryQuery {
            query_text: criteria.search.unwrap_or_default(),
            session_id: if criteria.include_other_sessions {
                None
            } else {
                Some(self.session_id.clone())
            },
            agent_id: criteria.agent_id,
            memory_types: criteria
                .memory_type
                .map(|mt| vec![mt])
                .unwrap_or_default(),
            tags: criteria.tags,
            token_budget: 0,
            limit: criteria.limit.unwrap_or(20),
            ..Default::default()
        };
        self.backend.retrieve(&query)
    }

    /// Get a specific memory by ID (within current session).
    pub fn get(&self, memory_id: &str) -> anyhow::Result<Option<MemoryEntry>> {
        let query = MemoryQuery {
            query_text: String::new(),
            session_id: Some(self.session_id.clone()),
            limit: 100,
            ..Default::default()
        };
        let entries = self.backend.retrieve(&query)?;
        Ok(entries.into_iter().find(|e| e.id == memory_id))
    }

    /// Update an existing memory. Returns true if updated.
    pub fn update(&mut self, memory_id: &str, update: UpdateRequest) -> anyhow::Result<bool> {
        let Some(mut entry) = self.get(memory_id)? else {
            return Ok(false);
        };

        if let Some(title) = update.title {
            entry.title = title;
        }
        if let Some(content) = update.content {
            entry.content = content;
        }
        if let Some(metadata) = update.metadata {
            entry.metadata = metadata;
        }
        if let Some(tags) = update.tags {
            entry.tags = tags;
        }
        if let Some(importance) = update.importance {
            entry.importance = importance;
        }

        // Touch access time
        entry.accessed_at = now_epoch();
        // Remove old version before re-storing
        let _ = self.backend.delete(memory_id);
        self.backend.store(&entry)?;
        Ok(true)
    }

    /// Delete a memory by ID (session-scoped).
    pub fn delete(&mut self, memory_id: &str) -> anyhow::Result<bool> {
        // Verify ownership
        if self.get(memory_id)?.is_none() {
            return Ok(false);
        }
        self.backend.delete(memory_id)
    }

    /// Store multiple memories. Returns IDs (None for failed items).
    pub fn store_batch(
        &mut self,
        requests: Vec<StoreRequest>,
    ) -> Vec<Option<String>> {
        requests
            .into_iter()
            .map(|req| {
                let title = req.title.clone();
                match self.store(req) {
                    Ok(id) => Some(id),
                    Err(e) => {
                        warn!("batch store failed for '{}': {e}", title);
                        None
                    }
                }
            })
            .collect()
    }

    /// Search memories by text.
    pub fn search(
        &self,
        query: &str,
        agent_id: Option<String>,
        limit: usize,
    ) -> anyhow::Result<Vec<MemoryEntry>> {
        self.retrieve(RetrieveCriteria {
            search: Some(query.to_string()),
            agent_id,
            limit: Some(limit),
            ..Default::default()
        })
    }

    /// Get recent memories.
    pub fn get_recent(
        &self,
        agent_id: Option<String>,
        limit: usize,
    ) -> anyhow::Result<Vec<MemoryEntry>> {
        self.retrieve(RetrieveCriteria {
            agent_id,
            limit: Some(limit),
            ..Default::default()
        })
    }

    /// Get high-importance memories.
    pub fn get_important(
        &self,
        min_importance: f64,
        limit: usize,
    ) -> anyhow::Result<Vec<MemoryEntry>> {
        let all = self.retrieve(RetrieveCriteria {
            limit: Some(limit * 5),
            ..Default::default()
        })?;
        Ok(all
            .into_iter()
            .filter(|e| e.importance >= min_importance)
            .take(limit)
            .collect())
    }

    /// List all sessions from the backend.
    pub fn list_sessions(&self) -> anyhow::Result<Vec<SessionInfo>> {
        self.backend.list_sessions()
    }

    /// List available memory type names.
    pub fn list_memory_types() -> Vec<&'static str> {
        vec![
            "episodic",
            "semantic",
            "procedural",
            "prospective",
            "working",
            "strategic",
            "code_context",
            "project_structure",
            "user_preference",
            "error_pattern",
            "conversation",
            "task",
        ]
    }

    /// Get the name of the underlying backend.
    pub fn backend_name(&self) -> &str {
        self.backend.backend_name()
    }
}

// ── Request / criteria types ──

/// Parameters for storing a memory.
#[derive(Debug, Clone)]
pub struct StoreRequest {
    pub agent_id: String,
    pub title: String,
    pub content: String,
    pub memory_type: MemoryType,
    pub metadata: HashMap<String, serde_json::Value>,
    pub tags: Vec<String>,
    pub importance: Option<f64>,
}

impl StoreRequest {
    pub fn new(
        agent_id: impl Into<String>,
        title: impl Into<String>,
        content: impl Into<String>,
        memory_type: MemoryType,
    ) -> Self {
        Self {
            agent_id: agent_id.into(),
            title: title.into(),
            content: content.into(),
            memory_type,
            metadata: HashMap::new(),
            tags: Vec::new(),
            importance: None,
        }
    }
}

/// Parameters for retrieving memories.
#[derive(Debug, Clone, Default)]
pub struct RetrieveCriteria {
    pub agent_id: Option<String>,
    pub memory_type: Option<MemoryType>,
    pub tags: Vec<String>,
    pub search: Option<String>,
    pub limit: Option<usize>,
    pub include_other_sessions: bool,
}

/// Parameters for updating a memory.
#[derive(Debug, Clone, Default)]
pub struct UpdateRequest {
    pub title: Option<String>,
    pub content: Option<String>,
    pub metadata: Option<HashMap<String, serde_json::Value>>,
    pub tags: Option<Vec<String>>,
    pub importance: Option<f64>,
}

// ── helpers ──

fn generate_session_id() -> String {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let uid = &uuid::Uuid::new_v4().to_string()[..8];
    format!("session_{ts}_{uid}")
}

fn now_epoch() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

#[cfg(test)]
#[path = "tests/manager_tests.rs"]
mod tests;
