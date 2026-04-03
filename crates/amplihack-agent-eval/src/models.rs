//! Data types for the eval framework.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::error::EvalError;
use crate::levels::TestLevel;

/// Result of grading an agent's answer.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct GradeResult {
    /// Score between 0.0 (complete miss) and 1.0 (perfect match).
    pub score: f64,
    /// Human-readable explanation of the grade.
    pub reasoning: String,
    /// Individual vote scores when multi-vote grading is used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vote_scores: Option<Vec<f64>>,
}

impl GradeResult {
    pub fn new(score: f64, reasoning: impl Into<String>) -> Result<Self, EvalError> {
        let reasoning = reasoning.into();
        if !(0.0..=1.0).contains(&score) {
            return Err(EvalError::grading(format!(
                "score must be 0.0..=1.0, got {score}"
            )));
        }
        if reasoning.is_empty() {
            return Err(EvalError::grading("reasoning must not be empty"));
        }
        Ok(Self {
            score,
            reasoning,
            vote_scores: None,
        })
    }

    pub fn with_votes(mut self, votes: Vec<f64>) -> Self {
        self.vote_scores = Some(votes);
        self
    }

    pub fn passed(&self, threshold: f64) -> bool {
        self.score >= threshold
    }
}

/// A question used in an eval test.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TestQuestion {
    pub id: String,
    pub question: String,
    pub context: Option<String>,
    pub level: TestLevel,
}

impl TestQuestion {
    pub fn new(
        id: impl Into<String>,
        question: impl Into<String>,
        level: TestLevel,
    ) -> Result<Self, EvalError> {
        let id = id.into();
        let question = question.into();
        if id.is_empty() {
            return Err(EvalError::config("test question id must not be empty"));
        }
        if question.is_empty() {
            return Err(EvalError::config("test question must not be empty"));
        }
        Ok(Self {
            id,
            question,
            context: None,
            level,
        })
    }

    pub fn with_context(mut self, ctx: impl Into<String>) -> Self {
        self.context = Some(ctx.into());
        self
    }
}

/// A complete test case with question and expected answer.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TestCase {
    pub question: TestQuestion,
    pub expected_answer: String,
    pub tags: Vec<String>,
}

impl TestCase {
    pub fn new(
        question: TestQuestion,
        expected_answer: impl Into<String>,
    ) -> Result<Self, EvalError> {
        let expected_answer = expected_answer.into();
        if expected_answer.is_empty() {
            return Err(EvalError::config("expected answer must not be empty"));
        }
        Ok(Self {
            question,
            expected_answer,
            tags: Vec::new(),
        })
    }

    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }
}

/// Result of running a single test level.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct LevelResult {
    pub level_id: u8,
    pub level_name: String,
    pub success: bool,
    pub scores: Vec<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

impl LevelResult {
    pub fn passed(level: TestLevel, scores: Vec<f64>) -> Self {
        Self {
            level_id: level.id(),
            level_name: level.display_name().to_string(),
            success: true,
            scores,
            error_message: None,
        }
    }

    pub fn failed(level: TestLevel, error: impl Into<String>) -> Self {
        Self {
            level_id: level.id(),
            level_name: level.display_name().to_string(),
            success: false,
            scores: Vec::new(),
            error_message: Some(error.into()),
        }
    }

    pub fn average_score(&self) -> f64 {
        if self.scores.is_empty() {
            return 0.0;
        }
        self.scores.iter().sum::<f64>() / self.scores.len() as f64
    }
}

/// Configuration for a progressive evaluation run.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ProgressiveConfig {
    pub output_dir: PathBuf,
    pub agent_name: String,
    pub levels_to_run: Vec<TestLevel>,
    pub memory_backend: String,
    pub sdk: String,
    pub grader_votes: u8,
}

impl ProgressiveConfig {
    pub fn new(agent_name: impl Into<String>, output_dir: PathBuf) -> Result<Self, EvalError> {
        let agent_name = agent_name.into();
        if agent_name.is_empty() {
            return Err(EvalError::config("agent_name must not be empty"));
        }
        Ok(Self {
            output_dir,
            agent_name,
            levels_to_run: TestLevel::all().to_vec(),
            memory_backend: "default".into(),
            sdk: "default".into(),
            grader_votes: 3,
        })
    }

    pub fn with_levels(mut self, levels: Vec<TestLevel>) -> Self {
        self.levels_to_run = levels;
        self
    }

    pub fn with_sdk(mut self, sdk: impl Into<String>) -> Self {
        self.sdk = sdk.into();
        self
    }

    pub fn with_grader_votes(mut self, votes: u8) -> Self {
        self.grader_votes = votes;
        self
    }
}

/// Aggregated result of a full progressive evaluation run.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ProgressiveResult {
    pub config: ProgressiveConfig,
    pub level_results: Vec<LevelResult>,
    pub total_score: f64,
    pub passed_levels: Vec<u8>,
    pub failed_levels: Vec<u8>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

impl ProgressiveResult {
    pub fn new(config: ProgressiveConfig) -> Self {
        Self {
            config,
            level_results: Vec::new(),
            total_score: 0.0,
            passed_levels: Vec::new(),
            failed_levels: Vec::new(),
            started_at: Utc::now(),
            finished_at: None,
        }
    }

    pub fn add_result(&mut self, result: LevelResult) {
        if result.success {
            self.passed_levels.push(result.level_id);
        } else {
            self.failed_levels.push(result.level_id);
        }
        self.level_results.push(result);
        self.recompute_total();
    }

    fn recompute_total(&mut self) {
        if self.level_results.is_empty() {
            self.total_score = 0.0;
            return;
        }
        let sum: f64 = self.level_results.iter().map(|r| r.average_score()).sum();
        self.total_score = sum / self.level_results.len() as f64;
    }

    pub fn finish(&mut self) {
        self.finished_at = Some(Utc::now());
    }
}

/// Configuration for the test harness.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct HarnessConfig {
    pub test_suite: String,
    pub agent_config: String,
    pub timeout_seconds: u64,
    pub retries: u8,
}

impl Default for HarnessConfig {
    fn default() -> Self {
        Self {
            test_suite: String::new(),
            agent_config: String::new(),
            timeout_seconds: 300,
            retries: 3,
        }
    }
}

impl HarnessConfig {
    pub fn new(
        test_suite: impl Into<String>,
        agent_config: impl Into<String>,
    ) -> Result<Self, EvalError> {
        let test_suite = test_suite.into();
        let agent_config = agent_config.into();
        if test_suite.is_empty() {
            return Err(EvalError::config("test_suite must not be empty"));
        }
        if agent_config.is_empty() {
            return Err(EvalError::config("agent_config must not be empty"));
        }
        Ok(Self {
            test_suite,
            agent_config,
            ..Default::default()
        })
    }

    pub fn with_timeout(mut self, seconds: u64) -> Self {
        self.timeout_seconds = seconds;
        self
    }

    pub fn with_retries(mut self, retries: u8) -> Self {
        self.retries = retries;
        self
    }
}

/// Configuration for the self-improvement loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SelfImproveConfig {
    pub max_iterations: u32,
    pub target_score: f64,
    pub reviewer_count: u8,
    pub auto_apply_patches: bool,
}

impl Default for SelfImproveConfig {
    fn default() -> Self {
        Self {
            max_iterations: 5,
            target_score: 0.8,
            reviewer_count: 3,
            auto_apply_patches: false,
        }
    }
}

impl SelfImproveConfig {
    pub fn validate(&self) -> Result<(), EvalError> {
        if self.max_iterations == 0 {
            return Err(EvalError::config("max_iterations must be > 0"));
        }
        if !(0.0..=1.0).contains(&self.target_score) {
            return Err(EvalError::config("target_score must be 0.0..=1.0"));
        }
        if self.reviewer_count == 0 {
            return Err(EvalError::config("reviewer_count must be > 0"));
        }
        Ok(())
    }
}
