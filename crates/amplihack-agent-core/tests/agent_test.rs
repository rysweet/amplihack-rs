use amplihack_agent_core::{
    Agent, AgentConfig, AgentState, GoalSeekingAgent,
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
fn observe_stores_input() {
    let mut agent = make_agent("obs");
    agent.observe("hello world").unwrap();
    assert_eq!(agent.state(), AgentState::Observing);
}

#[test]
fn observe_empty_input() {
    let mut agent = make_agent("obs-empty");
    agent.observe("").unwrap();
    assert_eq!(agent.state(), AgentState::Observing);
}

#[test]
fn observe_transitions_to_observing() {
    let mut agent = make_agent("obs-state");
    agent.observe("input").unwrap();
    assert_eq!(agent.state(), AgentState::Observing);
}

#[test]
fn observe_unicode_input() {
    let mut agent = make_agent("obs-unicode");
    agent.observe("日本語テスト 🦀").unwrap();
    assert_eq!(agent.state(), AgentState::Observing);
}

// ---------------------------------------------------------------------------
// OODA — orient
// ---------------------------------------------------------------------------

#[test]
fn orient_returns_context_with_input() {
    let mut agent = make_agent("orient");
    agent.observe("test").unwrap();
    let ctx = agent.orient().unwrap();
    assert_eq!(ctx, vec!["test".to_string()]);
    assert_eq!(agent.state(), AgentState::Orienting);
}

#[test]
fn orient_returns_context_strings() {
    let mut agent = make_agent("orient-ctx");
    agent.observe("what is rust?").unwrap();
    let ctx = agent.orient().unwrap();
    assert!(!ctx.is_empty());
    assert_eq!(ctx[0], "what is rust?");
}

// ---------------------------------------------------------------------------
// OODA — decide
// ---------------------------------------------------------------------------

#[test]
fn decide_classifies_question_as_answer() {
    let mut agent = make_agent("decide");
    agent.observe("how do I test?").unwrap();
    agent.orient().unwrap();
    let plan = agent.decide().unwrap();
    assert_eq!(plan, "answer");
    assert_eq!(agent.state(), AgentState::Deciding);
}

#[test]
fn decide_classifies_command_as_execute() {
    let mut agent = make_agent("decide-plan");
    agent.observe("run cargo test").unwrap();
    agent.orient().unwrap();
    let plan = agent.decide().unwrap();
    assert_eq!(plan, "execute");
}

// ---------------------------------------------------------------------------
// OODA — act
// ---------------------------------------------------------------------------

#[test]
fn act_produces_task_result() {
    let mut agent = make_agent("act");
    agent.observe("do something").unwrap();
    agent.orient().unwrap();
    agent.decide().unwrap();
    let result = agent.act().unwrap();
    assert!(result.success);
    assert!(result.output.contains("do something"));
}

#[test]
fn act_returns_to_idle() {
    let mut agent = make_agent("act-idle");
    agent.observe("task").unwrap();
    agent.orient().unwrap();
    agent.decide().unwrap();
    agent.act().unwrap();
    assert_eq!(agent.state(), AgentState::Idle);
}

// ---------------------------------------------------------------------------
// OODA — process (full cycle)
// ---------------------------------------------------------------------------

#[test]
fn process_chains_ooda_steps() {
    let mut agent = make_agent("process");
    let result = agent.process("hello").unwrap();
    assert!(result.success);
}

#[test]
fn process_returns_to_idle_after_success() {
    let mut agent = make_agent("proc-idle");
    agent.process("task").unwrap();
    assert_eq!(agent.state(), AgentState::Idle);
}

#[test]
fn process_with_question_input() {
    let mut agent = make_agent("proc-q");
    let result = agent.process("what is TDD?").unwrap();
    assert!(result.success);
    assert!(result.output.starts_with("Answer: "));
}

#[test]
fn process_with_command_input() {
    let mut agent = make_agent("proc-cmd");
    let result = agent.process("run the tests").unwrap();
    assert!(result.success);
    assert!(result.output.starts_with("Executed: "));
}

#[test]
fn process_with_content_input() {
    let mut agent = make_agent("proc-content");
    let result = agent.process("Rust was created by Mozilla.").unwrap();
    assert!(result.success);
    assert!(result.output.starts_with("Stored: "));
}

#[test]
fn process_empty_input() {
    let mut agent = make_agent("proc-empty");
    let result = agent.process("").unwrap();
    assert!(result.success);
    assert_eq!(agent.state(), AgentState::Idle);
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
fn reset_returns_to_idle() {
    let mut agent = make_agent("reset");
    agent.process("something").unwrap();
    agent.reset().unwrap();
    assert_eq!(agent.state(), AgentState::Idle);
    assert_eq!(agent.info().iterations, 0);
}

// ---------------------------------------------------------------------------
// Concurrent / edge cases
// ---------------------------------------------------------------------------

#[test]
fn multiple_process_calls() {
    let mut agent = make_agent("multi");
    let r1 = agent.process("first").unwrap();
    assert!(r1.success);
    assert_eq!(agent.info().iterations, 1);
    let r2 = agent.process("second").unwrap();
    assert!(r2.success);
    assert_eq!(agent.info().iterations, 2);
}

#[test]
fn long_input_handled() {
    let mut agent = make_agent("long");
    let long_input = "x".repeat(10_000);
    let result = agent.process(&long_input).unwrap();
    assert!(result.success);
    assert!(result.output.contains(&long_input));
}
