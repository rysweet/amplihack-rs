//! General-purpose agent capability evaluations beyond memory.
//!
//! Ports Python `amplihack/eval/general_capability_eval.py`:
//! - Tool use efficiency
//! - Planning
//! - Reasoning under uncertainty
//! - Cross-domain transfer
//! - Collaborative tasks

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// A single tool call recorded during agent execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub tool_name: String,
    #[serde(default)]
    pub arguments: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub result: String,
    #[serde(default)]
    pub timestamp_ms: u64,
}

/// Complete record of tool calls made by an agent for a task.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolTrajectory {
    pub task_description: String,
    #[serde(default)]
    pub calls: Vec<ToolCall>,
    #[serde(default)]
    pub total_time_ms: u64,
}

impl ToolTrajectory {
    /// Ordered list of tool names called.
    pub fn call_names(&self) -> Vec<&str> {
        self.calls.iter().map(|c| c.tool_name.as_str()).collect()
    }

    /// Distinct tools used.
    pub fn unique_tools(&self) -> HashSet<&str> {
        self.calls.iter().map(|c| c.tool_name.as_str()).collect()
    }

    pub fn call_count(&self) -> usize {
        self.calls.len()
    }
}

/// Result of running one scenario within an eval.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioResult {
    pub scenario_id: String,
    pub scenario_name: String,
    pub agent_response: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trajectory: Option<ToolTrajectory>,
    #[serde(default)]
    pub scores: HashMap<String, f64>,
    #[serde(default)]
    pub reasoning: String,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Aggregate result for one eval type (e.g. tool_use).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalTypeResult {
    pub eval_type: String,
    #[serde(default)]
    pub scenarios: Vec<ScenarioResult>,
    #[serde(default)]
    pub metric_averages: HashMap<String, f64>,
    #[serde(default)]
    pub overall_score: f64,
    #[serde(default)]
    pub duration_s: f64,
}

impl EvalTypeResult {
    /// Compute metric averages across all scenarios.
    pub fn compute_averages(&mut self) {
        if self.scenarios.is_empty() {
            return;
        }
        let mut all_metrics: HashMap<String, Vec<f64>> = HashMap::new();
        for s in &self.scenarios {
            for (k, v) in &s.scores {
                all_metrics.entry(k.clone()).or_default().push(*v);
            }
        }
        self.metric_averages = all_metrics
            .iter()
            .map(|(k, v)| (k.clone(), v.iter().sum::<f64>() / v.len() as f64))
            .collect();
        if !self.metric_averages.is_empty() {
            self.overall_score =
                self.metric_averages.values().sum::<f64>() / self.metric_averages.len() as f64;
        }
    }
}

/// Complete report across all eval types.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CapabilityReport {
    #[serde(default)]
    pub eval_results: Vec<EvalTypeResult>,
    #[serde(default)]
    pub agent_name: String,
    #[serde(default)]
    pub agent_sdk: String,
    #[serde(default)]
    pub agent_model: String,
    #[serde(default)]
    pub grader_model: String,
    #[serde(default)]
    pub timestamp: String,
    #[serde(default)]
    pub total_time_s: f64,
}

impl CapabilityReport {
    pub fn overall_score(&self) -> f64 {
        if self.eval_results.is_empty() {
            return 0.0;
        }
        self.eval_results
            .iter()
            .map(|r| r.overall_score)
            .sum::<f64>()
            / self.eval_results.len() as f64
    }
}

// ---------------------------------------------------------------------------
// Scenario definitions
// ---------------------------------------------------------------------------

/// Defines a task with a gold-standard tool sequence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolUseScenario {
    pub scenario_id: String,
    pub name: String,
    pub task: String,
    pub context_content: String,
    pub expected_tool_order: Vec<String>,
    pub unnecessary_tools: Vec<String>,
    pub max_calls: usize,
}

/// Defines a task requiring multi-step planning.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanningScenario {
    pub scenario_id: String,
    pub name: String,
    pub task: String,
    pub context_content: String,
    pub expected_subtasks: Vec<String>,
    /// (before, after) ordering constraints.
    pub expected_ordering_constraints: Vec<(String, String)>,
    pub success_criteria: String,
}

/// Defines a scenario with conflicting or incomplete information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UncertaintyScenario {
    pub scenario_id: String,
    pub name: String,
    pub question: String,
    pub evidence_pieces: Vec<EvidencePiece>,
    pub expected_behavior: String,
    pub key_criteria: Vec<String>,
}

/// A piece of evidence with source and confidence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidencePiece {
    pub content: String,
    pub confidence: f64,
    pub source: String,
}

/// Defines a scenario testing cross-domain knowledge transfer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferScenario {
    pub scenario_id: String,
    pub name: String,
    pub source_domain_content: String,
    pub target_domain_question: String,
    pub expected_analogy: String,
    pub key_criteria: Vec<String>,
}

/// Defines a task requiring multi-agent collaboration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollaborativeScenario {
    pub scenario_id: String,
    pub name: String,
    pub task: String,
    pub context_content: String,
    pub expected_delegations: Vec<String>,
    pub synthesis_criteria: Vec<String>,
}

// ---------------------------------------------------------------------------
// Tool use efficiency grading
// ---------------------------------------------------------------------------

pub mod grading;
pub use grading::*;

#[cfg(test)]
#[path = "../tests/general_capability_tests.rs"]
mod tests;
