//! Domain agent base trait and evaluation types.
//!
//! Ports `domain_agents/base.py`: DomainAgent ABC, EvalScenario, EvalLevel,
//! TaskResult, and DomainTeachingResult.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::error::Result;

/// A single evaluation scenario for testing domain agent capabilities.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvalScenario {
    pub scenario_id: String,
    pub name: String,
    pub input_data: HashMap<String, serde_json::Value>,
    pub expected_output: HashMap<String, serde_json::Value>,
    pub grading_rubric: String,
}

/// An evaluation level with test scenarios and passing threshold.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvalLevel {
    pub level_id: String,
    pub name: String,
    pub description: String,
    pub scenarios: Vec<EvalScenario>,
    pub passing_threshold: f64,
}

impl EvalLevel {
    pub fn new(
        level_id: impl Into<String>,
        name: impl Into<String>,
        description: impl Into<String>,
        scenarios: Vec<EvalScenario>,
    ) -> Self {
        Self {
            level_id: level_id.into(),
            name: name.into(),
            description: description.into(),
            scenarios,
            passing_threshold: 0.7,
        }
    }

    pub fn with_threshold(mut self, threshold: f64) -> Self {
        self.passing_threshold = threshold;
        self
    }
}

/// Result of executing a domain task.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskResult {
    pub success: bool,
    pub output: Option<serde_json::Value>,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
    pub error: Option<String>,
}

impl TaskResult {
    pub fn ok(output: serde_json::Value) -> Self {
        Self {
            success: true,
            output: Some(output),
            metadata: HashMap::new(),
            error: None,
        }
    }

    pub fn ok_with_meta(
        output: serde_json::Value,
        metadata: HashMap<String, serde_json::Value>,
    ) -> Self {
        Self {
            success: true,
            output: Some(output),
            metadata,
            error: None,
        }
    }

    pub fn fail(error: impl Into<String>) -> Self {
        Self {
            success: false,
            output: None,
            metadata: HashMap::new(),
            error: Some(error.into()),
        }
    }
}

/// Result of a domain teaching session (distinct from eval TeachingResult).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DomainTeachingResult {
    pub lesson_plan: String,
    pub instruction: String,
    pub student_questions: Vec<String>,
    pub agent_answers: Vec<String>,
    pub student_attempt: String,
    #[serde(default)]
    pub scores: HashMap<String, f64>,
}

/// Trait for domain-specific agents.
///
/// Each domain agent handles a specific type of task (code review,
/// meeting synthesis, etc.) and provides evaluation levels for testing.
pub trait DomainAgent: Send + Sync {
    /// The domain this agent handles.
    fn domain(&self) -> &str;

    /// The agent's name.
    fn agent_name(&self) -> &str;

    /// Return the system prompt for this domain agent.
    fn system_prompt(&self) -> String;

    /// Execute a domain-specific task.
    fn execute_task(&self, task: &HashMap<String, serde_json::Value>) -> Result<TaskResult>;

    /// Return evaluation levels for this domain.
    fn eval_levels(&self) -> Vec<EvalLevel>;

    /// Teach a student about a domain topic.
    fn teach(&self, topic: &str, student_level: &str) -> Result<DomainTeachingResult>;

    /// List available tool names.
    fn available_tools(&self) -> Vec<String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eval_scenario_serde_roundtrip() {
        let scenario = EvalScenario {
            scenario_id: "L1-001".into(),
            name: "test scenario".into(),
            input_data: HashMap::from([("code".into(), serde_json::json!("fn main() {}"))]),
            expected_output: HashMap::from([("min_issue_count".into(), serde_json::json!(1))]),
            grading_rubric: "Must find at least one issue".into(),
        };
        let json = serde_json::to_string(&scenario).unwrap();
        let back: EvalScenario = serde_json::from_str(&json).unwrap();
        assert_eq!(back, scenario);
    }

    #[test]
    fn eval_level_builder() {
        let level = EvalLevel::new("L1", "Basic", "Basic tests", vec![]).with_threshold(0.5);
        assert_eq!(level.level_id, "L1");
        assert!((level.passing_threshold - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn task_result_ok() {
        let r = TaskResult::ok(serde_json::json!({"score": 0.9}));
        assert!(r.success);
        assert!(r.error.is_none());
        assert!(r.output.is_some());
    }

    #[test]
    fn task_result_fail() {
        let r = TaskResult::fail("no code provided");
        assert!(!r.success);
        assert_eq!(r.error.as_deref(), Some("no code provided"));
        assert!(r.output.is_none());
    }

    #[test]
    fn domain_teaching_result_serde_roundtrip() {
        let result = DomainTeachingResult {
            lesson_plan: "Step 1".into(),
            instruction: "Do X".into(),
            student_questions: vec!["Why?".into()],
            agent_answers: vec!["Because".into()],
            student_attempt: "My attempt".into(),
            scores: HashMap::new(),
        };
        let json = serde_json::to_string(&result).unwrap();
        let back: DomainTeachingResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back, result);
    }
}
