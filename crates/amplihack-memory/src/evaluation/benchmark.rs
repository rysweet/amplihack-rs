//! Backend benchmarking: latency/throughput measurement and retrieval quality
//! evaluation against ground-truth test queries.
//!
//! Ported from Python `memory/evaluation/performance_evaluator.py` and
//! `memory/evaluation/quality_evaluator.py`.

use crate::backend::MemoryBackend;
use crate::models::{MemoryEntry, MemoryQuery, MemoryType};
use std::collections::HashSet;
use std::time::Instant;

use super::metrics::{
    BenchmarkMetrics, PerformanceContracts, QueryTestCase, RetrievalQualityMetrics,
};

const MAX_STORAGE_LATENCY_MS: f64 = 500.0;
const MAX_RETRIEVAL_LATENCY_MS: f64 = 50.0;
const MIN_STORAGE_THROUGHPUT: f64 = 2.0;
const MIN_RETRIEVAL_THROUGHPUT: f64 = 20.0;

/// Check benchmark results against performance contracts.
pub fn check_performance_contracts(metrics: &BenchmarkMetrics) -> PerformanceContracts {
    PerformanceContracts {
        storage_latency_ok: metrics.storage_latency_ms < MAX_STORAGE_LATENCY_MS,
        retrieval_latency_ok: metrics.retrieval_latency_ms < MAX_RETRIEVAL_LATENCY_MS,
        storage_throughput_ok: metrics.storage_throughput >= MIN_STORAGE_THROUGHPUT,
        retrieval_throughput_ok: metrics.retrieval_throughput >= MIN_RETRIEVAL_THROUGHPUT,
    }
}

/// Measures storage/retrieval latency and throughput for a backend.
pub struct BenchmarkEvaluator;

impl BenchmarkEvaluator {
    /// Run latency/throughput benchmarks on a backend.
    pub fn evaluate(backend: &mut dyn MemoryBackend, num_operations: usize) -> BenchmarkMetrics {
        let backend_name = backend.backend_name().to_string();
        let ops = num_operations.max(1);

        let entries: Vec<MemoryEntry> = (0..ops)
            .map(|i| {
                MemoryEntry::new(
                    "bench-session",
                    "bench-agent",
                    MemoryType::Semantic,
                    format!("Benchmark storage entry number {i} with padding content"),
                )
            })
            .collect();

        let mut store_latencies = Vec::with_capacity(ops);
        for entry in &entries {
            let start = Instant::now();
            let _ = backend.store(entry);
            store_latencies.push(start.elapsed().as_secs_f64() * 1000.0);
        }

        let queries: Vec<String> = (0..ops).map(|i| format!("entry number {i}")).collect();
        let mut retrieve_latencies = Vec::with_capacity(ops);
        for q in &queries {
            let query = MemoryQuery::new(q.as_str());
            let start = Instant::now();
            let _ = backend.retrieve(&query);
            retrieve_latencies.push(start.elapsed().as_secs_f64() * 1000.0);
        }

        let avg_store = mean(&store_latencies);
        let avg_retrieve = mean(&retrieve_latencies);

        BenchmarkMetrics {
            storage_latency_ms: avg_store,
            retrieval_latency_ms: avg_retrieve,
            storage_throughput: if avg_store > 0.0 {
                1000.0 / avg_store
            } else {
                0.0
            },
            retrieval_throughput: if avg_retrieve > 0.0 {
                1000.0 / avg_retrieve
            } else {
                0.0
            },
            num_memories: ops,
            backend_name,
        }
    }
}

/// Evaluates retrieval quality against ground-truth test queries.
///
/// Ported from Python `QualityEvaluator` which uses precision, recall,
/// and NDCG to measure retrieval effectiveness.
pub struct RetrievalQualityEvaluator;

impl RetrievalQualityEvaluator {
    /// Evaluate retrieval quality using pre-defined test queries.
    pub fn evaluate(
        backend: &dyn MemoryBackend,
        test_queries: &[QueryTestCase],
    ) -> RetrievalQualityMetrics {
        let backend_name = backend.backend_name().to_string();
        if test_queries.is_empty() {
            return RetrievalQualityMetrics {
                backend_name,
                ..Default::default()
            };
        }

        let mut precision_sum = 0.0;
        let mut recall_sum = 0.0;
        let mut relevance_sum = 0.0;
        let mut ndcg_sum = 0.0;

        for tc in test_queries {
            let mut query = MemoryQuery::new(&tc.query_text);
            if let Some(mt) = tc.memory_type {
                query = query.with_types(vec![mt]);
            }

            let results = backend.retrieve(&query).unwrap_or_default();
            let relevant: HashSet<&str> =
                tc.relevant_memory_ids.iter().map(|s| s.as_str()).collect();
            let retrieved_ids: Vec<&str> = results.iter().map(|e| e.id.as_str()).collect();
            let retrieved_set: HashSet<&str> = retrieved_ids.iter().copied().collect();
            let hits = retrieved_set.intersection(&relevant).count();

            let precision = if retrieved_ids.is_empty() {
                0.0
            } else {
                hits as f64 / retrieved_ids.len() as f64
            };
            let recall = if relevant.is_empty() {
                0.0
            } else {
                hits as f64 / relevant.len() as f64
            };
            let relevance = if results.is_empty() {
                0.0
            } else {
                hits as f64 / results.len() as f64
            };
            let ndcg = calculate_ndcg(&retrieved_ids, &relevant);

            precision_sum += precision;
            recall_sum += recall;
            relevance_sum += relevance;
            ndcg_sum += ndcg;
        }

        let n = test_queries.len() as f64;
        RetrievalQualityMetrics {
            relevance_score: relevance_sum / n,
            precision: precision_sum / n,
            recall: recall_sum / n,
            ndcg_score: ndcg_sum / n,
            num_queries: test_queries.len(),
            backend_name,
        }
    }
}

/// Calculate NDCG using the bit_length optimization from the Python source.
fn calculate_ndcg(retrieved: &[&str], relevant: &HashSet<&str>) -> f64 {
    if relevant.is_empty() {
        return 0.0;
    }
    let mut dcg = 0.0_f64;
    for (i, id) in retrieved.iter().enumerate() {
        if relevant.contains(id) {
            let bits = u64::BITS - ((i as u64 + 2).leading_zeros());
            dcg += 1.0 / bits as f64;
        }
    }
    let ideal_count = relevant.len().min(retrieved.len());
    let mut idcg = 0.0_f64;
    for i in 0..ideal_count {
        let bits = u64::BITS - ((i as u64 + 2).leading_zeros());
        idcg += 1.0 / bits as f64;
    }
    if idcg == 0.0 { 0.0 } else { dcg / idcg }
}

/// Create a default test set of queries with ground-truth relevance.
pub fn create_test_set(backend: &mut dyn MemoryBackend) -> Vec<QueryTestCase> {
    let test_data: Vec<(&str, MemoryType)> = vec![
        (
            "Discussed feature flags implementation approach",
            MemoryType::Episodic,
        ),
        (
            "Reviewed pull request for auth module changes",
            MemoryType::Episodic,
        ),
        (
            "Learned that Rust pattern matching is exhaustive",
            MemoryType::Semantic,
        ),
        (
            "Discovered serde derive macro for serialization",
            MemoryType::Semantic,
        ),
        (
            "Workflow: run cargo clippy before committing code",
            MemoryType::Procedural,
        ),
        (
            "Step by step guide for deploying to production",
            MemoryType::Procedural,
        ),
        (
            "TODO: refactor error handling in coordinator module",
            MemoryType::Prospective,
        ),
        (
            "Reminder to update documentation before release",
            MemoryType::Prospective,
        ),
        (
            "Currently debugging memory leak in graph store",
            MemoryType::Working,
        ),
        (
            "Active task: implementing bloom filter optimization",
            MemoryType::Working,
        ),
    ];

    let mut ids_by_type: std::collections::HashMap<&str, Vec<String>> =
        std::collections::HashMap::new();
    for (content, mt) in &test_data {
        let entry = MemoryEntry::new("test-session", "test-agent", *mt, *content);
        let id = entry.id.clone();
        let _ = backend.store(&entry);
        ids_by_type.entry(mt.as_str()).or_default().push(id);
    }

    vec![
        QueryTestCase {
            query_text: "feature flags implementation".into(),
            relevant_memory_ids: ids_by_type.get("episodic").cloned().unwrap_or_default(),
            memory_type: None,
        },
        QueryTestCase {
            query_text: "Rust pattern matching serde".into(),
            relevant_memory_ids: ids_by_type.get("semantic").cloned().unwrap_or_default(),
            memory_type: None,
        },
        QueryTestCase {
            query_text: "workflow deploying production".into(),
            relevant_memory_ids: ids_by_type.get("procedural").cloned().unwrap_or_default(),
            memory_type: None,
        },
    ]
}

fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.iter().sum::<f64>() / values.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::InMemoryBackend;

    #[test]
    fn benchmark_evaluator_runs() {
        let mut backend = InMemoryBackend::new();
        let metrics = BenchmarkEvaluator::evaluate(&mut backend, 10);
        assert_eq!(metrics.num_memories, 10);
        assert!(metrics.storage_latency_ms >= 0.0);
        assert!(metrics.storage_throughput > 0.0);
        assert_eq!(metrics.backend_name, "in_memory");
    }

    #[test]
    fn benchmark_min_operations() {
        let mut backend = InMemoryBackend::new();
        let metrics = BenchmarkEvaluator::evaluate(&mut backend, 0);
        assert_eq!(metrics.num_memories, 1);
    }

    #[test]
    fn performance_contracts_pass_for_in_memory() {
        let mut backend = InMemoryBackend::new();
        let metrics = BenchmarkEvaluator::evaluate(&mut backend, 10);
        let contracts = check_performance_contracts(&metrics);
        assert!(contracts.storage_latency_ok);
        assert!(contracts.retrieval_latency_ok);
    }

    #[test]
    fn performance_contracts_detect_violations() {
        let metrics = BenchmarkMetrics {
            storage_latency_ms: 600.0,
            retrieval_latency_ms: 100.0,
            storage_throughput: 1.0,
            retrieval_throughput: 10.0,
            ..Default::default()
        };
        let contracts = check_performance_contracts(&metrics);
        assert!(!contracts.storage_latency_ok);
        assert!(!contracts.retrieval_latency_ok);
        assert!(!contracts.storage_throughput_ok);
        assert!(!contracts.retrieval_throughput_ok);
    }

    #[test]
    fn retrieval_quality_empty_queries() {
        let backend = InMemoryBackend::new();
        let metrics = RetrievalQualityEvaluator::evaluate(&backend, &[]);
        assert_eq!(metrics.num_queries, 0);
    }

    #[test]
    fn retrieval_quality_with_test_set() {
        let mut backend = InMemoryBackend::new();
        let test_queries = create_test_set(&mut backend);
        let metrics = RetrievalQualityEvaluator::evaluate(&backend, &test_queries);
        assert_eq!(metrics.num_queries, 3);
        assert_eq!(metrics.backend_name, "in_memory");
    }

    #[test]
    fn ndcg_perfect_score() {
        let relevant: HashSet<&str> = ["a", "b"].into_iter().collect();
        let retrieved = vec!["a", "b"];
        assert!((calculate_ndcg(&retrieved, &relevant) - 1.0).abs() < 0.001);
    }

    #[test]
    fn ndcg_empty_relevant() {
        let relevant: HashSet<&str> = HashSet::new();
        assert_eq!(calculate_ndcg(&["a"], &relevant), 0.0);
    }

    #[test]
    fn ndcg_no_hits() {
        let relevant: HashSet<&str> = ["x", "y"].into_iter().collect();
        assert_eq!(calculate_ndcg(&["a", "b"], &relevant), 0.0);
    }

    #[test]
    fn ndcg_partial_match() {
        let relevant: HashSet<&str> = ["a", "b"].into_iter().collect();
        let score = calculate_ndcg(&["c", "a"], &relevant);
        assert!(score > 0.0 && score < 1.0);
    }

    #[test]
    fn create_test_set_populates_backend() {
        let mut backend = InMemoryBackend::new();
        let queries = create_test_set(&mut backend);
        assert_eq!(queries.len(), 3);
        for q in &queries {
            assert!(!q.relevant_memory_ids.is_empty());
        }
    }

    #[test]
    fn mean_empty() {
        assert_eq!(mean(&[]), 0.0);
    }

    #[test]
    fn mean_values() {
        assert!((mean(&[1.0, 2.0, 3.0]) - 2.0).abs() < f64::EPSILON);
    }
}
