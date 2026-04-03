//! Claude SDK adapter.
//!
//! Ports Python `amplihack/agents/goal_seeking/sdk_adapters/claude_sdk.py`.
//! Wraps the Claude SDK client behind the [`SdkAdapter`] trait.

use async_trait::async_trait;
use tracing::{debug, warn};

use crate::error::{AgentError, Result};

use super::base::{SdkAdapter, SdkClient};
use super::types::{AdapterResult, AgentTool, SdkAdapterConfig, SdkType};

/// Native tools available through the Claude SDK.
pub const NATIVE_TOOLS: &[&str] = &[
    "bash",
    "read_file",
    "write_file",
    "edit_file",
    "glob",
    "grep",
];

// ---------------------------------------------------------------------------
// ClaudeAdapter
// ---------------------------------------------------------------------------

/// SDK adapter for Anthropic's Claude.
///
/// Creates a fresh client per `run_agent` call (matching the Python pattern
/// of per-run `ClaudeSDKClient` instantiation). Uses `permission_mode =
/// bypassPermissions` equivalent configuration.
#[derive(Debug)]
pub struct ClaudeAdapter {
    config: SdkAdapterConfig,
    client: Option<Box<dyn SdkClient>>,
    tools: Vec<AgentTool>,
    system_prompt: String,
}

impl ClaudeAdapter {
    pub fn new(config: SdkAdapterConfig) -> Self {
        Self {
            config,
            client: None,
            tools: Vec::new(),
            system_prompt: String::new(),
        }
    }

    /// Attach an SDK client implementation.
    pub fn with_client(mut self, client: Box<dyn SdkClient>) -> Self {
        self.client = Some(client);
        self
    }

    /// Returns the registered custom tools.
    pub fn tools(&self) -> &[AgentTool] {
        &self.tools
    }
}

#[async_trait]
impl SdkAdapter for ClaudeAdapter {
    fn sdk_type(&self) -> SdkType {
        SdkType::Claude
    }

    fn create_agent(&mut self) -> Result<()> {
        self.system_prompt = self.build_system_prompt();
        debug!(name = %self.config.name, "Claude adapter agent created");
        Ok(())
    }

    async fn run_agent(&mut self, task: &str, max_turns: u32) -> Result<AdapterResult> {
        let client = self.client.as_ref().ok_or_else(|| {
            AgentError::ConfigError("Claude SDK client not configured".into())
        })?;

        match client
            .query(task, &self.system_prompt, &self.config.model, max_turns)
            .await
        {
            Ok(resp) => {
                let goal_achieved = !resp.content.is_empty();
                Ok(AdapterResult {
                    response: resp.content,
                    goal_achieved,
                    tools_used: resp.tool_calls,
                    turns: 1,
                    metadata: resp.metadata,
                    ..Default::default()
                })
            }
            Err(e) => {
                warn!(error = %e, "Claude adapter run failed");
                Ok(AdapterResult::failure(format!("SDK error: {e}")))
            }
        }
    }

    fn native_tools(&self) -> Vec<String> {
        NATIVE_TOOLS.iter().map(|s| (*s).to_string()).collect()
    }

    fn register_tool(&mut self, tool: AgentTool) -> Result<()> {
        debug!(tool = %tool.name, "Registering tool with Claude adapter");
        self.tools.push(tool);
        // Rebuild system prompt to include new tool descriptions.
        self.system_prompt = self.build_system_prompt();
        Ok(())
    }

    fn build_system_prompt(&self) -> String {
        let mut prompt = String::new();

        if self.tools.is_empty() {
            prompt.push_str(
                "You are a code-generation agent. \
                 Produce correct, well-tested code.\n",
            );
        } else {
            prompt.push_str(
                "You are a goal-seeking learner with access to tools.\n\
                 Use them to achieve the goal efficiently.\n\n\
                 Available custom tools:\n",
            );
            for tool in &self.tools {
                prompt.push_str(&format!("- {}: {}\n", tool.name, tool.description));
            }
        }

        if !self.config.instructions.is_empty() {
            prompt.push_str("\n## Custom Instructions\n");
            prompt.push_str(&self.config.instructions);
            prompt.push('\n');
        }

        prompt
    }

    fn config(&self) -> &SdkAdapterConfig {
        &self.config
    }

    async fn close(&mut self) -> Result<()> {
        if let Some(client) = self.client.as_mut() {
            client.close().await?;
        }
        self.client = None;
        debug!(name = %self.config.name, "Claude adapter closed");
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::super::base::SdkClientResponse;
    use super::*;

    /// A mock client for testing.
    #[derive(Debug)]
    struct MockClient {
        response: String,
        should_fail: bool,
    }

    impl MockClient {
        fn ok(response: &str) -> Box<dyn SdkClient> {
            Box::new(Self {
                response: response.to_string(),
                should_fail: false,
            })
        }

        fn failing() -> Box<dyn SdkClient> {
            Box::new(Self {
                response: String::new(),
                should_fail: true,
            })
        }
    }

    #[async_trait]
    impl SdkClient for MockClient {
        async fn query(
            &self,
            _prompt: &str,
            _system: &str,
            _model: &str,
            _max_turns: u32,
        ) -> Result<SdkClientResponse> {
            if self.should_fail {
                Err(AgentError::TaskFailed("mock failure".into()))
            } else {
                Ok(SdkClientResponse::new(&self.response))
            }
        }
    }

    fn test_config() -> SdkAdapterConfig {
        SdkAdapterConfig::new("test-claude", SdkType::Claude)
            .with_model("claude-test")
    }

    #[test]
    fn sdk_type_is_claude() {
        let adapter = ClaudeAdapter::new(test_config());
        assert_eq!(adapter.sdk_type(), SdkType::Claude);
    }

    #[test]
    fn native_tools_list() {
        let adapter = ClaudeAdapter::new(test_config());
        let tools = adapter.native_tools();
        assert!(tools.contains(&"bash".to_string()));
        assert!(tools.contains(&"grep".to_string()));
        assert_eq!(tools.len(), 6);
    }

    #[test]
    fn create_agent_builds_prompt() {
        let mut adapter = ClaudeAdapter::new(
            test_config().with_instructions("Be precise"),
        );
        adapter.create_agent().unwrap();
        assert!(adapter.system_prompt.contains("code-generation agent"));
        assert!(adapter.system_prompt.contains("Be precise"));
    }

    #[test]
    fn register_tool_rebuilds_prompt() {
        let mut adapter = ClaudeAdapter::new(test_config());
        adapter.create_agent().unwrap();
        assert!(adapter.system_prompt.contains("code-generation"));

        let tool = AgentTool::new("search_memory", "Search the memory store");
        adapter.register_tool(tool).unwrap();
        assert!(adapter.system_prompt.contains("goal-seeking"));
        assert!(adapter.system_prompt.contains("search_memory"));
        assert_eq!(adapter.tools().len(), 1);
    }

    #[test]
    fn prompt_without_tools_is_code_gen() {
        let adapter = ClaudeAdapter::new(test_config());
        let prompt = adapter.build_system_prompt();
        assert!(prompt.contains("code-generation"));
        assert!(!prompt.contains("goal-seeking"));
    }

    #[test]
    fn prompt_with_tools_is_goal_seeking() {
        let mut adapter = ClaudeAdapter::new(test_config());
        adapter.tools.push(AgentTool::new("t1", "tool one"));
        let prompt = adapter.build_system_prompt();
        assert!(prompt.contains("goal-seeking"));
        assert!(prompt.contains("t1: tool one"));
    }

    #[tokio::test]
    async fn run_agent_success() {
        let mut adapter = ClaudeAdapter::new(test_config())
            .with_client(MockClient::ok("Hello from Claude"));
        adapter.create_agent().unwrap();

        let result = adapter.run_agent("test task", 5).await.unwrap();
        assert!(result.goal_achieved);
        assert_eq!(result.response, "Hello from Claude");
    }

    #[tokio::test]
    async fn run_agent_empty_response() {
        let mut adapter = ClaudeAdapter::new(test_config())
            .with_client(MockClient::ok(""));
        adapter.create_agent().unwrap();

        let result = adapter.run_agent("test", 5).await.unwrap();
        assert!(!result.goal_achieved);
    }

    #[tokio::test]
    async fn run_agent_client_error_returns_failure() {
        let mut adapter = ClaudeAdapter::new(test_config())
            .with_client(MockClient::failing());
        adapter.create_agent().unwrap();

        let result = adapter.run_agent("test", 5).await.unwrap();
        assert!(!result.goal_achieved);
        assert!(result.response.contains("SDK error"));
    }

    #[tokio::test]
    async fn run_agent_no_client_returns_error() {
        let mut adapter = ClaudeAdapter::new(test_config());
        adapter.create_agent().unwrap();

        let err = adapter.run_agent("test", 5).await.unwrap_err();
        assert!(matches!(err, AgentError::ConfigError(_)));
    }

    #[tokio::test]
    async fn close_clears_client() {
        let mut adapter = ClaudeAdapter::new(test_config())
            .with_client(MockClient::ok("x"));
        adapter.close().await.unwrap();
        assert!(adapter.client.is_none());
    }

    #[test]
    fn config_accessor() {
        let adapter = ClaudeAdapter::new(test_config());
        assert_eq!(adapter.config().name, "test-claude");
        assert_eq!(adapter.config().model, "claude-test");
    }
}
