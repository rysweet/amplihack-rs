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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planner::ObjectivePlanner;
    use crate::synthesizer::SkillSynthesizer;

    fn make_parts() -> (GoalDefinition, ExecutionPlan, Vec<SkillDefinition>) {
        let goal = GoalDefinition::new("prompt", "build tool", "development").unwrap();
        let plan = ObjectivePlanner::new().plan(&goal).unwrap();
        let skills = SkillSynthesizer::new().synthesize(&plan).unwrap();
        (goal, plan, skills)
    }

    #[test]
    fn assemble_produces_ready_bundle() {
        let (goal, plan, skills) = make_parts();
        let bundle = AgentAssembler::new().assemble(&goal, &plan, skills).unwrap();
        assert_eq!(bundle.status, BundleStatus::Ready);
    }

    #[test]
    fn assemble_sets_domain_agent_name() {
        let (goal, plan, skills) = make_parts();
        let bundle = AgentAssembler::new().assemble(&goal, &plan, skills).unwrap();
        assert_eq!(bundle.name, "development-agent");
    }

    #[test]
    fn assembled_bundle_is_complete() {
        let (goal, plan, skills) = make_parts();
        let bundle = AgentAssembler::new().assemble(&goal, &plan, skills).unwrap();
        assert!(bundle.is_complete());
    }

    #[test]
    fn assemble_preserves_goal_and_plan() {
        let (goal, plan, skills) = make_parts();
        let skill_count = skills.len();
        let bundle = AgentAssembler::new().assemble(&goal, &plan, skills).unwrap();
        assert!(bundle.goal_definition.is_some());
        assert!(bundle.execution_plan.is_some());
        assert_eq!(bundle.skills.len(), skill_count);
    }

    #[test]
    fn default_impl() {
        let a = AgentAssembler::default();
        let (goal, plan, skills) = make_parts();
        assert!(a.assemble(&goal, &plan, skills).is_ok());
    }
}
