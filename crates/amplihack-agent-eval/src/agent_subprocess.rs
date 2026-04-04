//! Subprocess management for agent learning / testing phases.
//!
//! Ports Python `amplihack/evaluation/agent_subprocess.py`.
//! Provides isolated subprocess execution of learning and testing phases with
//! structured result serialisation.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::{debug, warn};

use crate::agent_adapter::SubprocessConfig;
use crate::error::EvalError;

// ---------------------------------------------------------------------------
// Trace & result types
// ---------------------------------------------------------------------------

/// A single step inside a reasoning trace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceStep {
    pub step_type: String,
    #[serde(default)]
    pub queries: Vec<String>,
    #[serde(default)]
    pub facts_found: usize,
}

/// Full reasoning trace for one question-answer cycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningTrace {
    pub question: String,
    pub intent: String,
    pub steps: Vec<TraceStep>,
    pub total_facts_collected: usize,
    pub total_queries_executed: usize,
    pub iterations: usize,
    pub final_confidence: f64,
    #[serde(default)]
    pub used_simple_path: bool,
}

/// Result of the learning phase.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearningPhaseResult {
    pub stored_count: usize,
    pub total_articles: usize,
    pub sdk: String,
    #[serde(default)]
    pub sdk_agent_created: bool,
}

/// A single answer produced during the testing phase.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestingAnswer {
    pub question: String,
    pub answer: String,
    pub confidence: f64,
    #[serde(default)]
    pub memories_used: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_trace: Option<ReasoningTrace>,
}

/// Result of the testing phase.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestingPhaseResult {
    pub answers: Vec<TestingAnswer>,
}

// ---------------------------------------------------------------------------
// Dynamic confidence computation
// ---------------------------------------------------------------------------

/// Compute dynamic confidence from a reasoning trace and memory stats.
///
/// Returns a value in `[0.0, 1.0]` combining fact coverage, query
/// effectiveness, and iteration depth.
pub fn compute_dynamic_confidence(
    trace: &ReasoningTrace,
    memory_stats: &HashMap<String, usize>,
) -> f64 {
    let total_experiences = *memory_stats.get("total_experiences").unwrap_or(&0);

    // Fact coverage ratio
    let coverage = if total_experiences > 0 {
        (trace.total_facts_collected as f64 / total_experiences as f64).min(1.0)
    } else {
        0.0
    };

    // Query effectiveness — at least 1 fact per query on average
    let query_eff = if trace.total_queries_executed > 0 {
        (trace.total_facts_collected as f64 / trace.total_queries_executed as f64).min(1.0)
    } else {
        0.0
    };

    // Depth bonus — more steps means more thorough search
    let depth = (trace.iterations as f64 / 5.0).min(1.0);

    // Simple path penalty
    let simple_penalty = if trace.used_simple_path { 0.2 } else { 0.0 };

    let raw = 0.4 * coverage + 0.35 * query_eff + 0.25 * depth - simple_penalty;
    raw.clamp(0.0, 1.0)
}

// ---------------------------------------------------------------------------
// Subprocess phase runner
// ---------------------------------------------------------------------------

/// Which phase to execute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Learning,
    Testing,
}

/// Configuration for an agent subprocess run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSubprocessConfig {
    pub phase: Phase,
    pub agent_name: String,
    pub sdk: String,
    pub input_file: PathBuf,
    #[serde(default)]
    pub subprocess: Option<SubprocessConfig>,
}

impl AgentSubprocessConfig {
    pub fn new(
        phase: Phase,
        agent_name: impl Into<String>,
        input_file: impl Into<PathBuf>,
    ) -> Self {
        Self {
            phase,
            agent_name: agent_name.into(),
            sdk: "mini".to_string(),
            input_file: input_file.into(),
            subprocess: None,
        }
    }

    pub fn with_sdk(mut self, sdk: impl Into<String>) -> Self {
        self.sdk = sdk.into();
        self
    }

    pub fn with_subprocess(mut self, config: SubprocessConfig) -> Self {
        self.subprocess = Some(config);
        self
    }
}

/// Run the configured phase and return JSON output.
///
/// In production, this spawns a child process. Here we validate the config
/// and construct the expected command arguments; actual process spawning is
/// deferred to the runtime integration layer.
pub fn build_phase_command(config: &AgentSubprocessConfig) -> Result<Vec<String>, EvalError> {
    if config.agent_name.is_empty() {
        return Err(EvalError::config("agent_name must not be empty"));
    }
    if !config.input_file.as_os_str().is_empty()
        && config.input_file.extension().is_none_or(|e| e != "json")
    {
        warn!(
            input_file = %config.input_file.display(),
            "input file does not have .json extension"
        );
    }

    let phase_str = match config.phase {
        Phase::Learning => "learning",
        Phase::Testing => "testing",
    };

    debug!(
        phase = phase_str,
        agent = %config.agent_name,
        sdk = %config.sdk,
        "Building agent subprocess command"
    );

    Ok(vec![
        "--phase".to_string(),
        phase_str.to_string(),
        "--agent-name".to_string(),
        config.agent_name.clone(),
        "--input-file".to_string(),
        config.input_file.display().to_string(),
        "--sdk".to_string(),
        config.sdk.clone(),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_trace() -> ReasoningTrace {
        ReasoningTrace {
            question: "What happened?".into(),
            intent: "recall".into(),
            steps: vec![TraceStep {
                step_type: "query".into(),
                queries: vec!["q1".into()],
                facts_found: 3,
            }],
            total_facts_collected: 5,
            total_queries_executed: 2,
            iterations: 3,
            final_confidence: 0.75,
            used_simple_path: false,
        }
    }

    #[test]
    fn dynamic_confidence_normal() {
        let trace = sample_trace();
        let mut stats = HashMap::new();
        stats.insert("total_experiences".to_string(), 10);
        let conf = compute_dynamic_confidence(&trace, &stats);
        assert!(conf > 0.0 && conf <= 1.0);
    }

    #[test]
    fn dynamic_confidence_zero_experiences() {
        let trace = sample_trace();
        let stats = HashMap::new();
        let conf = compute_dynamic_confidence(&trace, &stats);
        assert!((0.0..=1.0).contains(&conf));
    }

    #[test]
    fn dynamic_confidence_simple_path_penalty() {
        let mut trace = sample_trace();
        let mut stats = HashMap::new();
        stats.insert("total_experiences".to_string(), 10);

        let without = compute_dynamic_confidence(&trace, &stats);
        trace.used_simple_path = true;
        let with = compute_dynamic_confidence(&trace, &stats);
        assert!(with < without);
    }

    #[test]
    fn build_phase_command_learning() {
        let config = AgentSubprocessConfig::new(Phase::Learning, "test-agent", "data.json");
        let cmd = build_phase_command(&config).unwrap();
        assert!(cmd.contains(&"learning".to_string()));
        assert!(cmd.contains(&"test-agent".to_string()));
    }

    #[test]
    fn build_phase_command_testing() {
        let config =
            AgentSubprocessConfig::new(Phase::Testing, "agent", "quiz.json").with_sdk("claude");
        let cmd = build_phase_command(&config).unwrap();
        assert!(cmd.contains(&"testing".to_string()));
        assert!(cmd.contains(&"claude".to_string()));
    }

    #[test]
    fn build_phase_command_empty_agent() {
        let config = AgentSubprocessConfig::new(Phase::Learning, "", "data.json");
        assert!(build_phase_command(&config).is_err());
    }

    #[test]
    fn phase_serde_roundtrip() {
        let config = AgentSubprocessConfig::new(Phase::Learning, "a", "b.json").with_sdk("copilot");
        let json = serde_json::to_string(&config).unwrap();
        let restored: AgentSubprocessConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.phase, Phase::Learning);
        assert_eq!(restored.sdk, "copilot");
    }

    #[test]
    fn learning_phase_result_serde() {
        let r = LearningPhaseResult {
            stored_count: 10,
            total_articles: 12,
            sdk: "mini".into(),
            sdk_agent_created: true,
        };
        let json = serde_json::to_string(&r).unwrap();
        let restored: LearningPhaseResult = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.stored_count, 10);
        assert!(restored.sdk_agent_created);
    }

    #[test]
    fn testing_answer_serde() {
        let a = TestingAnswer {
            question: "Q?".into(),
            answer: "A".into(),
            confidence: 0.8,
            memories_used: 3,
            reasoning_trace: Some(sample_trace()),
        };
        let json = serde_json::to_string(&a).unwrap();
        let restored: TestingAnswer = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.confidence, 0.8);
        assert!(restored.reasoning_trace.is_some());
    }

    #[test]
    fn reasoning_trace_serde() {
        let trace = sample_trace();
        let json = serde_json::to_string(&trace).unwrap();
        let restored: ReasoningTrace = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.total_facts_collected, 5);
        assert_eq!(restored.steps.len(), 1);
    }
}
