//! Native Rust gym/eval API.
//!
//! Replaces the Python `simard_gym_bridge.py` with pure Rust types and
//! runner functions. Provides `list_scenarios`, `run_scenario`, and
//! `run_suite` — the three operations Simard needs from the eval framework.

use crate::error::EvalError;
use crate::grader::SimpleGrader;
use crate::levels::TestLevel;
use crate::long_horizon::{ALL_DIMENSIONS, LongHorizonConfig};
use crate::long_horizon_eval::LongHorizonMemoryEval;
use crate::models::{LevelResult, ProgressiveConfig};
use crate::progressive::ProgressiveSuite;
use crate::progressive_levels::{self, LevelScenario};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Result types matching the Python bridge protocol
// ---------------------------------------------------------------------------

/// Description of an available evaluation scenario.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GymScenario {
    pub id: String,
    pub name: String,
    pub description: String,
    pub level: String,
    pub question_count: usize,
    pub article_count: usize,
}

/// Result of running a single scenario.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GymScenarioResult {
    pub scenario_id: String,
    pub success: bool,
    pub score: f64,
    pub dimensions: HashMap<String, Option<f64>>,
    pub question_count: usize,
    pub questions_answered: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(default)]
    pub degraded_sources: Vec<String>,
}

/// Result of running the full progressive suite.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GymSuiteResult {
    pub suite_id: String,
    pub success: bool,
    pub overall_score: f64,
    pub dimensions: HashMap<String, f64>,
    pub scenario_results: Vec<GymScenarioResult>,
    pub scenarios_passed: usize,
    pub scenarios_total: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(default)]
    pub degraded_sources: Vec<String>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn zero_dims() -> HashMap<String, Option<f64>> {
    ALL_DIMENSIONS
        .iter()
        .map(|d| (d.to_string(), Some(0.0)))
        .collect()
}

fn zero_dims_f64() -> HashMap<String, f64> {
    ALL_DIMENSIONS
        .iter()
        .map(|d| (d.to_string(), 0.0))
        .collect()
}

fn fail_result(scenario_id: &str, msg: &str) -> GymScenarioResult {
    GymScenarioResult {
        scenario_id: scenario_id.to_string(),
        success: false,
        score: 0.0,
        dimensions: zero_dims(),
        question_count: 0,
        questions_answered: 0,
        error_message: Some(msg.to_string()),
        degraded_sources: Vec::new(),
    }
}

fn level_result_to_scenario(lr: &LevelResult, question_count: usize) -> GymScenarioResult {
    if lr.success {
        let avg = lr.average_score();
        let mut dims = zero_dims();
        dims.insert("factual_accuracy".to_string(), Some(avg));
        dims.insert("specificity".to_string(), Some(avg));
        GymScenarioResult {
            scenario_id: format!("L{}", lr.level_id),
            success: true,
            score: avg,
            dimensions: dims,
            question_count,
            questions_answered: lr.scores.len(),
            error_message: None,
            degraded_sources: Vec::new(),
        }
    } else {
        fail_result(
            &format!("L{}", lr.level_id),
            lr.error_message.as_deref().unwrap_or("unknown error"),
        )
    }
}

// ---------------------------------------------------------------------------
// GymRunner — the main API
// ---------------------------------------------------------------------------

/// Configuration for the gym runner.
#[derive(Debug, Clone)]
pub struct GymConfig {
    pub output_dir: PathBuf,
    pub agent_name: String,
    pub sdk: String,
    pub grader_votes: u8,
}

impl Default for GymConfig {
    fn default() -> Self {
        Self {
            output_dir: PathBuf::from("eval_output"),
            agent_name: "gym-eval".into(),
            sdk: "mini".into(),
            grader_votes: 3,
        }
    }
}

/// Native Rust gym runner replacing the Python bridge.
///
/// Provides the same three operations: `list_scenarios`, `run_scenario`,
/// and `run_suite`.
pub struct GymRunner {
    config: GymConfig,
    scenarios: Vec<LevelScenario>,
}

impl GymRunner {
    /// Create a new gym runner with default built-in scenarios.
    pub fn new(config: GymConfig) -> Self {
        let scenarios = progressive_levels::all_levels();
        Self { config, scenarios }
    }

    /// Create a gym runner with custom scenarios (for testing or extension).
    pub fn with_scenarios(config: GymConfig, scenarios: Vec<LevelScenario>) -> Self {
        Self { config, scenarios }
    }

    /// List all available evaluation scenarios.
    pub fn list_scenarios(&self) -> Vec<GymScenario> {
        let mut out: Vec<GymScenario> = self
            .scenarios
            .iter()
            .map(|s| GymScenario {
                id: s.level_id.clone(),
                name: s.level_name.clone(),
                description: s.description.clone(),
                level: s.level_id.clone(),
                question_count: s.questions.len(),
                article_count: s.articles.len(),
            })
            .collect();

        // Always include the long-horizon scenario.
        out.push(GymScenario {
            id: "long-horizon-memory".into(),
            name: "Long-horizon memory stress test".into(),
            description: "1000-turn dialogue testing memory at scale".into(),
            level: "long-horizon".into(),
            question_count: 0,
            article_count: 0,
        });

        out
    }

    /// Run a single evaluation scenario by ID.
    pub fn run_scenario(&self, scenario_id: &str) -> Result<GymScenarioResult, EvalError> {
        // Reject path-traversal attempts.
        if scenario_id.contains('/') || scenario_id.contains('\\') || scenario_id.contains("..") {
            return Ok(fail_result(
                scenario_id,
                &format!("scenario_id contains illegal path characters: '{scenario_id}'"),
            ));
        }

        if scenario_id == "long-horizon-memory" {
            return self.run_long_horizon();
        }

        let scenario = self
            .scenarios
            .iter()
            .find(|s| s.level_id == scenario_id)
            .ok_or_else(|| {
                EvalError::level_not_found(format!("scenario '{scenario_id}' not found"))
            })?;

        let test_cases = scenario.to_test_cases();
        let question_count = test_cases.len();

        let grader = SimpleGrader::new(self.config.grader_votes)?;
        let config = ProgressiveConfig {
            output_dir: self.config.output_dir.join(scenario_id),
            agent_name: format!("{}-{}", self.config.agent_name, scenario_id),
            levels_to_run: vec![scenario.level],
            memory_backend: "default".into(),
            sdk: self.config.sdk.clone(),
            grader_votes: self.config.grader_votes,
        };

        let suite = ProgressiveSuite::new(config, test_cases, Box::new(grader));
        match suite.run_level(scenario.level) {
            Ok(lr) => Ok(level_result_to_scenario(&lr, question_count)),
            Err(e) => Ok(fail_result(scenario_id, &e.to_string())),
        }
    }

    /// Run the full progressive suite.
    pub fn run_suite(&self, suite_id: &str) -> Result<GymSuiteResult, EvalError> {
        let all_cases: Vec<_> = self
            .scenarios
            .iter()
            .flat_map(|s| s.to_test_cases())
            .collect();
        let levels: Vec<TestLevel> = self.scenarios.iter().map(|s| s.level).collect();

        let grader = SimpleGrader::new(self.config.grader_votes)?;
        let config = ProgressiveConfig {
            output_dir: self.config.output_dir.join(suite_id),
            agent_name: self.config.agent_name.clone(),
            levels_to_run: levels,
            memory_backend: "default".into(),
            sdk: self.config.sdk.clone(),
            grader_votes: self.config.grader_votes,
        };

        let suite = ProgressiveSuite::new(config, all_cases, Box::new(grader));
        let result = suite.run_all()?;

        let scenario_results: Vec<GymScenarioResult> = result
            .level_results
            .iter()
            .map(|lr| {
                let qc = self
                    .scenarios
                    .iter()
                    .find(|s| s.level.id() == lr.level_id)
                    .map(|s| s.questions.len())
                    .unwrap_or(0);
                level_result_to_scenario(lr, qc)
            })
            .collect();

        let passed = scenario_results.iter().filter(|s| s.success).count();
        let ok_scores: Vec<f64> = scenario_results
            .iter()
            .filter(|s| s.success)
            .map(|s| s.score)
            .collect();
        let overall = if ok_scores.is_empty() {
            0.0
        } else {
            ok_scores.iter().sum::<f64>() / ok_scores.len() as f64
        };

        let mut agg = zero_dims_f64();
        if !ok_scores.is_empty() {
            for dim in ALL_DIMENSIONS {
                let vals: Vec<f64> = scenario_results
                    .iter()
                    .filter(|s| s.success)
                    .filter_map(|s| s.dimensions.get(*dim).and_then(|v| *v))
                    .collect();
                if !vals.is_empty() {
                    agg.insert(
                        dim.to_string(),
                        vals.iter().sum::<f64>() / vals.len() as f64,
                    );
                }
            }
        }

        let success =
            !result.failed_levels.is_empty() || result.level_results.iter().all(|lr| lr.success);

        Ok(GymSuiteResult {
            suite_id: suite_id.to_string(),
            success,
            overall_score: overall,
            dimensions: agg,
            scenario_results,
            scenarios_passed: passed,
            scenarios_total: result.level_results.len(),
            error_message: None,
            degraded_sources: Vec::new(),
        })
    }

    /// Run the long-horizon memory stress test.
    fn run_long_horizon(&self) -> Result<GymScenarioResult, EvalError> {
        let config = LongHorizonConfig {
            num_turns: 100,
            num_questions: 20,
            grader_votes: self.config.grader_votes,
            seed: 42,
            segment_size: None,
        };
        let grader = SimpleGrader::new(self.config.grader_votes)?;
        let eval = LongHorizonMemoryEval::new(
            config,
            Box::new(grader),
            self.config.output_dir.join("long-horizon"),
            format!("{}-lh", self.config.agent_name),
        )?;
        let report = eval.run()?;

        let mut dims: HashMap<String, Option<f64>> = zero_dims();
        for cb in &report.category_breakdown {
            for (dn, dv) in &cb.dimension_averages {
                if let Some(existing) = dims.get_mut(dn) {
                    *existing = Some(existing.unwrap_or(0.0_f64).max(*dv));
                }
            }
        }

        Ok(GymScenarioResult {
            scenario_id: "long-horizon-memory".to_string(),
            success: true,
            score: report.overall_score,
            dimensions: dims,
            question_count: report.num_questions,
            questions_answered: report.results.len(),
            error_message: None,
            degraded_sources: Vec::new(),
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> GymConfig {
        GymConfig {
            output_dir: PathBuf::from("test_gym_output"),
            agent_name: "test-gym".into(),
            sdk: "mini".into(),
            grader_votes: 1,
        }
    }

    #[test]
    fn list_scenarios_includes_all_levels_plus_long_horizon() {
        let runner = GymRunner::new(test_config());
        let scenarios = runner.list_scenarios();
        // 12 progressive levels + 1 long-horizon = 13
        assert_eq!(scenarios.len(), 13);
        assert!(scenarios.iter().any(|s| s.id == "long-horizon-memory"));
    }

    #[test]
    fn run_scenario_l1_succeeds() {
        let runner = GymRunner::new(test_config());
        let result = runner.run_scenario("L1-recall").unwrap();
        assert!(
            result.success,
            "L1 self-grading should pass: {:?}",
            result.error_message
        );
        assert!(result.score > 0.0);
        // Cleanup
        let _ = std::fs::remove_dir_all("test_gym_output");
    }

    #[test]
    fn run_scenario_invalid_id() {
        let runner = GymRunner::new(test_config());
        let result = runner.run_scenario("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn run_scenario_path_traversal_rejected() {
        let runner = GymRunner::new(test_config());
        let result = runner.run_scenario("../../etc/passwd").unwrap();
        assert!(!result.success);
        assert!(result.error_message.unwrap().contains("illegal path"));
    }

    #[test]
    fn run_suite_produces_results() {
        let runner = GymRunner::new(test_config());
        let result = runner.run_suite("progressive").unwrap();
        assert_eq!(result.scenarios_total, 12);
        assert!(result.scenarios_passed > 0);
        assert!(result.overall_score > 0.0);
        // Cleanup
        let _ = std::fs::remove_dir_all("test_gym_output");
    }

    #[test]
    fn run_long_horizon_scenario() {
        let runner = GymRunner::new(test_config());
        let result = runner.run_scenario("long-horizon-memory").unwrap();
        assert!(result.success, "Long-horizon self-grading should succeed");
        assert!(result.score > 0.0);
        assert!(result.questions_answered > 0);
        // Cleanup
        let _ = std::fs::remove_dir_all("test_gym_output");
    }

    #[test]
    fn gym_scenario_result_serializes() {
        let result = GymScenarioResult {
            scenario_id: "L1-recall".into(),
            success: true,
            score: 0.95,
            dimensions: zero_dims(),
            question_count: 2,
            questions_answered: 2,
            error_message: None,
            degraded_sources: Vec::new(),
        };
        let json = serde_json::to_string(&result).unwrap();
        let parsed: GymScenarioResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.scenario_id, "L1-recall");
        assert!((parsed.score - 0.95).abs() < f64::EPSILON);
    }
}
