//! Metrics computation and scoring engine for MCP evaluation.
//!
//! Scoring thresholds:
//! - INTEGRATE: quality ≥ 80% AND efficiency ≥ 1.5x baseline
//! - CONSIDER: quality ≥ 60% OR efficiency ≥ 1.2x baseline
//! - DONT_INTEGRATE: below both thresholds

use super::scenario::ScenarioResult;
use serde::Serialize;
use std::time::Duration;

/// Aggregated metrics from all scenario runs.
#[derive(Debug, Clone, Serialize)]
pub struct EvaluationMetrics {
    /// Average success rate across all scenarios (0.0 - 1.0).
    pub quality_score: f64,
    /// Efficiency multiplier vs baseline (>1.0 means faster than manual).
    pub efficiency_score: f64,
    /// Total operations measured.
    pub total_operations: usize,
    /// Total successes.
    pub total_successes: usize,
    /// Total time spent.
    pub total_duration: Duration,
}

/// Final recommendation based on scoring thresholds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Recommendation {
    /// Quality ≥ 80% AND efficiency ≥ 1.5x
    #[serde(rename = "INTEGRATE")]
    Integrate,
    /// Quality ≥ 60% OR efficiency ≥ 1.2x
    #[serde(rename = "CONSIDER")]
    Consider,
    /// Below both thresholds
    #[serde(rename = "DONT_INTEGRATE")]
    DontIntegrate,
}

impl std::fmt::Display for Recommendation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Integrate => write!(f, "INTEGRATE"),
            Self::Consider => write!(f, "CONSIDER"),
            Self::DontIntegrate => write!(f, "DONT_INTEGRATE"),
        }
    }
}

/// Computes metrics and recommendations from scenario results.
pub struct ScoringEngine {
    /// Quality threshold for INTEGRATE
    pub integrate_quality: f64,
    /// Efficiency threshold for INTEGRATE
    pub integrate_efficiency: f64,
    /// Quality threshold for CONSIDER
    pub consider_quality: f64,
    /// Efficiency threshold for CONSIDER
    pub consider_efficiency: f64,
    /// Baseline durations per scenario (used for efficiency calc).
    pub baseline_durations: Vec<(&'static str, Duration)>,
}

impl Default for ScoringEngine {
    fn default() -> Self {
        Self {
            integrate_quality: 0.80,
            integrate_efficiency: 1.5,
            consider_quality: 0.60,
            consider_efficiency: 1.2,
            baseline_durations: vec![
                ("Navigation", Duration::from_secs(10)),
                ("Analysis", Duration::from_secs(15)),
                ("Modification", Duration::from_secs(20)),
            ],
        }
    }
}

impl ScoringEngine {
    /// Compute aggregated metrics from a set of scenario results.
    pub fn compute(&self, results: &[ScenarioResult]) -> EvaluationMetrics {
        let total_operations: usize = results.iter().map(|r| r.measurements.len()).sum();
        let total_successes: usize = results
            .iter()
            .map(|r| r.measurements.iter().filter(|m| m.success).count())
            .sum();
        let total_duration: Duration = results.iter().map(|r| r.total_duration).sum();

        let quality_score = if total_operations == 0 {
            0.0
        } else {
            total_successes as f64 / total_operations as f64
        };

        // Compute efficiency as baseline_total / actual_total
        let baseline_total: Duration = results
            .iter()
            .map(|r| {
                self.baseline_durations
                    .iter()
                    .find(|(name, _)| *name == r.scenario_name)
                    .map(|(_, d)| *d)
                    .unwrap_or(Duration::from_secs(10))
            })
            .sum();

        let efficiency_score = if total_duration.is_zero() {
            0.0
        } else {
            baseline_total.as_secs_f64() / total_duration.as_secs_f64()
        };

        EvaluationMetrics {
            quality_score,
            efficiency_score,
            total_operations,
            total_successes,
            total_duration,
        }
    }

    /// Produce a recommendation based on computed metrics.
    pub fn recommend(&self, metrics: &EvaluationMetrics) -> Recommendation {
        if metrics.quality_score >= self.integrate_quality
            && metrics.efficiency_score >= self.integrate_efficiency
        {
            Recommendation::Integrate
        } else if metrics.quality_score >= self.consider_quality
            || metrics.efficiency_score >= self.consider_efficiency
        {
            Recommendation::Consider
        } else {
            Recommendation::DontIntegrate
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::mcp_eval::adapter::MeasurementResult;

    fn make_result(name: &str, success_rate: f64, duration_ms: u64) -> ScenarioResult {
        let ops = 4;
        let successes = (ops as f64 * success_rate).round() as usize;
        let measurements: Vec<MeasurementResult> = (0..ops)
            .map(|i| MeasurementResult {
                operation: format!("op_{}", i),
                duration: Duration::from_millis(duration_ms / ops as u64),
                success: i < successes,
                output: None,
            })
            .collect();

        ScenarioResult {
            scenario_name: name.to_string(),
            measurements,
            total_duration: Duration::from_millis(duration_ms),
            success_rate,
        }
    }

    #[test]
    fn scoring_engine_defaults() {
        let engine = ScoringEngine::default();
        assert_eq!(engine.integrate_quality, 0.80);
        assert_eq!(engine.integrate_efficiency, 1.5);
        assert_eq!(engine.consider_quality, 0.60);
        assert_eq!(engine.consider_efficiency, 1.2);
    }

    #[test]
    fn compute_metrics_from_results() {
        let engine = ScoringEngine::default();
        let results = vec![
            make_result("Navigation", 1.0, 500),
            make_result("Analysis", 0.75, 800),
        ];

        let metrics = engine.compute(&results);
        assert_eq!(metrics.total_operations, 8);
        assert!(metrics.quality_score > 0.8);
        assert!(metrics.efficiency_score > 1.0);
        assert_eq!(metrics.total_duration, Duration::from_millis(1300));
    }

    #[test]
    fn compute_metrics_empty_results() {
        let engine = ScoringEngine::default();
        let metrics = engine.compute(&[]);
        assert_eq!(metrics.quality_score, 0.0);
        assert_eq!(metrics.total_operations, 0);
    }

    #[test]
    fn recommend_integrate_when_high_quality_and_efficiency() {
        let engine = ScoringEngine::default();
        let metrics = EvaluationMetrics {
            quality_score: 0.95,
            efficiency_score: 2.0,
            total_operations: 12,
            total_successes: 11,
            total_duration: Duration::from_secs(5),
        };
        assert_eq!(engine.recommend(&metrics), Recommendation::Integrate);
    }

    #[test]
    fn recommend_consider_when_quality_above_60() {
        let engine = ScoringEngine::default();
        let metrics = EvaluationMetrics {
            quality_score: 0.65,
            efficiency_score: 1.0, // below 1.5
            total_operations: 12,
            total_successes: 8,
            total_duration: Duration::from_secs(15),
        };
        assert_eq!(engine.recommend(&metrics), Recommendation::Consider);
    }

    #[test]
    fn recommend_consider_when_efficiency_above_1_2() {
        let engine = ScoringEngine::default();
        let metrics = EvaluationMetrics {
            quality_score: 0.50, // below 60%
            efficiency_score: 1.3,
            total_operations: 12,
            total_successes: 6,
            total_duration: Duration::from_secs(10),
        };
        assert_eq!(engine.recommend(&metrics), Recommendation::Consider);
    }

    #[test]
    fn recommend_dont_integrate_when_both_below_thresholds() {
        let engine = ScoringEngine::default();
        let metrics = EvaluationMetrics {
            quality_score: 0.40,
            efficiency_score: 0.8,
            total_operations: 12,
            total_successes: 5,
            total_duration: Duration::from_secs(60),
        };
        assert_eq!(engine.recommend(&metrics), Recommendation::DontIntegrate);
    }

    #[test]
    fn recommend_boundary_integrate_exact_thresholds() {
        let engine = ScoringEngine::default();
        let metrics = EvaluationMetrics {
            quality_score: 0.80,
            efficiency_score: 1.5,
            total_operations: 10,
            total_successes: 8,
            total_duration: Duration::from_secs(10),
        };
        assert_eq!(engine.recommend(&metrics), Recommendation::Integrate);
    }

    #[test]
    fn recommend_boundary_consider_exact_quality() {
        let engine = ScoringEngine::default();
        let metrics = EvaluationMetrics {
            quality_score: 0.60,
            efficiency_score: 1.0,
            total_operations: 10,
            total_successes: 6,
            total_duration: Duration::from_secs(30),
        };
        assert_eq!(engine.recommend(&metrics), Recommendation::Consider);
    }

    #[test]
    fn recommendation_display() {
        assert_eq!(format!("{}", Recommendation::Integrate), "INTEGRATE");
        assert_eq!(format!("{}", Recommendation::Consider), "CONSIDER");
        assert_eq!(
            format!("{}", Recommendation::DontIntegrate),
            "DONT_INTEGRATE"
        );
    }

    #[test]
    fn recommendation_serializes_correctly() {
        let json = serde_json::to_string(&Recommendation::Integrate).unwrap();
        assert_eq!(json, "\"INTEGRATE\"");
        let json = serde_json::to_string(&Recommendation::DontIntegrate).unwrap();
        assert_eq!(json, "\"DONT_INTEGRATE\"");
    }

    #[test]
    fn metrics_serializes_to_json() {
        let metrics = EvaluationMetrics {
            quality_score: 0.85,
            efficiency_score: 1.8,
            total_operations: 12,
            total_successes: 10,
            total_duration: Duration::from_millis(1500),
        };
        let json = serde_json::to_string(&metrics).unwrap();
        assert!(json.contains("quality_score"));
        assert!(json.contains("efficiency_score"));
        assert!(json.contains("total_operations"));
    }
}
