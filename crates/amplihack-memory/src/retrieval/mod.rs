//! Retrieval strategies for memory search.
//!
//! Ported from Python `retrieval_strategies.py` — provides BM25/keyword search,
//! entity-centric retrieval, concept-based retrieval, tiered summarisation,
//! graph aggregation, and intent-based routing.

pub mod constants;
pub mod router;
pub mod scoring;
pub mod strategies;
pub mod types;

#[cfg(test)]
mod tests;

// Re-exports for convenience.
pub use constants::*;
pub use router::{
    aggregation_retrieval, entity_linked_retrieval, filter_facts_by_source_reference,
    infrastructure_relation_retrieval, multi_entity_retrieval, supplement_simple_retrieval,
};
pub use scoring::{deduplicate_facts, merge_facts, ngram_overlap_score};
pub use strategies::{
    concept_retrieval, entity_retrieval, estimate_total_fact_count, extract_entity_ids,
    simple_retrieval, summarize_old_facts, tiered_retrieval,
};
pub use types::{
    AggregationResult, Fact, IntentKind, MemorySearch, MemoryStatistics,
};
