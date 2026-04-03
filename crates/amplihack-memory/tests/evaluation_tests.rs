//! Tests for memory quality evaluation.

use amplihack_memory::evaluation::{
    PerformanceEvaluator, PerformanceMetrics, QualityEvaluator, QualityMetrics, QualityReport,
    ReliabilityEvaluator, ReliabilityMetrics, generate_report,
};
use amplihack_memory::models::{MemoryEntry, MemoryType};

fn semantic_entry(content: &str) -> MemoryEntry {
    MemoryEntry::new("test-session", "agent-1", MemoryType::Semantic, content)
}

// ── QualityEvaluator ──

#[test]
fn quality_evaluator_default_thresholds() {
    let eval = QualityEvaluator::new();
    assert_eq!(eval.min_importance(), 0.3);
}

#[test]
fn quality_evaluator_custom_thresholds() {
    let eval = QualityEvaluator::with_thresholds(0.5, 20);
    assert_eq!(eval.min_importance(), 0.5);
}

#[test]
fn quality_evaluate_returns_metrics() {
    let eval = QualityEvaluator::new();
    let entries = vec![
        semantic_entry("Test quality evaluation content here"),
        semantic_entry("Another meaningful entry for analysis"),
    ];
    let metrics = eval.evaluate(&entries);
    assert_eq!(metrics.total_entries, 2);
    assert!(metrics.average_importance > 0.0);
    assert!(metrics.average_content_length > 0.0);
    assert_eq!(metrics.duplicate_ratio, 0.0);
}

#[test]
fn quality_evaluate_empty_entries() {
    let eval = QualityEvaluator::new();
    let metrics = eval.evaluate(&[]);
    assert_eq!(metrics.total_entries, 0);
    assert_eq!(metrics.average_importance, 0.0);
}

#[test]
fn quality_evaluate_detects_duplicates() {
    let eval = QualityEvaluator::new();
    let entries = vec![
        semantic_entry("Duplicate content for testing purposes"),
        semantic_entry("Duplicate content for testing purposes"),
    ];
    let metrics = eval.evaluate(&entries);
    assert!(metrics.duplicate_ratio > 0.0);
}

#[test]
fn quality_score_entry_returns_bounded_score() {
    let eval = QualityEvaluator::new();
    let entry = semantic_entry("Score this entry with meaningful content");
    let score = eval.score_entry(&entry);
    assert!(score >= 0.0 && score <= 1.0);
}

#[test]
fn quality_score_trivial_entry_low() {
    let eval = QualityEvaluator::new();
    let entry = semantic_entry("hi");
    let score = eval.score_entry(&entry);
    // Trivial content should score low
    assert!(score < 0.5);
}

// ── ReliabilityEvaluator ──

#[test]
fn reliability_evaluator_window_size() {
    let eval = ReliabilityEvaluator::new(100);
    assert_eq!(eval.window_size(), 100);
}

#[test]
fn reliability_evaluate_computes_rates() {
    let eval = ReliabilityEvaluator::new(50);
    let store_log = vec![(true, 1.5), (true, 2.0), (false, 10.0)];
    let retrieve_log = vec![(true, 0.5), (true, 0.8)];
    let metrics = eval.evaluate(&store_log, &retrieve_log);
    // 2/3 store success
    assert!((metrics.store_success_rate - 2.0 / 3.0).abs() < 0.01);
    // 2/2 retrieve success
    assert_eq!(metrics.retrieve_success_rate, 1.0);
    assert_eq!(metrics.error_count, 1);
    assert!(metrics.average_store_latency_ms > 0.0);
}

#[test]
fn reliability_evaluate_empty_logs() {
    let eval = ReliabilityEvaluator::new(50);
    let metrics = eval.evaluate(&[], &[]);
    assert_eq!(metrics.store_success_rate, 1.0);
    assert_eq!(metrics.retrieve_success_rate, 1.0);
    assert_eq!(metrics.error_count, 0);
}

// ── PerformanceEvaluator ──

#[test]
fn performance_evaluator_k_value() {
    let eval = PerformanceEvaluator::new(10);
    assert_eq!(eval.k(), 10);
}

#[test]
fn performance_evaluate_computes_metrics() {
    let eval = PerformanceEvaluator::new(5);
    let mut entry = semantic_entry("Retrieved result content for evaluation");
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
fn performance_evaluate_no_expected() {
    let eval = PerformanceEvaluator::new(5);
    let results = vec![semantic_entry("Some result content here")];
    let metrics = eval.evaluate(&results, &[], 4000, 0);
    assert_eq!(metrics.recall_at_k, 0.0);
    assert_eq!(metrics.precision_at_k, 0.0);
}

// ── QualityMetrics struct ──

#[test]
fn quality_metrics_default() {
    let m = QualityMetrics::default();
    assert_eq!(m.total_entries, 0);
    assert_eq!(m.average_importance, 0.0);
    assert_eq!(m.trivial_ratio, 0.0);
    assert_eq!(m.duplicate_ratio, 0.0);
}

// ── ReliabilityMetrics struct ──

#[test]
fn reliability_metrics_default() {
    let m = ReliabilityMetrics::default();
    assert_eq!(m.store_success_rate, 0.0);
    assert_eq!(m.retrieve_success_rate, 0.0);
    assert_eq!(m.error_count, 0);
}

// ── PerformanceMetrics struct ──

#[test]
fn performance_metrics_default() {
    let m = PerformanceMetrics::default();
    assert_eq!(m.average_relevance_score, 0.0);
    assert_eq!(m.budget_utilization, 0.0);
    assert_eq!(m.recall_at_k, 0.0);
}

// ── QualityReport struct ──

#[test]
fn quality_report_default() {
    let r = QualityReport::default();
    assert_eq!(r.overall_score, 0.0);
    assert!(r.recommendations.is_empty());
}

#[test]
fn quality_report_serializes() {
    let r = QualityReport::default();
    let json = serde_json::to_value(&r).unwrap();
    assert_eq!(json["overall_score"], 0.0);
}

// ── generate_report ──

#[test]
fn generate_report_produces_report() {
    let entries = vec![semantic_entry("Report test entry with meaningful content")];
    let store_log = vec![(true, 1.0)];
    let retrieve_log = vec![(true, 0.5)];
    let report = generate_report(&entries, &store_log, &retrieve_log);
    assert!(report.overall_score >= 0.0 && report.overall_score <= 1.0);
    assert_eq!(report.quality.total_entries, 1);
    assert_eq!(report.reliability.store_success_rate, 1.0);
}

#[test]
fn generate_report_recommends_on_low_reliability() {
    let entries = vec![semantic_entry("Report entry for recommendation test")];
    let store_log = vec![(true, 1.0), (false, 5.0), (false, 5.0), (false, 5.0)];
    let retrieve_log = vec![(true, 0.5)];
    let report = generate_report(&entries, &store_log, &retrieve_log);
    assert!(!report.recommendations.is_empty());
}
