//! Pipeline traits that define the bundle generation stages.

use super::error::BundleGeneratorError;
use super::models::{AgentBundle, AgentRequirement, ExtractedIntent, GeneratedAgent, ParsedPrompt};

/// Parses natural language prompts into structured representations.
pub trait PromptParser: Send + Sync {
    /// Parse a raw prompt string.
    ///
    /// # Errors
    ///
    /// Returns [`BundleGeneratorError::Parsing`] on invalid input.
    fn parse(&self, prompt: &str) -> Result<ParsedPrompt, BundleGeneratorError>;
}

/// Extracts structured intent from a parsed prompt.
pub trait IntentExtractor: Send + Sync {
    /// Extract intent from a parsed prompt.
    ///
    /// # Errors
    ///
    /// Returns [`BundleGeneratorError::Extraction`] on ambiguous input.
    fn extract(&self, parsed: &ParsedPrompt) -> Result<ExtractedIntent, BundleGeneratorError>;
}

/// Generates agent content from requirements.
pub trait AgentGenerator: Send + Sync {
    /// Generate an agent from a requirement specification.
    ///
    /// # Errors
    ///
    /// Returns [`BundleGeneratorError::Generation`] if content creation fails.
    fn generate(
        &self,
        requirement: &AgentRequirement,
        context: &ExtractedIntent,
    ) -> Result<GeneratedAgent, BundleGeneratorError>;
}

/// Assembles generated agents into a bundle.
pub trait BundleBuilder: Send + Sync {
    /// Build a bundle from generated agents.
    ///
    /// # Errors
    ///
    /// Returns [`BundleGeneratorError::Validation`] if the bundle is invalid.
    fn build(
        &self,
        name: &str,
        agents: Vec<GeneratedAgent>,
        intent: &ExtractedIntent,
    ) -> Result<AgentBundle, BundleGeneratorError>;
}
