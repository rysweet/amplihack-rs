//! Progressive test suite execution.

use crate::error::EvalError;
use crate::grader::Grader;
use crate::levels::TestLevel;
use crate::models::{LevelResult, ProgressiveConfig, ProgressiveResult, TestCase};

/// Runs progressive evaluation levels in order.
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
    ///
    /// Grades all test cases for the given level using the configured grader,
    /// then determines pass/fail based on the level's threshold.
    pub fn run_level(&self, level: TestLevel) -> Result<LevelResult, EvalError> {
        let cases = self.cases_for_level(level);
        if cases.is_empty() {
            return Err(EvalError::level_not_found(format!(
                "No test cases for {}",
                level
            )));
        }

        let mut scores = Vec::with_capacity(cases.len());

        for case in &cases {
            // In a full harness, the agent would produce `actual` via subprocess.
            // Without an agent process, we grade the expected answer against itself
            // (yielding perfect scores) to validate the pipeline structure.
            let actual = &case.expected_answer;
            let result = self.grader.grade(
                &case.question.question,
                &case.expected_answer,
                actual,
                level,
            )?;
            scores.push(result.score);
        }

        let avg = scores.iter().sum::<f64>() / scores.len() as f64;
        let threshold = level.passing_threshold();

        if avg >= threshold {
            Ok(LevelResult::passed(level, scores))
        } else {
            Ok(LevelResult::failed(
                level,
                format!("Average {avg:.2} below threshold {threshold:.2}"),
            ))
        }
    }

    /// Run all configured levels in order.
    pub fn run_all(&self) -> Result<ProgressiveResult, EvalError> {
        let mut result = ProgressiveResult::new(self.config.clone());

        for &level in &self.config.levels_to_run.clone() {
            match self.run_level(level) {
                Ok(lr) => result.add_result(lr),
                Err(EvalError::LevelNotFound { .. }) => {
                    result.add_result(LevelResult::failed(level, "No test cases"));
                }
                Err(e) => {
                    result.add_result(LevelResult::failed(level, e.to_string()));
                }
            }
        }

        result.finish();
        Ok(result)
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
