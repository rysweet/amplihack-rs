//! Matrix evaluation harness — multi-agent comparative evaluation.
//!
//! Ports Python `amplihack/evaluation/matrix_eval.py`.
//! Runs the same question set against multiple agent configurations
//! and produces a ranked comparison report.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::error::EvalError;
use crate::long_horizon::LongHorizonConfig;

// ---------------------------------------------------------------------------
// Agent configuration
// ---------------------------------------------------------------------------

/// Configuration for a single agent in the matrix.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub name: String,
    pub sdk: String,
    #[serde(default)]
    pub multi_agent: bool,
    #[serde(default)]
    pub enable_spawning: bool,
}

impl AgentConfig {
    pub fn new(name: impl Into<String>, sdk: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            sdk: sdk.into(),
            multi_agent: false,
            enable_spawning: false,
        }
    }

    pub fn with_multi_agent(mut self, enabled: bool) -> Self {
        self.multi_agent = enabled;
        self
    }

    pub fn with_spawning(mut self, enabled: bool) -> Self {
        self.enable_spawning = enabled;
        self
    }
}

/// The default set of agent types to evaluate.
pub fn default_agent_types() -> Vec<AgentConfig> {
    vec![
        AgentConfig::new("mini", "mini"),
        AgentConfig::new("claude", "claude"),
        AgentConfig::new("copilot", "copilot"),
        AgentConfig::new("microsoft", "microsoft"),
        AgentConfig::new("multiagent-copilot", "copilot")
            .with_multi_agent(true)
            .with_spawning(true),
    ]
}

// ---------------------------------------------------------------------------
// Results
// ---------------------------------------------------------------------------

/// Per-agent category scores.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategoryScore {
    pub category: String,
    pub score: f64,
    pub num_questions: usize,
}

/// Result of evaluating a single agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatrixResult {
    pub agent_name: String,
    pub status: MatrixStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overall_score: Option<f64>,
    #[serde(default)]
    pub category_scores: Vec<CategoryScore>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    pub instantiation_time_s: f64,
    pub eval_time_s: f64,
}

/// Outcome status for an agent run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatrixStatus {
    Success,
    Skipped,
    Error,
}

impl MatrixResult {
    pub fn success(name: impl Into<String>, score: f64, time_s: f64) -> Self {
        Self {
            agent_name: name.into(),
            status: MatrixStatus::Success,
            overall_score: Some(score),
            category_scores: Vec::new(),
            error_message: None,
            instantiation_time_s: 0.0,
            eval_time_s: time_s,
        }
    }

    pub fn error(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            agent_name: name.into(),
            status: MatrixStatus::Error,
            overall_score: None,
            category_scores: Vec::new(),
            error_message: Some(message.into()),
            instantiation_time_s: 0.0,
            eval_time_s: 0.0,
        }
    }

    pub fn skipped(name: impl Into<String>) -> Self {
        Self {
            agent_name: name.into(),
            status: MatrixStatus::Skipped,
            overall_score: None,
            category_scores: Vec::new(),
            error_message: None,
            instantiation_time_s: 0.0,
            eval_time_s: 0.0,
        }
    }

    pub fn with_category_scores(mut self, scores: Vec<CategoryScore>) -> Self {
        self.category_scores = scores;
        self
    }
}

// ---------------------------------------------------------------------------
// Matrix report
// ---------------------------------------------------------------------------

/// Complete matrix evaluation report across all agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatrixReport {
    pub results: Vec<MatrixResult>,
    pub eval_config: LongHorizonConfig,
    pub agent_model: String,
    pub grader_model: String,
    pub timestamp: DateTime<Utc>,
    pub total_time_s: f64,
}

impl MatrixReport {
    pub fn new(eval_config: LongHorizonConfig) -> Self {
        Self {
            results: Vec::new(),
            eval_config,
            agent_model: String::new(),
            grader_model: String::new(),
            timestamp: Utc::now(),
            total_time_s: 0.0,
        }
    }

    pub fn add_result(&mut self, result: MatrixResult) {
        self.results.push(result);
    }

    /// Return agents ranked by overall score (highest first).
    pub fn ranking(&self) -> Vec<(&str, f64)> {
        let mut ranked: Vec<(&str, f64)> = self
            .results
            .iter()
            .filter_map(|r| r.overall_score.map(|s| (r.agent_name.as_str(), s)))
            .collect();
        ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        ranked
    }

    /// Best performing agent.
    pub fn best_agent(&self) -> Option<&MatrixResult> {
        self.results
            .iter()
            .filter(|r| r.status == MatrixStatus::Success)
            .max_by(|a, b| {
                a.overall_score
                    .unwrap_or(0.0)
                    .partial_cmp(&b.overall_score.unwrap_or(0.0))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    }

    /// Summary statistics.
    pub fn summary(&self) -> MatrixSummary {
        let total = self.results.len();
        let succeeded = self
            .results
            .iter()
            .filter(|r| r.status == MatrixStatus::Success)
            .count();
        let scores: Vec<f64> = self
            .results
            .iter()
            .filter_map(|r| r.overall_score)
            .collect();
        let avg = if scores.is_empty() {
            0.0
        } else {
            scores.iter().sum::<f64>() / scores.len() as f64
        };
        MatrixSummary {
            total_agents: total,
            succeeded,
            failed: total - succeeded,
            average_score: avg,
        }
    }

    /// Generate per-category comparison across agents.
    pub fn category_comparison(&self) -> HashMap<String, Vec<(&str, f64)>> {
        let mut map: HashMap<String, Vec<(&str, f64)>> = HashMap::new();
        for r in &self.results {
            for cs in &r.category_scores {
                map.entry(cs.category.clone())
                    .or_default()
                    .push((r.agent_name.as_str(), cs.score));
            }
        }
        for scores in map.values_mut() {
            scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        }
        map
    }
}

/// High-level summary of a matrix evaluation run.
#[derive(Debug, Clone)]
pub struct MatrixSummary {
    pub total_agents: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub average_score: f64,
}

/// Validate matrix eval configuration before running.
pub fn validate_matrix_config(
    agents: &[AgentConfig],
    eval_config: &LongHorizonConfig,
) -> Result<(), EvalError> {
    if agents.is_empty() {
        return Err(EvalError::config("at least one agent config is required"));
    }
    for (i, a) in agents.iter().enumerate() {
        if a.name.is_empty() {
            return Err(EvalError::config(format!(
                "agent[{i}] name must not be empty"
            )));
        }
        if a.sdk.is_empty() {
            return Err(EvalError::config(format!(
                "agent[{i}] sdk must not be empty"
            )));
        }
    }
    eval_config.validate()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_report() -> MatrixReport {
        let mut r = MatrixReport::new(LongHorizonConfig::default());
        r.add_result(MatrixResult::success("a", 0.5, 1.0));
        r.add_result(MatrixResult::success("b", 0.9, 2.0));
        r.add_result(MatrixResult::error("c", "fail"));
        r
    }

    #[test]
    fn default_agents_and_config_builder() {
        let agents = default_agent_types();
        assert_eq!(agents.len(), 5);
        assert!(agents.iter().any(|a| a.multi_agent));
        let c = AgentConfig::new("t", "m")
            .with_multi_agent(true)
            .with_spawning(true);
        assert!(c.multi_agent && c.enable_spawning);
    }

    #[test]
    fn matrix_result_variants() {
        let s = MatrixResult::success("a", 0.85, 12.5);
        assert_eq!(s.status, MatrixStatus::Success);
        assert_eq!(s.overall_score, Some(0.85));
        let e = MatrixResult::error("b", "timeout");
        assert_eq!(e.status, MatrixStatus::Error);
        assert!(e.overall_score.is_none());
        assert_eq!(MatrixResult::skipped("c").status, MatrixStatus::Skipped);
    }

    #[test]
    fn report_ranking_and_best() {
        let report = sample_report();
        let ranking = report.ranking();
        assert_eq!(ranking[0].0, "b");
        assert_eq!(report.best_agent().unwrap().agent_name, "b");
    }

    #[test]
    fn report_summary() {
        let s = sample_report().summary();
        assert_eq!(s.total_agents, 3);
        assert_eq!(s.succeeded, 2);
    }

    #[test]
    fn report_category_comparison() {
        let mut r = MatrixReport::new(LongHorizonConfig::default());
        let cs = |score| {
            vec![CategoryScore {
                category: "r".into(),
                score,
                num_questions: 5,
            }]
        };
        r.add_result(MatrixResult::success("a", 0.7, 1.0).with_category_scores(cs(0.8)));
        r.add_result(MatrixResult::success("b", 0.9, 1.0).with_category_scores(cs(0.95)));
        let comp = r.category_comparison();
        assert_eq!(comp["r"][0].0, "b");
    }

    #[test]
    fn validate_config() {
        let ec = LongHorizonConfig::default();
        assert!(validate_matrix_config(&default_agent_types(), &ec).is_ok());
        assert!(validate_matrix_config(&[], &ec).is_err());
        assert!(validate_matrix_config(&[AgentConfig::new("", "s")], &ec).is_err());
    }

    #[test]
    fn serde_roundtrips() {
        let c = AgentConfig::new("t", "m").with_multi_agent(true);
        let j = serde_json::to_string(&c).unwrap();
        assert!(serde_json::from_str::<AgentConfig>(&j).unwrap().multi_agent);

        let report = sample_report();
        let j = serde_json::to_string(&report).unwrap();
        assert_eq!(
            serde_json::from_str::<MatrixReport>(&j)
                .unwrap()
                .results
                .len(),
            3
        );
    }

    #[test]
    fn empty_report() {
        let r = MatrixReport::new(LongHorizonConfig::default());
        assert!(r.ranking().is_empty());
        assert!(r.best_agent().is_none());
        assert_eq!(r.summary().total_agents, 0);
    }
}
