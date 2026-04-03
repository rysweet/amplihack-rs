//! Cross-encoder reranking and Reciprocal Rank Fusion for the hive mind.
//!
//! Provides scoring functions and RRF merge for combining results from
//! multiple retrieval sources (keyword, vector, federated).
//!
//! - `hybrid_score`: combine keyword and vector scores.
//! - `hybrid_score_weighted`: multi-signal scoring (semantic + confirmation + trust).
//! - `trust_weighted_score`: similarity × trust × confidence.
//! - `rrf_merge`: Reciprocal Rank Fusion for merging ranked lists.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Constants (mirroring Python constants.py)
// ---------------------------------------------------------------------------

/// Default weight for semantic similarity.
pub const DEFAULT_SEMANTIC_WEIGHT: f64 = 0.5;
/// Default weight for confirmation count.
pub const DEFAULT_CONFIRMATION_WEIGHT: f64 = 0.3;
/// Default weight for source trust.
pub const DEFAULT_TRUST_WEIGHT: f64 = 0.2;
/// Default weight for keyword score.
pub const DEFAULT_KEYWORD_WEIGHT: f64 = 0.4;
/// Default weight for vector score.
pub const DEFAULT_VECTOR_WEIGHT: f64 = 0.6;
/// RRF smoothing constant.
pub const RRF_K: usize = 60;
/// Divisor used to normalize trust from \[0, 2\] to \[0, 1\].
pub const TRUST_NORMALIZATION_DIVISOR: f64 = 2.0;
/// Divisor used to normalize confirmation count.
pub const CONFIRMATION_NORMALIZATION_DIVISOR: f64 = 5.0;

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// A fact with an associated relevance score.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScoredFact {
    /// The fact identifier.
    pub fact_id: String,
    /// Relevance score (higher is better).
    pub score: f64,
    /// Which retrieval method produced this result.
    pub source: String,
}

impl ScoredFact {
    pub fn new(fact_id: impl Into<String>, score: f64, source: impl Into<String>) -> Self {
        Self {
            fact_id: fact_id.into(),
            score,
            source: source.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// Hybrid scoring
// ---------------------------------------------------------------------------

/// Combine keyword and vector retrieval scores.
pub fn hybrid_score(
    keyword_score: f64,
    vector_score: f64,
    keyword_weight: f64,
    vector_weight: f64,
) -> f64 {
    keyword_score * keyword_weight + vector_score * vector_weight
}

/// Compute a hybrid relevance score combining multiple signals.
///
/// Default weights: semantic (0.5), confirmation (0.3), trust (0.2).
pub fn hybrid_score_weighted(
    semantic_similarity: f64,
    confirmation_count: u32,
    source_trust: f64,
    w_semantic: f64,
    w_confirmation: f64,
    w_trust: f64,
) -> f64 {
    let conf_score = if confirmation_count > 0 {
        (confirmation_count as f64 / CONFIRMATION_NORMALIZATION_DIVISOR).min(1.0)
    } else {
        0.0
    };
    let trust_score = (source_trust / TRUST_NORMALIZATION_DIVISOR).min(1.0);
    w_semantic * semantic_similarity + w_confirmation * conf_score + w_trust * trust_score
}

/// Score combining similarity, source trust, and fact confidence.
///
/// Normalizes trust from \[0, 2\] to \[0, 1\] and clamps all inputs.
pub fn trust_weighted_score(
    similarity: f64,
    trust: f64,
    confidence: f64,
    w_similarity: f64,
    w_trust: f64,
    w_confidence: f64,
) -> f64 {
    let trust_norm = (trust.max(0.0) / TRUST_NORMALIZATION_DIVISOR).min(1.0);
    let confidence_norm = confidence.clamp(0.0, 1.0);
    let similarity_norm = similarity.clamp(0.0, 1.0);
    w_similarity * similarity_norm + w_trust * trust_norm + w_confidence * confidence_norm
}

// ---------------------------------------------------------------------------
// Reciprocal Rank Fusion
// ---------------------------------------------------------------------------

/// An item in a ranked list, identified by a key.
pub trait RankedItem {
    /// Unique key for deduplication across ranked lists.
    fn rank_key(&self) -> &str;
}

impl RankedItem for ScoredFact {
    fn rank_key(&self) -> &str {
        &self.fact_id
    }
}

/// Merge multiple ranked lists using Reciprocal Rank Fusion.
///
/// RRF score = Σ 1 / (k + rank_i) across all lists where the item appears.
/// This is robust to score-scale differences between retrieval methods.
pub fn rrf_merge(ranked_lists: &[&[impl RankedItem]], k: usize, limit: usize) -> Vec<ScoredFact> {
    let mut scores: HashMap<String, f64> = HashMap::new();
    let mut first_occurrence: HashMap<String, usize> = HashMap::new();

    for (list_idx, list) in ranked_lists.iter().enumerate() {
        for (rank, item) in list.iter().enumerate() {
            let key = item.rank_key().to_string();
            let rrf_score = 1.0 / (k + rank + 1) as f64; // +1 for 1-based ranking
            *scores.entry(key.clone()).or_default() += rrf_score;
            first_occurrence.entry(key).or_insert(list_idx);
        }
    }

    let mut result: Vec<ScoredFact> = scores
        .into_iter()
        .map(|(fact_id, score)| ScoredFact::new(fact_id, score, "rrf"))
        .collect();

    result.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    result.truncate(limit);
    result
}

/// Convenience: merge multiple `Vec<ScoredFact>` lists.
pub fn rrf_merge_scored(
    ranked_lists: &[Vec<ScoredFact>],
    k: usize,
    limit: usize,
) -> Vec<ScoredFact> {
    let refs: Vec<&[ScoredFact]> = ranked_lists.iter().map(|v| v.as_slice()).collect();
    rrf_merge(&refs, k, limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- hybrid_score -------------------------------------------------------

    #[test]
    fn hybrid_score_default_weights() {
        let s = hybrid_score(1.0, 1.0, DEFAULT_KEYWORD_WEIGHT, DEFAULT_VECTOR_WEIGHT);
        assert!((s - 1.0).abs() < 1e-9);
    }

    #[test]
    fn hybrid_score_zero_inputs() {
        assert!((hybrid_score(0.0, 0.0, 0.4, 0.6)).abs() < 1e-9);
    }

    #[test]
    fn hybrid_score_keyword_only() {
        let s = hybrid_score(0.8, 0.0, 1.0, 0.0);
        assert!((s - 0.8).abs() < 1e-9);
    }

    // -- hybrid_score_weighted ----------------------------------------------

    #[test]
    fn hybrid_score_weighted_all_max() {
        let s = hybrid_score_weighted(1.0, 5, 2.0, 0.5, 0.3, 0.2);
        assert!((s - 1.0).abs() < 1e-9);
    }

    #[test]
    fn hybrid_score_weighted_zero_confirmations() {
        let s = hybrid_score_weighted(0.8, 0, 1.0, 0.5, 0.3, 0.2);
        let expected = 0.5 * 0.8 + 0.3 * 0.0 + 0.2 * 0.5;
        assert!((s - expected).abs() < 1e-9);
    }

    #[test]
    fn hybrid_score_weighted_partial_confirmations() {
        // 3 confirmations: 3/5 = 0.6
        let s = hybrid_score_weighted(0.0, 3, 0.0, 0.5, 0.3, 0.2);
        let expected = 0.3 * 0.6;
        assert!((s - expected).abs() < 1e-9);
    }

    // -- trust_weighted_score -----------------------------------------------

    #[test]
    fn trust_weighted_all_ones() {
        let s = trust_weighted_score(1.0, 2.0, 1.0, 0.5, 0.3, 0.2);
        assert!((s - 1.0).abs() < 1e-9);
    }

    #[test]
    fn trust_weighted_clamps_negative() {
        let s = trust_weighted_score(-0.5, -1.0, -0.5, 0.5, 0.3, 0.2);
        assert!((s - 0.0).abs() < 1e-9);
    }

    #[test]
    fn trust_normalized() {
        // trust=1.0 → normalized to 0.5
        let s = trust_weighted_score(0.0, 1.0, 0.0, 0.0, 1.0, 0.0);
        assert!((s - 0.5).abs() < 1e-9);
    }

    // -- rrf_merge ----------------------------------------------------------

    #[test]
    fn rrf_single_list() {
        let list = vec![
            ScoredFact::new("a", 1.0, "test"),
            ScoredFact::new("b", 0.5, "test"),
        ];
        let result = rrf_merge_scored(&[list], RRF_K, 10);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].fact_id, "a");
        assert!(result[0].score > result[1].score);
    }

    #[test]
    fn rrf_merge_two_lists() {
        let list1 = vec![
            ScoredFact::new("a", 1.0, "kw"),
            ScoredFact::new("b", 0.5, "kw"),
        ];
        let list2 = vec![
            ScoredFact::new("b", 1.0, "vec"),
            ScoredFact::new("c", 0.5, "vec"),
        ];
        let result = rrf_merge_scored(&[list1, list2], RRF_K, 10);

        assert_eq!(result.len(), 3);
        // "b" appears in both lists → highest RRF score
        assert_eq!(result[0].fact_id, "b");
    }

    #[test]
    fn rrf_respects_limit() {
        let list = vec![
            ScoredFact::new("a", 1.0, "t"),
            ScoredFact::new("b", 0.9, "t"),
            ScoredFact::new("c", 0.8, "t"),
        ];
        let result = rrf_merge_scored(&[list], RRF_K, 2);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn rrf_empty_lists() {
        let result = rrf_merge_scored(&[], RRF_K, 10);
        assert!(result.is_empty());
    }

    #[test]
    fn rrf_deduplicates_by_fact_id() {
        let list1 = vec![ScoredFact::new("x", 1.0, "a")];
        let list2 = vec![ScoredFact::new("x", 0.5, "b")];
        let result = rrf_merge_scored(&[list1, list2], RRF_K, 10);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].fact_id, "x");
        assert_eq!(result[0].source, "rrf");
    }

    // -- ScoredFact ---------------------------------------------------------

    #[test]
    fn scored_fact_serde_roundtrip() {
        let sf = ScoredFact::new("fact-1", 0.85, "vector");
        let json = serde_json::to_string(&sf).unwrap();
        let decoded: ScoredFact = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.fact_id, "fact-1");
        assert!((decoded.score - 0.85).abs() < 1e-9);
    }
}
