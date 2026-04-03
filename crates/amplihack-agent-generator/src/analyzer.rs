use crate::error::Result;
use crate::models::GoalDefinition;

/// Analyzes a raw user prompt to extract a structured [`GoalDefinition`].
pub struct PromptAnalyzer;

impl PromptAnalyzer {
    pub fn new() -> Self {
        Self
    }

    /// Parse *prompt* into a structured goal with domain, constraints, and
    /// complexity classification.
    pub fn analyze(&self, _prompt: &str) -> Result<GoalDefinition> {
        todo!("PromptAnalyzer::analyze not yet implemented")
    }
}

impl Default for PromptAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}
