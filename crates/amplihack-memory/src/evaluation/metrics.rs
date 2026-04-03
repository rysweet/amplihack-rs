//! Metric and report structs for memory evaluation.
//!
//! Contains both content-level metrics (entry quality, operation reliability,
//! retrieval performance) and backend-level metrics ported from the Python
//! evaluation framework (retrieval quality, latency benchmarks, backend
//! reliability, comparison reports).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ── Content-level metrics (existing) ──────────────────────────────

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

/// Reliability metrics from operation logs.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReliabilityMetrics {
    pub store_success_rate: f64,
    pub retrieve_success_rate: f64,
    pub average_store_latency_ms: f64,
    pub average_retrieve_latency_ms: f64,
    pub error_count: usize,
}

/// Retrieval performance metrics (precision/recall/NDCG).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PerformanceMetrics {
    pub average_relevance_score: f64,
    pub budget_utilization: f64,
    pub recall_at_k: f64,
    pub precision_at_k: f64,
}

/// Overall quality report combining all content-level metrics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QualityReport {
    pub quality: QualityMetrics,
    pub reliability: ReliabilityMetrics,
    pub performance: PerformanceMetrics,
    pub overall_score: f64,
    pub recommendations: Vec<String>,
}

// ── Backend-level metrics (ported from Python) ────────────────────

/// Retrieval quality evaluated against ground-truth test queries.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RetrievalQualityMetrics {
    pub relevance_score: f64,
    pub precision: f64,
    pub recall: f64,
    pub ndcg_score: f64,
    pub num_queries: usize,
    pub backend_name: String,
}

/// Latency and throughput benchmarks for a backend.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BenchmarkMetrics {
    pub storage_latency_ms: f64,
    pub retrieval_latency_ms: f64,
    pub storage_throughput: f64,
    pub retrieval_throughput: f64,
    pub num_memories: usize,
    pub backend_name: String,
}

/// Pass/fail checks against performance targets.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PerformanceContracts {
    pub storage_latency_ok: bool,
    pub retrieval_latency_ok: bool,
    pub storage_throughput_ok: bool,
    pub retrieval_throughput_ok: bool,
}

/// Active reliability testing results for a backend.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BackendReliabilityMetrics {
    pub data_integrity_score: f64,
    pub concurrent_safety_score: f64,
    pub error_recovery_score: f64,
    pub num_tests: usize,
    pub backend_name: String,
}

/// Full comparison report for a single backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComparisonReport {
    pub backend_name: String,
    pub quality_metrics: RetrievalQualityMetrics,
    pub benchmark_metrics: BenchmarkMetrics,
    pub reliability_metrics: BackendReliabilityMetrics,
    pub overall_score: f64,
    pub recommendations: Vec<String>,
    pub timestamp: String,
}

/// Test query with known-relevant memory IDs for ground-truth evaluation.
#[derive(Debug, Clone)]
pub struct QueryTestCase {
    pub query_text: String,
    pub relevant_memory_ids: Vec<String>,
    pub memory_type: Option<crate::models::MemoryType>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_metrics_default() {
        let m = QualityMetrics::default();
        assert_eq!(m.total_entries, 0);
        assert_eq!(m.average_importance, 0.0);
    }

    #[test]
    fn reliability_metrics_default() {
        let m = ReliabilityMetrics::default();
        assert_eq!(m.store_success_rate, 0.0);
        assert_eq!(m.error_count, 0);
    }

    #[test]
    fn performance_metrics_default() {
        let m = PerformanceMetrics::default();
        assert_eq!(m.recall_at_k, 0.0);
    }

    #[test]
    fn benchmark_metrics_default() {
        let m = BenchmarkMetrics::default();
        assert_eq!(m.storage_latency_ms, 0.0);
        assert_eq!(m.storage_throughput, 0.0);
    }

    #[test]
    fn backend_reliability_default() {
        let m = BackendReliabilityMetrics::default();
        assert_eq!(m.data_integrity_score, 0.0);
        assert_eq!(m.num_tests, 0);
    }

    #[test]
    fn retrieval_quality_default() {
        let m = RetrievalQualityMetrics::default();
        assert_eq!(m.precision, 0.0);
        assert_eq!(m.ndcg_score, 0.0);
    }

    #[test]
    fn performance_contracts_default() {
        let c = PerformanceContracts::default();
        assert!(!c.storage_latency_ok);
        assert!(!c.retrieval_throughput_ok);
    }

    #[test]
    fn quality_report_serializes() {
        let r = QualityReport::default();
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["overall_score"], 0.0);
    }

    #[test]
    fn comparison_report_serializes() {
        let r = ComparisonReport {
            backend_name: "test".into(),
            quality_metrics: RetrievalQualityMetrics::default(),
            benchmark_metrics: BenchmarkMetrics::default(),
            reliability_metrics: BackendReliabilityMetrics::default(),
            overall_score: 0.75,
            recommendations: vec!["good backend".into()],
            timestamp: "2024-01-01T00:00:00Z".into(),
        };
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["overall_score"], 0.75);
        assert_eq!(json["backend_name"], "test");
    }

    #[test]
    fn query_test_case_creation() {
        let tc = QueryTestCase {
            query_text: "find patterns".into(),
            relevant_memory_ids: vec!["m1".into(), "m2".into()],
            memory_type: None,
        };
        assert_eq!(tc.relevant_memory_ids.len(), 2);
        assert!(tc.memory_type.is_none());
    }
}
