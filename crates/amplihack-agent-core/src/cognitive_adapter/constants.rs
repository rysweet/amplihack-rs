//! Constants for the cognitive adapter retrieval pipeline.
//!
//! Mirrors Python `retrieval_constants.py` and `hive_mind/constants.py`.

use std::collections::HashSet;
use std::sync::LazyLock;

// ---------------------------------------------------------------------------
// Retrieval tuning
// ---------------------------------------------------------------------------

/// Extra candidate multiplier before n-gram re-ranking.
pub const SEARCH_CANDIDATE_MULTIPLIER: usize = 3;

/// Multiplier for full-scan fallback when keyword search returns nothing.
pub const FALLBACK_SCAN_MULTIPLIER: usize = 5;

/// Unigram weight in n-gram overlap scoring.
pub const UNIGRAM_WEIGHT: f64 = 0.65;

/// Bigram weight in n-gram overlap scoring.
pub const BIGRAM_WEIGHT: f64 = 0.35;

/// Minimum similarity for graph edges.
pub const SIMILARITY_THRESHOLD: f64 = 0.3;

/// Maximum outgoing edges per node in the similarity graph.
pub const MAX_EDGES_PER_NODE: usize = 10;

/// Hop depth for graph traversal queries.
pub const HOP_DEPTH: usize = 2;

// ---------------------------------------------------------------------------
// Hive / quality gating
// ---------------------------------------------------------------------------

/// Default quality threshold for promoting facts to hive.
pub const DEFAULT_QUALITY_THRESHOLD: f64 = 0.3;

/// Default confidence gate for accepting hive results.
pub const DEFAULT_CONFIDENCE_GATE: f64 = 0.3;

/// Default Kuzu buffer pool size (256 MiB).
pub const KUZU_BUFFER_POOL_SIZE: usize = 256 * 1024 * 1024;

/// Default maximum working-memory slots per task.
pub const MAX_WORKING_SLOTS: usize = 20;

// ---------------------------------------------------------------------------
// Stop words
// ---------------------------------------------------------------------------

/// Stop words removed from queries before backend search.
pub static QUERY_STOP_WORDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "what", "is", "the", "a", "an", "are", "was", "were", "how", "does", "do", "and", "or",
        "of", "in", "to", "for", "with", "on", "at", "by", "from", "that", "this", "it", "as",
        "be", "been", "has", "have", "had", "will", "would", "could", "should", "did", "which",
        "who", "when", "where", "why", "any", "some", "all", "both", "each", "few", "more",
        "most", "other", "such", "into", "through", "during", "before", "after", "than", "then",
        "these", "those", "there", "their", "they", "its", "our", "your", "my", "we", "i", "you",
        "he", "she", "me", "him", "her", "them", "used", "found", "given", "made", "came", "went",
        "said", "got",
    ]
    .into_iter()
    .collect()
});
