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
                "optimization" => (
                    "refactoring",
                    "Optimize and refine the implementation",
                    0.8,
                ),
                other => (other, "General-purpose skill", 0.5),
            };

            let mut skill = SkillDefinition::new(
                skill_name,
                PathBuf::from(format!("skills/{skill_name}.yaml")),
                description,
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
