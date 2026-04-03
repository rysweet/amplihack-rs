//! Core SDK adapter traits.
//!
//! Ports Python `amplihack/agents/goal_seeking/sdk_adapters/base.py` abstract base.
//! Defines [`SdkAdapter`] (the main adapter trait) and [`SdkClient`] (the
//! underlying SDK communication layer).

use std::collections::HashMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Result;

use super::types::{AdapterResult, AgentTool, SdkAdapterConfig, SdkType};

// ---------------------------------------------------------------------------
// SdkClientResponse
// ---------------------------------------------------------------------------

/// Response returned by an [`SdkClient::query`] call.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SdkClientResponse {
    /// The text content of the response.
    pub content: String,
    /// Names of tools that were invoked during the run.
    #[serde(default)]
    pub tool_calls: Vec<String>,
    /// Arbitrary metadata returned by the SDK.
    #[serde(default)]
    pub metadata: HashMap<String, Value>,
}

impl SdkClientResponse {
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            ..Default::default()
        }
    }

    pub fn with_tool_calls(mut self, calls: Vec<String>) -> Self {
        self.tool_calls = calls;
        self
    }
}

// ---------------------------------------------------------------------------
// SdkClient trait
// ---------------------------------------------------------------------------

/// Trait abstracting the underlying SDK client (HTTP/gRPC/subprocess).
///
/// Each SDK (Claude, Copilot, Microsoft) provides a different client library.
/// Implement this trait to plug in the actual SDK communication layer.
/// The adapters hold a `Box<dyn SdkClient>` for runtime polymorphism.
#[async_trait]
pub trait SdkClient: Send + Sync + std::fmt::Debug {
    /// Send a query to the LLM and get a response.
    async fn query(
        &self,
        prompt: &str,
        system: &str,
        model: &str,
        max_turns: u32,
    ) -> Result<SdkClientResponse>;

    /// Close and clean up client resources.
    async fn close(&mut self) -> Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// SdkAdapter trait
// ---------------------------------------------------------------------------

/// The core SDK adapter trait.
///
/// Each adapter wraps an [`SdkClient`] and provides a uniform interface
/// for creating agents, running tasks, and managing tools.
///
/// Mirrors the four abstract methods of Python `GoalSeekingAgent`:
/// - `_create_sdk_agent()` → [`SdkAdapter::create_agent`]
/// - `_run_sdk_agent(task, max_turns)` → [`SdkAdapter::run_agent`]
/// - `_get_native_tools()` → [`SdkAdapter::native_tools`]
/// - `_register_tool_with_sdk(tool)` → [`SdkAdapter::register_tool`]
#[async_trait]
pub trait SdkAdapter: Send + Sync + std::fmt::Debug {
    /// Which SDK backend this adapter uses.
    fn sdk_type(&self) -> SdkType;

    /// Initialize the underlying SDK agent/client.
    fn create_agent(&mut self) -> Result<()>;

    /// Run a task through the SDK agent.
    async fn run_agent(&mut self, task: &str, max_turns: u32) -> Result<AdapterResult>;

    /// List of native tools provided by the SDK.
    fn native_tools(&self) -> Vec<String>;

    /// Register a custom tool with the SDK.
    fn register_tool(&mut self, tool: AgentTool) -> Result<()>;

    /// Build the system prompt for this adapter.
    fn build_system_prompt(&self) -> String;

    /// Get the adapter configuration.
    fn config(&self) -> &SdkAdapterConfig;

    /// Close and clean up resources.
    async fn close(&mut self) -> Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_response_builder() {
        let resp = SdkClientResponse::new("hello world")
            .with_tool_calls(vec!["bash".into(), "grep".into()]);
        assert_eq!(resp.content, "hello world");
        assert_eq!(resp.tool_calls.len(), 2);
    }

    #[test]
    fn client_response_default() {
        let resp = SdkClientResponse::default();
        assert!(resp.content.is_empty());
        assert!(resp.tool_calls.is_empty());
        assert!(resp.metadata.is_empty());
    }

    #[test]
    fn client_response_serde() {
        let resp = SdkClientResponse::new("test");
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: SdkClientResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.content, "test");
    }
}
