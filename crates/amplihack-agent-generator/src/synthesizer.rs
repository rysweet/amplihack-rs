use std::path::PathBuf;

use crate::error::Result;
use crate::models::{ExecutionPlan, SkillDefinition};

/// Generates [`SkillDefinition`]s needed to fulfil an [`ExecutionPlan`].
pub struct SkillSynthesizer;

impl SkillSynthesizer {
    pub fn new() -> Self {
        Self
    }

    /// Synthesize skills that cover every required capability in *plan*.
    pub fn synthesize(&self, plan: &ExecutionPlan) -> Result<Vec<SkillDefinition>> {
        let mut skills = Vec::new();

        for phase in &plan.phases {
            let (skill_name, description, score) = match phase.name.as_str() {
                "analysis" => (
                    "prompt_analysis",
                    "Analyze and understand the goal requirements",
                    0.9,
                ),
                "implementation" => (
                    "code_generation",
                    "Generate code to implement the solution",
                    0.9,
                ),
                "validation" => ("testing", "Test and verify the implementation", 0.85),
                "optimization" => ("refactoring", "Optimize and refine the implementation", 0.8),
                other => (other, "General-purpose skill", 0.5),
            };

            let content = format!("# {skill_name}\n\n{description}");
            let mut skill = SkillDefinition::new(
                skill_name,
                PathBuf::from(format!("skills/{skill_name}.yaml")),
                content,
            )?;
            skill.description = description.to_string();
            skill.match_score = score;
            skills.push(skill);
        }

        Ok(skills)
    }
}

impl Default for SkillSynthesizer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Complexity, GoalDefinition};
    use crate::planner::ObjectivePlanner;

    fn make_plan(complexity: Complexity) -> ExecutionPlan {
        let mut g = GoalDefinition::new("p", "goal", "dev").unwrap();
        g.complexity = complexity;
        ObjectivePlanner::new().plan(&g).unwrap()
    }

    #[test]
    fn synthesize_creates_skill_per_phase() {
        let plan = make_plan(Complexity::Simple);
        let skills = SkillSynthesizer::new().synthesize(&plan).unwrap();
        assert_eq!(skills.len(), plan.phase_count());
    }

    #[test]
    fn known_phases_get_named_skills() {
        let plan = make_plan(Complexity::Simple);
        let skills = SkillSynthesizer::new().synthesize(&plan).unwrap();
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"prompt_analysis"));
        assert!(names.contains(&"code_generation"));
        assert!(names.contains(&"testing"));
    }

    #[test]
    fn optimization_phase_gets_refactoring_skill() {
        let plan = make_plan(Complexity::Moderate);
        let skills = SkillSynthesizer::new().synthesize(&plan).unwrap();
        assert!(skills.iter().any(|s| s.name == "refactoring"));
    }

    #[test]
    fn skills_have_nonzero_match_scores() {
        let plan = make_plan(Complexity::Simple);
        let skills = SkillSynthesizer::new().synthesize(&plan).unwrap();
        for s in &skills {
            assert!(s.match_score > 0.0, "skill {} has zero score", s.name);
        }
    }

    #[test]
    fn skills_have_descriptions() {
        let plan = make_plan(Complexity::Simple);
        let skills = SkillSynthesizer::new().synthesize(&plan).unwrap();
        for s in &skills {
            assert!(!s.description.is_empty(), "skill {} has no description", s.name);
        }
    }

    #[test]
    fn default_impl() {
        let s = SkillSynthesizer;
        let plan = make_plan(Complexity::Simple);
        assert!(s.synthesize(&plan).is_ok());
    }
}
