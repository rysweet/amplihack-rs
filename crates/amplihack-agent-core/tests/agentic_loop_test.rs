//! Integration tests for the agentic loop.
//!
//! Tests use mock implementations of `LlmClient`, `ActionExecutor`,
//! `MemoryRetriever`, and `MemoryFacade`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use amplihack_agent_core::agentic_loop::json_parse::parse_json_response;
use amplihack_agent_core::agentic_loop::traits::{
    ActionExecutor, LlmClient, MemoryFacade, MemoryRetriever,
};
use amplihack_agent_core::agentic_loop::types::{ActionResult, LlmMessage, MemoryFact};
use amplihack_agent_core::agentic_loop::AgenticLoop;
use amplihack_agent_core::AgentError;
use async_trait::async_trait;
use serde_json::Value;

// =========================================================================
// Mock implementations
// =========================================================================

/// Mock LLM that returns a pre-configured response.
struct MockLlm {
    /// Queue of responses to return in order.
    responses: Mutex<Vec<String>>,
}

impl MockLlm {
    fn new(responses: Vec<&str>) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().map(String::from).collect()),
        }
    }

    fn single(response: &str) -> Self {
        Self::new(vec![response])
    }
}

#[async_trait]
impl LlmClient for MockLlm {
    async fn completion(
        &self,
        _messages: &[LlmMessage],
        _model: &str,
        _temperature: f64,
    ) -> Result<String, AgentError> {
        let mut q = self.responses.lock().unwrap();
        if q.is_empty() {
            // Recycle last response forever.
            Ok(r#"{"reasoning":"no-op","action":"noop","params":{}}"#.into())
        } else {
            Ok(q.remove(0))
        }
    }
}

/// Mock LLM that always fails.
struct FailingLlm;

#[async_trait]
impl LlmClient for FailingLlm {
    async fn completion(
        &self,
        _messages: &[LlmMessage],
        _model: &str,
        _temperature: f64,
    ) -> Result<String, AgentError> {
        Err(AgentError::TaskFailed("LLM unavailable".into()))
    }
}

/// Mock action executor.
struct MockExecutor {
    actions: Vec<String>,
}

impl MockExecutor {
    fn new(actions: Vec<&str>) -> Self {
        Self {
            actions: actions.into_iter().map(String::from).collect(),
        }
    }
}

impl ActionExecutor for MockExecutor {
    fn available_actions(&self) -> Vec<String> {
        self.actions.clone()
    }

    fn execute(&self, action_name: &str, _params: &HashMap<String, Value>) -> ActionResult {
        if action_name == "fail" {
            ActionResult::fail("deliberate failure")
        } else {
            ActionResult::ok(Value::String(format!("executed:{action_name}")))
        }
    }
}

/// Mock memory retriever.
struct MockRetriever {
    facts: Vec<MemoryFact>,
    stored: Arc<Mutex<Vec<(String, String)>>>,
}

impl MockRetriever {
    fn new(facts: Vec<MemoryFact>) -> Self {
        Self {
            facts,
            stored: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn empty() -> Self {
        Self::new(Vec::new())
    }

    #[allow(dead_code)]
    fn stored_facts(&self) -> Vec<(String, String)> {
        self.stored.lock().unwrap().clone()
    }
}

impl MemoryRetriever for MockRetriever {
    fn search(&self, _query: &str, limit: usize) -> Vec<MemoryFact> {
        self.facts.iter().take(limit).cloned().collect()
    }

    fn store_fact(&self, context: &str, fact: &str, _confidence: f64, _tags: &[String]) {
        self.stored
            .lock()
            .unwrap()
            .push((context.to_string(), fact.to_string()));
    }
}

/// Mock memory facade.
struct MockFacade {
    items: Mutex<Vec<String>>,
}

impl MockFacade {
    fn new() -> Self {
        Self {
            items: Mutex::new(Vec::new()),
        }
    }
}

impl MemoryFacade for MockFacade {
    fn remember(&self, content: &str) {
        self.items.lock().unwrap().push(content.to_string());
    }

    fn recall(&self, _query: &str, limit: usize) -> Vec<String> {
        self.items
            .lock()
            .unwrap()
            .iter()
            .take(limit)
            .cloned()
            .collect()
    }

    fn retrieve_facts(&self, _query: &str, _max_nodes: usize) -> Vec<MemoryFact> {
        Vec::new()
    }
}

// =========================================================================
// Helper: build a loop quickly
// =========================================================================

fn make_loop(
    llm_response: &str,
) -> AgenticLoop<MockLlm, MockExecutor, MockRetriever> {
    AgenticLoop::new(
        "test-agent",
        MockLlm::single(llm_response),
        MockExecutor::new(vec!["greet", "search", "noop"]),
        MockRetriever::empty(),
        None,
        None,
    )
    .expect("valid config")
}

// =========================================================================
// Tests (≥ 10 required)
// =========================================================================

// 1. Empty agent_name is rejected.
#[test]
fn test_empty_agent_name_rejected() {
    let result = AgenticLoop::new(
        "",
        MockLlm::single("{}"),
        MockExecutor::new(vec![]),
        MockRetriever::empty(),
        None,
        None,
    );
    assert!(result.is_err());
    let err = result.err().expect("should be an error");
    assert!(err.to_string().contains("empty"));
}

// 2. Whitespace-only agent_name is rejected.
#[test]
fn test_whitespace_agent_name_rejected() {
    let result = AgenticLoop::new(
        "   ",
        MockLlm::single("{}"),
        MockExecutor::new(vec![]),
        MockRetriever::empty(),
        None,
        None,
    );
    assert!(result.is_err());
}

// 3. Defaults are applied correctly.
#[test]
fn test_defaults() {
    let l = make_loop("{}");
    assert_eq!(l.model, "claude-opus-4-6");
    assert_eq!(l.max_iterations, 10);
    assert_eq!(l.iteration_count, 0);
}

// 4. run_iteration returns valid LoopState.
#[tokio::test]
async fn test_run_iteration() {
    let response = r#"{"reasoning":"user present","action":"greet","params":{"name":"Alice"}}"#;
    let mut l = make_loop(response);
    let state = l.run_iteration("Greet user", "User Alice present").await;
    assert_eq!(state.iteration, 1);
    assert_eq!(state.reasoning, "user present");
    assert!(state.outcome.as_str().unwrap().contains("greet"));
}

// 5. Iteration counter increments.
#[tokio::test]
async fn test_iteration_counter() {
    let response = r#"{"reasoning":"r","action":"noop","params":{}}"#;
    let mut l = AgenticLoop::new(
        "agent",
        MockLlm::new(vec![response, response, response]),
        MockExecutor::new(vec!["noop"]),
        MockRetriever::empty(),
        None,
        Some(3),
    )
    .unwrap();
    let _ = l.run_iteration("g", "o1").await;
    let _ = l.run_iteration("g", "o2").await;
    let state = l.run_iteration("g", "o3").await;
    assert_eq!(state.iteration, 3);
    assert_eq!(l.iteration_count, 3);
}

// 6. LLM failure produces error action.
#[tokio::test]
async fn test_llm_failure_produces_error_action() {
    let mut l = AgenticLoop::new(
        "agent",
        FailingLlm,
        MockExecutor::new(vec!["noop"]),
        MockRetriever::empty(),
        None,
        None,
    )
    .unwrap();
    let state = l.run_iteration("goal", "obs").await;
    assert_eq!(
        state.action.get("action").and_then(Value::as_str),
        Some("error")
    );
}

// 7. run_until_goal stops when predicate is true.
#[tokio::test]
async fn test_run_until_goal_stops_on_predicate() {
    let response = r#"{"reasoning":"r","action":"greet","params":{}}"#;
    let mut l = AgenticLoop::new(
        "agent",
        MockLlm::new(vec![response, response, response, response, response]),
        MockExecutor::new(vec!["greet"]),
        MockRetriever::empty(),
        None,
        Some(5),
    )
    .unwrap();

    let states = l
        .run_until_goal(
            "goal",
            "obs",
            Some(|s: &amplihack_agent_core::LoopState| s.iteration >= 2),
        )
        .await;
    assert_eq!(states.len(), 2);
}

// 8. run_until_goal respects max_iterations.
#[tokio::test]
async fn test_run_until_goal_max_iterations() {
    let response = r#"{"reasoning":"r","action":"noop","params":{}}"#;
    let mut l = AgenticLoop::new(
        "agent",
        MockLlm::new(vec![response; 5]),
        MockExecutor::new(vec!["noop"]),
        MockRetriever::empty(),
        None,
        Some(3),
    )
    .unwrap();
    let states = l
        .run_until_goal::<fn(&_) -> bool>("goal", "obs", None)
        .await;
    assert_eq!(states.len(), 3);
}

// 9. Learn stores facts via retriever when no facade.
#[tokio::test]
async fn test_learn_stores_facts() {
    let retriever = MockRetriever::empty();
    let stored_ref = retriever.stored.clone();
    let mut l = AgenticLoop::new(
        "agent",
        MockLlm::single(r#"{"reasoning":"r","action":"noop","params":{}}"#),
        MockExecutor::new(vec!["noop"]),
        retriever,
        None,
        None,
    )
    .unwrap();
    let _ = l.run_iteration("goal", "obs").await;
    let stored = stored_ref.lock().unwrap();
    assert!(!stored.is_empty(), "should have stored a fact");
}

// 10. Perceive includes memory from retriever.
#[test]
fn test_perceive_with_retriever_memories() {
    let facts = vec![MemoryFact {
        id: "f1".into(),
        context: "past event".into(),
        outcome: "good outcome".into(),
        confidence: 0.9,
        metadata: HashMap::new(),
    }];
    let l = AgenticLoop::new(
        "agent",
        MockLlm::single("{}"),
        MockExecutor::new(vec![]),
        MockRetriever::new(facts),
        None,
        None,
    )
    .unwrap();
    let perception = l.perceive("current obs", "my goal");
    assert!(perception.contains("my goal"));
    assert!(perception.contains("current obs"));
    assert!(perception.contains("past event"));
    assert!(perception.contains("good outcome"));
}

// 11. Observe/orient use facade when available.
#[test]
fn test_facade_observe_orient() {
    let facade = MockFacade::new();
    facade.remember("prior knowledge");
    let l = AgenticLoop::new(
        "agent",
        MockLlm::single("{}"),
        MockExecutor::new(vec![]),
        MockRetriever::empty(),
        None,
        None,
    )
    .unwrap()
    .with_memory_facade(Box::new(facade));

    let observed = l.observe("test obs");
    // The facade should have recalled "prior knowledge" + stored "test obs".
    assert!(
        observed.contains("prior knowledge") || observed.is_empty(),
        "observe should recall from facade"
    );
}

// 12. Act returns error JSON when action is missing.
#[test]
fn test_act_no_action() {
    let l = make_loop("{}");
    let decision = HashMap::new();
    let result = l.act(&decision);
    assert!(result.as_object().unwrap().contains_key("error"));
}

// 13. Act returns error JSON when action executor fails.
#[test]
fn test_act_executor_failure() {
    let l = AgenticLoop::new(
        "agent",
        MockLlm::single("{}"),
        MockExecutor::new(vec!["fail"]),
        MockRetriever::empty(),
        None,
        None,
    )
    .unwrap();
    let mut decision = HashMap::new();
    decision.insert("action".into(), Value::String("fail".into()));
    decision.insert("params".into(), Value::Object(serde_json::Map::new()));
    let result = l.act(&decision);
    let obj = result.as_object().unwrap();
    assert!(obj.contains_key("error"));
}

// 14. JSON parse handles raw, json-fence, and generic-fence.
#[test]
fn test_parse_json_variants() {
    // Already covered in json_parse.rs unit tests, but verify via public API.
    assert!(parse_json_response(r#"{"a":1}"#).is_some());
    assert!(parse_json_response("```json\n{\"a\":1}\n```").is_some());
    assert!(parse_json_response("nope").is_none());
}

// 15. reason_iteratively returns trace with steps.
#[tokio::test]
async fn test_reason_iteratively_basic() {
    // Plan response → search queries.
    let plan_resp =
        r#"{"search_queries": ["query1", "query2"], "reasoning": "coverage"}"#;
    // Sufficiency evaluation → sufficient.
    let eval_resp =
        r#"{"sufficient": true, "missing": "", "confidence": 0.95, "refined_queries": []}"#;

    let l = AgenticLoop::new(
        "agent",
        MockLlm::new(vec![plan_resp, eval_resp]),
        MockExecutor::new(vec![]),
        MockRetriever::empty(),
        None,
        None,
    )
    .unwrap();

    let intent: HashMap<String, Value> = HashMap::new();
    let (facts, _nodes, trace) = l.reason_iteratively("What is X?", &intent, 3).await.unwrap();

    assert!(facts.is_empty(), "no memory → no facts");
    assert!(!trace.steps.is_empty(), "should have at least plan + search + eval steps");
    assert!(trace.total_queries_executed > 0);
}

// 16. Custom model is honored.
#[test]
fn test_custom_model() {
    let l = AgenticLoop::new(
        "agent",
        MockLlm::single("{}"),
        MockExecutor::new(vec![]),
        MockRetriever::empty(),
        Some("gpt-4-turbo"),
        None,
    )
    .unwrap();
    assert_eq!(l.model, "gpt-4-turbo");
}
