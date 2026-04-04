//! Core PERCEIVE→REASON→ACT→LEARN loop.
//!
//! Ports the Python `AgenticLoop` class — constructor, `perceive`, `reason`,
//! `act`, `learn`, `run_iteration`, `run_until_goal`, `observe`, and `orient`.

use std::collections::HashMap;

use serde_json::Value;
use tracing::error;

use crate::error::AgentError;

use super::json_parse::parse_json_response;
use super::loop_helpers::{error_action, truncate};
use super::traits::{ActionExecutor, LlmClient, MemoryFacade, MemoryRetriever};
use super::types::{LlmMessage, LoopState};

/// Default LLM model (mirrors Python `DEFAULT_MODEL`).
pub const DEFAULT_MODEL: &str = "claude-opus-4-6";

// ---------------------------------------------------------------------------
// AgenticLoop
// ---------------------------------------------------------------------------

/// Main PERCEIVE→REASON→ACT→LEARN loop for goal-seeking agents.
///
/// # Type parameters
///
/// The loop is generic over its three dependencies so that tests can inject
/// simple mocks while production code can use real clients.
pub struct AgenticLoop<L, A, M>
where
    L: LlmClient,
    A: ActionExecutor,
    M: MemoryRetriever,
{
    pub agent_name: String,
    pub model: String,
    pub max_iterations: usize,
    pub iteration_count: usize,

    llm: L,
    executor: A,
    retriever: M,
    facade: Option<Box<dyn MemoryFacade>>,
}

impl<L, A, M> AgenticLoop<L, A, M>
where
    L: LlmClient,
    A: ActionExecutor,
    M: MemoryRetriever,
{
    /// Create a new loop.
    ///
    /// # Errors
    ///
    /// Returns [`AgentError::ConfigError`] if `agent_name` is empty.
    pub fn new(
        agent_name: impl Into<String>,
        llm: L,
        executor: A,
        retriever: M,
        model: Option<&str>,
        max_iterations: Option<usize>,
    ) -> Result<Self, AgentError> {
        let name: String = agent_name.into();
        let trimmed = name.trim().to_string();
        if trimmed.is_empty() {
            return Err(AgentError::ConfigError("agent_name cannot be empty".into()));
        }
        Ok(Self {
            agent_name: trimmed,
            model: model.unwrap_or(DEFAULT_MODEL).to_string(),
            max_iterations: max_iterations.unwrap_or(10),
            iteration_count: 0,
            llm,
            executor,
            retriever,
            facade: None,
        })
    }

    /// Attach an optional high-level memory facade.
    pub fn with_memory_facade(mut self, facade: Box<dyn MemoryFacade>) -> Self {
        self.facade = Some(facade);
        self
    }

    // ------------------------------------------------------------------
    // OBSERVE
    // ------------------------------------------------------------------

    /// OBSERVE phase: ingest an observation and recall immediate context.
    pub fn observe(&self, observation: &str) -> String {
        if let Some(ref facade) = self.facade {
            facade.remember(observation);
            let recalled = facade.recall(observation, 3);
            if recalled.is_empty() {
                String::new()
            } else {
                recalled.join("\n")
            }
        } else {
            String::new()
        }
    }

    // ------------------------------------------------------------------
    // ORIENT
    // ------------------------------------------------------------------

    /// ORIENT phase: build a world model from domain knowledge.
    pub fn orient(&self, query: &str) -> String {
        if let Some(ref facade) = self.facade {
            let recalled = facade.recall(query, 5);
            if recalled.is_empty() {
                String::new()
            } else {
                recalled.join("\n")
            }
        } else {
            String::new()
        }
    }

    // ------------------------------------------------------------------
    // PERCEIVE
    // ------------------------------------------------------------------

    /// PERCEIVE phase: combine observation, goal, and memory context.
    pub fn perceive(&self, observation: &str, goal: &str) -> String {
        let prior_context = self.observe(observation);
        let world_model = self.orient(observation);

        let mut perception = format!("Goal: {goal}\nObservation: {observation}\n");

        if !prior_context.is_empty() || !world_model.is_empty() {
            let combined: String = [&prior_context, &world_model]
                .iter()
                .filter(|s| !s.is_empty())
                .copied()
                .cloned()
                .collect::<Vec<_>>()
                .join("\n");
            if !combined.is_empty() {
                perception.push_str(&format!("\nPrior knowledge:\n{combined}\n"));
            }
        } else {
            // Fallback to lower-level retriever search.
            let memories = self.retriever.search(observation, 3);
            if !memories.is_empty() {
                perception.push_str("\nRelevant past experiences:\n");
                for (i, mem) in memories.iter().enumerate() {
                    perception
                        .push_str(&format!("{}. {} → {}\n", i + 1, mem.context, mem.outcome,));
                }
            }
        }

        perception
    }

    // ------------------------------------------------------------------
    // REASON  (async — calls LLM)
    // ------------------------------------------------------------------

    /// REASON phase: call the LLM to decide what action to take.
    pub async fn reason(&self, perception: &str) -> HashMap<String, Value> {
        let actions = self.executor.available_actions();
        let prompt = format!(
            "You are a goal-seeking agent. Based on the perception, decide what action to take.\n\n\
             {perception}\n\n\
             Available actions: {}\n\n\
             Think step by step:\n\
             1. What is the current situation?\n\
             2. What action would best help achieve the goal?\n\
             3. What parameters does that action need?\n\n\
             Respond in this JSON format:\n\
             {{\"reasoning\": \"Your reasoning here\", \"action\": \"action_name\", \"params\": {{\"param1\": \"value1\"}}}}",
            actions.join(", "),
        );

        let messages = vec![
            LlmMessage::system("You are a helpful goal-seeking agent."),
            LlmMessage::user(prompt),
        ];

        match self.llm.completion(&messages, &self.model, 0.7).await {
            Ok(response_text) => {
                if let Some(parsed) = parse_json_response(&response_text) {
                    return parsed;
                }
                // Fallback: unparseable response.
                error_action(
                    "Failed to parse LLM response",
                    "Invalid LLM response format",
                )
            }
            Err(e) => {
                error!("LLM reasoning call failed: {e}");
                error_action(
                    "LLM call failed due to an internal error",
                    "Internal reasoning error",
                )
            }
        }
    }

    // ------------------------------------------------------------------
    // ACT
    // ------------------------------------------------------------------

    /// ACT phase: execute the chosen action.
    pub fn act(&self, action_decision: &HashMap<String, Value>) -> Value {
        let action_name = action_decision
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or("");

        if action_name.is_empty() {
            return serde_json::json!({"error": "No action specified"});
        }

        let params: HashMap<String, Value> = action_decision
            .get("params")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        let result = self.executor.execute(action_name, &params);
        if result.success {
            result.output
        } else {
            serde_json::json!({"error": result.error.unwrap_or_default()})
        }
    }

    // ------------------------------------------------------------------
    // LEARN
    // ------------------------------------------------------------------

    /// LEARN phase: store experience in memory.
    pub fn learn(
        &self,
        perception: &str,
        reasoning: &str,
        action: &HashMap<String, Value>,
        outcome: &Value,
    ) -> String {
        let success = !outcome.as_object().is_some_and(|m| m.contains_key("error"));

        let action_name = action
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let params_str = action
            .get("params")
            .map(|v| v.to_string())
            .unwrap_or_else(|| "{}".to_string());

        let learning = format!("Action: {action_name} with {params_str}\nOutcome: {outcome}");
        let outcome_summary = truncate(&learning, 500);

        if let Some(ref facade) = self.facade {
            facade.remember(outcome_summary);
        } else {
            let ctx_string = format!("{perception}\nReasoning: {reasoning}");
            let context = truncate(&ctx_string, 500);
            let confidence = if success { 0.9 } else { 0.5 };
            self.retriever.store_fact(
                context,
                outcome_summary,
                confidence,
                &[action_name.to_string(), "agent_loop".to_string()],
            );
        }

        learning
    }

    // ------------------------------------------------------------------
    // run_iteration  (async)
    // ------------------------------------------------------------------

    /// Run one iteration of the PERCEIVE→REASON→ACT→LEARN loop.
    pub async fn run_iteration(&mut self, goal: &str, observation: &str) -> LoopState {
        self.iteration_count += 1;

        let perception = self.perceive(observation, goal);
        let action_decision = self.reason(&perception).await;
        let outcome = self.act(&action_decision);

        let reasoning_text = action_decision
            .get("reasoning")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        let learning = self.learn(&perception, &reasoning_text, &action_decision, &outcome);

        LoopState {
            perception,
            reasoning: reasoning_text,
            action: action_decision,
            learning,
            outcome,
            iteration: self.iteration_count,
        }
    }

    // ------------------------------------------------------------------
    // run_until_goal  (async)
    // ------------------------------------------------------------------

    /// Run the loop until a goal predicate succeeds or `max_iterations` is hit.
    pub async fn run_until_goal<F>(
        &mut self,
        goal: &str,
        initial_observation: &str,
        is_goal_achieved: Option<F>,
    ) -> Vec<LoopState>
    where
        F: Fn(&LoopState) -> bool,
    {
        let mut states = Vec::new();
        let mut observation = initial_observation.to_string();

        for _ in 0..self.max_iterations {
            let state = self.run_iteration(goal, &observation).await;

            if let Some(ref check) = is_goal_achieved
                && check(&state)
            {
                states.push(state);
                break;
            }

            observation = format!("Previous action result: {}", state.outcome);
            states.push(state);
        }

        states
    }

    /// Read-only access to the LLM client (used by the reasoning sub-module).
    pub(crate) fn llm(&self) -> &L {
        &self.llm
    }

    /// Read-only access to the memory facade (used by the reasoning sub-module).
    pub(crate) fn facade(&self) -> Option<&dyn MemoryFacade> {
        self.facade.as_deref()
    }

    /// Read-only access to the retriever (used by the reasoning sub-module).
    pub(crate) fn retriever(&self) -> &M {
        &self.retriever
    }
}
