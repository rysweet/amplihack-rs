//! LearningAgent — orchestrates learning and evaluation phases.
//!
//! Ports Python `amplihack/agents/goal_seeking/learning_agent.py` (structural):
//! - LearningAgent configuration and lifecycle
//! - Learning/evaluation phase orchestration
//! - Memory backend integration (CognitiveAdapter / FlatRetriever)
//! - Thread-local state snapshots

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Phase of the learning agent lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LearningPhase {
    Idle,
    Learning,
    Evaluating,
    Teaching,
    Reflecting,
}

impl std::fmt::Display for LearningPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Idle => write!(f, "idle"),
            Self::Learning => write!(f, "learning"),
            Self::Evaluating => write!(f, "evaluating"),
            Self::Teaching => write!(f, "teaching"),
            Self::Reflecting => write!(f, "reflecting"),
        }
    }
}

/// Configuration for the learning agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearningAgentConfig {
    pub agent_name: String,
    pub model: String,
    pub max_learning_turns: usize,
    pub max_eval_turns: usize,
    pub memory_backend: MemoryBackendKind,
    #[serde(default)]
    pub variant_prompt: Option<String>,
}

impl Default for LearningAgentConfig {
    fn default() -> Self {
        Self {
            agent_name: "learning-agent".to_string(),
            model: "claude-opus-4-6".to_string(),
            max_learning_turns: 100,
            max_eval_turns: 50,
            memory_backend: MemoryBackendKind::Cognitive,
            variant_prompt: None,
        }
    }
}

/// Memory backend kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemoryBackendKind {
    Cognitive,
    FlatRetriever,
    GraphRag,
}

/// Snapshot of agent state at a point in time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSnapshot {
    pub phase: LearningPhase,
    pub facts_stored: usize,
    pub facts_retrieved: usize,
    pub turn_count: usize,
    pub timestamp: String,
}

/// Result from a learning phase.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearningPhaseResult {
    pub phase: LearningPhase,
    pub turns_completed: usize,
    pub facts_stored: usize,
    pub success: bool,
    #[serde(default)]
    pub error: Option<String>,
}

/// Result from an evaluation phase.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalPhaseResult {
    pub questions_answered: usize,
    pub score: f64,
    pub level_scores: HashMap<String, f64>,
    pub success: bool,
}

/// The LearningAgent — orchestrates learn-then-test cycles.
///
/// Structural port: actual LLM calls require pluggable backends.
/// This provides the types, state machine, and orchestration logic.
pub struct LearningAgent {
    config: LearningAgentConfig,
    phase: LearningPhase,
    facts_stored: usize,
    facts_retrieved: usize,
    turn_count: usize,
    snapshots: Vec<AgentSnapshot>,
}

impl LearningAgent {
    pub fn new(config: LearningAgentConfig) -> Self {
        Self {
            config,
            phase: LearningPhase::Idle,
            facts_stored: 0,
            facts_retrieved: 0,
            turn_count: 0,
            snapshots: Vec::new(),
        }
    }

    /// Get current phase.
    pub fn phase(&self) -> LearningPhase {
        self.phase
    }

    /// Get agent name.
    pub fn name(&self) -> &str {
        &self.config.agent_name
    }

    /// Get agent config.
    pub fn config(&self) -> &LearningAgentConfig {
        &self.config
    }

    /// Take a snapshot of current state.
    pub fn snapshot(&mut self) -> AgentSnapshot {
        let snap = AgentSnapshot {
            phase: self.phase,
            facts_stored: self.facts_stored,
            facts_retrieved: self.facts_retrieved,
            turn_count: self.turn_count,
            timestamp: chrono::Utc::now().to_rfc3339(),
        };
        self.snapshots.push(snap.clone());
        snap
    }

    /// Get all snapshots.
    pub fn snapshots(&self) -> &[AgentSnapshot] {
        &self.snapshots
    }

    /// Begin learning phase.
    pub fn begin_learning(&mut self) -> Result<(), String> {
        if self.phase != LearningPhase::Idle {
            return Err(format!("Cannot start learning from phase '{}'", self.phase));
        }
        self.phase = LearningPhase::Learning;
        self.turn_count = 0;
        Ok(())
    }

    /// Record a fact stored during learning.
    pub fn record_fact_stored(&mut self) {
        self.facts_stored += 1;
        self.turn_count += 1;
    }

    /// End learning phase.
    pub fn end_learning(&mut self) -> LearningPhaseResult {
        let result = LearningPhaseResult {
            phase: LearningPhase::Learning,
            turns_completed: self.turn_count,
            facts_stored: self.facts_stored,
            success: true,
            error: None,
        };
        self.phase = LearningPhase::Idle;
        result
    }

    /// Begin evaluation phase.
    pub fn begin_evaluation(&mut self) -> Result<(), String> {
        if self.phase != LearningPhase::Idle {
            return Err(format!(
                "Cannot start evaluation from phase '{}'",
                self.phase
            ));
        }
        self.phase = LearningPhase::Evaluating;
        self.turn_count = 0;
        Ok(())
    }

    /// Record a fact retrieved during evaluation.
    pub fn record_fact_retrieved(&mut self) {
        self.facts_retrieved += 1;
        self.turn_count += 1;
    }

    /// End evaluation phase.
    pub fn end_evaluation(
        &mut self,
        score: f64,
        level_scores: HashMap<String, f64>,
    ) -> EvalPhaseResult {
        let result = EvalPhaseResult {
            questions_answered: self.turn_count,
            score,
            level_scores,
            success: true,
        };
        self.phase = LearningPhase::Idle;
        result
    }

    /// Reset agent to idle state.
    pub fn reset(&mut self) {
        self.phase = LearningPhase::Idle;
        self.facts_stored = 0;
        self.facts_retrieved = 0;
        self.turn_count = 0;
        self.snapshots.clear();
    }

    /// Get summary statistics.
    pub fn stats(&self) -> HashMap<String, serde_json::Value> {
        let mut stats = HashMap::new();
        stats.insert(
            "phase".to_string(),
            serde_json::Value::String(self.phase.to_string()),
        );
        stats.insert(
            "facts_stored".to_string(),
            serde_json::json!(self.facts_stored),
        );
        stats.insert(
            "facts_retrieved".to_string(),
            serde_json::json!(self.facts_retrieved),
        );
        stats.insert("turn_count".to_string(), serde_json::json!(self.turn_count));
        stats.insert(
            "snapshots".to_string(),
            serde_json::json!(self.snapshots.len()),
        );
        stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let cfg = LearningAgentConfig::default();
        assert_eq!(cfg.max_learning_turns, 100);
        assert_eq!(cfg.memory_backend, MemoryBackendKind::Cognitive);
    }

    #[test]
    fn lifecycle() {
        let mut agent = LearningAgent::new(LearningAgentConfig::default());
        assert_eq!(agent.phase(), LearningPhase::Idle);

        agent.begin_learning().unwrap();
        assert_eq!(agent.phase(), LearningPhase::Learning);

        agent.record_fact_stored();
        agent.record_fact_stored();
        let result = agent.end_learning();
        assert!(result.success);
        assert_eq!(result.facts_stored, 2);
        assert_eq!(agent.phase(), LearningPhase::Idle);
    }

    #[test]
    fn evaluation_lifecycle() {
        let mut agent = LearningAgent::new(LearningAgentConfig::default());
        agent.begin_evaluation().unwrap();
        agent.record_fact_retrieved();

        let scores = HashMap::from([("L1".to_string(), 0.9)]);
        let result = agent.end_evaluation(0.85, scores);
        assert!(result.success);
        assert_eq!(result.questions_answered, 1);
    }

    #[test]
    fn invalid_phase_transition() {
        let mut agent = LearningAgent::new(LearningAgentConfig::default());
        agent.begin_learning().unwrap();
        assert!(agent.begin_evaluation().is_err());
    }

    #[test]
    fn snapshot() {
        let mut agent = LearningAgent::new(LearningAgentConfig::default());
        agent.begin_learning().unwrap();
        let snap = agent.snapshot();
        assert_eq!(snap.phase, LearningPhase::Learning);
        assert_eq!(agent.snapshots().len(), 1);
    }

    #[test]
    fn reset() {
        let mut agent = LearningAgent::new(LearningAgentConfig::default());
        agent.begin_learning().unwrap();
        agent.record_fact_stored();
        agent.reset();
        assert_eq!(agent.phase(), LearningPhase::Idle);
        assert!(agent.snapshots().is_empty());
    }

    #[test]
    fn stats() {
        let agent = LearningAgent::new(LearningAgentConfig::default());
        let stats = agent.stats();
        assert_eq!(stats["phase"], "idle");
        assert_eq!(stats["facts_stored"], 0);
    }

    #[test]
    fn config_serde() {
        let cfg = LearningAgentConfig::default();
        let json = serde_json::to_string(&cfg).unwrap();
        let back: LearningAgentConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.agent_name, "learning-agent");
    }

    #[test]
    fn phase_display() {
        assert_eq!(LearningPhase::Learning.to_string(), "learning");
        assert_eq!(LearningPhase::Idle.to_string(), "idle");
    }
}
