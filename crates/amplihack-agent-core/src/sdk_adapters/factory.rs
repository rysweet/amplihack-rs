//! SDK adapter factory.
//!
//! Ports Python `amplihack/agents/goal_seeking/sdk_adapters/factory.py`.
//! Provides [`create_adapter`] to instantiate the correct adapter by type.

use crate::error::{AgentError, Result};

use super::base::SdkAdapter;
use super::claude::ClaudeAdapter;
use super::copilot::CopilotAdapter;
use super::microsoft::MicrosoftAdapter;
use super::types::{SdkAdapterConfig, SdkType};

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

/// Create an SDK adapter for the given configuration.
///
/// Returns a boxed [`SdkAdapter`] trait object. The caller must attach an
/// [`SdkClient`](super::base::SdkClient) implementation via the concrete
/// adapter's `with_client` method before calling `run_agent`.
///
/// # Errors
///
/// Returns [`AgentError::ConfigError`] for `SdkType::Mini` (not yet ported).
///
/// # Examples
///
/// ```rust,no_run
/// use amplihack_agent_core::sdk_adapters::{create_adapter, SdkAdapterConfig, SdkType};
///
/// let config = SdkAdapterConfig::new("my-agent", SdkType::Claude);
/// let adapter = create_adapter(config).unwrap();
/// assert_eq!(adapter.sdk_type(), SdkType::Claude);
/// ```
pub fn create_adapter(config: SdkAdapterConfig) -> Result<Box<dyn SdkAdapter>> {
    match config.sdk_type {
        SdkType::Claude => Ok(Box::new(ClaudeAdapter::new(config))),
        SdkType::Copilot => Ok(Box::new(CopilotAdapter::new(config))),
        SdkType::Microsoft => Ok(Box::new(MicrosoftAdapter::new(config))),
        SdkType::Mini => Err(AgentError::ConfigError(
            "Mini framework adapter not yet ported to Rust".into(),
        )),
    }
}

/// Create an SDK adapter from a string SDK type name and agent name.
///
/// Convenience wrapper around [`create_adapter`] for CLI/config-driven usage.
pub fn create_adapter_by_name(name: &str, sdk: &str) -> Result<Box<dyn SdkAdapter>> {
    let sdk_type: SdkType = sdk.parse()?;
    let config = SdkAdapterConfig::new(name, sdk_type);
    create_adapter(config)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_claude_adapter() {
        let config = SdkAdapterConfig::new("test", SdkType::Claude);
        let adapter = create_adapter(config).unwrap();
        assert_eq!(adapter.sdk_type(), SdkType::Claude);
    }

    #[test]
    fn create_copilot_adapter() {
        let config = SdkAdapterConfig::new("test", SdkType::Copilot);
        let adapter = create_adapter(config).unwrap();
        assert_eq!(adapter.sdk_type(), SdkType::Copilot);
    }

    #[test]
    fn create_microsoft_adapter() {
        let config = SdkAdapterConfig::new("test", SdkType::Microsoft);
        let adapter = create_adapter(config).unwrap();
        assert_eq!(adapter.sdk_type(), SdkType::Microsoft);
    }

    #[test]
    fn create_mini_returns_error() {
        let config = SdkAdapterConfig::new("test", SdkType::Mini);
        let err = create_adapter(config).unwrap_err();
        assert!(matches!(err, AgentError::ConfigError(_)));
        assert!(err.to_string().contains("Mini"));
    }

    #[test]
    fn create_by_name_claude() {
        let adapter = create_adapter_by_name("agent-1", "claude").unwrap();
        assert_eq!(adapter.sdk_type(), SdkType::Claude);
        assert_eq!(adapter.config().name, "agent-1");
    }

    #[test]
    fn create_by_name_copilot() {
        let adapter = create_adapter_by_name("agent-2", "copilot").unwrap();
        assert_eq!(adapter.sdk_type(), SdkType::Copilot);
    }

    #[test]
    fn create_by_name_microsoft() {
        let adapter = create_adapter_by_name("agent-3", "microsoft").unwrap();
        assert_eq!(adapter.sdk_type(), SdkType::Microsoft);
    }

    #[test]
    fn create_by_name_invalid() {
        let err = create_adapter_by_name("agent", "invalid").unwrap_err();
        assert!(matches!(err, AgentError::ConfigError(_)));
    }

    #[test]
    fn create_by_name_case_insensitive() {
        let adapter = create_adapter_by_name("a", "CLAUDE").unwrap();
        assert_eq!(adapter.sdk_type(), SdkType::Claude);

        let adapter = create_adapter_by_name("a", "Copilot").unwrap();
        assert_eq!(adapter.sdk_type(), SdkType::Copilot);
    }

    #[test]
    fn factory_returns_correct_native_tools() {
        let claude = create_adapter_by_name("a", "claude").unwrap();
        assert!(claude.native_tools().contains(&"bash".to_string()));

        let copilot = create_adapter_by_name("a", "copilot").unwrap();
        assert!(copilot.native_tools().contains(&"git".to_string()));

        let ms = create_adapter_by_name("a", "microsoft").unwrap();
        assert!(ms.native_tools().is_empty()); // No tools until registered
    }

    #[test]
    fn factory_adapters_share_config_interface() {
        for sdk in &["claude", "copilot", "microsoft"] {
            let adapter = create_adapter_by_name("test-agent", sdk).unwrap();
            assert_eq!(adapter.config().name, "test-agent");
            // All adapters should produce a non-empty system prompt.
            let prompt = adapter.build_system_prompt();
            assert!(!prompt.is_empty());
        }
    }
}
