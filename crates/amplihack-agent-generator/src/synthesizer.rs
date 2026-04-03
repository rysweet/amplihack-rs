use crate::error::Result;
use crate::models::{ExecutionPlan, SkillDefinition};

/// Generates [`SkillDefinition`]s needed to fulfil an [`ExecutionPlan`].
pub struct SkillSynthesizer;

impl SkillSynthesizer {
    pub fn new() -> Self {
        Self
    }

    /// Synthesize skills that cover every required capability in *plan*.
    pub fn synthesize(&self, _plan: &ExecutionPlan) -> Result<Vec<SkillDefinition>> {
        todo!("SkillSynthesizer::synthesize not yet implemented")
    }
}

impl Default for SkillSynthesizer {
    fn default() -> Self {
        Self::new()
    }
}
