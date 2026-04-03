//! Domain evaluation harness — generic evaluator for domain agents.
//!
//! Matches Python `amplihack/eval/domain_eval_harness.py`:
//! - Scenario-based evaluation per level
//! - Score computation with rubric matching
//! - Aggregated eval reports

use crate::error::EvalError;
use crate::levels::TestLevel;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{debug, info};

/// Result of running a single evaluation scenario.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioResult {
    pub scenario_id: String,
    pub level: TestLevel,
    pub score: f64,
    pub passed: bool,
    pub output: String,
    pub expected: String,
    pub details: HashMap<String, serde_json::Value>,
}

impl ScenarioResult {
    pub fn new(
        scenario_id: impl Into<String>,
        level: TestLevel,
        score: f64,
        output: impl Into<String>,
        expected: impl Into<String>,
    ) -> Self {
        Self {
            scenario_id: scenario_id.into(),
            level,
            score,
            passed: score >= level.passing_threshold(),
            output: output.into(),
            expected: expected.into(),
            details: HashMap::new(),
        }
    }
}

/// Aggregated results for one evaluation level.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LevelReport {
    pub level: TestLevel,
    pub scenarios_run: usize,
    pub scenarios_passed: usize,
    pub average_score: f64,
    pub passed: bool,
    pub scenario_results: Vec<ScenarioResult>,
}

impl LevelReport {
    pub fn from_results(level: TestLevel, results: Vec<ScenarioResult>) -> Self {
        let scenarios_run = results.len();
        let scenarios_passed = results.iter().filter(|r| r.passed).count();
        let average_score = if results.is_empty() {
            0.0
        } else {
            results.iter().map(|r| r.score).sum::<f64>() / results.len() as f64
        };
        let passed = average_score >= level.passing_threshold();
        Self {
            level,
            scenarios_run,
            scenarios_passed,
            average_score,
            passed,
            scenario_results: results,
        }
    }
}

/// Full evaluation report for a domain agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalReport {
    pub agent_name: String,
    pub domain: String,
    pub overall_score: f64,
    pub levels_passed: usize,
    pub levels_failed: usize,
    pub level_reports: Vec<LevelReport>,
}

impl EvalReport {
    pub fn from_levels(
        agent_name: impl Into<String>,
        domain: impl Into<String>,
        level_reports: Vec<LevelReport>,
    ) -> Self {
        let levels_passed = level_reports.iter().filter(|r| r.passed).count();
        let levels_failed = level_reports.len() - levels_passed;
        let overall_score = if level_reports.is_empty() {
            0.0
        } else {
            level_reports.iter().map(|r| r.average_score).sum::<f64>()
                / level_reports.len() as f64
        };
        Self {
            agent_name: agent_name.into(),
            domain: domain.into(),
            overall_score,
            levels_passed,
            levels_failed,
            level_reports,
        }
    }

    pub fn passed(&self) -> bool {
        self.levels_failed == 0
    }
}

/// An evaluation scenario definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalScenario {
    pub id: String,
    pub level: TestLevel,
    pub input: String,
    pub expected_output: String,
    #[serde(default)]
    pub rubric: Vec<RubricItem>,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// A rubric criterion for grading.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RubricItem {
    pub criterion: String,
    pub weight: f64,
    #[serde(default)]
    pub must_mention: Vec<String>,
}

/// Trait for agents that can be domain-evaluated.
pub trait DomainEvalAgent {
    fn name(&self) -> &str;
    fn domain(&self) -> &str;
    fn execute(&self, input: &str) -> Result<String, EvalError>;
    fn reset(&mut self) -> Result<(), EvalError>;
}

/// Generic domain evaluation harness.
pub struct DomainEvalHarness {
    scenarios: Vec<EvalScenario>,
}

impl DomainEvalHarness {
    pub fn new(scenarios: Vec<EvalScenario>) -> Self {
        Self { scenarios }
    }

    /// Run all scenarios for a specific level.
    pub fn run_level(
        &self,
        agent: &mut dyn DomainEvalAgent,
        level: TestLevel,
    ) -> Result<LevelReport, EvalError> {
        let level_scenarios: Vec<_> =
            self.scenarios.iter().filter(|s| s.level == level).collect();

        debug!(
            level = %level,
            count = level_scenarios.len(),
            "Running domain eval level"
        );

        let mut results = Vec::new();
        for scenario in &level_scenarios {
            agent.reset()?;
            let output = agent.execute(&scenario.input)?;
            let score = self.grade_scenario(scenario, &output);
            results.push(ScenarioResult::new(
                &scenario.id,
                level,
                score,
                &output,
                &scenario.expected_output,
            ));
        }

        Ok(LevelReport::from_results(level, results))
    }

    /// Run all scenarios across all represented levels.
    pub fn run_all(
        &self,
        agent: &mut dyn DomainEvalAgent,
    ) -> Result<EvalReport, EvalError> {
        let mut levels: Vec<TestLevel> = self.scenarios.iter().map(|s| s.level).collect();
        levels.sort_by_key(|l| l.id());
        levels.dedup();

        info!(
            agent = agent.name(),
            domain = agent.domain(),
            levels = levels.len(),
            "Starting domain evaluation"
        );

        let mut level_reports = Vec::new();
        for level in &levels {
            let report = self.run_level(agent, *level)?;
            level_reports.push(report);
        }

        Ok(EvalReport::from_levels(
            agent.name(),
            agent.domain(),
            level_reports,
        ))
    }

    /// Grade a scenario output against expected and rubric.
    fn grade_scenario(&self, scenario: &EvalScenario, output: &str) -> f64 {
        if scenario.rubric.is_empty() {
            return self.grade_by_similarity(output, &scenario.expected_output);
        }

        let total_weight: f64 = scenario.rubric.iter().map(|r| r.weight).sum();
        if total_weight == 0.0 {
            return 0.0;
        }

        let mut weighted_score = 0.0;
        let output_lower = output.to_lowercase();

        for item in &scenario.rubric {
            let mut item_score = 0.0;
            if item.must_mention.is_empty() {
                // No specific mentions required; check if output is non-empty
                if !output.trim().is_empty() {
                    item_score = 1.0;
                }
            } else {
                let matched = item
                    .must_mention
                    .iter()
                    .filter(|m| output_lower.contains(&m.to_lowercase()))
                    .count();
                item_score = matched as f64 / item.must_mention.len() as f64;
            }
            weighted_score += item_score * item.weight;
        }

        weighted_score / total_weight
    }

    /// Simple similarity grading when no rubric is provided.
    fn grade_by_similarity(&self, output: &str, expected: &str) -> f64 {
        if output.trim().is_empty() && expected.trim().is_empty() {
            return 1.0;
        }
        if output.trim().is_empty() || expected.trim().is_empty() {
            return 0.0;
        }

        let output_lower = output.to_lowercase();
        let expected_lower = expected.to_lowercase();
        let output_words: std::collections::HashSet<&str> =
            output_lower.split_whitespace().collect();
        let expected_words: std::collections::HashSet<&str> =
            expected_lower.split_whitespace().collect();

        if expected_words.is_empty() {
            return if output_words.is_empty() { 1.0 } else { 0.5 };
        }

        let intersection = output_words.intersection(&expected_words).count();
        let union = output_words.union(&expected_words).count();

        if union == 0 {
            0.0
        } else {
            intersection as f64 / union as f64
        }
    }

    pub fn scenario_count(&self) -> usize {
        self.scenarios.len()
    }

    pub fn levels(&self) -> Vec<TestLevel> {
        let mut levels: Vec<TestLevel> = self.scenarios.iter().map(|s| s.level).collect();
        levels.sort_by_key(|l| l.id());
        levels.dedup();
        levels
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockAgent {
        name: String,
        domain: String,
        response: String,
    }

    impl MockAgent {
        fn new(response: &str) -> Self {
            Self {
                name: "mock-agent".into(),
                domain: "testing".into(),
                response: response.into(),
            }
        }
    }

    impl DomainEvalAgent for MockAgent {
        fn name(&self) -> &str {
            &self.name
        }
        fn domain(&self) -> &str {
            &self.domain
        }
        fn execute(&self, _input: &str) -> Result<String, EvalError> {
            Ok(self.response.clone())
        }
        fn reset(&mut self) -> Result<(), EvalError> {
            Ok(())
        }
    }

    fn scenario(id: &str, level: TestLevel, expected: &str) -> EvalScenario {
        EvalScenario {
            id: id.into(),
            level,
            input: "test input".into(),
            expected_output: expected.into(),
            rubric: vec![],
            tags: vec![],
        }
    }

    fn rubric_scenario(id: &str, level: TestLevel, mentions: Vec<&str>) -> EvalScenario {
        EvalScenario {
            id: id.into(),
            level,
            input: "test input".into(),
            expected_output: String::new(),
            rubric: vec![RubricItem {
                criterion: "completeness".into(),
                weight: 1.0,
                must_mention: mentions.into_iter().map(String::from).collect(),
            }],
            tags: vec![],
        }
    }

    #[test]
    fn scenario_result_pass_threshold() {
        let result = ScenarioResult::new("s1", TestLevel::L1Recall, 0.95, "out", "exp");
        assert!(result.passed); // L1 threshold is 0.9
    }

    #[test]
    fn level_report_aggregation() {
        let results = vec![
            ScenarioResult::new("s1", TestLevel::L1Recall, 0.9, "a", "a"),
            ScenarioResult::new("s2", TestLevel::L1Recall, 0.5, "b", "b"),
        ];
        let report = LevelReport::from_results(TestLevel::L1Recall, results);
        assert_eq!(report.scenarios_run, 2);
        assert_eq!(report.scenarios_passed, 1);
        assert!((report.average_score - 0.7).abs() < f64::EPSILON);
    }

    #[test]
    fn eval_report_overall() {
        let reports = vec![
            LevelReport::from_results(
                TestLevel::L1Recall,
                vec![ScenarioResult::new("s1", TestLevel::L1Recall, 0.9, "a", "a")],
            ),
            LevelReport::from_results(
                TestLevel::L2MultiSourceSynthesis,
                vec![ScenarioResult::new(
                    "s2",
                    TestLevel::L2MultiSourceSynthesis,
                    0.3,
                    "b",
                    "b",
                )],
            ),
        ];
        let report = EvalReport::from_levels("agent", "domain", reports);
        assert_eq!(report.levels_passed, 1);
        assert_eq!(report.levels_failed, 1);
        assert!(!report.passed());
    }

    #[test]
    fn harness_run_level() {
        let scenarios = vec![
            scenario("s1", TestLevel::L1Recall, "the answer"),
            scenario("s2", TestLevel::L1Recall, "the answer"),
            scenario("s3", TestLevel::L2MultiSourceSynthesis, "other"),
        ];
        let harness = DomainEvalHarness::new(scenarios);
        let mut agent = MockAgent::new("the answer");
        let report = harness.run_level(&mut agent, TestLevel::L1Recall).unwrap();
        assert_eq!(report.scenarios_run, 2);
        assert!(report.average_score > 0.0);
    }

    #[test]
    fn harness_run_all() {
        let scenarios = vec![
            scenario("s1", TestLevel::L1Recall, "hello world"),
            scenario("s2", TestLevel::L2MultiSourceSynthesis, "hello world"),
        ];
        let harness = DomainEvalHarness::new(scenarios);
        let mut agent = MockAgent::new("hello world");
        let report = harness.run_all(&mut agent).unwrap();
        assert_eq!(report.level_reports.len(), 2);
        assert!(report.overall_score > 0.0);
    }

    #[test]
    fn rubric_grading() {
        let scenarios = vec![rubric_scenario(
            "s1",
            TestLevel::L1Recall,
            vec!["security", "encryption"],
        )];
        let harness = DomainEvalHarness::new(scenarios);
        let mut agent = MockAgent::new("We use AES encryption for security");
        let report = harness.run_level(&mut agent, TestLevel::L1Recall).unwrap();
        assert_eq!(report.scenario_results[0].score, 1.0);
    }

    #[test]
    fn rubric_partial_match() {
        let scenarios = vec![rubric_scenario(
            "s1",
            TestLevel::L1Recall,
            vec!["security", "encryption", "hashing"],
        )];
        let harness = DomainEvalHarness::new(scenarios);
        let mut agent = MockAgent::new("We use security measures");
        let report = harness.run_level(&mut agent, TestLevel::L1Recall).unwrap();
        let score = report.scenario_results[0].score;
        assert!((score - 1.0 / 3.0).abs() < 0.01);
    }

    #[test]
    fn similarity_grading_identical() {
        let harness = DomainEvalHarness::new(vec![]);
        assert!((harness.grade_by_similarity("hello world", "hello world") - 1.0).abs() < 0.01);
    }

    #[test]
    fn similarity_grading_empty() {
        let harness = DomainEvalHarness::new(vec![]);
        assert_eq!(harness.grade_by_similarity("", "expected"), 0.0);
        assert_eq!(harness.grade_by_similarity("output", ""), 0.0);
        assert_eq!(harness.grade_by_similarity("", ""), 1.0);
    }

    #[test]
    fn harness_levels() {
        let scenarios = vec![
            scenario("s1", TestLevel::L1Recall, "a"),
            scenario("s2", TestLevel::L3TemporalReasoning, "b"),
            scenario("s3", TestLevel::L1Recall, "c"),
        ];
        let harness = DomainEvalHarness::new(scenarios);
        assert_eq!(harness.scenario_count(), 3);
        let levels = harness.levels();
        assert_eq!(levels.len(), 2);
        assert_eq!(levels[0], TestLevel::L1Recall);
    }

    #[test]
    fn eval_report_serde() {
        let report = EvalReport::from_levels("test", "domain", vec![]);
        let json = serde_json::to_string(&report).unwrap();
        let restored: EvalReport = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.agent_name, "test");
    }
}
