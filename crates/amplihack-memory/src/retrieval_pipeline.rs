//! Multi-stage retrieval pipeline.
//!
//! Stages: filter → rank → deduplicate → budget enforcement.
//! Each stage transforms a set of scored entries, producing the
//! final retrieval result within token budget constraints.

use crate::models::{MemoryEntry, MemoryQuery};
use crate::quality::{matches_query, relevance_score};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// A memory entry with an associated relevance score.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoredEntry {
    pub entry: MemoryEntry,
    pub score: f64,
    pub stage_scores: Vec<(String, f64)>,
}

impl ScoredEntry {
    pub fn new(entry: MemoryEntry, score: f64) -> Self {
        Self {
            entry,
            score,
            stage_scores: Vec::new(),
        }
    }

    pub fn with_stage_score(mut self, stage: &str, score: f64) -> Self {
        self.stage_scores.push((stage.to_string(), score));
        self
    }

    /// Estimated token count (~4 chars per token).
    pub fn estimated_tokens(&self) -> usize {
        (self.entry.content.len() + self.entry.title.len()) / 4
    }
}

/// Stages in the retrieval pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RetrievalStage {
    /// Filter entries by query criteria.
    Filter,
    /// Rank entries by relevance score.
    Rank,
    /// Remove duplicate or near-duplicate entries.
    Deduplicate,
    /// Enforce token budget limits.
    BudgetEnforce,
}

impl RetrievalStage {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Filter => "filter",
            Self::Rank => "rank",
            Self::Deduplicate => "deduplicate",
            Self::BudgetEnforce => "budget_enforce",
        }
    }
}

/// Result of a retrieval pipeline execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalResult {
    pub entries: Vec<ScoredEntry>,
    pub total_candidates: usize,
    pub total_tokens_used: usize,
    pub stages_applied: Vec<RetrievalStage>,
    pub truncated: bool,
}

/// Configuration for the retrieval pipeline.
#[derive(Debug, Clone)]
pub struct RetrievalPipelineConfig {
    /// Similarity threshold for dedup (reserved for future fuzzy dedup).
    /// Currently dedup uses exact fingerprint matching.
    pub dedup_similarity_threshold: f64,
    pub min_score_threshold: f64,
    pub max_results: usize,
}

impl Default for RetrievalPipelineConfig {
    fn default() -> Self {
        Self {
            dedup_similarity_threshold: 0.85,
            min_score_threshold: 0.0,
            max_results: 50,
        }
    }
}

/// Multi-stage retrieval pipeline.
pub struct RetrievalPipeline {
    config: RetrievalPipelineConfig,
    stages: Vec<RetrievalStage>,
}

impl RetrievalPipeline {
    /// Create a pipeline with the default stage ordering.
    pub fn new() -> Self {
        Self {
            config: RetrievalPipelineConfig::default(),
            stages: vec![
                RetrievalStage::Filter,
                RetrievalStage::Rank,
                RetrievalStage::Deduplicate,
                RetrievalStage::BudgetEnforce,
            ],
        }
    }

    /// Create a pipeline with custom configuration.
    pub fn with_config(config: RetrievalPipelineConfig) -> Self {
        Self {
            config,
            stages: vec![
                RetrievalStage::Filter,
                RetrievalStage::Rank,
                RetrievalStage::Deduplicate,
                RetrievalStage::BudgetEnforce,
            ],
        }
    }

    /// Override the pipeline stages.
    pub fn with_stages(mut self, stages: Vec<RetrievalStage>) -> Self {
        self.stages = stages;
        self
    }

    /// Execute the retrieval pipeline against a set of candidate entries.
    pub fn execute(
        &self,
        candidates: Vec<MemoryEntry>,
        query: &MemoryQuery,
    ) -> anyhow::Result<RetrievalResult> {
        let total_candidates = candidates.len();

        // Initial scoring
        let query_words: HashSet<&str> = query.query_text.split_whitespace().collect();
        let mut scored: Vec<ScoredEntry> = candidates
            .into_iter()
            .map(|entry| {
                let score = relevance_score(&entry, &query_words);
                ScoredEntry::new(entry, score)
            })
            .collect();

        let mut stages_applied = Vec::new();

        for &stage in &self.stages {
            match stage {
                RetrievalStage::Filter => {
                    scored = self.filter(scored, query);
                    stages_applied.push(RetrievalStage::Filter);
                }
                RetrievalStage::Rank => {
                    scored = self.rank(scored, query);
                    stages_applied.push(RetrievalStage::Rank);
                }
                RetrievalStage::Deduplicate => {
                    scored = self.deduplicate(scored);
                    stages_applied.push(RetrievalStage::Deduplicate);
                }
                RetrievalStage::BudgetEnforce => {
                    let budget = if query.token_budget > 0 {
                        query.token_budget
                    } else {
                        4000
                    };
                    let (entries, truncated_flag) = self.enforce_budget(scored, budget);
                    scored = entries;
                    stages_applied.push(RetrievalStage::BudgetEnforce);
                    let tokens_used: usize = scored.iter().map(|s| s.estimated_tokens()).sum();
                    return Ok(RetrievalResult {
                        entries: scored,
                        total_candidates,
                        total_tokens_used: tokens_used,
                        stages_applied,
                        truncated: truncated_flag,
                    });
                }
            }
        }

        let tokens_used: usize = scored.iter().map(|s| s.estimated_tokens()).sum();
        Ok(RetrievalResult {
            entries: scored,
            total_candidates,
            total_tokens_used: tokens_used,
            stages_applied,
            truncated: false,
        })
    }

    /// Run only the filter stage.
    pub fn filter(
        &self,
        candidates: Vec<ScoredEntry>,
        query: &MemoryQuery,
    ) -> Vec<ScoredEntry> {
        candidates
            .into_iter()
            .filter(|s| {
                matches_query(&s.entry, query) && s.score >= self.config.min_score_threshold
            })
            .take(self.config.max_results)
            .map(|s| {
                let filter_score = s.score;
                s.with_stage_score("filter", filter_score)
            })
            .collect()
    }

    /// Run only the rank stage.
    pub fn rank(&self, mut entries: Vec<ScoredEntry>, query: &MemoryQuery) -> Vec<ScoredEntry> {
        let query_words: HashSet<&str> = query.query_text.split_whitespace().collect();

        for entry in &mut entries {
            let new_score = relevance_score(&entry.entry, &query_words);
            entry.score = new_score;
            entry
                .stage_scores
                .push(("rank".to_string(), new_score));
        }

        entries.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        entries
    }

    /// Run only the dedup stage.
    ///
    /// Uses content fingerprinting to remove near-duplicates.
    pub fn deduplicate(&self, entries: Vec<ScoredEntry>) -> Vec<ScoredEntry> {
        let mut seen = HashSet::new();
        entries
            .into_iter()
            .filter(|s| seen.insert(s.entry.content_fingerprint()))
            .collect()
    }

    /// Run only the budget enforcement stage.
    ///
    /// Returns `(kept_entries, was_truncated)`.
    pub fn enforce_budget(
        &self,
        entries: Vec<ScoredEntry>,
        token_budget: usize,
    ) -> (Vec<ScoredEntry>, bool) {
        let mut used = 0usize;
        let mut kept = Vec::new();
        let total = entries.len();

        for entry in entries {
            let tokens = entry.estimated_tokens().max(1);
            if used + tokens > token_budget && !kept.is_empty() {
                return (kept, true);
            }
            used += tokens;
            kept.push(entry);
        }

        let truncated = kept.len() < total;
        (kept, truncated)
    }

    /// Get the configured stages.
    pub fn stages(&self) -> &[RetrievalStage] {
        &self.stages
    }

    /// Get the config.
    pub fn config(&self) -> &RetrievalPipelineConfig {
        &self.config
    }
}

impl Default for RetrievalPipeline {
    fn default() -> Self {
        Self::new()
    }
}
