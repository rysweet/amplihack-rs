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
/// Port of Python `GoalSeekingAgent`.
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
    fn observe(&mut self, input: &str) -> Result<()> {
        self.current_input = input.to_string();
        self.context.clear();
        self.action_plan.clear();
        self.state = AgentState::Observing;
        Ok(())
    }

    fn orient(&mut self) -> Result<Vec<String>> {
        self.state = AgentState::Orienting;
        self.context = vec![self.current_input.clone()];
        Ok(self.context.clone())
    }

    fn decide(&mut self) -> Result<String> {
        self.state = AgentState::Deciding;
        let lower = self.current_input.trim().to_lowercase();

        let question_words = [
            "what", "who", "where", "when", "why", "how", "which", "is", "are", "do", "does",
            "can", "could", "would", "should",
        ];
        let command_words = [
            "run", "execute", "create", "delete", "build", "test", "deploy", "install", "fix",
            "update", "start", "stop",
        ];

        if lower.ends_with('?') {
            self.action_plan = "answer".to_string();
        } else {
            let first_word = lower.split_whitespace().next().unwrap_or("");
            if question_words.contains(&first_word) {
                self.action_plan = "answer".to_string();
            } else if command_words.contains(&first_word) {
                self.action_plan = "execute".to_string();
            } else {
                self.action_plan = "store".to_string();
            }
        }

        Ok(self.action_plan.clone())
    }

    fn act(&mut self) -> Result<TaskResult> {
        self.state = AgentState::Acting;
        let output = match self.action_plan.as_str() {
            "answer" => format!("Answer: {}", self.current_input),
            "execute" => format!("Executed: {}", self.current_input),
            _ => format!("Stored: {}", self.current_input),
        };
        let result = TaskResult::ok(output, 0.0);
        self.state = AgentState::Idle;
        self.iteration += 1;
        Ok(result)
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
        self.current_input.clear();
        self.context.clear();
        self.action_plan.clear();
        self.state = AgentState::Idle;
        self.iteration = 0;
        Ok(())
    }
}
