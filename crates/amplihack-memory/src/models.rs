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

    /// Convert to a HashMap (matches Python `MemoryEntry.to_dict()`).
    pub fn to_dict(&self) -> HashMap<String, serde_json::Value> {
        let mut map = HashMap::new();
        map.insert("id".into(), serde_json::json!(self.id));
        map.insert("session_id".into(), serde_json::json!(self.session_id));
        map.insert("agent_id".into(), serde_json::json!(self.agent_id));
        map.insert(
            "memory_type".into(),
            serde_json::json!(self.memory_type.as_str()),
        );
        map.insert("title".into(), serde_json::json!(self.title));
        map.insert("content".into(), serde_json::json!(self.content));
        map.insert("metadata".into(), serde_json::json!(self.metadata));
        map.insert("created_at".into(), serde_json::json!(self.created_at));
        map.insert("accessed_at".into(), serde_json::json!(self.accessed_at));
        map.insert("tags".into(), serde_json::json!(self.tags));
        map.insert("importance".into(), serde_json::json!(self.importance));
        map
    }

    /// Construct from a HashMap (matches Python `MemoryEntry.from_dict()`).
    pub fn from_dict(map: &HashMap<String, serde_json::Value>) -> anyhow::Result<Self> {
        let json_val =
            serde_json::Value::Object(map.iter().map(|(k, v)| (k.clone(), v.clone())).collect());
        serde_json::from_value(json_val).map_err(Into::into)
    }

    /// Serialize to JSON string (matches Python `MemoryEntry.to_json()`).
    pub fn to_json(&self) -> anyhow::Result<String> {
        serde_json::to_string(self).map_err(Into::into)
    }

    /// Deserialize from JSON string (matches Python `MemoryEntry.from_json()`).
    pub fn from_json(json: &str) -> anyhow::Result<Self> {
        serde_json::from_str(json).map_err(Into::into)
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

impl SessionInfo {
    /// Convert to a HashMap (matches Python `SessionInfo.to_dict()`).
    pub fn to_dict(&self) -> HashMap<String, serde_json::Value> {
        let mut map = HashMap::new();
        map.insert("session_id".into(), serde_json::json!(self.session_id));
        map.insert("agent_ids".into(), serde_json::json!(self.agent_ids));
        map.insert("memory_count".into(), serde_json::json!(self.memory_count));
        map.insert("created_at".into(), serde_json::json!(self.created_at));
        map.insert(
            "last_accessed".into(),
            serde_json::json!(self.last_accessed),
        );
        map
    }
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

    /// Build a SQL WHERE clause from this query (matches Python `MemoryQuery.to_sql_where()`).
    pub fn to_sql_where(&self) -> (String, Vec<String>) {
        let mut clauses = Vec::new();
        let mut params = Vec::new();

        if let Some(ref sid) = self.session_id {
            clauses.push("session_id = ?".to_string());
            params.push(sid.clone());
        }
        if let Some(ref aid) = self.agent_id {
            clauses.push("agent_id = ?".to_string());
            params.push(aid.clone());
        }
        if !self.memory_types.is_empty() {
            let placeholders: Vec<_> = self.memory_types.iter().map(|_| "?").collect();
            clauses.push(format!("memory_type IN ({})", placeholders.join(", ")));
            for mt in &self.memory_types {
                params.push(mt.as_str().to_string());
            }
        }
        if !self.tags.is_empty() {
            for tag in &self.tags {
                clauses.push("tags LIKE ?".to_string());
                params.push(format!("%{tag}%"));
            }
        }
        if !self.query_text.is_empty() {
            clauses.push("content LIKE ?".to_string());
            params.push(format!("%{}%", self.query_text));
        }

        let where_clause = if clauses.is_empty() {
            "1=1".to_string()
        } else {
            clauses.join(" AND ")
        };
        (where_clause, params)
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

    #[test]
    fn entry_to_dict_roundtrip() {
        let e = MemoryEntry::new("s1", "a1", MemoryType::Semantic, "knowledge");
        let dict = e.to_dict();
        assert_eq!(dict["content"], serde_json::json!("knowledge"));
        assert_eq!(dict["memory_type"], serde_json::json!("semantic"));
        assert_eq!(dict["session_id"], serde_json::json!("s1"));
    }

    #[test]
    fn entry_to_json_from_json() {
        let e = MemoryEntry::new("s1", "a1", MemoryType::Episodic, "event happened");
        let json = e.to_json().unwrap();
        let e2 = MemoryEntry::from_json(&json).unwrap();
        assert_eq!(e.id, e2.id);
        assert_eq!(e.content, e2.content);
        assert_eq!(e.memory_type, e2.memory_type);
    }

    #[test]
    fn session_info_to_dict() {
        let si = SessionInfo {
            session_id: "s1".into(),
            agent_ids: vec!["a1".into()],
            memory_count: 5,
            created_at: 1000.0,
            last_accessed: 2000.0,
        };
        let dict = si.to_dict();
        assert_eq!(dict["session_id"], serde_json::json!("s1"));
        assert_eq!(dict["memory_count"], serde_json::json!(5));
    }

    #[test]
    fn query_to_sql_where_empty() {
        let q = MemoryQuery::new("");
        let (clause, params) = q.to_sql_where();
        assert_eq!(clause, "1=1");
        assert!(params.is_empty());
    }

    #[test]
    fn query_to_sql_where_with_filters() {
        let q = MemoryQuery::new("test")
            .with_session("s1")
            .with_types(vec![MemoryType::Semantic]);
        let (clause, params) = q.to_sql_where();
        assert!(clause.contains("session_id = ?"));
        assert!(clause.contains("memory_type IN (?)"));
        assert!(clause.contains("content LIKE ?"));
        assert_eq!(params.len(), 3);
        assert_eq!(params[0], "s1");
        assert_eq!(params[1], "semantic");
        assert!(params[2].contains("test"));
    }
}
