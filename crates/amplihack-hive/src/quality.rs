//! Content quality scoring and gating for the hive mind.
//!
//! Provides quality assessment for facts entering or being retrieved from
//! the hive, preventing low-quality content from polluting the shared
//! knowledge base.
//!
//! - Heuristic scoring: length, structure, specificity, concept alignment.
//! - Configurable thresholds for different deployment scenarios.

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MIN_CONTENT_LENGTH: usize = 10;
const IDEAL_MIN_LENGTH: usize = 30;
const IDEAL_MAX_LENGTH: usize = 500;

/// Default minimum quality score for promotion into the hive.
pub const DEFAULT_QUALITY_THRESHOLD: f64 = 0.3;

/// Default minimum confidence for cross-group broadcast.
pub const DEFAULT_BROADCAST_THRESHOLD: f64 = 0.9;

static VAGUE_WORDS: LazyLock<Vec<&str>> = LazyLock::new(|| {
    vec![
        "something",
        "stuff",
        "things",
        "whatever",
        "maybe",
        "probably",
        "idk",
        "dunno",
        "etc",
        "somehow",
    ]
});

static SPECIFIC_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        Regex::new(r"\d+(\.\d+)?").unwrap(),
        Regex::new(r"\b[A-Z][a-z]+(?:\s[A-Z][a-z]+)+\b").unwrap(),
        Regex::new(r"(?i)\b(?:because|therefore|since|due to|caused by)\b").unwrap(),
        Regex::new(r"(?i)\b(?:always|never|must|requires|ensures)\b").unwrap(),
    ]
});

// ---------------------------------------------------------------------------
// Quality scoring
// ---------------------------------------------------------------------------

/// Score the quality of a fact's content (0.0–1.0).
///
/// Evaluates length, specificity, structure, and concept alignment.
pub fn score_content_quality(content: &str, concept: &str) -> f64 {
    let content = content.trim();
    if content.is_empty() {
        return 0.0;
    }

    let mut score: f64 = 0.0;
    let mut max_score: f64 = 0.0;

    // --- Length score (0–0.3) ---
    max_score += 0.3;
    let length = content.len();
    if length < MIN_CONTENT_LENGTH {
        score += 0.05;
    } else if (IDEAL_MIN_LENGTH..=IDEAL_MAX_LENGTH).contains(&length) {
        score += 0.3;
    } else if length < IDEAL_MIN_LENGTH {
        score += 0.15;
    } else {
        score += 0.2; // too long
    }

    // --- Specificity score (0–0.3) ---
    max_score += 0.3;
    let words: Vec<&str> = content.split_whitespace().collect();
    if words.len() >= 3 {
        let lower_words: Vec<String> = words.iter().map(|w| w.to_lowercase()).collect();
        let vague_count = lower_words
            .iter()
            .filter(|w| VAGUE_WORDS.contains(&w.as_str()))
            .count();
        let vague_ratio = vague_count as f64 / words.len() as f64;
        let specificity = (1.0 - vague_ratio * 5.0).max(0.0);
        let pattern_bonus: f64 = SPECIFIC_PATTERNS
            .iter()
            .filter(|p| p.is_match(content))
            .count() as f64
            * 0.05;
        score += (specificity * 0.2 + pattern_bonus).min(0.3);
    }

    // --- Structure score (0–0.2) ---
    max_score += 0.2;
    let sentences: Vec<&str> = content
        .split(['.', '!', '?'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if !sentences.is_empty() {
        score += 0.1;
    }
    if sentences.len() >= 2 {
        score += 0.05;
    }
    if content.contains([',', ':', ';', '(', ')', '-']) {
        score += 0.05;
    }

    // --- Concept alignment (0–0.2) ---
    max_score += 0.2;
    let concept = concept.trim();
    if !concept.is_empty() {
        let concept_words: Vec<String> = concept
            .split_whitespace()
            .filter(|w| w.len() > 1)
            .map(|w| w.to_lowercase())
            .collect();
        let content_words: Vec<String> = words
            .iter()
            .filter(|w| w.len() > 1)
            .map(|w| w.to_lowercase())
            .collect();
        if !concept_words.is_empty() {
            let overlap = concept_words
                .iter()
                .filter(|cw| content_words.contains(cw))
                .count();
            let ratio = overlap as f64 / concept_words.len() as f64;
            score += (ratio * 0.2).min(0.2);
        }
    }

    if max_score > 0.0 {
        (score / max_score).min(1.0)
    } else {
        0.0
    }
}

// ---------------------------------------------------------------------------
// Quality gate
// ---------------------------------------------------------------------------

/// Configurable quality gate for hive mind operations.
///
/// Controls promotion thresholds, retrieval confidence, and broadcast thresholds.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QualityGate {
    /// Minimum quality for facts entering the hive.
    pub promotion_threshold: f64,
    /// Minimum confidence for facts returned in queries.
    pub retrieval_confidence_threshold: f64,
    /// Minimum confidence for cross-group replication.
    pub broadcast_threshold: f64,
    #[serde(skip)]
    quality_cache: HashMap<String, f64>,
}

impl Default for QualityGate {
    fn default() -> Self {
        Self {
            promotion_threshold: DEFAULT_QUALITY_THRESHOLD,
            retrieval_confidence_threshold: 0.0,
            broadcast_threshold: DEFAULT_BROADCAST_THRESHOLD,
            quality_cache: HashMap::new(),
        }
    }
}

impl QualityGate {
    /// Create a gate with custom promotion threshold.
    pub fn with_promotion_threshold(mut self, threshold: f64) -> Self {
        self.promotion_threshold = threshold;
        self
    }

    /// Check if content meets quality threshold for promotion.
    pub fn should_promote(&self, content: &str, concept: &str) -> bool {
        score_content_quality(content, concept) >= self.promotion_threshold
    }

    /// Check if a fact meets confidence threshold for retrieval.
    pub fn should_retrieve(&self, confidence: f64) -> bool {
        confidence >= self.retrieval_confidence_threshold
    }

    /// Check if a fact meets threshold for cross-group broadcast.
    pub fn should_broadcast(&self, confidence: f64) -> bool {
        confidence >= self.broadcast_threshold
    }

    /// Score content quality with caching by content+concept hash.
    pub fn score(&mut self, content: &str, concept: &str) -> f64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        content.hash(&mut hasher);
        concept.hash(&mut hasher);
        let key = format!("{:016x}", hasher.finish());

        *self
            .quality_cache
            .entry(key)
            .or_insert_with(|| score_content_quality(content, concept))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_content_scores_zero() {
        assert_eq!(score_content_quality("", ""), 0.0);
        assert_eq!(score_content_quality("   ", ""), 0.0);
    }

    #[test]
    fn very_short_content_scores_low() {
        let score = score_content_quality("hi", "");
        assert!(score < 0.3, "expected < 0.3, got {score}");
    }

    #[test]
    fn ideal_length_content_scores_higher() {
        let good = "DNA stores genetic information in a double helix structure.";
        let short = "DNA exists";
        assert!(score_content_quality(good, "") > score_content_quality(short, ""));
    }

    #[test]
    fn vague_content_penalized() {
        let vague = "something about stuff and whatever things";
        let specific = "DNA replication requires helicase enzyme (always present in E. coli)";
        assert!(score_content_quality(specific, "") > score_content_quality(vague, ""));
    }

    #[test]
    fn concept_alignment_boosts_score() {
        let content = "Rust provides memory safety without garbage collection";
        let aligned = score_content_quality(content, "Rust memory");
        let unaligned = score_content_quality(content, "Python web");
        assert!(aligned > unaligned);
    }

    #[test]
    fn structured_content_scores_higher() {
        let structured = "The process has two stages. First, initialization. Then, execution.";
        let flat = "process stages initialization execution";
        assert!(score_content_quality(structured, "") > score_content_quality(flat, ""));
    }

    #[test]
    fn quality_gate_promote() {
        let gate = QualityGate::default();
        let good = "DNA stores genetic information in a double helix structure.";
        assert!(gate.should_promote(good, "genetics"));
        assert!(!gate.should_promote("stuff", "genetics"));
    }

    #[test]
    fn quality_gate_retrieve() {
        let gate = QualityGate {
            retrieval_confidence_threshold: 0.5,
            ..Default::default()
        };
        assert!(gate.should_retrieve(0.8));
        assert!(!gate.should_retrieve(0.3));
    }

    #[test]
    fn quality_gate_broadcast() {
        let gate = QualityGate::default();
        assert!(gate.should_broadcast(0.95));
        assert!(!gate.should_broadcast(0.5));
    }

    #[test]
    fn quality_gate_cached_score() {
        let mut gate = QualityGate::default();
        let content = "DNA stores genetic information.";
        let s1 = gate.score(content, "genetics");
        let s2 = gate.score(content, "genetics");
        assert!((s1 - s2).abs() < f64::EPSILON);
    }

    #[test]
    fn score_bounded_zero_to_one() {
        for text in &["a", "hello world test", "x".repeat(1000).as_str()] {
            let s = score_content_quality(text, "");
            assert!(
                (0.0..=1.0).contains(&s),
                "score {s} out of range for '{text}'"
            );
        }
    }

    #[test]
    fn with_promotion_threshold_builder() {
        let gate = QualityGate::default().with_promotion_threshold(0.8);
        assert!((gate.promotion_threshold - 0.8).abs() < f64::EPSILON);
    }
}
