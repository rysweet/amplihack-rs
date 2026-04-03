//! Multi-stage storage pipeline.
//!
//! Stages: validate → deduplicate → classify → store.
//! Each stage inspects or transforms the memory entry before
//! final persistence, rejecting invalid or duplicate content.

use crate::models::{MemoryEntry, MemoryType};
use crate::quality::is_trivial;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

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
    seen_fingerprints: HashSet<u64>,
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
            seen_fingerprints: HashSet::new(),
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
            seen_fingerprints: HashSet::new(),
        }
    }

    /// Override the pipeline stages.
    pub fn with_stages(mut self, stages: Vec<StorageStage>) -> Self {
        self.stages = stages;
        self
    }

    /// Execute the storage pipeline for a memory entry.
    pub fn execute(&mut self, entry: &MemoryEntry) -> anyhow::Result<StorageResult> {
        let mut stages_applied = Vec::new();
        let mut memory_type = entry.memory_type;

        for &stage in &self.stages.clone() {
            match stage {
                StorageStage::Validate => {
                    if let Err(reason) = self.validate(entry) {
                        let mut result = StorageResult::rejected(memory_type, reason);
                        stages_applied.push(StorageStage::Validate);
                        result.stages_applied = stages_applied;
                        return Ok(result);
                    }
                    stages_applied.push(StorageStage::Validate);
                }
                StorageStage::Deduplicate => {
                    if self.config.enable_dedup
                        && let Some(existing_id) = self.check_duplicate(entry)
                    {
                        let reason = RejectionReason::Duplicate { existing_id };
                        let mut result = StorageResult::rejected(memory_type, reason);
                        stages_applied.push(StorageStage::Deduplicate);
                        result.stages_applied = stages_applied;
                        return Ok(result);
                    }
                    stages_applied.push(StorageStage::Deduplicate);
                }
                StorageStage::Classify => {
                    if self.config.enable_classification {
                        memory_type = self.classify(entry);
                    }
                    stages_applied.push(StorageStage::Classify);
                }
                StorageStage::Store => {
                    stages_applied.push(StorageStage::Store);
                }
            }
        }

        let importance = crate::quality::score_importance(&entry.content, memory_type);
        let mut result = StorageResult::accepted(entry.id.clone(), memory_type, importance);
        result.stages_applied = stages_applied;
        Ok(result)
    }

    /// Run only the validation stage.
    pub fn validate(&self, entry: &MemoryEntry) -> Result<(), RejectionReason> {
        let len = entry.content.trim().len();
        if len < self.config.min_content_length {
            return Err(RejectionReason::TooShort {
                min_length: self.config.min_content_length,
                actual: len,
            });
        }
        if entry.content.len() > self.config.max_content_length {
            return Err(RejectionReason::TooLarge {
                max_length: self.config.max_content_length,
                actual: entry.content.len(),
            });
        }
        if self.config.trivial_filter && is_trivial(&entry.content, self.config.min_content_length)
        {
            return Err(RejectionReason::TrivialContent);
        }
        Ok(())
    }

    /// Run only the deduplication check.
    /// Returns the existing entry's fingerprint as hex if duplicate found.
    pub fn check_duplicate(&self, entry: &MemoryEntry) -> Option<String> {
        let fp = entry.content_fingerprint();
        if self.seen_fingerprints.contains(&fp) {
            Some(format!("fp:{fp:016x}"))
        } else {
            None
        }
    }

    /// Run only the classification stage.
    ///
    /// Infers memory type from content heuristics when the entry
    /// has a generic type.
    pub fn classify(&self, entry: &MemoryEntry) -> MemoryType {
        let content = &entry.content;
        let lower = content.to_lowercase();

        // If the entry already has a non-semantic cognitive type, keep it
        if entry.memory_type != MemoryType::Semantic && entry.memory_type.is_cognitive() {
            return entry.memory_type;
        }

        // Heuristic classification
        if lower.contains("how to") || lower.contains("step ") || lower.contains("procedure") {
            return MemoryType::Procedural;
        }
        if lower.contains("todo") || lower.contains("reminder") || lower.contains("plan to") {
            return MemoryType::Prospective;
        }
        if lower.contains("fn ") || lower.contains("def ") || lower.contains("class ") {
            return MemoryType::CodeContext;
        }

        entry.memory_type
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
