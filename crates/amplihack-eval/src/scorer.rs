//! Score calculation and run comparison.
//!
//! [`Scorer`] computes a [`RunScore`] from a [`BenchmarkResult`], applying
//! configurable pass/fail thresholds and weighting.  [`RunComparison`]
//! summarises the delta between two runs.

use serde::{Deserialize, Serialize};

use crate::benchmark::BenchmarkResult;

/// Configuration for the scorer.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ScorerConfig {
    /// Minimum mean score to consider the run as passing overall.
    pub pass_threshold: f64,
    /// Weight applied to pass-rate when computing the composite score.
    /// Composite = pass_rate_weight * pass_rate + (1 - pass_rate_weight) * mean_score
    pub pass_rate_weight: f64,
}

impl Default for ScorerConfig {
    fn default() -> Self {
        Self {
            pass_threshold: 0.7,
            pass_rate_weight: 0.5,
        }
    }
}

impl ScorerConfig {
    /// Create a config with the given pass threshold.
    pub fn with_threshold(pass_threshold: f64) -> Self {
        Self {
            pass_threshold,
            ..Default::default()
        }
    }
}

/// Scores derived from a single benchmark run.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RunScore {
    /// Name of the benchmark.
    pub benchmark_name: String,
    /// Total number of cases evaluated.
    pub total_cases: usize,
    /// Number of cases that passed.
    pub passed_cases: usize,
    /// Pass rate in [0.0, 1.0].
    pub pass_rate: f64,
    /// Mean per-case score in [0.0, 1.0].
    pub mean_score: f64,
    /// Composite score (weighted average of pass_rate and mean_score).
    pub composite_score: f64,
    /// Whether the run cleared the pass threshold.
    pub overall_passed: bool,
    /// Total wall-clock time across all cases (ms).
    pub total_duration_ms: u64,
}

impl RunScore {
    /// Convenience: was the run a failure?
    pub fn failed(&self) -> bool {
        !self.overall_passed
    }
}

/// Comparison of two [`RunScore`] results (baseline vs. candidate).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RunComparison {
    pub benchmark_name: String,
    /// Composite score delta: candidate − baseline.
    pub composite_delta: f64,
    /// Mean score delta: candidate − baseline.
    pub mean_score_delta: f64,
    /// Pass-rate delta: candidate − baseline.
    pub pass_rate_delta: f64,
    /// Whether the candidate improved over the baseline.
    pub improved: bool,
}

/// Computes scores from benchmark results.
pub struct Scorer {
    config: ScorerConfig,
}

impl Scorer {
    pub fn new(config: ScorerConfig) -> Self {
        Self { config }
    }

    /// Score a benchmark result.
    pub fn score(&self, result: &BenchmarkResult) -> RunScore {
        let total = result.total();
        let passed = result.passed();
        let pass_rate = if total == 0 {
            0.0
        } else {
            passed as f64 / total as f64
        };
        let mean_score = result.mean_score();
        let w = self.config.pass_rate_weight.clamp(0.0, 1.0);
        let composite = w * pass_rate + (1.0 - w) * mean_score;

        RunScore {
            benchmark_name: result.benchmark_name.clone(),
            total_cases: total,
            passed_cases: passed,
            pass_rate,
            mean_score,
            composite_score: composite,
            overall_passed: composite >= self.config.pass_threshold,
            total_duration_ms: result.total_duration_ms(),
        }
    }

    /// Compare a candidate run against a baseline.
    pub fn compare(&self, baseline: &RunScore, candidate: &RunScore) -> RunComparison {
        let composite_delta = candidate.composite_score - baseline.composite_score;
        let mean_score_delta = candidate.mean_score - baseline.mean_score;
        let pass_rate_delta = candidate.pass_rate - baseline.pass_rate;
        RunComparison {
            benchmark_name: candidate.benchmark_name.clone(),
            composite_delta,
            mean_score_delta,
            pass_rate_delta,
            improved: composite_delta > 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::benchmark::BenchmarkResult;

    fn make_result(cases: &[(&str, bool, f64, u64)]) -> BenchmarkResult {
        let mut r = BenchmarkResult::new("test");
        for &(id, passed, score, ms) in cases {
            r.add_case(id, passed, score, ms);
        }
        r.finish();
        r
    }

    #[test]
    fn all_pass() {
        let r = make_result(&[("a", true, 1.0, 5), ("b", true, 0.8, 10)]);
        let s = Scorer::new(ScorerConfig::default()).score(&r);
        assert_eq!(s.pass_rate, 1.0);
        assert!((s.mean_score - 0.9).abs() < 1e-9);
        assert!(s.overall_passed);
    }

    #[test]
    fn all_fail() {
        let r = make_result(&[("a", false, 0.1, 5)]);
        let s = Scorer::new(ScorerConfig::default()).score(&r);
        assert_eq!(s.pass_rate, 0.0);
        assert!(!s.overall_passed);
    }

    #[test]
    fn empty_result() {
        let r = BenchmarkResult::new("empty");
        let s = Scorer::new(ScorerConfig::default()).score(&r);
        assert_eq!(s.total_cases, 0);
        assert_eq!(s.pass_rate, 0.0);
        assert!(!s.overall_passed);
    }

    #[test]
    fn compare_improvement() {
        let r1 = make_result(&[("a", false, 0.3, 5)]);
        let r2 = make_result(&[("a", true, 0.9, 5)]);
        let scorer = Scorer::new(ScorerConfig::default());
        let s1 = scorer.score(&r1);
        let s2 = scorer.score(&r2);
        let cmp = scorer.compare(&s1, &s2);
        assert!(cmp.improved);
        assert!(cmp.composite_delta > 0.0);
    }

    #[test]
    fn compare_regression() {
        let r1 = make_result(&[("a", true, 0.9, 5)]);
        let r2 = make_result(&[("a", false, 0.3, 5)]);
        let scorer = Scorer::new(ScorerConfig::default());
        let s1 = scorer.score(&r1);
        let s2 = scorer.score(&r2);
        let cmp = scorer.compare(&s1, &s2);
        assert!(!cmp.improved);
        assert!(cmp.composite_delta < 0.0);
    }

    #[test]
    fn custom_threshold() {
        let r = make_result(&[("a", true, 0.5, 5)]);
        let config = ScorerConfig::with_threshold(0.9);
        let s = Scorer::new(config).score(&r);
        // composite = 0.5 * 1.0 + 0.5 * 0.5 = 0.75, which is < 0.9
        assert!(!s.overall_passed);
    }

    #[test]
    fn composite_score_formula() {
        let r = make_result(&[("a", true, 0.6, 0), ("b", false, 0.4, 0)]);
        let config = ScorerConfig {
            pass_rate_weight: 0.5,
            pass_threshold: 0.0,
        };
        let s = Scorer::new(config).score(&r);
        // pass_rate = 0.5, mean_score = 0.5, composite = 0.5
        assert!((s.composite_score - 0.5).abs() < 1e-9);
    }
}
