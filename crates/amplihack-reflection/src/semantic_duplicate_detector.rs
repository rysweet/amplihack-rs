//! Semantic duplicate detection (port of `semantic_duplicate_detector.py`).
//!
//! The Python module called the Claude SDK with a `difflib` fallback. The
//! Rust port keeps the deterministic similarity fallback only.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DuplicateDetectionResult {
    pub is_duplicate: bool,
    pub similar_issues: Vec<serde_json::Value>,
    pub confidence: f64,
    pub reason: String,
}

#[derive(Debug, Clone, Copy)]
pub struct SemanticDuplicateDetector {
    /// Similarity threshold above which content is considered a duplicate.
    pub threshold: f64,
}

impl Default for SemanticDuplicateDetector {
    fn default() -> Self {
        Self { threshold: 0.75 }
    }
}

impl SemanticDuplicateDetector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn similarity(&self, a: &str, b: &str) -> f64 {
        if a.is_empty() && b.is_empty() {
            return 1.0;
        }
        let toks_a = tokenise(a);
        let toks_b = tokenise(b);
        if toks_a.is_empty() || toks_b.is_empty() {
            return 0.0;
        }
        let mut shared = 0usize;
        for tok in &toks_a {
            if toks_b.contains(tok) {
                shared += 1;
            }
        }
        // Jaccard-style similarity over deduped token sets.
        let union: std::collections::HashSet<&String> =
            toks_a.iter().chain(toks_b.iter()).collect();
        if union.is_empty() {
            0.0
        } else {
            shared as f64 / union.len() as f64
        }
    }

    pub fn compare(&self, new_content: &str, existing: &str) -> DuplicateDetectionResult {
        let score = self.similarity(new_content, existing);
        DuplicateDetectionResult {
            is_duplicate: score >= self.threshold,
            similar_issues: vec![],
            confidence: score,
            reason: if score >= self.threshold {
                format!(
                    "Token similarity {score:.2} ≥ threshold {:.2}",
                    self.threshold
                )
            } else {
                format!(
                    "Token similarity {score:.2} below threshold {:.2}",
                    self.threshold
                )
            },
        }
    }
}

fn tokenise(s: &str) -> Vec<String> {
    s.to_ascii_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty() && w.len() > 2)
        .map(|w| w.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Float comparison helper: avoids clippy::float_cmp on computed scores.
    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn default_and_new_share_the_same_threshold() {
        let a = SemanticDuplicateDetector::new();
        let b = SemanticDuplicateDetector::default();
        assert!(approx(a.threshold, 0.75));
        assert!(approx(b.threshold, 0.75));
        assert!(approx(a.threshold, b.threshold));
    }

    #[test]
    fn similarity_of_two_empty_strings_is_one() {
        let d = SemanticDuplicateDetector::new();
        assert!(approx(d.similarity("", ""), 1.0));
    }

    #[test]
    fn similarity_is_zero_when_first_side_has_no_usable_tokens() {
        let d = SemanticDuplicateDetector::new();
        // "a b c" tokens are all <= 2 chars and get filtered out.
        assert!(approx(d.similarity("a b c", "apple banana"), 0.0));
    }

    #[test]
    fn similarity_is_zero_when_second_side_has_no_usable_tokens() {
        let d = SemanticDuplicateDetector::new();
        assert!(approx(d.similarity("apple banana", "a b c"), 0.0));
    }

    #[test]
    fn identical_content_is_fully_similar() {
        let d = SemanticDuplicateDetector::new();
        assert!(approx(d.similarity("apple banana", "apple banana"), 1.0));
    }

    #[test]
    fn partial_overlap_yields_jaccard_ratio() {
        let d = SemanticDuplicateDetector::new();
        // toks_a = {apple, banana, cherry}, toks_b = {apple, grape}
        // shared = 1 (apple), union = {apple, banana, cherry, grape} = 4 => 0.25
        assert!(approx(
            d.similarity("apple banana cherry", "apple grape"),
            0.25
        ));
    }

    #[test]
    fn disjoint_content_scores_zero_through_union_path() {
        let d = SemanticDuplicateDetector::new();
        // Both sides tokenise to non-empty sets with no shared tokens:
        // shared = 0, union = 4 => 0.0 (exercises the shared/union branch).
        assert!(approx(d.similarity("apple banana", "cherry grape"), 0.0));
    }

    #[test]
    fn tokenisation_is_case_insensitive() {
        let d = SemanticDuplicateDetector::new();
        assert!(approx(d.similarity("Apple BANANA", "apple banana"), 1.0));
    }

    #[test]
    fn short_tokens_are_filtered_out_before_comparison() {
        let d = SemanticDuplicateDetector::new();
        // "ab" and "cd" are 2 chars and dropped; only "apple" remains on the
        // left, matching the single "apple" on the right => 1.0.
        assert!(approx(d.similarity("ab cd apple", "apple"), 1.0));
    }

    #[test]
    fn punctuation_is_treated_as_a_token_separator() {
        let d = SemanticDuplicateDetector::new();
        assert!(approx(
            d.similarity("apple,banana;cherry", "apple banana cherry"),
            1.0
        ));
    }

    #[test]
    fn repeated_tokens_are_counted_as_a_multiset_against_a_deduped_union() {
        let d = SemanticDuplicateDetector::new();
        // toks_a = [apple, apple], toks_b = [apple]; shared counts each
        // occurrence (2) while union is deduped (1), so the score exceeds 1.0.
        let score = d.similarity("apple apple", "apple");
        assert!(score > 1.0, "expected multiset score > 1.0, got {score}");
        assert!(approx(score, 2.0));
    }

    #[test]
    fn compare_flags_duplicates_above_threshold() {
        let d = SemanticDuplicateDetector::new();
        let result = d.compare("apple banana cherry", "apple banana cherry");
        assert!(result.is_duplicate);
        assert!(approx(result.confidence, 1.0));
        assert!(result.similar_issues.is_empty());
        assert!(result.reason.contains("\u{2265} threshold"));
        assert!(result.reason.contains("0.75"));
    }

    #[test]
    fn compare_rejects_content_below_threshold() {
        let d = SemanticDuplicateDetector::new();
        let result = d.compare("apple banana cherry", "apple grape");
        assert!(!result.is_duplicate);
        assert!(approx(result.confidence, 0.25));
        assert!(result.similar_issues.is_empty());
        assert!(result.reason.contains("below threshold"));
        assert!(result.reason.contains("0.75"));
    }

    #[test]
    fn compare_respects_a_custom_threshold() {
        let d = SemanticDuplicateDetector { threshold: 0.2 };
        // Score is 0.25, which clears the lowered 0.2 threshold.
        let result = d.compare("apple banana cherry", "apple grape");
        assert!(result.is_duplicate);
        assert!(approx(result.confidence, 0.25));
        assert!(result.reason.contains("0.20"));
    }

    #[test]
    fn compare_treats_two_empty_strings_as_duplicates() {
        let d = SemanticDuplicateDetector::new();
        let result = d.compare("", "");
        assert!(result.is_duplicate);
        assert!(approx(result.confidence, 1.0));
    }

    #[test]
    fn detection_result_round_trips_through_json() {
        let d = SemanticDuplicateDetector::new();
        let result = d.compare("apple banana", "apple banana");
        let json = serde_json::to_string(&result).expect("serialize result");
        let back: DuplicateDetectionResult =
            serde_json::from_str(&json).expect("deserialize result");
        assert_eq!(back.is_duplicate, result.is_duplicate);
        assert!(approx(back.confidence, result.confidence));
        assert_eq!(back.reason, result.reason);
        assert_eq!(back.similar_issues.len(), result.similar_issues.len());
    }
}
