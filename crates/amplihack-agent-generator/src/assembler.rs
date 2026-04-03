use crate::error::Result;
use crate::models::{ExecutionPlan, GoalAgentBundle, GoalDefinition, SkillDefinition};

/// Combines a goal, plan, and skills into a deployable [`GoalAgentBundle`].
pub struct AgentAssembler;

impl AgentAssembler {
    pub fn new() -> Self {
        Self
    }

    /// Assemble a complete agent bundle from the constituent parts.
    pub fn assemble(
        &self,
        _goal: &GoalDefinition,
        _plan: &ExecutionPlan,
        _skills: Vec<SkillDefinition>,
    ) -> Result<GoalAgentBundle> {
        todo!("AgentAssembler::assemble not yet implemented")
    }
}

impl Default for AgentAssembler {
    fn default() -> Self {
        Self::new()
    }
}
