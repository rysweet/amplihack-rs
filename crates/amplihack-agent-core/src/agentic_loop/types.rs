//! Data types for the agentic loop.
//!
//! Ports from Python `agentic_loop.py`:
//! - `LoopState`, `RetrievalPlan`, `ReasoningStep`, `ReasoningTrace`,
//!   `SufficiencyEvaluation`

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// LoopState
// ---------------------------------------------------------------------------

/// State for one iteration of the PERCEIVE→REASON→ACT→LEARN loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopState {
    /// What the agent observes.
    pub perception: String,
    /// Agent's reasoning about the situation.
    pub reasoning: String,
    /// Action decided by the agent (`{"action": "..", "params": {..}}`).
    pub action: HashMap<String, Value>,
    /// What the agent learned from the outcome.
    pub learning: String,
    /// Result of the action.
    pub outcome: Value,
    /// 1-based iteration number.
    pub iteration: usize,
}

// ---------------------------------------------------------------------------
// RetrievalPlan
// ---------------------------------------------------------------------------

/// Plan for what information to retrieve from memory.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RetrievalPlan {
    /// Targeted queries to run against memory.
    pub search_queries: Vec<String>,
    /// Why these queries were chosen.
    pub reasoning: String,
}

// ---------------------------------------------------------------------------
// ReasoningStep
// ---------------------------------------------------------------------------

/// A single step in the reasoning trace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningStep {
    /// Type of step: `"plan"`, `"search"`, `"evaluate"`, `"refine"`.
    pub step_type: String,
    /// Search queries generated or executed.
    #[serde(default)]
    pub queries: Vec<String>,
    /// Number of new facts found this step.
    #[serde(default)]
    pub facts_found: usize,
    /// Sufficiency evaluation if applicable.
    #[serde(default)]
    pub evaluation: HashMap<String, Value>,
    /// LLM reasoning for this step.
    #[serde(default)]
    pub reasoning: String,
}

impl ReasoningStep {
    /// Create a plan or refine step.
    pub fn plan_or_refine(step_type: &str, queries: Vec<String>, reasoning: String) -> Self {
        Self {
            step_type: step_type.to_string(),
            queries,
            facts_found: 0,
            evaluation: HashMap::new(),
            reasoning,
        }
    }

    /// Create a search step.
    pub fn search(queries: Vec<String>, facts_found: usize) -> Self {
        Self {
            step_type: "search".to_string(),
            queries,
            facts_found,
            evaluation: HashMap::new(),
            reasoning: String::new(),
        }
    }

    /// Create an evaluation step.
    pub fn evaluate(sufficient: bool, confidence: f64, missing: &str) -> Self {
        let mut evaluation = HashMap::new();
        evaluation.insert("sufficient".to_string(), Value::Bool(sufficient));
        evaluation.insert(
            "confidence".to_string(),
            serde_json::to_value(confidence).unwrap_or(Value::Null),
        );
        evaluation.insert("missing".to_string(), Value::String(missing.to_string()));
        Self {
            step_type: "evaluate".to_string(),
            queries: Vec::new(),
            facts_found: 0,
            evaluation,
            reasoning: String::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// ReasoningTrace
// ---------------------------------------------------------------------------

/// Complete trace of the reasoning process for metacognition evaluation.
///
/// Captures the full plan→search→evaluate→refine cycle.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReasoningTrace {
    /// The original question.
    pub question: String,
    /// Classified intent.
    pub intent: HashMap<String, Value>,
    /// List of reasoning steps taken.
    pub steps: Vec<ReasoningStep>,
    /// Total unique facts collected.
    pub total_facts_collected: usize,
    /// Total search queries run.
    pub total_queries_executed: usize,
    /// Number of plan-search-evaluate iterations.
    pub iterations: usize,
    /// Final sufficiency confidence.
    pub final_confidence: f64,
    /// Whether simple retrieval was used (no iteration).
    pub used_simple_path: bool,
    /// Additional metadata.
    #[serde(default)]
    pub metadata: HashMap<String, Value>,
}

// ---------------------------------------------------------------------------
// SufficiencyEvaluation
// ---------------------------------------------------------------------------

/// Evaluation of whether collected facts are sufficient to answer.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SufficiencyEvaluation {
    /// Whether we have enough information.
    pub sufficient: bool,
    /// Description of what is still missing.
    pub missing: String,
    /// Confidence that we can answer (0.0–1.0).
    pub confidence: f64,
    /// New queries to try if insufficient.
    pub refined_queries: Vec<String>,
}

// ---------------------------------------------------------------------------
// ActionResult
// ---------------------------------------------------------------------------

/// Result from executing an action via [`ActionExecutor`](super::traits::ActionExecutor).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionResult {
    pub success: bool,
    pub output: Value,
    pub error: Option<String>,
}

impl ActionResult {
    pub fn ok(output: Value) -> Self {
        Self {
            success: true,
            output,
            error: None,
        }
    }

    pub fn fail(error: impl Into<String>) -> Self {
        Self {
            success: false,
            output: Value::Null,
            error: Some(error.into()),
        }
    }
}

// ---------------------------------------------------------------------------
// MemoryFact
// ---------------------------------------------------------------------------

/// A fact retrieved from memory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryFact {
    /// Identifier (for deduplication).
    pub id: String,
    /// The context / concept label.
    pub context: String,
    /// The actual content / outcome.
    pub outcome: String,
    /// Confidence score.
    #[serde(default = "default_confidence")]
    pub confidence: f64,
    /// Arbitrary metadata.
    #[serde(default)]
    pub metadata: HashMap<String, Value>,
}

fn default_confidence() -> f64 {
    1.0
}

// ---------------------------------------------------------------------------
// LlmMessage
// ---------------------------------------------------------------------------

/// A single message in an LLM conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmMessage {
    pub role: String,
    pub content: String,
}

impl LlmMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".to_string(),
            content: content.into(),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: content.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loop_state_serde_roundtrip() {
        let state = LoopState {
            perception: "goal".into(),
            reasoning: "because".into(),
            action: {
                let mut m = HashMap::new();
                m.insert("action".into(), Value::String("greet".into()));
                m
            },
            learning: "learned".into(),
            outcome: Value::String("ok".into()),
            iteration: 1,
        };
        let json = serde_json::to_string(&state).unwrap();
        let parsed: LoopState = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.iteration, 1);
        assert_eq!(parsed.perception, "goal");
    }

    #[test]
    fn sufficiency_evaluation_defaults() {
        let eval = SufficiencyEvaluation::default();
        assert!(!eval.sufficient);
        assert_eq!(eval.confidence, 0.0);
        assert!(eval.refined_queries.is_empty());
    }

    #[test]
    fn retrieval_plan_defaults() {
        let plan = RetrievalPlan::default();
        assert!(plan.search_queries.is_empty());
        assert!(plan.reasoning.is_empty());
    }

    #[test]
    fn action_result_ok() {
        let r = ActionResult::ok(Value::String("done".into()));
        assert!(r.success);
        assert!(r.error.is_none());
    }

    #[test]
    fn action_result_fail() {
        let r = ActionResult::fail("boom");
        assert!(!r.success);
        assert_eq!(r.error.as_deref(), Some("boom"));
    }

    #[test]
    fn reasoning_step_constructors() {
        let plan = ReasoningStep::plan_or_refine("plan", vec!["q1".into()], "reason".into());
        assert_eq!(plan.step_type, "plan");
        assert_eq!(plan.queries.len(), 1);

        let search = ReasoningStep::search(vec!["q1".into()], 5);
        assert_eq!(search.facts_found, 5);

        let eval = ReasoningStep::evaluate(true, 0.9, "");
        assert_eq!(eval.step_type, "evaluate");
    }

    #[test]
    fn llm_message_constructors() {
        let sys = LlmMessage::system("you are helpful");
        assert_eq!(sys.role, "system");
        let usr = LlmMessage::user("hello");
        assert_eq!(usr.role, "user");
    }

    #[test]
    fn memory_fact_serde() {
        let fact = MemoryFact {
            id: "f1".into(),
            context: "ctx".into(),
            outcome: "out".into(),
            confidence: 0.8,
            metadata: HashMap::new(),
        };
        let json = serde_json::to_string(&fact).unwrap();
        let parsed: MemoryFact = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, "f1");
        assert!((parsed.confidence - 0.8).abs() < f64::EPSILON);
    }

    #[test]
    fn reasoning_trace_defaults() {
        let trace = ReasoningTrace::default();
        assert!(trace.question.is_empty());
        assert!(trace.steps.is_empty());
        assert_eq!(trace.iterations, 0);
        assert!(!trace.used_simple_path);
    }
}
