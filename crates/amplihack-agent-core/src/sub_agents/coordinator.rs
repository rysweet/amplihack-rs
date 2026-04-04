//! Coordinator Agent: Task classification and routing to specialist sub-agents.
//!
//! The coordinator receives a question + intent classification and determines:
//! 1. Which retrieval strategy the [`super::MemoryAgent`] should use
//! 2. Whether dedicated reasoning is needed (and what type)
//! 3. Whether teaching mode should be activated
//!
//! It does **not** perform any retrieval or synthesis itself.

use std::collections::HashMap;

use serde_json::Value;

use super::types::TaskRoute;

// ---------------------------------------------------------------------------
// CoordinatorAgent
// ---------------------------------------------------------------------------

/// Routes tasks to specialist sub-agents based on intent classification.
///
/// # Example
///
/// ```
/// use amplihack_agent_core::sub_agents::CoordinatorAgent;
///
/// let coord = CoordinatorAgent::new("test");
/// let mut intent = std::collections::HashMap::new();
/// intent.insert("intent".to_string(), serde_json::json!("meta_memory"));
/// let route = coord.classify("How many projects?", &intent);
/// assert_eq!(route.retrieval_strategy, "aggregation");
/// ```
pub struct CoordinatorAgent {
    pub agent_name: String,
}

impl CoordinatorAgent {
    pub fn new(agent_name: impl Into<String>) -> Self {
        Self {
            agent_name: agent_name.into(),
        }
    }

    /// Classify a question and determine the execution route.
    pub fn classify(&self, question: &str, intent: &HashMap<String, Value>) -> TaskRoute {
        let intent_type = intent
            .get("intent")
            .and_then(|v| v.as_str())
            .unwrap_or("simple_recall");
        let q_lower = question.to_lowercase();

        // Teaching check first (takes priority)
        if q_lower.contains("teach") || q_lower.contains("explain to") {
            return TaskRoute {
                needs_teaching: true,
                ..TaskRoute::default()
            };
        }

        match intent_type {
            "meta_memory" => TaskRoute {
                retrieval_strategy: "aggregation".into(),
                ..TaskRoute::default()
            },
            "simple_recall" | "incremental_update" => TaskRoute::default(),
            "temporal_comparison" => TaskRoute {
                retrieval_strategy: "temporal".into(),
                needs_reasoning: true,
                reasoning_type: "temporal".into(),
                ..TaskRoute::default()
            },
            "mathematical_computation" => TaskRoute {
                needs_reasoning: true,
                reasoning_type: "mathematical".into(),
                ..TaskRoute::default()
            },
            "causal_counterfactual" => TaskRoute {
                needs_reasoning: true,
                reasoning_type: "causal".into(),
                ..TaskRoute::default()
            },
            "multi_source_synthesis" => TaskRoute {
                needs_reasoning: true,
                reasoning_type: "multi_source".into(),
                ..TaskRoute::default()
            },
            "contradiction_resolution" => TaskRoute {
                needs_reasoning: true,
                reasoning_type: "contradiction".into(),
                ..TaskRoute::default()
            },
            "ratio_trend_analysis" => TaskRoute {
                retrieval_strategy: "temporal".into(),
                needs_reasoning: true,
                reasoning_type: "ratio_trend".into(),
                ..TaskRoute::default()
            },
            _ => TaskRoute {
                needs_reasoning: true,
                reasoning_type: "general".into(),
                ..TaskRoute::default()
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn intent(s: &str) -> HashMap<String, Value> {
        let mut m = HashMap::new();
        m.insert("intent".into(), Value::String(s.into()));
        m
    }

    #[test]
    fn classify_meta_memory() {
        let coord = CoordinatorAgent::new("test");
        let route = coord.classify("How many projects?", &intent("meta_memory"));
        assert_eq!(route.retrieval_strategy, "aggregation");
        assert!(!route.needs_reasoning);
    }

    #[test]
    fn classify_simple_recall() {
        let coord = CoordinatorAgent::new("test");
        let route = coord.classify("What is Sarah's pet?", &intent("simple_recall"));
        assert_eq!(route.retrieval_strategy, "auto");
        assert!(!route.needs_reasoning);
    }

    #[test]
    fn classify_temporal() {
        let coord = CoordinatorAgent::new("test");
        let route = coord.classify("When did it change?", &intent("temporal_comparison"));
        assert_eq!(route.retrieval_strategy, "temporal");
        assert!(route.needs_reasoning);
        assert_eq!(route.reasoning_type, "temporal");
    }

    #[test]
    fn classify_mathematical() {
        let coord = CoordinatorAgent::new("test");
        let route = coord.classify("Calculate the sum", &intent("mathematical_computation"));
        assert!(route.needs_reasoning);
        assert_eq!(route.reasoning_type, "mathematical");
    }

    #[test]
    fn classify_teaching_takes_priority() {
        let coord = CoordinatorAgent::new("test");
        let route = coord.classify("Teach me about math", &intent("mathematical_computation"));
        assert!(route.needs_teaching);
        assert!(!route.needs_reasoning);
    }

    #[test]
    fn classify_unknown_intent_defaults_to_general() {
        let coord = CoordinatorAgent::new("test");
        let route = coord.classify("Some obscure question", &intent("custom_intent"));
        assert!(route.needs_reasoning);
        assert_eq!(route.reasoning_type, "general");
    }

    #[test]
    fn classify_missing_intent_field() {
        let coord = CoordinatorAgent::new("test");
        let route = coord.classify("Hello?", &HashMap::new());
        // Missing intent defaults to "simple_recall"
        assert_eq!(route.retrieval_strategy, "auto");
        assert!(!route.needs_reasoning);
    }

    #[test]
    fn classify_causal() {
        let coord = CoordinatorAgent::new("test");
        let route = coord.classify("Why did X cause Y?", &intent("causal_counterfactual"));
        assert!(route.needs_reasoning);
        assert_eq!(route.reasoning_type, "causal");
    }

    #[test]
    fn classify_multi_source() {
        let coord = CoordinatorAgent::new("test");
        let route = coord.classify("Combine info", &intent("multi_source_synthesis"));
        assert!(route.needs_reasoning);
        assert_eq!(route.reasoning_type, "multi_source");
    }

    #[test]
    fn classify_contradiction() {
        let coord = CoordinatorAgent::new("test");
        let route = coord.classify("Resolve conflict", &intent("contradiction_resolution"));
        assert!(route.needs_reasoning);
        assert_eq!(route.reasoning_type, "contradiction");
    }

    #[test]
    fn classify_ratio_trend() {
        let coord = CoordinatorAgent::new("test");
        let route = coord.classify("Trend analysis", &intent("ratio_trend_analysis"));
        assert_eq!(route.retrieval_strategy, "temporal");
        assert!(route.needs_reasoning);
        assert_eq!(route.reasoning_type, "ratio_trend");
    }
}
