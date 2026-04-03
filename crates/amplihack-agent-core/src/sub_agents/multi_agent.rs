//! Multi-Agent Orchestrator: Composes coordinator, memory agent, and spawner.
//!
//! Replaces the Python `MultiAgentLearningAgent` with Rust composition.
//! Unlike the Python version which uses class inheritance, this module
//! separates orchestration (routing + retrieval) from synthesis (LLM calls).

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, warn};

use crate::agentic_loop::MemoryFact;

use super::agent_spawner::AgentSpawner;
use super::coordinator::CoordinatorAgent;
use super::memory_agent::MemoryAgent;
use super::types::{rerank_facts_by_query, SpawnedAgentStatus, SubAgentMemory, TaskRoute};

// ---------------------------------------------------------------------------
// MultiAgentConfig
// ---------------------------------------------------------------------------

/// Configuration for the multi-agent orchestrator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiAgentConfig {
    pub agent_name: String,
    pub enable_spawning: bool,
    pub parent_memory_path: String,
    #[serde(default)]
    pub use_hierarchical: bool,
    #[serde(default = "default_spawn_timeout")]
    pub spawn_timeout_secs: f64,
}

fn default_spawn_timeout() -> f64 {
    15.0
}

impl MultiAgentConfig {
    pub fn new(agent_name: impl Into<String>, parent_memory_path: impl Into<String>) -> Self {
        Self {
            agent_name: agent_name.into(),
            enable_spawning: false,
            parent_memory_path: parent_memory_path.into(),
            use_hierarchical: true,
            spawn_timeout_secs: default_spawn_timeout(),
        }
    }

    pub fn with_spawning(mut self, enabled: bool) -> Self {
        self.enable_spawning = enabled;
        self
    }
}

// ---------------------------------------------------------------------------
// AnswerContext
// ---------------------------------------------------------------------------

/// The result of the orchestration pipeline: facts + routing metadata.
///
/// The caller uses this to drive LLM synthesis separately.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnswerContext {
    /// Retrieved facts, ordered by relevance.
    pub facts: Vec<MemoryFact>,
    /// The route chosen by the coordinator.
    pub route: TaskRoute,
    /// The intent classification used for routing.
    pub intent: HashMap<String, Value>,
    /// Whether temporal sorting was applied.
    pub temporal_sorted: bool,
}

// ---------------------------------------------------------------------------
// MultiAgentOrchestrator
// ---------------------------------------------------------------------------

/// Orchestrates coordinator, memory agent, and optional spawner.
///
/// # Example
///
/// ```
/// use amplihack_agent_core::sub_agents::{
///     MultiAgentOrchestrator, MultiAgentConfig, SubAgentMemory, AnswerContext,
/// };
/// use amplihack_agent_core::agentic_loop::MemoryFact;
/// use std::collections::HashMap;
///
/// struct InMemory(Vec<MemoryFact>);
/// impl SubAgentMemory for InMemory {
///     fn search(&self, _q: &str, limit: usize) -> Vec<MemoryFact> {
///         self.0.iter().take(limit).cloned().collect()
///     }
///     fn get_all_facts(&self, limit: usize) -> Vec<MemoryFact> {
///         self.0.iter().take(limit).cloned().collect()
///     }
///     fn store_fact(&self, _c: &str, _f: &str, _conf: f64, _t: &[String]) {}
/// }
///
/// let config = MultiAgentConfig::new("test", "/data");
/// let mem = InMemory(vec![]);
/// let orch = MultiAgentOrchestrator::new(config, mem);
/// ```
pub struct MultiAgentOrchestrator<M: SubAgentMemory> {
    coordinator: CoordinatorAgent,
    memory_agent: MemoryAgent<M>,
    spawner: Option<AgentSpawner>,
    config: MultiAgentConfig,
}

impl<M: SubAgentMemory> MultiAgentOrchestrator<M> {
    /// Create a new orchestrator with the given configuration and memory backend.
    pub fn new(config: MultiAgentConfig, memory: M) -> Self {
        let coordinator = CoordinatorAgent::new(&config.agent_name);
        let memory_agent = MemoryAgent::new(memory, &config.agent_name);

        let spawner = if config.enable_spawning {
            match AgentSpawner::new(
                &config.agent_name,
                &config.parent_memory_path,
                "mini",
                4,
            ) {
                Ok(s) => Some(s),
                Err(e) => {
                    warn!(error = %e, "Failed to initialize spawner");
                    None
                }
            }
        } else {
            None
        };

        Self {
            coordinator,
            memory_agent,
            spawner,
            config,
        }
    }

    /// Run the orchestration pipeline: classify → retrieve → (optionally spawn).
    ///
    /// Returns an [`AnswerContext`] with retrieved facts and routing metadata.
    /// The caller is responsible for LLM synthesis using the returned context.
    pub fn retrieve_for_question(
        &mut self,
        question: &str,
        intent: &HashMap<String, Value>,
        _question_level: &str,
    ) -> AnswerContext {
        let route = self.coordinator.classify(question, intent);
        debug!(
            strategy = %route.retrieval_strategy,
            reasoning = route.needs_reasoning,
            r_type = %route.reasoning_type,
            "MultiAgent route"
        );

        let max_facts = if route.needs_reasoning { 60 } else { 30 };
        let mut facts = self.memory_agent.retrieve(question, intent, max_facts);

        // Optionally spawn retrieval sub-agents for multi-hop questions
        if self.spawner.is_some()
            && route.needs_reasoning
            && matches!(
                route.reasoning_type.as_str(),
                "multi_hop" | "causal" | "multi_source"
            )
            && let Some(spawned_facts) = self.spawn_retrieval(question)
        {
            let existing: std::collections::HashSet<String> =
                facts.iter().map(|f| f.outcome.clone()).collect();
            for f in spawned_facts {
                if !existing.contains(&f.outcome) {
                    facts.push(f);
                }
            }
        }

        // Fallback: get all facts if nothing found
        if facts.is_empty() {
            facts = self.memory_agent.memory().get_all_facts(50);
        }

        // Rerank unless temporal or meta-memory
        let intent_type = intent
            .get("intent")
            .and_then(|v| v.as_str())
            .unwrap_or("simple_recall");
        let needs_temporal = intent
            .get("needs_temporal")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let temporal_sorted = needs_temporal;

        if needs_temporal {
            facts.sort_by(|a, b| {
                let ta = a
                    .metadata
                    .get("temporal_index")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(999_999);
                let tb = b
                    .metadata
                    .get("temporal_index")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(999_999);
                ta.cmp(&tb)
            });
        } else if intent_type != "meta_memory" {
            facts = rerank_facts_by_query(&facts, question, None);
        }

        AnswerContext {
            facts,
            route,
            intent: intent.clone(),
            temporal_sorted,
        }
    }

    /// Access the underlying memory agent.
    pub fn memory_agent(&self) -> &MemoryAgent<M> {
        &self.memory_agent
    }

    /// Access the coordinator.
    pub fn coordinator(&self) -> &CoordinatorAgent {
        &self.coordinator
    }

    fn spawn_retrieval(&mut self, question: &str) -> Option<Vec<MemoryFact>> {
        let spawner = self.spawner.as_mut()?;
        let task = format!("Find all facts related to: {question}");
        if spawner.spawn(&task, "retrieval").is_err() {
            return None;
        }

        let timeout = Duration::from_secs_f64(self.config.spawn_timeout_secs);
        let results = spawner.collect_results(timeout);

        let mut facts = Vec::new();
        for agent in results {
            if agent.status == SpawnedAgentStatus::Completed
                && let Some(ref result_text) = agent.result
            {
                for line in result_text.lines() {
                    let line = line.trim();
                    if let Some(rest) = line.strip_prefix("- ")
                        && let Some((ctx, outcome)) = rest.split_once(':')
                    {
                        facts.push(MemoryFact {
                            id: format!("spawned_{}", facts.len()),
                            context: ctx.trim().to_string(),
                            outcome: outcome.trim().to_string(),
                            confidence: 0.7,
                            metadata: {
                                let mut m = HashMap::new();
                                m.insert(
                                    "source".into(),
                                    Value::String("spawned_retrieval".into()),
                                );
                                m
                            },
                        });
                    }
                }
            }
        }

        spawner.clear();
        Some(facts)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    struct MockMem {
        facts: Vec<MemoryFact>,
    }

    impl SubAgentMemory for MockMem {
        fn search(&self, _q: &str, limit: usize) -> Vec<MemoryFact> {
            self.facts.iter().take(limit).cloned().collect()
        }
        fn get_all_facts(&self, limit: usize) -> Vec<MemoryFact> {
            self.facts.iter().take(limit).cloned().collect()
        }
        fn store_fact(&self, _c: &str, _f: &str, _conf: f64, _t: &[String]) {}
    }

    fn fact(id: &str, ctx: &str, outcome: &str) -> MemoryFact {
        MemoryFact {
            id: id.into(),
            context: ctx.into(),
            outcome: outcome.into(),
            confidence: 0.8,
            metadata: HashMap::new(),
        }
    }

    fn make_intent(s: &str) -> HashMap<String, Value> {
        let mut m = HashMap::new();
        m.insert("intent".into(), Value::String(s.into()));
        m
    }

    #[test]
    fn orchestrator_creation() {
        let config = MultiAgentConfig::new("test", "/data");
        let mem = MockMem { facts: vec![] };
        let orch = MultiAgentOrchestrator::new(config, mem);
        assert_eq!(orch.coordinator().agent_name, "test");
    }

    #[test]
    fn orchestrator_with_spawning() {
        let config = MultiAgentConfig::new("test", "/data").with_spawning(true);
        let mem = MockMem { facts: vec![] };
        let orch = MultiAgentOrchestrator::new(config, mem);
        assert!(orch.spawner.is_some());
    }

    #[test]
    fn retrieve_returns_facts() {
        let config = MultiAgentConfig::new("test", "/data");
        let facts = vec![fact("1", "Sarah", "has a cat"), fact("2", "Bob", "likes dogs")];
        let mem = MockMem { facts };
        let mut orch = MultiAgentOrchestrator::new(config, mem);

        let ctx = orch.retrieve_for_question("pets?", &make_intent("simple_recall"), "L1");
        assert_eq!(ctx.facts.len(), 2);
        assert!(!ctx.temporal_sorted);
    }

    #[test]
    fn retrieve_empty_fallback() {
        let config = MultiAgentConfig::new("test", "/data");
        let mem = MockMem { facts: vec![] };
        let mut orch = MultiAgentOrchestrator::new(config, mem);

        let ctx = orch.retrieve_for_question("anything?", &make_intent("simple_recall"), "L1");
        assert!(ctx.facts.is_empty());
    }

    #[test]
    fn retrieve_temporal_sorts() {
        let config = MultiAgentConfig::new("test", "/data");
        let mut f1 = fact("1", "early", "first event");
        f1.metadata
            .insert("temporal_index".into(), serde_json::json!(100));
        let mut f2 = fact("2", "late", "second event");
        f2.metadata
            .insert("temporal_index".into(), serde_json::json!(1));
        let mem = MockMem {
            facts: vec![f1, f2],
        };
        let mut orch = MultiAgentOrchestrator::new(config, mem);

        let mut intent = make_intent("simple_recall");
        intent.insert("needs_temporal".into(), Value::Bool(true));
        let ctx = orch.retrieve_for_question("timeline?", &intent, "L1");
        assert!(ctx.temporal_sorted);
        assert_eq!(ctx.facts[0].id, "2"); // lower temporal_index first
    }

    #[test]
    fn config_defaults() {
        let cfg = MultiAgentConfig::new("a", "/p");
        assert!(!cfg.enable_spawning);
        assert!(cfg.use_hierarchical);
        assert!((cfg.spawn_timeout_secs - 15.0).abs() < f64::EPSILON);
    }

    #[test]
    fn config_serde_roundtrip() {
        let cfg = MultiAgentConfig::new("agent", "/path").with_spawning(true);
        let json = serde_json::to_string(&cfg).unwrap();
        let parsed: MultiAgentConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.agent_name, "agent");
        assert!(parsed.enable_spawning);
    }
}
