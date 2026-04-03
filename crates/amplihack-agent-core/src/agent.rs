//! Agent trait — OODA loop abstraction.
//!
//! Matches Python `amplihack/agents/goal_seeking/goal_seeking_agent.py`:
//! - observe() → gather input
//! - orient()  → recall memory, build context
//! - decide()  → classify intent, choose action
//! - act()     → execute action, produce output
//! - process() → full OODA cycle

use crate::error::Result;
use crate::models::{AgentConfig, AgentInfo, AgentState, TaskResult};

// ---------------------------------------------------------------------------
// Agent trait
// ---------------------------------------------------------------------------

/// Core trait for goal-seeking agents implementing the OODA loop.
///
/// All methods are synchronous. Implementations should manage their own
/// internal state transitions through the OODA cycle:
/// Idle → Observing → Orienting → Deciding → Acting → Idle.
pub trait Agent {
    /// Observe: gather and store raw input.
    ///
    /// Transitions state from Idle to Observing.
    fn observe(&mut self, input: &str) -> Result<()>;

    /// Orient: recall relevant memory and build context.
    ///
    /// Transitions state from Observing to Orienting.
    fn orient(&mut self) -> Result<Vec<String>>;

    /// Decide: classify intent and choose an action plan.
    ///
    /// Transitions state from Orienting to Deciding.
    fn decide(&mut self) -> Result<String>;

    /// Act: execute the chosen action and produce output.
    ///
    /// Transitions state from Deciding to Acting, then back to Idle.
    fn act(&mut self) -> Result<TaskResult>;

    /// Run a full OODA cycle on the given input.
    ///
    /// Default implementation chains observe → orient → decide → act.
    fn process(&mut self, input: &str) -> Result<TaskResult> {
        self.observe(input)?;
        self.orient()?;
        self.decide()?;
        self.act()
    }

    /// Return the agent's current state.
    fn state(&self) -> AgentState;

    /// Return the agent's configuration.
    fn config(&self) -> &AgentConfig;

    /// Return a snapshot of the agent's current status.
    fn info(&self) -> AgentInfo;

    /// Reset the agent to Idle state.
    fn reset(&mut self) -> Result<()>;
}

// ---------------------------------------------------------------------------
// GoalSeekingAgent — stub implementation
// ---------------------------------------------------------------------------

/// Default OODA-loop agent backed by memory.
///
/// Port of Python `GoalSeekingAgent`. All method bodies are `todo!()`
/// stubs that will be filled in after tests are written.
#[allow(dead_code)] // Fields used once todo!() stubs are implemented
pub struct GoalSeekingAgent {
    config: AgentConfig,
    state: AgentState,
    current_input: String,
    context: Vec<String>,
    action_plan: String,
    iteration: usize,
}

impl GoalSeekingAgent {
    pub fn new(config: AgentConfig) -> Self {
        Self {
            config,
            state: AgentState::Idle,
            current_input: String::new(),
            context: Vec::new(),
            action_plan: String::new(),
            iteration: 0,
        }
    }
}

impl Agent for GoalSeekingAgent {
    fn observe(&mut self, _input: &str) -> Result<()> {
        todo!("observe: store input and transition to Observing")
    }

    fn orient(&mut self) -> Result<Vec<String>> {
        todo!("orient: recall memory and build context")
    }

    fn decide(&mut self) -> Result<String> {
        todo!("decide: classify intent and choose action")
    }

    fn act(&mut self) -> Result<TaskResult> {
        todo!("act: execute action and produce output")
    }

    fn state(&self) -> AgentState {
        self.state
    }

    fn config(&self) -> &AgentConfig {
        &self.config
    }

    fn info(&self) -> AgentInfo {
        AgentInfo {
            agent_id: format!("agent-{}", self.config.agent_name),
            agent_name: self.config.agent_name.clone(),
            state: self.state,
            model: self.config.model.clone(),
            iterations: self.iteration,
            uptime_secs: 0.0,
        }
    }

    fn reset(&mut self) -> Result<()> {
        todo!("reset: clear state and return to Idle")
    }
}
