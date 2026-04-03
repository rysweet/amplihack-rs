//! Content-level quality evaluation for stored memories.

use crate::models::MemoryEntry;
use crate::quality::is_trivial;
use std::collections::{HashMap, HashSet};

use super::metrics::QualityMetrics;

/// Evaluates quality of stored memories.
pub struct QualityEvaluator {
    min_importance_threshold: f64,
    min_content_length: usize,
}

impl QualityEvaluator {
    pub fn new() -> Self {
        Self {
            min_importance_threshold: 0.3,
            min_content_length: 10,
        }
    }

    pub fn with_thresholds(min_importance: f64, min_content_length: usize) -> Self {
        Self {
            min_importance_threshold: min_importance,
            min_content_length,
        }
    }

    /// Evaluate quality metrics for a set of entries.
    pub fn evaluate(&self, entries: &[MemoryEntry]) -> QualityMetrics {
        if entries.is_empty() {
            return QualityMetrics::default();
        }
        let total = entries.len();
        let mut importance_sum = 0.0;
        let mut len_sum = 0usize;
        let mut trivial_count = 0usize;
        let mut dup_count = 0usize;
        let mut fingerprints = HashSet::with_capacity(total);
        let mut type_dist: HashMap<String, usize> = HashMap::new();

        for e in entries {
            importance_sum += e.importance;
            len_sum += e.content.len();
            if is_trivial(&e.content, self.min_content_length) {
                trivial_count += 1;
            }
            if !fingerprints.insert(e.content_fingerprint()) {
                dup_count += 1;
            }
            *type_dist
                .entry(e.memory_type.as_str().to_string())
                .or_insert(0) += 1;
        }

        QualityMetrics {
            total_entries: total,
            average_importance: importance_sum / total as f64,
            average_content_length: len_sum as f64 / total as f64,
            trivial_ratio: trivial_count as f64 / total as f64,
            duplicate_ratio: dup_count as f64 / total as f64,
            type_distribution: type_dist,
        }
    }

    /// Score a single entry's quality (0.0 to 1.0).
    pub fn score_entry(&self, entry: &MemoryEntry) -> f64 {
        let mut score: f64 = 0.0;
        score += entry.importance * 0.4;
        let len = entry.content.len();
        let len_score = if len < self.min_content_length {
            0.0
        } else if len < 50 {
            0.3
        } else if len < 200 {
            0.6
        } else {
            1.0
        };
        score += len_score * 0.3;
        let trivial_penalty = if is_trivial(&entry.content, self.min_content_length) {
            0.0
        } else {
            1.0
        };
        score += trivial_penalty * 0.2;
        if entry.importance >= self.min_importance_threshold {
            score += 0.1;
        }
        score.clamp(0.0, 1.0)
    }

    pub fn min_importance(&self) -> f64 {
        self.min_importance_threshold
    }
}

impl Default for QualityEvaluator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::MemoryType;

    fn semantic_entry(content: &str) -> MemoryEntry {
        MemoryEntry::new("s1", "a1", MemoryType::Semantic, content)
    }

    #[test]
    fn default_thresholds() {
        let eval = QualityEvaluator::new();
        assert_eq!(eval.min_importance(), 0.3);
    }

    #[test]
    fn custom_thresholds() {
        let eval = QualityEvaluator::with_thresholds(0.5, 20);
        assert_eq!(eval.min_importance(), 0.5);
    }

    #[test]
    fn evaluate_returns_metrics() {
        let eval = QualityEvaluator::new();
        let entries = vec![
            semantic_entry("Test quality evaluation content here"),
            semantic_entry("Another meaningful entry for analysis"),
        ];
        let metrics = eval.evaluate(&entries);
        assert_eq!(metrics.total_entries, 2);
        assert!(metrics.average_importance > 0.0);
        assert_eq!(metrics.duplicate_ratio, 0.0);
    }

    #[test]
    fn evaluate_empty() {
        let eval = QualityEvaluator::new();
        let metrics = eval.evaluate(&[]);
        assert_eq!(metrics.total_entries, 0);
    }

    #[test]
    fn detects_duplicates() {
        let eval = QualityEvaluator::new();
        let entries = vec![
            semantic_entry("Duplicate content for testing purposes"),
            semantic_entry("Duplicate content for testing purposes"),
        ];
        let metrics = eval.evaluate(&entries);
        assert!(metrics.duplicate_ratio > 0.0);
    }

    #[test]
    fn score_entry_bounded() {
        let eval = QualityEvaluator::new();
        let entry = semantic_entry("Score this entry with meaningful content");
        let score = eval.score_entry(&entry);
        assert!((0.0..=1.0).contains(&score));
    }

    #[test]
    fn trivial_entry_scores_low() {
        let eval = QualityEvaluator::new();
        let entry = semantic_entry("hi");
        assert!(eval.score_entry(&entry) < 0.5);
    }

    #[test]
    fn type_distribution_tracked() {
        let eval = QualityEvaluator::new();
        let entries = vec![
            semantic_entry("Semantic content entry for testing"),
            MemoryEntry::new("s1", "a1", MemoryType::Working, "Working content entry here"),
        ];
        let metrics = eval.evaluate(&entries);
        assert_eq!(metrics.type_distribution.get("semantic"), Some(&1));
        assert_eq!(metrics.type_distribution.get("working"), Some(&1));
    }
}
