use crate::error::Result;
use crate::models::{
    BundleStatus, ExecutionPlan, GoalAgentBundle, GoalDefinition, SkillDefinition,
};

/// Combines a goal, plan, and skills into a deployable [`GoalAgentBundle`].
pub struct AgentAssembler;

impl AgentAssembler {
    pub fn new() -> Self {
        Self
    }

    /// Assemble a complete agent bundle from the constituent parts.
    pub fn assemble(
        &self,
        goal: &GoalDefinition,
        plan: &ExecutionPlan,
        skills: Vec<SkillDefinition>,
    ) -> Result<GoalAgentBundle> {
        let bundle_name = format!("{}-agent", goal.domain);
        let mut bundle = GoalAgentBundle::new(bundle_name, "0.1.0")?;
        bundle.goal_definition = Some(goal.clone());
        bundle.execution_plan = Some(plan.clone());
        bundle.skills = skills;
        bundle.status = BundleStatus::Ready;
        Ok(bundle)
    }
}

impl Default for AgentAssembler {
    fn default() -> Self {
        Self::new()
    }
}
