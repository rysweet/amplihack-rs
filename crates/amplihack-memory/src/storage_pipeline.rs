//! Multi-stage storage pipeline.
//!
//! Stages: validate → deduplicate → classify → store.
//! Each stage inspects or transforms the memory entry before
//! final persistence, rejecting invalid or duplicate content.

use crate::models::{MemoryEntry, MemoryType};
use serde::{Deserialize, Serialize};

/// Stages in the storage pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StorageStage {
    /// Validate entry content and metadata.
    Validate,
    /// Check for duplicate content.
    Deduplicate,
    /// Classify or reclassify memory type.
    Classify,
    /// Persist to backend.
    Store,
}

impl StorageStage {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Validate => "validate",
            Self::Deduplicate => "deduplicate",
            Self::Classify => "classify",
            Self::Store => "store",
        }
    }
}

/// Reason a storage operation was rejected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RejectionReason {
    /// Content was empty or too short.
    TooShort { min_length: usize, actual: usize },
    /// Content was a trivial phrase.
    TrivialContent,
    /// Duplicate of an existing entry.
    Duplicate { existing_id: String },
    /// Content exceeded maximum size.
    TooLarge { max_length: usize, actual: usize },
    /// Invalid memory type for this content.
    InvalidType,
    /// Custom rejection reason.
    Custom(String),
}

/// Result of a storage pipeline execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageResult {
    pub accepted: bool,
    pub entry_id: Option<String>,
    pub final_memory_type: MemoryType,
    pub stages_applied: Vec<StorageStage>,
    #[serde(skip)]
    pub rejection_reason: Option<RejectionReason>,
    pub importance_score: f64,
}

impl StorageResult {
    pub fn accepted(entry_id: String, memory_type: MemoryType, importance: f64) -> Self {
        Self {
            accepted: true,
            entry_id: Some(entry_id),
            final_memory_type: memory_type,
            stages_applied: Vec::new(),
            rejection_reason: None,
            importance_score: importance,
        }
    }

    pub fn rejected(memory_type: MemoryType, reason: RejectionReason) -> Self {
        Self {
            accepted: false,
            entry_id: None,
            final_memory_type: memory_type,
            stages_applied: Vec::new(),
            rejection_reason: Some(reason),
            importance_score: 0.0,
        }
    }
}

/// Configuration for the storage pipeline.
#[derive(Debug, Clone)]
pub struct StoragePipelineConfig {
    pub min_content_length: usize,
    pub max_content_length: usize,
    pub enable_dedup: bool,
    pub enable_classification: bool,
    pub trivial_filter: bool,
}

impl Default for StoragePipelineConfig {
    fn default() -> Self {
        Self {
            min_content_length: 10,
            max_content_length: 100_000,
            enable_dedup: true,
            enable_classification: true,
            trivial_filter: true,
        }
    }
}

/// Multi-stage storage pipeline.
pub struct StoragePipeline {
    config: StoragePipelineConfig,
    stages: Vec<StorageStage>,
}

impl StoragePipeline {
    /// Create a pipeline with default stage ordering.
    pub fn new() -> Self {
        Self {
            config: StoragePipelineConfig::default(),
            stages: vec![
                StorageStage::Validate,
                StorageStage::Deduplicate,
                StorageStage::Classify,
                StorageStage::Store,
            ],
        }
    }

    /// Create a pipeline with custom configuration.
    pub fn with_config(config: StoragePipelineConfig) -> Self {
        Self {
            config,
            stages: vec![
                StorageStage::Validate,
                StorageStage::Deduplicate,
                StorageStage::Classify,
                StorageStage::Store,
            ],
        }
    }

    /// Override the pipeline stages.
    pub fn with_stages(mut self, stages: Vec<StorageStage>) -> Self {
        self.stages = stages;
        self
    }

    /// Execute the storage pipeline for a memory entry.
    pub fn execute(&mut self, _entry: &MemoryEntry) -> anyhow::Result<StorageResult> {
        todo!("storage pipeline execution")
    }

    /// Run only the validation stage.
    pub fn validate(&self, _entry: &MemoryEntry) -> Result<(), RejectionReason> {
        todo!("validate stage")
    }

    /// Run only the deduplication check.
    pub fn check_duplicate(&self, _entry: &MemoryEntry) -> Option<String> {
        todo!("dedup check")
    }

    /// Run only the classification stage.
    pub fn classify(&self, _entry: &MemoryEntry) -> MemoryType {
        todo!("classify stage")
    }

    /// Get the configured stages.
    pub fn stages(&self) -> &[StorageStage] {
        &self.stages
    }

    /// Get the config.
    pub fn config(&self) -> &StoragePipelineConfig {
        &self.config
    }
}

impl Default for StoragePipeline {
    fn default() -> Self {
        Self::new()
    }
}
