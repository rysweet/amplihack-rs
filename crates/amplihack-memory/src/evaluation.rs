//! Memory quality evaluation.
//!
//! Provides evaluators for quality, reliability, and performance
//! metrics of the memory system. Used to score and report on
//! overall memory health.

use crate::models::MemoryEntry;
use crate::quality::is_trivial;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Quality metrics for stored memories.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QualityMetrics {
    pub total_entries: usize,
    pub average_importance: f64,
    pub average_content_length: f64,
    pub trivial_ratio: f64,
    pub duplicate_ratio: f64,
    pub type_distribution: HashMap<String, usize>,
}

/// Reliability metrics for the memory system.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReliabilityMetrics {
    pub store_success_rate: f64,
    pub retrieve_success_rate: f64,
    pub average_store_latency_ms: f64,
    pub average_retrieve_latency_ms: f64,
    pub error_count: usize,
}

/// Performance metrics for retrieval quality.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PerformanceMetrics {
    pub average_relevance_score: f64,
    pub budget_utilization: f64,
    pub recall_at_k: f64,
    pub precision_at_k: f64,
}

/// Overall quality report combining all metrics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QualityReport {
    pub quality: QualityMetrics,
    pub reliability: ReliabilityMetrics,
    pub performance: PerformanceMetrics,
    pub overall_score: f64,
    pub recommendations: Vec<String>,
}

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
        let avg_importance = entries.iter().map(|e| e.importance).sum::<f64>() / total as f64;
        let avg_len =
            entries.iter().map(|e| e.content.len()).sum::<usize>() as f64 / total as f64;

        let trivial_count = entries
            .iter()
            .filter(|e| is_trivial(&e.content, self.min_content_length))
            .count();

        let mut fingerprints = HashSet::new();
        let mut dup_count = 0usize;
        for e in entries {
            if !fingerprints.insert(e.content_fingerprint()) {
                dup_count += 1;
            }
        }

        let mut type_dist = HashMap::new();
        for e in entries {
            *type_dist
                .entry(e.memory_type.as_str().to_string())
                .or_insert(0) += 1;
        }

        QualityMetrics {
            total_entries: total,
            average_importance: avg_importance,
            average_content_length: avg_len,
            trivial_ratio: trivial_count as f64 / total as f64,
            duplicate_ratio: dup_count as f64 / total as f64,
            type_distribution: type_dist,
        }
    }

    /// Score a single entry's quality (0.0 to 1.0).
    pub fn score_entry(&self, entry: &MemoryEntry) -> f64 {
        let mut score: f64 = 0.0;

        // Importance component (40%)
        score += entry.importance * 0.4;

        // Content length component (30%) — longer is generally better
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

        // Non-trivial content (20%)
        let trivial_penalty = if is_trivial(&entry.content, self.min_content_length) {
            0.0
        } else {
            1.0
        };
        score += trivial_penalty * 0.2;

        // Above importance threshold bonus (10%)
        if entry.importance >= self.min_importance_threshold {
            score += 0.1;
        }

        score.clamp(0.0, 1.0)
    }

    /// Get the minimum importance threshold.
    pub fn min_importance(&self) -> f64 {
        self.min_importance_threshold
    }
}

impl Default for QualityEvaluator {
    fn default() -> Self {
        Self::new()
    }
}

/// Evaluates reliability of memory operations.
pub struct ReliabilityEvaluator {
    window_size: usize,
}

impl ReliabilityEvaluator {
    pub fn new(window_size: usize) -> Self {
        Self { window_size }
    }

    /// Evaluate reliability metrics from operation logs.
    ///
    /// Each log entry is `(success: bool, latency_ms: f64)`.
    pub fn evaluate(
        &self,
        store_results: &[(bool, f64)],
        retrieve_results: &[(bool, f64)],
    ) -> ReliabilityMetrics {
        let store_window = if store_results.len() > self.window_size {
            &store_results[store_results.len() - self.window_size..]
        } else {
            store_results
        };
        let retrieve_window = if retrieve_results.len() > self.window_size {
            &retrieve_results[retrieve_results.len() - self.window_size..]
        } else {
            retrieve_results
        };

        let store_success = store_window.iter().filter(|(ok, _)| *ok).count();
        let retrieve_success = retrieve_window.iter().filter(|(ok, _)| *ok).count();

        let store_rate = if store_window.is_empty() {
            1.0
        } else {
            store_success as f64 / store_window.len() as f64
        };
        let retrieve_rate = if retrieve_window.is_empty() {
            1.0
        } else {
            retrieve_success as f64 / retrieve_window.len() as f64
        };

        let avg_store_lat = if store_window.is_empty() {
            0.0
        } else {
            store_window.iter().map(|(_, lat)| lat).sum::<f64>() / store_window.len() as f64
        };
        let avg_retrieve_lat = if retrieve_window.is_empty() {
            0.0
        } else {
            retrieve_window.iter().map(|(_, lat)| lat).sum::<f64>()
                / retrieve_window.len() as f64
        };

        let errors = store_window.iter().filter(|(ok, _)| !*ok).count()
            + retrieve_window.iter().filter(|(ok, _)| !*ok).count();

        ReliabilityMetrics {
            store_success_rate: store_rate,
            retrieve_success_rate: retrieve_rate,
            average_store_latency_ms: avg_store_lat,
            average_retrieve_latency_ms: avg_retrieve_lat,
            error_count: errors,
        }
    }

    /// Get the window size.
    pub fn window_size(&self) -> usize {
        self.window_size
    }
}

/// Evaluates retrieval performance.
pub struct PerformanceEvaluator {
    k: usize,
}

impl PerformanceEvaluator {
    pub fn new(k: usize) -> Self {
        Self { k }
    }

    /// Evaluate performance metrics for retrieval results.
    ///
    /// - `results`: the retrieved entries
    /// - `expected`: IDs of entries considered relevant
    /// - `budget`: total token budget
    /// - `budget_used`: tokens actually consumed
    pub fn evaluate(
        &self,
        results: &[MemoryEntry],
        expected: &[String],
        budget: usize,
        budget_used: usize,
    ) -> PerformanceMetrics {
        let k = self.k.min(results.len());
        let top_k = &results[..k];
        let expected_set: HashSet<&str> = expected.iter().map(|s| s.as_str()).collect();

        let relevant_in_k = top_k
            .iter()
            .filter(|e| expected_set.contains(e.id.as_str()))
            .count();

        let precision = if k == 0 {
            0.0
        } else {
            relevant_in_k as f64 / k as f64
        };

        let recall = if expected.is_empty() {
            0.0
        } else {
            relevant_in_k as f64 / expected.len() as f64
        };

        // NDCG-style relevance: binary relevance with log discount
        let mut dcg = 0.0;
        for (i, entry) in top_k.iter().enumerate() {
            if expected_set.contains(entry.id.as_str()) {
                dcg += 1.0 / (i as f64 + 2.0).log2();
            }
        }
        let ideal_count = expected.len().min(k);
        let mut idcg = 0.0;
        for i in 0..ideal_count {
            idcg += 1.0 / (i as f64 + 2.0).log2();
        }
        let avg_relevance = if idcg == 0.0 { 0.0 } else { dcg / idcg };

        let budget_util = if budget == 0 {
            0.0
        } else {
            budget_used as f64 / budget as f64
        };

        PerformanceMetrics {
            average_relevance_score: avg_relevance,
            budget_utilization: budget_util,
            recall_at_k: recall,
            precision_at_k: precision,
        }
    }

    /// Get k value.
    pub fn k(&self) -> usize {
        self.k
    }
}

/// Generate a comprehensive quality report.
pub fn generate_report(
    entries: &[MemoryEntry],
    store_log: &[(bool, f64)],
    retrieve_log: &[(bool, f64)],
) -> QualityReport {
    let quality_eval = QualityEvaluator::new();
    let reliability_eval = ReliabilityEvaluator::new(1000);
    let perf_eval = PerformanceEvaluator::new(10);

    let quality = quality_eval.evaluate(entries);
    let reliability = reliability_eval.evaluate(store_log, retrieve_log);
    let performance = perf_eval.evaluate(entries, &[], 4000, 0);

    // Overall = 0.4 * quality_score + 0.3 * reliability_score + 0.3 * perf_score
    let quality_score = 1.0 - quality.trivial_ratio - quality.duplicate_ratio;
    let reliability_score =
        (reliability.store_success_rate + reliability.retrieve_success_rate) / 2.0;
    let perf_score = performance.average_relevance_score;
    let overall = quality_score * 0.4 + reliability_score * 0.3 + perf_score * 0.3;

    let mut recommendations = Vec::new();
    if quality.trivial_ratio > 0.2 {
        recommendations.push("High trivial content ratio — tighten content filter".into());
    }
    if quality.duplicate_ratio > 0.1 {
        recommendations.push("Duplicate ratio elevated — enable dedup detection".into());
    }
    if reliability.store_success_rate < 0.95 {
        recommendations.push("Store reliability below 95% — investigate failures".into());
    }
    if reliability.retrieve_success_rate < 0.95 {
        recommendations.push("Retrieve reliability below 95% — investigate failures".into());
    }
    if quality.average_importance < 0.3 {
        recommendations.push("Low average importance — review scoring heuristics".into());
    }

    QualityReport {
        quality,
        reliability,
        performance,
        overall_score: overall.clamp(0.0, 1.0),
        recommendations,
    }
}
