//! Data models for the five-type memory system.
//!
//! Matches Python `amplihack/memory/models.py`:
//! - MemoryType enum (5 cognitive + 6 legacy variants)
//! - MemoryEntry, SessionInfo, MemoryQuery

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// Five cognitive memory types plus legacy variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryType {
    /// Session-specific events and interactions.
    Episodic,
    /// Cross-session knowledge and facts.
    Semantic,
    /// How-to and workflow knowledge.
    Procedural,
    /// Future intentions and reminders.
    Prospective,
    /// Active task context (short-lived, auto-cleared).
    Working,
    /// Goals and strategic plans.
    Strategic,
    // Legacy variants
    /// Legacy: code context memory.
    CodeContext,
    /// Legacy: project structure memory.
    ProjectStructure,
    /// Legacy: user preferences memory.
    UserPreference,
    /// Legacy: error/debugging memory.
    ErrorPattern,
    /// Legacy: conversation history.
    Conversation,
    /// Legacy: task-specific memory.
    Task,
}

impl MemoryType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Episodic => "episodic",
            Self::Semantic => "semantic",
            Self::Procedural => "procedural",
            Self::Prospective => "prospective",
            Self::Working => "working",
            Self::Strategic => "strategic",
            Self::CodeContext => "code_context",
            Self::ProjectStructure => "project_structure",
            Self::UserPreference => "user_preference",
            Self::ErrorPattern => "error_pattern",
            Self::Conversation => "conversation",
            Self::Task => "task",
        }
    }

    /// Table name in graph store for this memory type.
    pub fn table_name(&self) -> &'static str {
        match self {
            Self::Episodic => "episodic_memory",
            Self::Semantic => "semantic_memory",
            Self::Procedural => "procedural_memory",
            Self::Prospective => "prospective_memory",
            Self::Working => "working_memory",
            Self::Strategic => "strategic_memory",
            _ => "semantic_memory",
        }
    }

    /// Whether this is a core cognitive type.
    pub fn is_cognitive(&self) -> bool {
        matches!(
            self,
            Self::Episodic
                | Self::Semantic
                | Self::Procedural
                | Self::Prospective
                | Self::Working
                | Self::Strategic
        )
    }
}

/// A single memory entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub session_id: String,
    pub agent_id: String,
    pub memory_type: MemoryType,
    pub title: String,
    pub content: String,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
    pub created_at: f64,
    pub accessed_at: f64,
    #[serde(default)]
    pub tags: Vec<String>,
    pub importance: f64,
}

impl MemoryEntry {
    pub fn new(
        session_id: impl Into<String>,
        agent_id: impl Into<String>,
        memory_type: MemoryType,
        content: impl Into<String>,
    ) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: session_id.into(),
            agent_id: agent_id.into(),
            memory_type,
            title: String::new(),
            content: content.into(),
            metadata: HashMap::new(),
            created_at: now,
            accessed_at: now,
            tags: Vec::new(),
            importance: 0.5,
        }
    }

    /// Content hash for deduplication (length + prefix + suffix + hash).
    pub fn content_fingerprint(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.content.len().hash(&mut hasher);
        self.content
            .get(..20.min(self.content.len()))
            .hash(&mut hasher);
        let len = self.content.len();
        self.content
            .get(len.saturating_sub(20)..len)
            .hash(&mut hasher);
        self.content.hash(&mut hasher);
        hasher.finish()
    }
}

/// Session metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub agent_ids: Vec<String>,
    pub memory_count: usize,
    pub created_at: f64,
    pub last_accessed: f64,
}

/// Query parameters for memory retrieval.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryQuery {
    pub query_text: String,
    pub session_id: Option<String>,
    pub agent_id: Option<String>,
    pub memory_types: Vec<MemoryType>,
    pub tags: Vec<String>,
    pub token_budget: usize,
    pub limit: usize,
    pub time_range_secs: Option<f64>,
    pub include_code_context: bool,
}

impl MemoryQuery {
    pub fn new(query_text: impl Into<String>) -> Self {
        Self {
            query_text: query_text.into(),
            token_budget: 4000,
            limit: 20,
            ..Default::default()
        }
    }

    pub fn with_session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    pub fn with_budget(mut self, tokens: usize) -> Self {
        self.token_budget = tokens;
        self
    }

    pub fn with_types(mut self, types: Vec<MemoryType>) -> Self {
        self.memory_types = types;
        self
    }
}

/// Storage request for the coordinator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageRequest {
    pub content: String,
    pub memory_type: MemoryType,
    pub session_id: String,
    pub agent_id: String,
    pub context: String,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
    pub importance: Option<f64>,
}

impl StorageRequest {
    pub fn new(
        content: impl Into<String>,
        memory_type: MemoryType,
        session_id: impl Into<String>,
    ) -> Self {
        Self {
            content: content.into(),
            memory_type,
            session_id: session_id.into(),
            agent_id: "default".into(),
            context: String::new(),
            metadata: HashMap::new(),
            importance: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_type_round_trips() {
        let mt = MemoryType::Semantic;
        let json = serde_json::to_string(&mt).unwrap();
        let back: MemoryType = serde_json::from_str(&json).unwrap();
        assert_eq!(back, MemoryType::Semantic);
    }

    #[test]
    fn cognitive_types_identified() {
        assert!(MemoryType::Episodic.is_cognitive());
        assert!(MemoryType::Working.is_cognitive());
        assert!(!MemoryType::CodeContext.is_cognitive());
        assert!(!MemoryType::Task.is_cognitive());
    }

    #[test]
    fn table_names_match_python() {
        assert_eq!(MemoryType::Episodic.table_name(), "episodic_memory");
        assert_eq!(MemoryType::Semantic.table_name(), "semantic_memory");
        assert_eq!(MemoryType::Procedural.table_name(), "procedural_memory");
        assert_eq!(MemoryType::Working.table_name(), "working_memory");
    }

    #[test]
    fn entry_fingerprint_stable() {
        let e = MemoryEntry::new("s1", "a1", MemoryType::Semantic, "hello world");
        let fp1 = e.content_fingerprint();
        let fp2 = e.content_fingerprint();
        assert_eq!(fp1, fp2);
    }

    #[test]
    fn entry_fingerprint_differs_for_different_content() {
        let e1 = MemoryEntry::new("s1", "a1", MemoryType::Semantic, "hello world");
        let e2 = MemoryEntry::new("s1", "a1", MemoryType::Semantic, "goodbye world");
        assert_ne!(e1.content_fingerprint(), e2.content_fingerprint());
    }

    #[test]
    fn query_builder() {
        let q = MemoryQuery::new("test query")
            .with_session("s1")
            .with_budget(2000)
            .with_types(vec![MemoryType::Semantic, MemoryType::Episodic]);
        assert_eq!(q.query_text, "test query");
        assert_eq!(q.session_id, Some("s1".to_string()));
        assert_eq!(q.token_budget, 2000);
        assert_eq!(q.memory_types.len(), 2);
    }

    #[test]
    fn storage_request_defaults() {
        let req = StorageRequest::new("content", MemoryType::Working, "s1");
        assert_eq!(req.agent_id, "default");
        assert!(req.importance.is_none());
        assert!(req.metadata.is_empty());
    }

    #[test]
    fn memory_entry_serializes() {
        let e = MemoryEntry::new("s1", "a1", MemoryType::Procedural, "how to test");
        let json = serde_json::to_value(&e).unwrap();
        assert_eq!(json["memory_type"], "procedural");
        assert!(!json["id"].as_str().unwrap().is_empty());
    }
}
