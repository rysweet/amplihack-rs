//! Recipe models — data types for recipe execution.
//!
//! Matches Python `amplihack/recipes/models.py`:
//! - `StepType`, `StepStatus` enums
//! - `Step`, `Recipe` definition types
//! - `StepResult`, `RecipeResult` execution result types

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use thiserror::Error;

/// Type of a recipe step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepType {
    Agent,
    Shell,
    Prompt,
    SubRecipe,
    Checkpoint,
    Parallel,
}

impl StepType {
    /// Infer step type from step fields when not explicitly provided.
    pub fn infer(step_fields: &HashMap<String, serde_yaml::Value>) -> Self {
        if step_fields.contains_key("recipe") || step_fields.contains_key("sub_recipe") {
            StepType::SubRecipe
        } else if step_fields.contains_key("parallel") {
            StepType::Parallel
        } else if step_fields.contains_key("shell") || step_fields.contains_key("command") {
            StepType::Shell
        } else if step_fields.contains_key("checkpoint") {
            StepType::Checkpoint
        } else if step_fields.contains_key("prompt") || step_fields.contains_key("agent") {
            StepType::Agent
        } else {
            StepType::Prompt
        }
    }
}

impl fmt::Display for StepType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StepType::Agent => write!(f, "agent"),
            StepType::Shell => write!(f, "shell"),
            StepType::Prompt => write!(f, "prompt"),
            StepType::SubRecipe => write!(f, "sub_recipe"),
            StepType::Checkpoint => write!(f, "checkpoint"),
            StepType::Parallel => write!(f, "parallel"),
        }
    }
}

/// Execution status of a recipe step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Skipped,
}

impl fmt::Display for StepStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StepStatus::Pending => write!(f, "pending"),
            StepStatus::Running => write!(f, "running"),
            StepStatus::Succeeded => write!(f, "succeeded"),
            StepStatus::Failed => write!(f, "failed"),
            StepStatus::Skipped => write!(f, "skipped"),
        }
    }
}

/// A single step in a recipe.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub step_type: StepType,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub condition: Option<String>,
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
    #[serde(default)]
    pub retry_count: Option<u32>,
    #[serde(default)]
    pub allow_failure: bool,
    #[serde(default)]
    pub context: HashMap<String, serde_json::Value>,
}

impl Step {
    pub fn new(id: impl Into<String>, name: impl Into<String>, step_type: StepType) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            step_type,
            description: None,
            prompt: None,
            command: None,
            agent: None,
            condition: None,
            timeout_seconds: None,
            retry_count: None,
            allow_failure: false,
            context: HashMap::new(),
        }
    }
}

/// A complete recipe definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recipe {
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub description: Option<String>,
    pub steps: Vec<Step>,
    #[serde(default)]
    pub context: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub on_failure: Option<String>,
}

fn default_version() -> String {
    "1.0.0".to_string()
}

impl Recipe {
    pub fn new(name: impl Into<String>, steps: Vec<Step>) -> Self {
        Self {
            name: name.into(),
            version: default_version(),
            description: None,
            steps,
            context: HashMap::new(),
            on_failure: None,
        }
    }

    pub fn step_count(&self) -> usize {
        self.steps.len()
    }

    pub fn get_step(&self, id: &str) -> Option<&Step> {
        self.steps.iter().find(|s| s.id == id)
    }

    /// All unique step IDs.
    pub fn step_ids(&self) -> Vec<&str> {
        self.steps.iter().map(|s| s.id.as_str()).collect()
    }
}

/// Result of executing a single step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepResult {
    pub step_id: String,
    pub status: StepStatus,
    #[serde(default)]
    pub output: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub duration_seconds: f64,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

impl StepResult {
    pub fn success(step_id: impl Into<String>, output: impl Into<String>) -> Self {
        Self {
            step_id: step_id.into(),
            status: StepStatus::Succeeded,
            output: Some(output.into()),
            error: None,
            duration_seconds: 0.0,
            metadata: HashMap::new(),
        }
    }

    pub fn failure(
        step_id: impl Into<String>,
        error: impl Into<String>,
    ) -> Self {
        Self {
            step_id: step_id.into(),
            status: StepStatus::Failed,
            output: None,
            error: Some(error.into()),
            duration_seconds: 0.0,
            metadata: HashMap::new(),
        }
    }

    pub fn skipped(step_id: impl Into<String>) -> Self {
        Self {
            step_id: step_id.into(),
            status: StepStatus::Skipped,
            output: None,
            error: None,
            duration_seconds: 0.0,
            metadata: HashMap::new(),
        }
    }

    pub fn is_success(&self) -> bool {
        self.status == StepStatus::Succeeded
    }

    /// Truncated output for display.
    pub fn truncated_output(&self, max_len: usize) -> Option<String> {
        self.output.as_ref().map(|o| {
            if o.len() <= max_len {
                o.clone()
            } else {
                format!("{}…", &o[..max_len])
            }
        })
    }
}

/// Aggregate result of executing a complete recipe.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecipeResult {
    pub recipe_name: String,
    pub success: bool,
    pub step_results: Vec<StepResult>,
    #[serde(default)]
    pub total_duration_seconds: f64,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

impl RecipeResult {
    pub fn new(recipe_name: impl Into<String>) -> Self {
        Self {
            recipe_name: recipe_name.into(),
            success: true,
            step_results: Vec::new(),
            total_duration_seconds: 0.0,
            metadata: HashMap::new(),
        }
    }

    pub fn add_step(&mut self, result: StepResult) {
        if result.status == StepStatus::Failed {
            self.success = false;
        }
        self.total_duration_seconds += result.duration_seconds;
        self.step_results.push(result);
    }

    pub fn step_count(&self) -> usize {
        self.step_results.len()
    }

    pub fn succeeded_count(&self) -> usize {
        self.step_results
            .iter()
            .filter(|r| r.status == StepStatus::Succeeded)
            .count()
    }

    pub fn failed_count(&self) -> usize {
        self.step_results
            .iter()
            .filter(|r| r.status == StepStatus::Failed)
            .count()
    }

    pub fn skipped_count(&self) -> usize {
        self.step_results
            .iter()
            .filter(|r| r.status == StepStatus::Skipped)
            .count()
    }
}

/// Error from recipe step execution.
#[derive(Debug, Error)]
pub enum StepExecutionError {
    #[error("Step '{step_id}' timed out after {timeout_secs}s")]
    Timeout { step_id: String, timeout_secs: u64 },

    #[error("Step '{step_id}' failed: {message}")]
    ExecutionFailed { step_id: String, message: String },

    #[error("Step '{step_id}' condition failed: {condition}")]
    ConditionFailed { step_id: String, condition: String },

    #[error("Step '{step_id}' agent not found: {agent}")]
    AgentNotFound { step_id: String, agent: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_type_inference() {
        let mut fields = HashMap::new();
        fields.insert("shell".into(), serde_yaml::Value::String("echo hi".into()));
        assert_eq!(StepType::infer(&fields), StepType::Shell);

        let mut fields = HashMap::new();
        fields.insert("recipe".into(), serde_yaml::Value::String("sub".into()));
        assert_eq!(StepType::infer(&fields), StepType::SubRecipe);

        let mut fields = HashMap::new();
        fields.insert("prompt".into(), serde_yaml::Value::String("ask".into()));
        assert_eq!(StepType::infer(&fields), StepType::Agent);

        let empty: HashMap<String, serde_yaml::Value> = HashMap::new();
        assert_eq!(StepType::infer(&empty), StepType::Prompt);
    }

    #[test]
    fn step_type_display() {
        assert_eq!(StepType::Agent.to_string(), "agent");
        assert_eq!(StepType::Shell.to_string(), "shell");
        assert_eq!(StepType::SubRecipe.to_string(), "sub_recipe");
    }

    #[test]
    fn step_status_display() {
        assert_eq!(StepStatus::Succeeded.to_string(), "succeeded");
        assert_eq!(StepStatus::Failed.to_string(), "failed");
        assert_eq!(StepStatus::Skipped.to_string(), "skipped");
    }

    #[test]
    fn step_construction() {
        let step = Step::new("s1", "First step", StepType::Shell);
        assert_eq!(step.id, "s1");
        assert_eq!(step.name, "First step");
        assert_eq!(step.step_type, StepType::Shell);
        assert!(!step.allow_failure);
    }

    #[test]
    fn recipe_construction() {
        let steps = vec![
            Step::new("s1", "init", StepType::Shell),
            Step::new("s2", "build", StepType::Agent),
        ];
        let recipe = Recipe::new("test-recipe", steps);
        assert_eq!(recipe.name, "test-recipe");
        assert_eq!(recipe.step_count(), 2);
        assert_eq!(recipe.version, "1.0.0");
        assert!(recipe.get_step("s1").is_some());
        assert!(recipe.get_step("s3").is_none());
    }

    #[test]
    fn recipe_step_ids() {
        let steps = vec![
            Step::new("a", "first", StepType::Shell),
            Step::new("b", "second", StepType::Agent),
        ];
        let recipe = Recipe::new("ids", steps);
        assert_eq!(recipe.step_ids(), vec!["a", "b"]);
    }

    #[test]
    fn step_result_success() {
        let result = StepResult::success("s1", "done");
        assert!(result.is_success());
        assert_eq!(result.output.as_deref(), Some("done"));
        assert!(result.error.is_none());
    }

    #[test]
    fn step_result_failure() {
        let result = StepResult::failure("s1", "boom");
        assert!(!result.is_success());
        assert_eq!(result.error.as_deref(), Some("boom"));
    }

    #[test]
    fn step_result_truncated_output() {
        let result = StepResult::success("s1", "a".repeat(1000));
        let truncated = result.truncated_output(10).unwrap();
        assert!(truncated.starts_with("aaaaaaaaaa"));
        assert!(truncated.ends_with('…'));
        assert!(truncated.len() > 10);
    }

    #[test]
    fn recipe_result_aggregation() {
        let mut result = RecipeResult::new("test");
        assert!(result.success);

        let mut s1 = StepResult::success("s1", "ok");
        s1.duration_seconds = 1.5;
        result.add_step(s1);

        let mut s2 = StepResult::failure("s2", "err");
        s2.duration_seconds = 0.5;
        result.add_step(s2);

        result.add_step(StepResult::skipped("s3"));

        assert!(!result.success);
        assert_eq!(result.step_count(), 3);
        assert_eq!(result.succeeded_count(), 1);
        assert_eq!(result.failed_count(), 1);
        assert_eq!(result.skipped_count(), 1);
        assert!((result.total_duration_seconds - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn recipe_result_serde_roundtrip() {
        let mut result = RecipeResult::new("serde-test");
        result.add_step(StepResult::success("s1", "output"));
        let json = serde_json::to_string(&result).unwrap();
        let restored: RecipeResult = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.recipe_name, "serde-test");
        assert_eq!(restored.step_count(), 1);
        assert!(restored.success);
    }

    #[test]
    fn step_serde_roundtrip() {
        let mut step = Step::new("s1", "test", StepType::Agent);
        step.prompt = Some("Do something".into());
        step.timeout_seconds = Some(60);
        let json = serde_json::to_string(&step).unwrap();
        let restored: Step = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.id, "s1");
        assert_eq!(restored.step_type, StepType::Agent);
        assert_eq!(restored.timeout_seconds, Some(60));
    }

    #[test]
    fn step_execution_error_messages() {
        let e = StepExecutionError::Timeout {
            step_id: "s1".into(),
            timeout_secs: 30,
        };
        assert!(e.to_string().contains("timed out"));
        assert!(e.to_string().contains("30"));

        let e = StepExecutionError::ExecutionFailed {
            step_id: "s2".into(),
            message: "exit code 1".into(),
        };
        assert!(e.to_string().contains("exit code 1"));
    }
}
