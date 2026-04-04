//! Copilot SDK adapter.
//!
//! Ports Python `amplihack/agents/goal_seeking/sdk_adapters/copilot_sdk.py`.
//! Wraps the GitHub Copilot SDK client behind the [`SdkAdapter`] trait.

use async_trait::async_trait;
use tracing::{debug, info, warn};

use crate::error::{AgentError, Result};

use super::base::{SdkAdapter, SdkClient};
use super::types::{AdapterResult, AgentTool, SdkAdapterConfig, SdkType};

/// Native tools available through the Copilot SDK.
pub const NATIVE_TOOLS: &[&str] = &["file_system", "git", "web_requests"];

/// Default timeout for Copilot agent runs (seconds).
pub const DEFAULT_TIMEOUT: f64 = 300.0;

/// Maximum allowed timeout (seconds).
pub const MAX_TIMEOUT: f64 = 600.0;

// ---------------------------------------------------------------------------
// CopilotAdapter
// ---------------------------------------------------------------------------

/// SDK adapter for GitHub Copilot.
///
/// Creates a fresh client + session per `run_agent` call to avoid event loop
/// issues (matching the Python pattern). Enforces bounded timeouts and tracks
/// tool usage via events.
#[derive(Debug)]
pub struct CopilotAdapter {
    config: SdkAdapterConfig,
    client: Option<Box<dyn SdkClient>>,
    tools: Vec<AgentTool>,
    system_prompt: String,
    streaming: bool,
    timeout: f64,
    cli_path: Option<String>,
    tools_used: Vec<String>,
}

impl CopilotAdapter {
    pub fn new(config: SdkAdapterConfig) -> Self {
        let timeout = config.timeout_secs.clamp(1.0, MAX_TIMEOUT);
        Self {
            config,
            client: None,
            tools: Vec::new(),
            system_prompt: String::new(),
            streaming: false,
            timeout,
            cli_path: None,
            tools_used: Vec::new(),
        }
    }

    pub fn with_client(mut self, client: Box<dyn SdkClient>) -> Self {
        self.client = Some(client);
        self
    }

    pub fn with_streaming(mut self, streaming: bool) -> Self {
        self.streaming = streaming;
        self
    }

    pub fn with_timeout(mut self, timeout: f64) -> Self {
        self.timeout = timeout.clamp(1.0, MAX_TIMEOUT);
        self
    }

    pub fn with_cli_path(mut self, path: impl Into<String>) -> Self {
        self.cli_path = Some(path.into());
        self
    }

    /// Returns the list of tools used during the last `run_agent` call.
    pub fn tools_used(&self) -> &[String] {
        &self.tools_used
    }

    /// Returns the effective timeout in seconds.
    pub fn timeout(&self) -> f64 {
        self.timeout
    }

    /// Returns the registered custom tools.
    pub fn tools(&self) -> &[AgentTool] {
        &self.tools
    }

    /// Returns whether streaming is enabled.
    pub fn streaming(&self) -> bool {
        self.streaming
    }
}

#[async_trait]
impl SdkAdapter for CopilotAdapter {
    fn sdk_type(&self) -> SdkType {
        SdkType::Copilot
    }

    fn create_agent(&mut self) -> Result<()> {
        self.system_prompt = self.build_system_prompt();
        info!(
            name = %self.config.name,
            timeout = self.timeout,
            cli_path = ?self.cli_path,
            "Copilot adapter agent created"
        );
        Ok(())
    }

    async fn run_agent(&mut self, task: &str, max_turns: u32) -> Result<AdapterResult> {
        self.tools_used.clear();

        let client = self
            .client
            .as_ref()
            .ok_or_else(|| AgentError::ConfigError("Copilot SDK client not configured".into()))?;

        debug!(task_len = task.len(), max_turns, "Copilot adapter running");

        match client
            .query(task, &self.system_prompt, &self.config.model, max_turns)
            .await
        {
            Ok(resp) => {
                self.tools_used = resp.tool_calls.clone();
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
            Err(AgentError::TimeoutError(secs)) => {
                warn!(secs, "Copilot agent timed out");
                Ok(AdapterResult::failure(format!(
                    "Copilot agent timed out after {secs}s"
                )))
            }
            Err(e) => {
                warn!(error = %e, "Copilot adapter run failed");
                Ok(AdapterResult::failure(format!("SDK error: {e}")))
            }
        }
    }

    fn native_tools(&self) -> Vec<String> {
        NATIVE_TOOLS.iter().map(|s| (*s).to_string()).collect()
    }

    fn register_tool(&mut self, tool: AgentTool) -> Result<()> {
        debug!(tool = %tool.name, "Registering tool with Copilot adapter");
        self.tools.push(tool);
        self.system_prompt = self.build_system_prompt();
        Ok(())
    }

    fn build_system_prompt(&self) -> String {
        let mut prompt = String::new();

        if self.tools.is_empty() {
            prompt.push_str(
                "You are a code-generation agent powered by GitHub Copilot.\n\
                 Produce correct, well-tested code.\n",
            );
        } else {
            prompt.push_str(
                "You are a goal-seeking learner powered by GitHub Copilot.\n\
                 Use available tools to achieve the goal efficiently.\n\n\
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
        self.tools_used.clear();
        debug!(name = %self.config.name, "Copilot adapter closed");
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

    #[derive(Debug)]
    struct MockClient {
        response: String,
        tool_calls: Vec<String>,
        error: Option<AgentError>,
    }

    impl MockClient {
        fn ok(response: &str) -> Box<dyn SdkClient> {
            Box::new(Self {
                response: response.to_string(),
                tool_calls: vec![],
                error: None,
            })
        }
        fn with_tools(response: &str, tools: Vec<String>) -> Box<dyn SdkClient> {
            Box::new(Self {
                response: response.to_string(),
                tool_calls: tools,
                error: None,
            })
        }
        fn timeout() -> Box<dyn SdkClient> {
            Box::new(Self {
                response: String::new(),
                tool_calls: vec![],
                error: Some(AgentError::TimeoutError(300)),
            })
        }
        fn failing() -> Box<dyn SdkClient> {
            Box::new(Self {
                response: String::new(),
                tool_calls: vec![],
                error: Some(AgentError::TaskFailed("mock failure".into())),
            })
        }
    }

    #[async_trait]
    impl SdkClient for MockClient {
        async fn query(&self, _: &str, _: &str, _: &str, _: u32) -> Result<SdkClientResponse> {
            match &self.error {
                Some(AgentError::TimeoutError(s)) => Err(AgentError::TimeoutError(*s)),
                Some(AgentError::TaskFailed(s)) => Err(AgentError::TaskFailed(s.clone())),
                Some(_) => Err(AgentError::TaskFailed("unknown".into())),
                None => {
                    Ok(SdkClientResponse::new(&self.response)
                        .with_tool_calls(self.tool_calls.clone()))
                }
            }
        }
    }

    fn test_config() -> SdkAdapterConfig {
        SdkAdapterConfig::new("test-copilot", SdkType::Copilot).with_model("copilot-test")
    }

    #[test]
    fn sdk_type_and_native_tools() {
        let adapter = CopilotAdapter::new(test_config());
        assert_eq!(adapter.sdk_type(), SdkType::Copilot);
        let tools = adapter.native_tools();
        assert!(tools.contains(&"file_system".to_string()));
        assert!(tools.contains(&"git".to_string()));
        assert_eq!(tools.len(), 3);
    }

    #[test]
    fn timeout_clamping() {
        assert!(
            (CopilotAdapter::new(test_config())
                .with_timeout(0.0)
                .timeout()
                - 1.0)
                .abs()
                < f64::EPSILON
        );
        assert!(
            (CopilotAdapter::new(test_config())
                .with_timeout(999.0)
                .timeout()
                - MAX_TIMEOUT)
                .abs()
                < f64::EPSILON
        );
        assert!(
            (CopilotAdapter::new(test_config())
                .with_timeout(120.0)
                .timeout()
                - 120.0)
                .abs()
                < f64::EPSILON
        );
    }

    #[test]
    fn builder_options() {
        let adapter = CopilotAdapter::new(test_config())
            .with_streaming(true)
            .with_cli_path("/usr/bin/copilot");
        assert!(adapter.streaming());
        assert_eq!(adapter.cli_path.as_deref(), Some("/usr/bin/copilot"));
    }

    #[test]
    fn create_agent_and_register_tool() {
        let mut adapter = CopilotAdapter::new(test_config().with_instructions("Be concise"));
        adapter.create_agent().unwrap();
        assert!(adapter.system_prompt.contains("GitHub Copilot"));
        assert!(adapter.system_prompt.contains("code-generation"));

        adapter
            .register_tool(AgentTool::new("mem_search", "Search memory"))
            .unwrap();
        assert!(adapter.system_prompt.contains("goal-seeking"));
        assert!(adapter.system_prompt.contains("mem_search"));
    }

    #[tokio::test]
    async fn run_agent_success() {
        let mut adapter =
            CopilotAdapter::new(test_config()).with_client(MockClient::ok("Copilot response"));
        adapter.create_agent().unwrap();
        let result = adapter.run_agent("do something", 5).await.unwrap();
        assert!(result.goal_achieved);
        assert_eq!(result.response, "Copilot response");
    }

    #[tokio::test]
    async fn run_agent_tracks_tools() {
        let mut adapter = CopilotAdapter::new(test_config()).with_client(MockClient::with_tools(
            "done",
            vec!["file_system".into(), "git".into()],
        ));
        adapter.create_agent().unwrap();
        let result = adapter.run_agent("task", 5).await.unwrap();
        assert_eq!(result.tools_used.len(), 2);
        assert_eq!(adapter.tools_used().len(), 2);
    }

    #[tokio::test]
    async fn run_agent_timeout_and_error() {
        let mut a1 = CopilotAdapter::new(test_config()).with_client(MockClient::timeout());
        a1.create_agent().unwrap();
        let r1 = a1.run_agent("task", 5).await.unwrap();
        assert!(!r1.goal_achieved);
        assert!(r1.response.contains("timed out"));

        let mut a2 = CopilotAdapter::new(test_config()).with_client(MockClient::failing());
        a2.create_agent().unwrap();
        let r2 = a2.run_agent("task", 5).await.unwrap();
        assert!(!r2.goal_achieved);
        assert!(r2.response.contains("SDK error"));
    }

    #[tokio::test]
    async fn run_agent_no_client() {
        let mut adapter = CopilotAdapter::new(test_config());
        adapter.create_agent().unwrap();
        assert!(matches!(
            adapter.run_agent("task", 5).await.unwrap_err(),
            AgentError::ConfigError(_)
        ));
    }

    #[tokio::test]
    async fn close_and_run_clears_state() {
        let mut adapter = CopilotAdapter::new(test_config()).with_client(MockClient::ok("x"));
        adapter.tools_used = vec!["git".into()];
        adapter.close().await.unwrap();
        assert!(adapter.client.is_none());
        assert!(adapter.tools_used.is_empty());

        // run_agent clears previous tools_used
        let mut a2 = CopilotAdapter::new(test_config()).with_client(MockClient::ok("x"));
        a2.create_agent().unwrap();
        a2.tools_used = vec!["old_tool".into()];
        let _ = a2.run_agent("task", 5).await.unwrap();
        assert!(a2.tools_used().is_empty());
    }
}
