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
    use crate::models::PlanPhase;
    use std::path::PathBuf;
    use uuid::Uuid;

    fn sample_goal() -> GoalDefinition {
        GoalDefinition::new("prompt", "goal", "security").unwrap()
    }

    fn sample_plan() -> ExecutionPlan {
        let phase = PlanPhase::new("analysis", "desc", vec!["cap".into()]).unwrap();
        ExecutionPlan::new(Uuid::new_v4(), vec![phase]).unwrap()
    }

    fn sample_skills() -> Vec<SkillDefinition> {
        vec![SkillDefinition::new("sk", PathBuf::from("p"), "content").unwrap()]
    }

    #[test]
    fn assemble_produces_ready_bundle() {
        let asm = AgentAssembler::new();
        let bundle = asm.assemble(&sample_goal(), &sample_plan(), sample_skills()).unwrap();
        assert_eq!(bundle.status, BundleStatus::Ready);
        assert!(bundle.goal_definition.is_some());
        assert!(bundle.execution_plan.is_some());
        assert!(!bundle.skills.is_empty());
        assert!(bundle.is_complete());
    }

    #[test]
    fn assemble_names_bundle_after_domain() {
        let asm = AgentAssembler::new();
        let bundle = asm.assemble(&sample_goal(), &sample_plan(), sample_skills()).unwrap();
        assert_eq!(bundle.name, "security-agent");
    }
}
