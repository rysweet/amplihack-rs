use amplihack_agent_core::{
    Agent, AgentConfig, AgentInfo, AgentState, GoalSeekingAgent, TaskResult,
};
use amplihack_memory::MemoryType;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_agent(name: &str) -> GoalSeekingAgent {
    let cfg = AgentConfig::new(name, "test-model");
    GoalSeekingAgent::new(cfg)
}

fn make_configured_agent() -> GoalSeekingAgent {
    let cfg = AgentConfig::new("configured", "gpt-4")
        .with_storage_path("/data/agents")
        .with_memory_type(MemoryType::Semantic)
        .with_timeout(60);
    GoalSeekingAgent::new(cfg)
}

// ---------------------------------------------------------------------------
// State basics
// ---------------------------------------------------------------------------

#[test]
fn agent_starts_idle() {
    let agent = make_agent("a1");
    assert_eq!(agent.state(), AgentState::Idle);
}

#[test]
fn agent_info_reflects_config() {
    let agent = make_configured_agent();
    let info = agent.info();
    assert_eq!(info.agent_name, "configured");
    assert_eq!(info.model, "gpt-4");
    assert_eq!(info.state, AgentState::Idle);
    assert_eq!(info.iterations, 0);
}

#[test]
fn agent_config_accessible() {
    let agent = make_configured_agent();
    let cfg = agent.config();
    assert_eq!(cfg.agent_name, "configured");
    assert_eq!(cfg.model, "gpt-4");
    assert_eq!(cfg.memory_type, MemoryType::Semantic);
    assert_eq!(cfg.timeout_secs, 60);
}

// ---------------------------------------------------------------------------
// OODA — observe
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "not yet implemented")]
fn observe_stores_input() {
    let mut agent = make_agent("obs");
    agent.observe("hello world").unwrap();
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn observe_empty_input() {
    let mut agent = make_agent("obs-empty");
    agent.observe("").unwrap();
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn observe_transitions_to_observing() {
    let mut agent = make_agent("obs-state");
    agent.observe("input").unwrap();
    // After implementation, this should assert:
    // assert_eq!(agent.state(), AgentState::Observing);
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn observe_unicode_input() {
    let mut agent = make_agent("obs-unicode");
    agent.observe("日本語テスト 🦀").unwrap();
}

// ---------------------------------------------------------------------------
// OODA — orient
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "not yet implemented")]
fn orient_recalls_memory() {
    let mut agent = make_agent("orient");
    agent.observe("test").unwrap();
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn orient_returns_context_strings() {
    let mut agent = make_agent("orient-ctx");
    agent.observe("what is rust?").unwrap();
}

// ---------------------------------------------------------------------------
// OODA — decide
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "not yet implemented")]
fn decide_classifies_intent() {
    let mut agent = make_agent("decide");
    agent.observe("how do I test?").unwrap();
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn decide_produces_action_plan() {
    let mut agent = make_agent("decide-plan");
    agent.observe("run cargo test").unwrap();
}

// ---------------------------------------------------------------------------
// OODA — act
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "not yet implemented")]
fn act_produces_task_result() {
    let mut agent = make_agent("act");
    agent.observe("do something").unwrap();
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn act_returns_to_idle() {
    let mut agent = make_agent("act-idle");
    agent.observe("task").unwrap();
}

// ---------------------------------------------------------------------------
// OODA — process (full cycle)
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "not yet implemented")]
fn process_chains_ooda_steps() {
    let mut agent = make_agent("process");
    let result = agent.process("hello").unwrap();
    assert!(result.success);
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn process_returns_to_idle_after_success() {
    let mut agent = make_agent("proc-idle");
    agent.process("task").unwrap();
    assert_eq!(agent.state(), AgentState::Idle);
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn process_with_question_input() {
    let mut agent = make_agent("proc-q");
    let result = agent.process("what is TDD?").unwrap();
    assert!(result.success);
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn process_with_command_input() {
    let mut agent = make_agent("proc-cmd");
    let result = agent.process("run the tests").unwrap();
    assert!(result.success);
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn process_with_content_input() {
    let mut agent = make_agent("proc-content");
    let result = agent.process("Rust was created by Mozilla.").unwrap();
    assert!(result.success);
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn process_empty_input() {
    let mut agent = make_agent("proc-empty");
    agent.process("").unwrap();
}

// ---------------------------------------------------------------------------
// State transitions
// ---------------------------------------------------------------------------

#[test]
fn idle_is_terminal() {
    assert!(AgentState::Idle.is_terminal());
}

#[test]
fn error_is_terminal() {
    assert!(AgentState::Error.is_terminal());
}

#[test]
fn ooda_states_not_terminal() {
    assert!(!AgentState::Observing.is_terminal());
    assert!(!AgentState::Orienting.is_terminal());
    assert!(!AgentState::Deciding.is_terminal());
    assert!(!AgentState::Acting.is_terminal());
}

#[test]
fn state_next_follows_ooda() {
    let mut state = AgentState::Idle;
    let expected = [
        AgentState::Observing,
        AgentState::Orienting,
        AgentState::Deciding,
        AgentState::Acting,
        AgentState::Idle,
    ];
    for exp in &expected {
        state = state.next().unwrap();
        assert_eq!(state, *exp);
    }
}

#[test]
fn error_state_has_no_next() {
    assert_eq!(AgentState::Error.next(), None);
}

// ---------------------------------------------------------------------------
// Reset
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "not yet implemented")]
fn reset_returns_to_idle() {
    let mut agent = make_agent("reset");
    agent.reset().unwrap();
    assert_eq!(agent.state(), AgentState::Idle);
}

// ---------------------------------------------------------------------------
// Concurrent / edge cases
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "not yet implemented")]
fn multiple_process_calls() {
    let mut agent = make_agent("multi");
    agent.process("first").unwrap();
    agent.process("second").unwrap();
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn long_input_handled() {
    let mut agent = make_agent("long");
    let long_input = "x".repeat(10_000);
    agent.process(&long_input).unwrap();
}
