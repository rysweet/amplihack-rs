//! Backend comparison: orchestrates quality, benchmark, and reliability
//! evaluations across backends and generates reports.
//!
//! Ported from Python `memory/evaluation/comparison.py`.

use crate::backend::MemoryBackend;

use super::benchmark::{
    BenchmarkEvaluator, RetrievalQualityEvaluator, check_performance_contracts, create_test_set,
};
use super::metrics::{
    BackendReliabilityMetrics, BenchmarkMetrics, ComparisonReport, RetrievalQualityMetrics,
};
use super::reliability_testing::BackendReliabilityEvaluator;

/// Orchestrates full evaluation of a backend: quality, benchmark, and
/// reliability, producing a [`ComparisonReport`].
pub struct BackendComparison;

impl BackendComparison {
    /// Evaluate a single backend across all dimensions.
    pub fn evaluate_backend(backend: &mut dyn MemoryBackend) -> ComparisonReport {
        let backend_name = backend.backend_name().to_string();

        let reliability_metrics = BackendReliabilityEvaluator::evaluate(backend);
        let test_queries = create_test_set(backend);
        let quality_metrics = RetrievalQualityEvaluator::evaluate(backend, &test_queries);
        let benchmark_metrics = BenchmarkEvaluator::evaluate(backend, 100);

        let overall = Self::calculate_overall_score(
            &quality_metrics, &benchmark_metrics, &reliability_metrics,
        );
        let recommendations = Self::generate_recommendations(
            &backend_name, &quality_metrics, &benchmark_metrics, &reliability_metrics,
        );

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .to_string();

        ComparisonReport {
            backend_name,
            quality_metrics,
            benchmark_metrics,
            reliability_metrics,
            overall_score: overall,
            recommendations,
            timestamp,
        }
    }

    /// Weighted overall score: quality 40%, performance 30%, reliability 30%.
    fn calculate_overall_score(
        quality: &RetrievalQualityMetrics,
        benchmark: &BenchmarkMetrics,
        reliability: &BackendReliabilityMetrics,
    ) -> f64 {
        let quality_score = (quality.precision + quality.recall) / 2.0;

        let contracts = check_performance_contracts(benchmark);
        let perf_parts = [
            if contracts.storage_latency_ok { 1.0 } else { 0.5 },
            if contracts.retrieval_latency_ok { 1.0 } else { 0.5 },
        ];
        let perf_score = perf_parts.iter().sum::<f64>() / perf_parts.len() as f64;

        let rel_score = (reliability.data_integrity_score
            + reliability.concurrent_safety_score
            + reliability.error_recovery_score)
            / 3.0;

        (quality_score * 0.4 + perf_score * 0.3 + rel_score * 0.3).clamp(0.0, 1.0)
    }

    /// Generate actionable recommendations based on evaluation results.
    fn generate_recommendations(
        backend_name: &str,
        quality: &RetrievalQualityMetrics,
        benchmark: &BenchmarkMetrics,
        reliability: &BackendReliabilityMetrics,
    ) -> Vec<String> {
        let mut recs = Vec::new();
        if quality.precision > 0.8 && quality.recall > 0.8 {
            recs.push(format!("{backend_name} excels at retrieval quality"));
        } else if quality.precision > 0.7 {
            recs.push(format!("{backend_name} has good precision"));
        }
        if benchmark.storage_latency_ms < 100.0 {
            recs.push("Fast storage — good for high-write workloads".into());
        }
        if benchmark.retrieval_latency_ms < 10.0 {
            recs.push("Ultra-fast retrieval — excellent for real-time use".into());
        }
        if reliability.data_integrity_score > 0.95 {
            recs.push("Excellent data integrity".into());
        }
        if reliability.concurrent_safety_score > 0.9 {
            recs.push("Handles concurrent operations well".into());
        }
        match backend_name {
            "sqlite" => recs.push("Best for single-process, simple deployments".into()),
            "in_memory" => recs.push("Best for testing and ephemeral workloads".into()),
            _ => {}
        }
        recs
    }

    /// Generate a markdown comparison report for multiple backends.
    pub fn generate_markdown_report(reports: &[ComparisonReport]) -> String {
        let mut md = String::from("# Memory Backend Evaluation Report\n\n");
        md.push_str("| Backend | Overall | Quality | Performance | Reliability |\n");
        md.push_str("|---------|---------|---------|-------------|-------------|\n");

        let mut sorted: Vec<&ComparisonReport> = reports.iter().collect();
        sorted.sort_by(|a, b| {
            b.overall_score
                .partial_cmp(&a.overall_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        for r in &sorted {
            let q = (r.quality_metrics.precision + r.quality_metrics.recall) / 2.0;
            let contracts = check_performance_contracts(&r.benchmark_metrics);
            let p_ok = [contracts.storage_latency_ok, contracts.retrieval_latency_ok]
                .iter()
                .filter(|x| **x)
                .count();
            let rel = (r.reliability_metrics.data_integrity_score
                + r.reliability_metrics.concurrent_safety_score
                + r.reliability_metrics.error_recovery_score)
                / 3.0;
            md.push_str(&format!(
                "| {} | {:.2} | {:.2} | {}/{} | {:.2} |\n",
                r.backend_name, r.overall_score, q, p_ok, 2, rel,
            ));
        }

        for r in &sorted {
            md.push_str(&format!("\n## {}\n\n", r.backend_name));
            md.push_str("### Quality\n");
            md.push_str(&format!("- Precision: {:.3}\n", r.quality_metrics.precision));
            md.push_str(&format!("- Recall: {:.3}\n", r.quality_metrics.recall));
            md.push_str(&format!("- NDCG: {:.3}\n", r.quality_metrics.ndcg_score));

            md.push_str("\n### Performance\n");
            let contracts = check_performance_contracts(&r.benchmark_metrics);
            let check = |ok: bool| if ok { "✅" } else { "❌" };
            md.push_str(&format!(
                "- Storage latency: {:.2}ms {}\n",
                r.benchmark_metrics.storage_latency_ms, check(contracts.storage_latency_ok)
            ));
            md.push_str(&format!(
                "- Retrieval latency: {:.2}ms {}\n",
                r.benchmark_metrics.retrieval_latency_ms, check(contracts.retrieval_latency_ok)
            ));

            md.push_str("\n### Reliability\n");
            md.push_str(&format!("- Data integrity: {:.2}\n", r.reliability_metrics.data_integrity_score));
            md.push_str(&format!("- Sequential safety: {:.2}\n", r.reliability_metrics.concurrent_safety_score));
            md.push_str(&format!("- Error recovery: {:.2}\n", r.reliability_metrics.error_recovery_score));

            if !r.recommendations.is_empty() {
                md.push_str("\n### Recommendations\n");
                for rec in &r.recommendations {
                    md.push_str(&format!("- {rec}\n"));
                }
            }
        }

        md
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::InMemoryBackend;

    #[test]
    fn comparison_evaluate_backend() {
        let mut backend = InMemoryBackend::new();
        let report = BackendComparison::evaluate_backend(&mut backend);
        assert_eq!(report.backend_name, "in_memory");
        assert!((0.0..=1.0).contains(&report.overall_score));
        assert!(!report.timestamp.is_empty());
    }

    #[test]
    fn comparison_overall_score_bounded() {
        let quality = RetrievalQualityMetrics { precision: 1.0, recall: 1.0, ..Default::default() };
        let benchmark = BenchmarkMetrics { storage_latency_ms: 1.0, retrieval_latency_ms: 1.0, ..Default::default() };
        let reliability = BackendReliabilityMetrics {
            data_integrity_score: 1.0, concurrent_safety_score: 1.0, error_recovery_score: 1.0, ..Default::default()
        };
        let score = BackendComparison::calculate_overall_score(&quality, &benchmark, &reliability);
        assert!((0.0..=1.0).contains(&score));
    }

    #[test]
    fn comparison_recommendations_quality() {
        let quality = RetrievalQualityMetrics { precision: 0.9, recall: 0.9, ..Default::default() };
        let recs = BackendComparison::generate_recommendations(
            "test", &quality, &BenchmarkMetrics::default(), &BackendReliabilityMetrics::default(),
        );
        assert!(recs.iter().any(|r| r.contains("excels")));
    }

    #[test]
    fn comparison_recommendations_backend_specific() {
        let recs = BackendComparison::generate_recommendations(
            "sqlite", &RetrievalQualityMetrics::default(), &BenchmarkMetrics::default(),
            &BackendReliabilityMetrics::default(),
        );
        assert!(recs.iter().any(|r| r.contains("single-process")));
    }

    #[test]
    fn markdown_report_generation() {
        let mut backend = InMemoryBackend::new();
        let report = BackendComparison::evaluate_backend(&mut backend);
        let md = BackendComparison::generate_markdown_report(&[report]);
        assert!(md.contains("# Memory Backend Evaluation Report"));
        assert!(md.contains("in_memory"));
        assert!(md.contains("Precision"));
    }

    #[test]
    fn markdown_report_empty() {
        let md = BackendComparison::generate_markdown_report(&[]);
        assert!(md.contains("# Memory Backend Evaluation Report"));
    }

    #[test]
    fn markdown_report_sorted_by_score() {
        let report1 = ComparisonReport {
            backend_name: "low_score".into(),
            quality_metrics: RetrievalQualityMetrics::default(),
            benchmark_metrics: BenchmarkMetrics::default(),
            reliability_metrics: BackendReliabilityMetrics::default(),
            overall_score: 0.3,
            recommendations: vec![],
            timestamp: "0".into(),
        };
        let report2 = ComparisonReport {
            backend_name: "high_score".into(),
            quality_metrics: RetrievalQualityMetrics::default(),
            benchmark_metrics: BenchmarkMetrics::default(),
            reliability_metrics: BackendReliabilityMetrics::default(),
            overall_score: 0.9,
            recommendations: vec![],
            timestamp: "0".into(),
        };
        let md = BackendComparison::generate_markdown_report(&[report1, report2]);
        let high_pos = md.find("high_score").unwrap();
        let low_pos = md.find("low_score").unwrap();
        assert!(high_pos < low_pos, "higher score should appear first");
    }
}
