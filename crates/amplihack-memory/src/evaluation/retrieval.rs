//! Retrieval-level evaluation: precision/recall/NDCG and operation reliability.
//!
//! - `PerformanceEvaluator`: measures retrieval quality against expected results
//! - `ReliabilityEvaluator`: computes success rates from operation logs

use crate::models::MemoryEntry;
use std::collections::HashSet;

use super::metrics::{PerformanceMetrics, ReliabilityMetrics};

/// Evaluates retrieval quality with precision@k, recall@k, and NDCG.
pub struct PerformanceEvaluator {
    k: usize,
}

impl PerformanceEvaluator {
    pub fn new(k: usize) -> Self {
        Self { k }
    }

    /// Evaluate retrieval results against expected relevant IDs.
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

        // NDCG: binary relevance with log2 discount
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

    pub fn k(&self) -> usize {
        self.k
    }
}

/// Evaluates reliability from operation logs.
pub struct ReliabilityEvaluator {
    window_size: usize,
}

impl ReliabilityEvaluator {
    pub fn new(window_size: usize) -> Self {
        Self { window_size }
    }

    /// Evaluate reliability from `(success, latency_ms)` logs.
    pub fn evaluate(
        &self,
        store_results: &[(bool, f64)],
        retrieve_results: &[(bool, f64)],
    ) -> ReliabilityMetrics {
        let store = Self::compute_window_stats(store_results, self.window_size);
        let retrieve = Self::compute_window_stats(retrieve_results, self.window_size);

        ReliabilityMetrics {
            store_success_rate: store.0,
            retrieve_success_rate: retrieve.0,
            average_store_latency_ms: store.1,
            average_retrieve_latency_ms: retrieve.1,
            error_count: store.2 + retrieve.2,
        }
    }

    fn compute_window_stats(results: &[(bool, f64)], window: usize) -> (f64, f64, usize) {
        let w = if results.len() > window {
            &results[results.len() - window..]
        } else {
            results
        };
        if w.is_empty() {
            return (1.0, 0.0, 0);
        }
        let successes = w.iter().filter(|(ok, _)| *ok).count();
        let failures = w.len() - successes;
        let rate = successes as f64 / w.len() as f64;
        let avg_lat = w.iter().map(|(_, lat)| lat).sum::<f64>() / w.len() as f64;
        (rate, avg_lat, failures)
    }

    pub fn window_size(&self) -> usize {
        self.window_size
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
    fn performance_k_value() {
        let eval = PerformanceEvaluator::new(10);
        assert_eq!(eval.k(), 10);
    }

    #[test]
    fn performance_computes_metrics() {
        let eval = PerformanceEvaluator::new(5);
        let mut entry = semantic_entry("Result content");
        entry.id = "expected-id".to_string();
        let results = vec![entry];
        let expected = vec!["expected-id".to_string()];
        let metrics = eval.evaluate(&results, &expected, 4000, 2000);
        assert_eq!(metrics.precision_at_k, 1.0);
        assert_eq!(metrics.recall_at_k, 1.0);
        assert_eq!(metrics.budget_utilization, 0.5);
        assert!(metrics.average_relevance_score > 0.0);
    }

    #[test]
    fn performance_no_expected() {
        let eval = PerformanceEvaluator::new(5);
        let results = vec![semantic_entry("Some result")];
        let metrics = eval.evaluate(&results, &[], 4000, 0);
        assert_eq!(metrics.recall_at_k, 0.0);
        assert_eq!(metrics.precision_at_k, 0.0);
    }

    #[test]
    fn performance_empty_results() {
        let eval = PerformanceEvaluator::new(5);
        let expected = vec!["id1".to_string()];
        let metrics = eval.evaluate(&[], &expected, 100, 0);
        assert_eq!(metrics.precision_at_k, 0.0);
        assert_eq!(metrics.recall_at_k, 0.0);
    }

    #[test]
    fn reliability_window_size() {
        let eval = ReliabilityEvaluator::new(100);
        assert_eq!(eval.window_size(), 100);
    }

    #[test]
    fn reliability_computes_rates() {
        let eval = ReliabilityEvaluator::new(50);
        let store_log = vec![(true, 1.5), (true, 2.0), (false, 10.0)];
        let retrieve_log = vec![(true, 0.5), (true, 0.8)];
        let metrics = eval.evaluate(&store_log, &retrieve_log);
        assert!((metrics.store_success_rate - 2.0 / 3.0).abs() < 0.01);
        assert_eq!(metrics.retrieve_success_rate, 1.0);
        assert_eq!(metrics.error_count, 1);
    }

    #[test]
    fn reliability_empty_logs() {
        let eval = ReliabilityEvaluator::new(50);
        let metrics = eval.evaluate(&[], &[]);
        assert_eq!(metrics.store_success_rate, 1.0);
        assert_eq!(metrics.error_count, 0);
    }

    #[test]
    fn reliability_windowing() {
        let eval = ReliabilityEvaluator::new(2);
        // Only last 2 entries should be considered
        let log = vec![(false, 5.0), (true, 1.0), (true, 2.0)];
        let metrics = eval.evaluate(&log, &[]);
        assert_eq!(metrics.store_success_rate, 1.0);
    }

    #[test]
    fn ndcg_perfect_ranking() {
        let eval = PerformanceEvaluator::new(3);
        let entries: Vec<MemoryEntry> = (0..3)
            .map(|i| {
                let mut e = semantic_entry(&format!("entry {i}"));
                e.id = format!("id{i}");
                e
            })
            .collect();
        // All are relevant and in perfect order
        let expected: Vec<String> = (0..3).map(|i| format!("id{i}")).collect();
        let metrics = eval.evaluate(&entries, &expected, 4000, 1000);
        assert!((metrics.average_relevance_score - 1.0).abs() < 0.001);
    }
}
