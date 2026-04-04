//! LLM agent abstractions for documentation generation.
//!
//! Provides LLM provider management, API key rotation, and prompt templates.

pub mod provider;
pub mod template_manager;
pub mod templates;

pub use provider::{
    ApiKeyManager, KeyManagerConfig, KeyStatistics, KeyStatus, LlmProvider, LlmProviderConfig,
    ModelProvider, ReasoningEffort, discover_keys_for_provider, parse_structured_output,
    validate_key,
};
pub use template_manager::TemplateManager;
pub use templates::PromptTemplate;
