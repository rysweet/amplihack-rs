//! Memory evaluation framework.
//!
//! **Content-level** (static analysis of stored entries):
//! - [`QualityEvaluator`] — entry quality scoring
//! - [`ReliabilityEvaluator`] — success rates from operation logs
//! - [`PerformanceEvaluator`] — precision/recall/NDCG on retrieval results
//!
//! **Backend-level** (active testing against a [`MemoryBackend`](crate::backend::MemoryBackend)):
//! - [`RetrievalQualityEvaluator`] — ground-truth retrieval quality
//! - [`BenchmarkEvaluator`] — latency and throughput measurement
//! - [`BackendReliabilityEvaluator`] — integrity, safety, error recovery
//! - [`BackendComparison`] — orchestrates all evaluations and generates reports

pub mod benchmark;
pub mod comparison;
pub mod content;
pub mod metrics;
pub mod reliability_testing;
pub mod retrieval;

pub use content::QualityEvaluator;
pub use metrics::{PerformanceMetrics, QualityMetrics, QualityReport, ReliabilityMetrics};
pub use retrieval::{PerformanceEvaluator, ReliabilityEvaluator};

pub use benchmark::{
    BenchmarkEvaluator, RetrievalQualityEvaluator, check_performance_contracts, create_test_set,
};
pub use comparison::BackendComparison;
pub use metrics::{
    BackendReliabilityMetrics, BenchmarkMetrics, ComparisonReport, PerformanceContracts,
    QueryTestCase, RetrievalQualityMetrics,
};
pub use reliability_testing::BackendReliabilityEvaluator;

use crate::models::MemoryEntry;

/// Generate a comprehensive quality report from entries and operation logs.
pub fn generate_report(
    entries: &[MemoryEntry],
    store_log: &[(bool, f64)],
    retrieve_log: &[(bool, f64)],
) -> QualityReport {
    let quality_eval = QualityEvaluator::new();
    let reliability_eval = ReliabilityEvaluator::new(1000);

    let quality = quality_eval.evaluate(entries);
    let reliability = reliability_eval.evaluate(store_log, retrieve_log);
    let performance = PerformanceMetrics::default();

    let quality_score = (1.0 - quality.trivial_ratio - quality.duplicate_ratio).max(0.0);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::MemoryType;

    fn semantic_entry(content: &str) -> MemoryEntry {
        MemoryEntry::new("s1", "a1", MemoryType::Semantic, content)
    }

    #[test]
    fn generate_report_produces_report() {
        let entries = vec![semantic_entry("Report test entry with meaningful content")];
        let report = generate_report(&entries, &[(true, 1.0)], &[(true, 0.5)]);
        assert!((0.0..=1.0).contains(&report.overall_score));
        assert_eq!(report.quality.total_entries, 1);
        assert_eq!(report.reliability.store_success_rate, 1.0);
    }

    #[test]
    fn generate_report_recommends_on_low_reliability() {
        let entries = vec![semantic_entry("Report entry for recommendation test")];
        let store_log = vec![(true, 1.0), (false, 5.0), (false, 5.0), (false, 5.0)];
        let report = generate_report(&entries, &store_log, &[(true, 0.5)]);
        assert!(!report.recommendations.is_empty());
    }

    #[test]
    fn generate_report_empty_data() {
        let report = generate_report(&[], &[], &[]);
        assert_eq!(report.quality.total_entries, 0);
        assert_eq!(report.reliability.store_success_rate, 1.0);
    }
}
