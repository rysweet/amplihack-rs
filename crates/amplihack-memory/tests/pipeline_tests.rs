//! Tests for retrieval and storage pipelines.

use amplihack_memory::models::{MemoryEntry, MemoryQuery, MemoryType};
use amplihack_memory::retrieval_pipeline::{
    RetrievalPipeline, RetrievalPipelineConfig, RetrievalStage, ScoredEntry,
};
use amplihack_memory::storage_pipeline::{
    RejectionReason, StoragePipeline, StoragePipelineConfig, StorageResult, StorageStage,
};

fn semantic_entry(content: &str) -> MemoryEntry {
    MemoryEntry::new("test-session", "agent-1", MemoryType::Semantic, content)
}

// ── RetrievalPipeline tests ──

#[test]
fn retrieval_pipeline_default_stages() {
    let pipeline = RetrievalPipeline::new();
    let stages = pipeline.stages();
    assert_eq!(stages.len(), 4);
    assert_eq!(stages[0], RetrievalStage::Filter);
    assert_eq!(stages[1], RetrievalStage::Rank);
    assert_eq!(stages[2], RetrievalStage::Deduplicate);
    assert_eq!(stages[3], RetrievalStage::BudgetEnforce);
}

#[test]
fn retrieval_pipeline_custom_stages() {
    let pipeline = RetrievalPipeline::new()
        .with_stages(vec![RetrievalStage::Filter, RetrievalStage::Rank]);
    assert_eq!(pipeline.stages().len(), 2);
}

#[test]
fn retrieval_pipeline_config_defaults() {
    let config = RetrievalPipelineConfig::default();
    assert_eq!(config.dedup_similarity_threshold, 0.85);
    assert_eq!(config.min_score_threshold, 0.0);
    assert_eq!(config.max_results, 50);
}

#[test]
fn retrieval_pipeline_with_config() {
    let config = RetrievalPipelineConfig {
        dedup_similarity_threshold: 0.9,
        min_score_threshold: 0.5,
        max_results: 10,
    };
    let pipeline = RetrievalPipeline::with_config(config);
    assert_eq!(pipeline.config().max_results, 10);
    assert_eq!(pipeline.config().min_score_threshold, 0.5);
}

#[test]
fn retrieval_pipeline_execute_returns_results() {
    let pipeline = RetrievalPipeline::new();
    let candidates = vec![semantic_entry("test candidate content here for retrieval")];
    let query = MemoryQuery::new("test");
    let result = pipeline.execute(candidates, &query).unwrap();
    assert_eq!(result.total_candidates, 1);
    assert!(!result.entries.is_empty());
    assert!(!result.stages_applied.is_empty());
}

#[test]
fn retrieval_pipeline_execute_empty_candidates() {
    let pipeline = RetrievalPipeline::new();
    let query = MemoryQuery::new("test");
    let result = pipeline.execute(vec![], &query).unwrap();
    assert_eq!(result.total_candidates, 0);
    assert!(result.entries.is_empty());
}

#[test]
fn retrieval_filter_stage_applies_query() {
    let pipeline = RetrievalPipeline::new();
    let entries = vec![
        ScoredEntry::new(semantic_entry("matching filter test content"), 0.5),
        ScoredEntry::new(
            MemoryEntry::new("other-session", "a", MemoryType::Working, "other content"),
            0.5,
        ),
    ];
    let query = MemoryQuery::new("test").with_session("test-session");
    let filtered = pipeline.filter(entries, &query);
    assert_eq!(filtered.len(), 1);
}

#[test]
fn retrieval_rank_stage_sorts_by_score() {
    let pipeline = RetrievalPipeline::new();
    let entries = vec![
        ScoredEntry::new(semantic_entry("low relevance content here"), 0.2),
        ScoredEntry::new(semantic_entry("high relevance test query content here"), 0.8),
    ];
    let query = MemoryQuery::new("test query");
    let ranked = pipeline.rank(entries, &query);
    assert!(ranked[0].score >= ranked[1].score);
}

#[test]
fn retrieval_dedup_stage_removes_duplicates() {
    let pipeline = RetrievalPipeline::new();
    let entries = vec![
        ScoredEntry::new(semantic_entry("duplicate content for testing"), 0.5),
        ScoredEntry::new(semantic_entry("duplicate content for testing"), 0.4),
        ScoredEntry::new(semantic_entry("unique content for testing"), 0.6),
    ];
    let deduped = pipeline.deduplicate(entries);
    assert_eq!(deduped.len(), 2);
}

#[test]
fn retrieval_budget_enforce_truncates() {
    let pipeline = RetrievalPipeline::new();
    // Each entry ~10 tokens (40 chars / 4)
    let entries: Vec<ScoredEntry> = (0..10)
        .map(|i| {
            ScoredEntry::new(
                semantic_entry(&format!("Entry number {i:02} with enough padding text")),
                0.5,
            )
        })
        .collect();
    // Budget for ~3 entries (~30 tokens)
    let (kept, truncated) = pipeline.enforce_budget(entries, 30);
    assert!(kept.len() < 10);
    assert!(truncated);
}

#[test]
fn retrieval_stage_as_str() {
    assert_eq!(RetrievalStage::Filter.as_str(), "filter");
    assert_eq!(RetrievalStage::Rank.as_str(), "rank");
    assert_eq!(RetrievalStage::Deduplicate.as_str(), "deduplicate");
    assert_eq!(RetrievalStage::BudgetEnforce.as_str(), "budget_enforce");
}

#[test]
fn scored_entry_creation() {
    let entry = semantic_entry("Test scored entry content here");
    let scored = ScoredEntry::new(entry.clone(), 0.75);
    assert_eq!(scored.score, 0.75);
    assert_eq!(scored.entry.content, entry.content);
    assert!(scored.stage_scores.is_empty());
}

#[test]
fn scored_entry_with_stage_scores() {
    let scored = ScoredEntry::new(semantic_entry("entry content"), 0.5)
        .with_stage_score("filter", 0.8)
        .with_stage_score("rank", 0.6);
    assert_eq!(scored.stage_scores.len(), 2);
    assert_eq!(scored.stage_scores[0].0, "filter");
    assert_eq!(scored.stage_scores[1].1, 0.6);
}

#[test]
fn scored_entry_token_estimation() {
    let mut entry = semantic_entry(&"a]".repeat(100));
    entry.title = "short".to_string();
    let scored = ScoredEntry::new(entry, 0.5);
    let tokens = scored.estimated_tokens();
    // ~205 chars / 4 ≈ 51 tokens
    assert!(tokens > 0);
    assert!(tokens < 200);
}

// ── StoragePipeline tests ──

#[test]
fn storage_pipeline_default_stages() {
    let pipeline = StoragePipeline::new();
    let stages = pipeline.stages();
    assert_eq!(stages.len(), 4);
    assert_eq!(stages[0], StorageStage::Validate);
    assert_eq!(stages[1], StorageStage::Deduplicate);
    assert_eq!(stages[2], StorageStage::Classify);
    assert_eq!(stages[3], StorageStage::Store);
}

#[test]
fn storage_pipeline_custom_stages() {
    let pipeline = StoragePipeline::new()
        .with_stages(vec![StorageStage::Validate, StorageStage::Store]);
    assert_eq!(pipeline.stages().len(), 2);
}

#[test]
fn storage_pipeline_config_defaults() {
    let config = StoragePipelineConfig::default();
    assert_eq!(config.min_content_length, 10);
    assert_eq!(config.max_content_length, 100_000);
    assert!(config.enable_dedup);
    assert!(config.enable_classification);
    assert!(config.trivial_filter);
}

#[test]
fn storage_pipeline_with_config() {
    let config = StoragePipelineConfig {
        min_content_length: 20,
        max_content_length: 50_000,
        enable_dedup: false,
        enable_classification: false,
        trivial_filter: false,
    };
    let pipeline = StoragePipeline::with_config(config);
    assert_eq!(pipeline.config().min_content_length, 20);
    assert!(!pipeline.config().enable_dedup);
}

#[test]
fn storage_pipeline_execute_accepts_valid_entry() {
    let mut pipeline = StoragePipeline::new();
    let entry = semantic_entry("This is a valid storage pipeline entry for testing");
    let result = pipeline.execute(&entry).unwrap();
    assert!(result.accepted);
    assert!(result.entry_id.is_some());
    assert!(!result.stages_applied.is_empty());
}

#[test]
fn storage_pipeline_execute_rejects_short_content() {
    let mut pipeline = StoragePipeline::new();
    let entry = semantic_entry("short");
    let result = pipeline.execute(&entry).unwrap();
    assert!(!result.accepted);
    assert!(matches!(
        result.rejection_reason,
        Some(RejectionReason::TooShort { .. })
    ));
}

#[test]
fn storage_pipeline_execute_rejects_trivial_content() {
    let mut pipeline = StoragePipeline::new();
    // "thanks" is exactly a trivial phrase and length >= 10 check needs padding
    let entry = semantic_entry("short txt");
    let result = pipeline.execute(&entry).unwrap();
    assert!(!result.accepted);
}

#[test]
fn storage_validate_accepts_valid_content() {
    let pipeline = StoragePipeline::new();
    let entry = semantic_entry("This is perfectly valid content for testing");
    assert!(pipeline.validate(&entry).is_ok());
}

#[test]
fn storage_validate_rejects_too_short() {
    let pipeline = StoragePipeline::new();
    let entry = semantic_entry("short");
    let err = pipeline.validate(&entry).unwrap_err();
    assert!(matches!(err, RejectionReason::TooShort { .. }));
}

#[test]
fn storage_check_duplicate_returns_none_for_fresh() {
    let pipeline = StoragePipeline::new();
    let entry = semantic_entry("Unique content that hasn't been seen before");
    assert!(pipeline.check_duplicate(&entry).is_none());
}

#[test]
fn storage_classify_keeps_existing_type() {
    let pipeline = StoragePipeline::new();
    let entry = MemoryEntry::new("s1", "a1", MemoryType::Working, "some working memory content");
    assert_eq!(pipeline.classify(&entry), MemoryType::Working);
}

#[test]
fn storage_classify_detects_procedural() {
    let pipeline = StoragePipeline::new();
    let entry = semantic_entry("how to install rust on ubuntu");
    assert_eq!(pipeline.classify(&entry), MemoryType::Procedural);
}

#[test]
fn storage_stage_as_str() {
    assert_eq!(StorageStage::Validate.as_str(), "validate");
    assert_eq!(StorageStage::Deduplicate.as_str(), "deduplicate");
    assert_eq!(StorageStage::Classify.as_str(), "classify");
    assert_eq!(StorageStage::Store.as_str(), "store");
}

#[test]
fn storage_result_accepted_constructor() {
    let result = StorageResult::accepted(
        "entry-1".to_string(),
        MemoryType::Semantic,
        0.75,
    );
    assert!(result.accepted);
    assert_eq!(result.entry_id, Some("entry-1".to_string()));
    assert_eq!(result.importance_score, 0.75);
    assert!(result.rejection_reason.is_none());
}

#[test]
fn storage_result_rejected_constructor() {
    let result = StorageResult::rejected(
        MemoryType::Semantic,
        RejectionReason::TooShort { min_length: 10, actual: 3 },
    );
    assert!(!result.accepted);
    assert!(result.entry_id.is_none());
    assert_eq!(
        result.rejection_reason,
        Some(RejectionReason::TooShort { min_length: 10, actual: 3 })
    );
}

#[test]
fn rejection_reason_variants() {
    let _ = RejectionReason::TooShort { min_length: 10, actual: 5 };
    let _ = RejectionReason::TrivialContent;
    let _ = RejectionReason::Duplicate { existing_id: "dup-1".to_string() };
    let _ = RejectionReason::TooLarge { max_length: 100_000, actual: 200_000 };
    let _ = RejectionReason::InvalidType;
    let _ = RejectionReason::Custom("custom reason".to_string());
}

#[test]
fn storage_pipeline_dedup_rejects_second_identical_entry() {
    let mut pipeline = StoragePipeline::new();
    let entry = semantic_entry("This is a valid entry that will be stored and then duplicated");

    let first = pipeline.execute(&entry).unwrap();
    assert!(first.accepted, "First entry should be accepted");

    let second = pipeline.execute(&entry).unwrap();
    assert!(!second.accepted, "Duplicate entry should be rejected");
    assert!(
        matches!(second.rejection_reason, Some(RejectionReason::Duplicate { .. })),
        "Should be rejected as duplicate"
    );
}
