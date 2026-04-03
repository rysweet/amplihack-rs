//! Tests for retrieval and storage pipelines.
//!
//! Tests compile but FAIL because pipeline methods use todo!().

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
#[should_panic(expected = "not yet implemented")]
fn retrieval_pipeline_execute_not_implemented() {
    let pipeline = RetrievalPipeline::new();
    let candidates = vec![semantic_entry("test candidate content here")];
    let query = MemoryQuery::new("test");
    let _ = pipeline.execute(candidates, &query);
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn retrieval_filter_stage_not_implemented() {
    let pipeline = RetrievalPipeline::new();
    let entries = vec![ScoredEntry::new(semantic_entry("filter test"), 0.5)];
    let _ = pipeline.filter(entries, &MemoryQuery::new("test"));
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn retrieval_rank_stage_not_implemented() {
    let pipeline = RetrievalPipeline::new();
    let entries = vec![ScoredEntry::new(semantic_entry("rank test"), 0.5)];
    let _ = pipeline.rank(entries, &MemoryQuery::new("test"));
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn retrieval_dedup_stage_not_implemented() {
    let pipeline = RetrievalPipeline::new();
    let entries = vec![ScoredEntry::new(semantic_entry("dedup test"), 0.5)];
    let _ = pipeline.deduplicate(entries);
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn retrieval_budget_enforce_not_implemented() {
    let pipeline = RetrievalPipeline::new();
    let entries = vec![ScoredEntry::new(semantic_entry("budget test"), 0.5)];
    let _ = pipeline.enforce_budget(entries, 1000);
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
    let mut entry = semantic_entry("a]".repeat(100).as_str());
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
#[should_panic(expected = "not yet implemented")]
fn storage_pipeline_execute_not_implemented() {
    let mut pipeline = StoragePipeline::new();
    let entry = semantic_entry("test storage pipeline entry");
    let _ = pipeline.execute(&entry);
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn storage_validate_not_implemented() {
    let pipeline = StoragePipeline::new();
    let _ = pipeline.validate(&semantic_entry("validate test"));
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn storage_check_duplicate_not_implemented() {
    let pipeline = StoragePipeline::new();
    let _ = pipeline.check_duplicate(&semantic_entry("dedup check"));
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn storage_classify_not_implemented() {
    let pipeline = StoragePipeline::new();
    let _ = pipeline.classify(&semantic_entry("classify test"));
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
