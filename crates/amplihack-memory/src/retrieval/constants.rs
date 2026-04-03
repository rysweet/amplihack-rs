//! Named constants for retrieval path configuration.
//!
//! Centralises all magic numbers used in knowledge-base size checks,
//! tier boundaries, search candidate multipliers, and scoring weights.
//! Ported from Python `retrieval_constants.py`.

// ---------------------------------------------------------------------------
// Core retrieval limits
// ---------------------------------------------------------------------------

/// Maximum facts to pull from the store in a single `get_all_facts()` call.
pub const MAX_RETRIEVAL_LIMIT: usize = 15_000;

/// KB size at or below which facts are returned verbatim (no tiered compression).
pub const VERBATIM_RETRIEVAL_THRESHOLD: usize = 1_000;

/// Tier 1: the N most-recent facts are kept verbatim.
pub const TIER1_VERBATIM_SIZE: usize = 200;

/// Tier 2 boundary (entity-level summaries). Tier 3 covers everything older.
pub const TIER2_ENTITY_SIZE: usize = VERBATIM_RETRIEVAL_THRESHOLD;

/// Extra candidates to fetch before n-gram re-ranking.
pub const SEARCH_CANDIDATE_MULTIPLIER: usize = 3;

/// Multiplier for the broad full-scan fallback.
pub const FALLBACK_SCAN_MULTIPLIER: usize = 5;

// ---------------------------------------------------------------------------
// Entity & concept retrieval
// ---------------------------------------------------------------------------

/// Max phrases used in concept-based retrieval.
pub const CONCEPT_PHRASE_LIMIT: usize = 8;

/// Limit for concept search results.
pub const CONCEPT_SEARCH_LIMIT: usize = 15;

/// Limit for exact concept matching.
pub const CONCEPT_EXACT_SEARCH_LIMIT: usize = 50;

/// Standard entity search limit.
pub const ENTITY_SEARCH_LIMIT: usize = 100;

/// Deeper retrieval limit for CVE/incident-style queries.
pub const INCIDENT_QUERY_SEARCH_LIMIT: usize = 200;

/// Per-entity fact limit in entity retrieval.
pub const ENTITY_FACT_LIMIT: usize = 80;

/// Facts per entity in multi-entity queries.
pub const MULTI_ENTITY_LIMIT: usize = 40;

/// Text search limit for structured entity IDs.
pub const ENTITY_ID_TEXT_SEARCH_LIMIT: usize = 20;

/// Topics shown from conflicting-information analysis.
pub const CONFLICTING_TOPICS_LIMIT: usize = 20;

// ---------------------------------------------------------------------------
// Scoring weights
// ---------------------------------------------------------------------------

/// N-gram overlap scoring: unigram weight (must sum with BIGRAM_WEIGHT to 1.0).
pub const UNIGRAM_WEIGHT: f64 = 0.65;

/// N-gram overlap scoring: bigram weight.
pub const BIGRAM_WEIGHT: f64 = 0.35;

/// Small weight applied to confidence as a secondary sort key.
pub const CONFIDENCE_SORT_WEIGHT: f64 = 0.01;

// ---------------------------------------------------------------------------
// Distributed hive
// ---------------------------------------------------------------------------

/// Multiplier for hive search broad-fetch.
pub const HIVE_SEARCH_MULTIPLIER: usize = 3;

/// Max keywords for building a hive query string.
pub const QUERY_KEYWORD_LIMIT: usize = 4;

/// Fact limit for contradiction-detection queries.
pub const CONTRADICTION_CHECK_LIMIT: usize = 50;

/// Overlap threshold for contradiction detection between facts.
pub const CONTRADICTION_OVERLAP_THRESHOLD: f64 = 0.4;

// ---------------------------------------------------------------------------
// Graph edge configuration
// ---------------------------------------------------------------------------

/// Minimum Jaccard similarity to create a `SIMILAR_TO` edge.
pub const SIMILARITY_THRESHOLD: f64 = 0.3;

/// Maximum `SIMILAR_TO` edges created per node.
pub const MAX_EDGES_PER_NODE: usize = 10;

/// Hop depth for `SIMILAR_TO` graph traversal.
pub const HOP_DEPTH: usize = 2;

// ---------------------------------------------------------------------------
// Summary truncation
// ---------------------------------------------------------------------------

/// Max facts per group fed into deterministic summarisation.
pub const SUMMARY_GROUP_MAX_FACTS: usize = 30;

/// Max characters per individual fact text in a summary.
pub const SUMMARY_FACT_TEXT_MAX_LEN: usize = 200;

/// Target length for combined summary text.
pub const SUMMARY_COMBINED_TARGET_LEN: usize = 500;

/// Minimum offset for finding a sentence boundary in truncated text.
pub const SUMMARY_TRUNCATE_MIN_OFFSET: usize = 100;
