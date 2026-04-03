//! Progressive test suite execution.

use crate::error::EvalError;
use crate::grader::Grader;
use crate::levels::TestLevel;
use crate::models::{LevelResult, ProgressiveConfig, ProgressiveResult, TestCase};

/// Runs progressive evaluation levels in order.
#[allow(dead_code)] // Fields used once todo!() stubs are implemented
pub struct ProgressiveSuite {
    config: ProgressiveConfig,
    test_cases: Vec<TestCase>,
    grader: Box<dyn Grader>,
}

impl ProgressiveSuite {
    pub fn new(
        config: ProgressiveConfig,
        test_cases: Vec<TestCase>,
        grader: Box<dyn Grader>,
    ) -> Self {
        Self {
            config,
            test_cases,
            grader,
        }
    }

    /// Run a single evaluation level.
    pub fn run_level(&self, _level: TestLevel) -> Result<LevelResult, EvalError> {
        todo!("ProgressiveSuite::run_level not yet implemented")
    }

    /// Run all configured levels in order.
    pub fn run_all(&self) -> Result<ProgressiveResult, EvalError> {
        todo!("ProgressiveSuite::run_all not yet implemented")
    }

    /// Compute summary statistics from level results.
    pub fn compute_summary(results: &[LevelResult]) -> ProgressiveSummary {
        let total = results.len();
        let passed = results.iter().filter(|r| r.success).count();
        let avg_score = if total == 0 {
            0.0
        } else {
            results.iter().map(|r| r.average_score()).sum::<f64>() / total as f64
        };
        ProgressiveSummary {
            total_levels: total,
            passed_levels: passed,
            failed_levels: total - passed,
            average_score: avg_score,
        }
    }

    /// Get test cases for a specific level.
    pub fn cases_for_level(&self, level: TestLevel) -> Vec<&TestCase> {
        self.test_cases
            .iter()
            .filter(|tc| tc.question.level == level)
            .collect()
    }

    /// Access the config.
    pub fn config(&self) -> &ProgressiveConfig {
        &self.config
    }
}

/// Summary statistics for a progressive eval run.
#[derive(Debug, Clone)]
pub struct ProgressiveSummary {
    pub total_levels: usize,
    pub passed_levels: usize,
    pub failed_levels: usize,
    pub average_score: f64,
}
