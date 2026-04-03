use uuid::Uuid;

use crate::error::Result;
use crate::models::{Complexity, ExecutionPlan, GoalDefinition, PlanPhase};

/// Converts a [`GoalDefinition`] into a phased [`ExecutionPlan`].
pub struct ObjectivePlanner;

impl ObjectivePlanner {
    pub fn new() -> Self {
        Self
    }

    /// Produce an execution plan with ordered phases, dependency graph, and
    /// parallel-opportunity annotations.
    pub fn plan(&self, goal: &GoalDefinition) -> Result<ExecutionPlan> {
        let goal_id = Uuid::new_v4();
        let add_optimization = goal.complexity >= Complexity::Moderate;

        let mut phases = Vec::new();

        let mut analysis = PlanPhase::new(
            "analysis",
            format!("Analyze the goal: {}", goal.goal),
            vec!["analysis".into()],
        )?;
        analysis.estimated_duration = if add_optimization {
            "10m".into()
        } else {
            "5m".into()
        };
        phases.push(analysis);

        let mut implementation = PlanPhase::new(
            "implementation",
            format!("Implement the solution for: {}", goal.goal),
            vec!["implementation".into()],
        )?;
        implementation.estimated_duration = if add_optimization {
            "20m".into()
        } else {
            "10m".into()
        };
        implementation.dependencies = vec!["analysis".into()];
        implementation.parallel_safe = false;
        phases.push(implementation);

        let mut validation = PlanPhase::new(
            "validation",
            "Verify the implementation meets requirements".to_string(),
            vec!["testing".into()],
        )?;
        validation.estimated_duration = if add_optimization {
            "10m".into()
        } else {
            "5m".into()
        };
        validation.dependencies = vec!["implementation".into()];
        phases.push(validation);

        if add_optimization {
            let mut optimization = PlanPhase::new(
                "optimization",
                "Optimize and refine the implementation".to_string(),
                vec!["optimization".into()],
            )?;
            optimization.estimated_duration = "10m".into();
            optimization.dependencies = vec!["validation".into()];
            phases.push(optimization);
        }

        let mut plan = ExecutionPlan::new(goal_id, phases)?;

        plan.total_estimated_duration = if add_optimization {
            "50m".into()
        } else {
            "20m".into()
        };

        plan.required_skills = plan
            .phases
            .iter()
            .flat_map(|p| p.required_capabilities.iter().cloned())
            .collect::<Vec<_>>();
        plan.required_skills.sort();
        plan.required_skills.dedup();

        if goal.complexity >= Complexity::Complex {
            plan.risk_factors
                .push("High complexity may require iterative refinement".into());
        }
        if !goal.constraints.is_empty() {
            plan.risk_factors
                .push("Constraints may limit implementation options".into());
        }

        Ok(plan)
    }
}

impl Default for ObjectivePlanner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn simple_goal() -> GoalDefinition {
        GoalDefinition::new("prompt", "fix a bug", "dev").unwrap()
    }

    fn complex_goal() -> GoalDefinition {
        let mut g = GoalDefinition::new("prompt", "big refactor", "dev").unwrap();
        g.complexity = Complexity::Complex;
        g.constraints.push("must be safe".into());
        g
    }

    #[test]
    fn simple_goal_produces_three_phases() {
        let plan = ObjectivePlanner::new().plan(&simple_goal()).unwrap();
        assert_eq!(plan.phase_count(), 3);
        assert_eq!(plan.phases[0].name, "analysis");
        assert_eq!(plan.phases[1].name, "implementation");
        assert_eq!(plan.phases[2].name, "validation");
    }

    #[test]
    fn moderate_goal_adds_optimization_phase() {
        let mut g = simple_goal();
        g.complexity = Complexity::Moderate;
        let plan = ObjectivePlanner::new().plan(&g).unwrap();
        assert_eq!(plan.phase_count(), 4);
        assert_eq!(plan.phases[3].name, "optimization");
    }

    #[test]
    fn complex_goal_adds_risk_factor() {
        let plan = ObjectivePlanner::new().plan(&complex_goal()).unwrap();
        assert!(plan
            .risk_factors
            .iter()
            .any(|r| r.contains("High complexity")));
    }

    #[test]
    fn constraints_add_risk_factor() {
        let plan = ObjectivePlanner::new().plan(&complex_goal()).unwrap();
        assert!(plan
            .risk_factors
            .iter()
            .any(|r| r.contains("Constraints")));
    }

    #[test]
    fn implementation_depends_on_analysis() {
        let plan = ObjectivePlanner::new().plan(&simple_goal()).unwrap();
        assert!(plan.phases[1].dependencies.contains(&"analysis".to_string()));
        assert!(!plan.phases[1].parallel_safe);
    }

    #[test]
    fn required_skills_deduped_and_sorted() {
        let plan = ObjectivePlanner::new().plan(&simple_goal()).unwrap();
        let skills = &plan.required_skills;
        let mut sorted = skills.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(skills, &sorted);
    }

    #[test]
    fn total_estimated_duration_simple() {
        let plan = ObjectivePlanner::new().plan(&simple_goal()).unwrap();
        assert_eq!(plan.total_estimated_duration, "20m");
    }

    #[test]
    fn total_estimated_duration_complex() {
        let plan = ObjectivePlanner::new().plan(&complex_goal()).unwrap();
        assert_eq!(plan.total_estimated_duration, "50m");
    }

    #[test]
    fn default_impl() {
        let p = ObjectivePlanner;
        assert!(p.plan(&simple_goal()).is_ok());
    }
}
