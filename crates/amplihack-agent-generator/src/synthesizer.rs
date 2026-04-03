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
    use crate::models::PlanPhase;
    use uuid::Uuid;

    fn make_plan(phase_names: &[&str]) -> ExecutionPlan {
        let phases = phase_names
            .iter()
            .map(|name| PlanPhase::new(*name, "desc", vec!["cap".into()]).unwrap())
            .collect();
        ExecutionPlan::new(Uuid::new_v4(), phases).unwrap()
    }

    #[test]
    fn synthesize_produces_one_skill_per_phase() {
        let plan = make_plan(&["analysis", "implementation", "validation"]);
        let skills = SkillSynthesizer::new().synthesize(&plan).unwrap();
        assert_eq!(skills.len(), 3);
    }

    #[test]
    fn synthesize_known_phase_scores() {
        let plan = make_plan(&["analysis", "implementation", "validation", "optimization"]);
        let skills = SkillSynthesizer::new().synthesize(&plan).unwrap();
        assert!((skills[0].match_score - 0.9).abs() < f64::EPSILON);
        assert!((skills[1].match_score - 0.9).abs() < f64::EPSILON);
        assert!((skills[2].match_score - 0.85).abs() < f64::EPSILON);
        assert!((skills[3].match_score - 0.8).abs() < f64::EPSILON);
    }

    #[test]
    fn synthesize_unknown_phase_gets_default_score() {
        let plan = make_plan(&["custom_phase"]);
        let skills = SkillSynthesizer::new().synthesize(&plan).unwrap();
        assert_eq!(skills[0].name, "custom_phase");
        assert!((skills[0].match_score - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn synthesized_skill_content_contains_name() {
        let plan = make_plan(&["analysis"]);
        let skills = SkillSynthesizer::new().synthesize(&plan).unwrap();
        assert!(skills[0].content.contains("prompt_analysis"));
    }
}
