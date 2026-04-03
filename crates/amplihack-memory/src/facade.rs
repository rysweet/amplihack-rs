//! High-level MemoryFacade — simplified API over coordinator + backend.
//!
//! Provides three main methods: store_memory(), recall(), forget().
//! Automatically selects the best available backend.

use crate::auto_backend::{create_backend, detect_backend};
use crate::backend::MemoryBackend;
use crate::config::MemoryConfig;
use crate::models::{MemoryEntry, MemoryQuery, MemoryType, SessionInfo};
use crate::quality::score_importance;

/// Options for storing a memory through the facade.
#[derive(Debug, Clone)]
pub struct StoreOptions {
    pub memory_type: MemoryType,
    pub session_id: String,
    pub agent_id: String,
    pub importance: Option<f64>,
    pub tags: Vec<String>,
}

impl Default for StoreOptions {
    fn default() -> Self {
        Self {
            memory_type: MemoryType::Semantic,
            session_id: "default".to_string(),
            agent_id: "default".to_string(),
            importance: None,
            tags: Vec::new(),
        }
    }
}

impl StoreOptions {
    pub fn new(memory_type: MemoryType, session_id: impl Into<String>) -> Self {
        Self {
            memory_type,
            session_id: session_id.into(),
            ..Default::default()
        }
    }
}

/// Options for recalling memories through the facade.
#[derive(Debug, Clone)]
pub struct RecallOptions {
    pub session_id: Option<String>,
    pub memory_types: Vec<MemoryType>,
    pub token_budget: usize,
    pub limit: usize,
}

impl Default for RecallOptions {
    fn default() -> Self {
        Self {
            session_id: None,
            memory_types: Vec::new(),
            token_budget: 4000,
            limit: 20,
        }
    }
}

/// Simplified high-level memory API.
///
/// Wraps a backend with configuration, providing store_memory / recall / forget.
pub struct MemoryFacade {
    backend: Box<dyn MemoryBackend>,
    config: MemoryConfig,
}

impl MemoryFacade {
    /// Create a facade with a specific backend.
    pub fn new(backend: Box<dyn MemoryBackend>, config: MemoryConfig) -> Self {
        Self { backend, config }
    }

    /// Create a facade with auto-detected backend.
    pub fn auto(config: MemoryConfig) -> anyhow::Result<Self> {
        let detected = detect_backend(&config);
        let backend = create_backend(&detected)?;
        Ok(Self { backend, config })
    }

    /// Store a memory. Returns the entry ID.
    pub fn store_memory(&mut self, content: &str, options: StoreOptions) -> anyhow::Result<String> {
        let mut entry = MemoryEntry::new(
            &options.session_id,
            &options.agent_id,
            options.memory_type,
            content,
        );
        entry.importance = options
            .importance
            .unwrap_or_else(|| score_importance(content, options.memory_type));
        entry.tags = options.tags;
        self.backend.store(&entry)
    }

    /// Recall memories matching a query string.
    ///
    /// Results are truncated to fit within `options.token_budget` (estimated as
    /// `content.len() / 4` tokens per entry).
    pub fn recall(&self, query: &str, options: RecallOptions) -> anyhow::Result<Vec<MemoryEntry>> {
        let q = MemoryQuery {
            query_text: query.to_string(),
            session_id: options.session_id,
            memory_types: options.memory_types,
            token_budget: options.token_budget,
            limit: options.limit,
            ..Default::default()
        };
        let results = self.backend.retrieve(&q)?;

        // Enforce token_budget: estimate tokens as content.len() / 4.
        let mut budget_remaining = options.token_budget;
        let mut truncated = Vec::new();
        for entry in results {
            let estimated_tokens = entry.content.len() / 4;
            if estimated_tokens > budget_remaining {
                break;
            }
            budget_remaining -= estimated_tokens;
            truncated.push(entry);
        }
        Ok(truncated)
    }

    /// Forget (delete) a memory by ID. Returns true if it existed.
    pub fn forget(&mut self, entry_id: &str) -> anyhow::Result<bool> {
        self.backend.delete(entry_id)
    }

    /// List all sessions known to the backend.
    pub fn list_sessions(&self) -> anyhow::Result<Vec<SessionInfo>> {
        self.backend.list_sessions()
    }

    /// Get the name of the active backend.
    pub fn backend_name(&self) -> &str {
        self.backend.backend_name()
    }

    /// Get a reference to the config.
    pub fn config(&self) -> &MemoryConfig {
        &self.config
    }
}
