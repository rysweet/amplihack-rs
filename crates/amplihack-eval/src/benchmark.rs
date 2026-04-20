//! Benchmark execution and timing.
//!
//! A `Benchmark` describes what to run.  A `BenchmarkResult` records what
//! happened, including per-case outcomes and wall-clock durations.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::Instant;

use crate::error::EvalError;

/// A named benchmark with an optional description.
///
/// `Benchmark` is the configuration object — it does not execute anything
/// itself.  Execution is the caller's responsibility; results are recorded in
/// [`BenchmarkResult`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Benchmark {
    /// Unique name for this benchmark.
    pub name: String,
    /// Optional human-readable description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl Benchmark {
    /// Create a new benchmark with the given name.
    pub fn new(name: impl Into<String>) -> Result<Self, EvalError> {
        let name = name.into();
        if name.is_empty() {
            return Err(EvalError::invalid_benchmark("name must not be empty"));
        }
        Ok(Self {
            name,
            description: None,
        })
    }

    /// Attach a description.
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Start timing a run, returning a [`Timer`] that produces elapsed
    /// milliseconds when dropped or `.stop()`-ped.
    pub fn start_timer(&self) -> Timer {
        Timer::start()
    }
}

/// Wall-clock timer for a single benchmark run.
///
/// Call [`Timer::stop`] to get elapsed milliseconds, or simply drop it.
pub struct Timer {
    start: Instant,
}

impl Timer {
    pub(crate) fn start() -> Self {
        Self {
            start: Instant::now(),
        }
    }

    /// Stop the timer and return elapsed milliseconds.
    pub fn stop(self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    /// Peek at elapsed milliseconds without consuming the timer.
    pub fn elapsed_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }
}

/// Result of a single benchmark case.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CaseResult {
    /// Case identifier.
    pub case_id: String,
    /// Whether the case passed (score >= threshold).
    pub passed: bool,
    /// Normalised score in [0.0, 1.0].
    pub score: f64,
    /// Wall-clock time in milliseconds.
    pub duration_ms: u64,
    /// Optional human-readable notes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

impl CaseResult {
    /// Create a new case result.
    pub fn new(
        case_id: impl Into<String>,
        passed: bool,
        score: f64,
        duration_ms: u64,
    ) -> Result<Self, EvalError> {
        let case_id = case_id.into();
        if case_id.is_empty() {
            return Err(EvalError::invalid_benchmark("case_id must not be empty"));
        }
        if !(0.0..=1.0).contains(&score) {
            return Err(EvalError::invalid_score(score));
        }
        Ok(Self {
            case_id,
            passed,
            score,
            duration_ms,
            notes: None,
        })
    }

    /// Attach notes to the case result.
    pub fn with_notes(mut self, notes: impl Into<String>) -> Self {
        self.notes = Some(notes.into());
        self
    }
}

/// Aggregated result of a benchmark run (all cases).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BenchmarkResult {
    /// Name of the benchmark that was run.
    pub benchmark_name: String,
    /// Individual case results.
    pub cases: Vec<CaseResult>,
    /// Timestamp when the run started.
    pub started_at: DateTime<Utc>,
    /// Timestamp when the run finished (`None` until [`finish`] is called).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<DateTime<Utc>>,
}

impl BenchmarkResult {
    /// Create an empty result for the named benchmark.
    pub fn new(benchmark_name: impl Into<String>) -> Self {
        Self {
            benchmark_name: benchmark_name.into(),
            cases: Vec::new(),
            started_at: Utc::now(),
            finished_at: None,
        }
    }

    /// Record a case result (infallible convenience wrapper).
    ///
    /// Silently skips invalid results rather than panicking; callers that need
    /// strict validation should use [`CaseResult::new`] directly.
    pub fn add_case(
        &mut self,
        case_id: impl Into<String>,
        passed: bool,
        score: f64,
        duration_ms: u64,
    ) {
        if let Ok(c) = CaseResult::new(case_id, passed, score, duration_ms) {
            self.cases.push(c);
        }
    }

    /// Push a pre-built [`CaseResult`].
    pub fn push(&mut self, case: CaseResult) {
        self.cases.push(case);
    }

    /// Mark the run as finished at the current wall-clock time.
    pub fn finish(&mut self) {
        self.finished_at = Some(Utc::now());
    }

    /// Total number of cases.
    pub fn total(&self) -> usize {
        self.cases.len()
    }

    /// Number of passing cases.
    pub fn passed(&self) -> usize {
        self.cases.iter().filter(|c| c.passed).count()
    }

    /// Number of failing cases.
    pub fn failed(&self) -> usize {
        self.total() - self.passed()
    }

    /// Mean score across all cases, or 0.0 if empty.
    pub fn mean_score(&self) -> f64 {
        if self.cases.is_empty() {
            return 0.0;
        }
        self.cases.iter().map(|c| c.score).sum::<f64>() / self.cases.len() as f64
    }

    /// Total wall-clock time across all cases.
    pub fn total_duration_ms(&self) -> u64 {
        self.cases.iter().map(|c| c.duration_ms).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn benchmark_new_rejects_empty_name() {
        assert!(Benchmark::new("").is_err());
    }

    #[test]
    fn benchmark_new_ok() {
        let b = Benchmark::new("my-bench").unwrap();
        assert_eq!(b.name, "my-bench");
        assert!(b.description.is_none());
    }

    #[test]
    fn benchmark_with_description() {
        let b = Benchmark::new("x").unwrap().with_description("desc");
        assert_eq!(b.description.as_deref(), Some("desc"));
    }

    #[test]
    fn timer_measures_elapsed() {
        let t = Timer::start();
        assert!(t.elapsed_ms() < 1000, "timer should not take > 1s");
        let ms = t.stop();
        assert!(ms < 1000);
    }

    #[test]
    fn case_result_rejects_invalid_score() {
        assert!(CaseResult::new("c", true, 1.5, 0).is_err());
        assert!(CaseResult::new("c", true, -0.1, 0).is_err());
    }

    #[test]
    fn case_result_rejects_empty_id() {
        assert!(CaseResult::new("", true, 0.5, 0).is_err());
    }

    #[test]
    fn benchmark_result_statistics() {
        let mut r = BenchmarkResult::new("bench");
        r.add_case("a", true, 1.0, 10);
        r.add_case("b", false, 0.0, 20);
        r.add_case("c", true, 0.5, 30);
        r.finish();

        assert_eq!(r.total(), 3);
        assert_eq!(r.passed(), 2);
        assert_eq!(r.failed(), 1);
        assert!((r.mean_score() - 0.5).abs() < 1e-9);
        assert_eq!(r.total_duration_ms(), 60);
        assert!(r.finished_at.is_some());
    }

    #[test]
    fn benchmark_result_empty() {
        let r = BenchmarkResult::new("empty");
        assert_eq!(r.mean_score(), 0.0);
        assert_eq!(r.total_duration_ms(), 0);
    }
}
