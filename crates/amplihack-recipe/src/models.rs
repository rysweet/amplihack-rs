//! Recipe models — data types for recipe execution.
//!
//! Matches Python `amplihack/recipes/models.py`:
//! - `StepType`, `StepStatus` enums
//! - `Step`, `Recipe` definition types
//! - `StepResult`, `RecipeResult` execution result types

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
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
    /// Infer step type from step field names when not explicitly provided.
    pub fn infer(step_keys: &HashSet<String>) -> Self {
        if step_keys.contains("recipe") || step_keys.contains("sub_recipe") {
            StepType::SubRecipe
        } else if step_keys.contains("parallel") {
            StepType::Parallel
        } else if step_keys.contains("shell") || step_keys.contains("command") {
            StepType::Shell
        } else if step_keys.contains("checkpoint") {
            StepType::Checkpoint
        } else if step_keys.contains("prompt") || step_keys.contains("agent") {
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

    pub fn failure(step_id: impl Into<String>, error: impl Into<String>) -> Self {
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
#[path = "tests/models_tests.rs"]
mod tests;
