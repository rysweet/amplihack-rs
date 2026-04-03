//! High-level MemoryFacade — simplified API over coordinator + backend.
//!
//! Provides three main methods: store_memory(), recall(), forget().
//! Automatically selects the best available backend.

use crate::backend::MemoryBackend;
use crate::config::MemoryConfig;
use crate::models::{MemoryEntry, MemoryType, SessionInfo};

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
    pub fn auto(_config: MemoryConfig) -> anyhow::Result<Self> {
        todo!("auto-detect backend")
    }

    /// Store a memory. Returns the entry ID.
    pub fn store_memory(
        &mut self,
        _content: &str,
        _options: StoreOptions,
    ) -> anyhow::Result<String> {
        todo!("store_memory")
    }

    /// Recall memories matching a query string.
    pub fn recall(
        &self,
        _query: &str,
        _options: RecallOptions,
    ) -> anyhow::Result<Vec<MemoryEntry>> {
        todo!("recall")
    }

    /// Forget (delete) a memory by ID. Returns true if it existed.
    pub fn forget(&mut self, _entry_id: &str) -> anyhow::Result<bool> {
        todo!("forget")
    }

    /// List all sessions known to the backend.
    pub fn list_sessions(&self) -> anyhow::Result<Vec<SessionInfo>> {
        todo!("list_sessions")
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
