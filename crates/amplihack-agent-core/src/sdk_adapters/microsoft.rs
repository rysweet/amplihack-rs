//! Microsoft Agent Framework SDK adapter.
//!
//! Ports Python `amplihack/agents/goal_seeking/sdk_adapters/microsoft_sdk.py`.
//! Wraps the Microsoft Agent Framework (OpenAI-backed) behind [`SdkAdapter`].

use async_trait::async_trait;
use tracing::{debug, warn};

use crate::error::{AgentError, Result};

use super::base::{SdkAdapter, SdkClient};
use super::types::{AdapterResult, AgentTool, SdkAdapterConfig, SdkType};

/// Default system prompt template (used when no external prompt file is found).
///
/// Mirrors the fallback in Python `MicrosoftGoalSeekingAgent._build_system_prompt`.
const SYSTEM_TEMPLATE: &str = "\
You are a goal-seeking agent powered by the Microsoft Agent Framework.\n\
Your objective is to accomplish the user's task using available tools and knowledge.\n\
\n\
## Approach\n\
1. Analyze the task carefully\n\
2. Use available tools to gather information and take actions\n\
3. Track what you learn and apply it\n\
4. Verify your results before reporting completion\n";

// ---------------------------------------------------------------------------
// MicrosoftAdapter
// ---------------------------------------------------------------------------

/// SDK adapter for the Microsoft Agent Framework.
///
/// Uses session-based execution (stateful conversation). The SDK agent is
/// rebuilt whenever new tools are registered (matching the Python pattern of
/// calling `_create_sdk_agent()` on tool registration).
#[derive(Debug)]
pub struct MicrosoftAdapter {
    config: SdkAdapterConfig,
    client: Option<Box<dyn SdkClient>>,
    tools: Vec<AgentTool>,
    system_prompt: String,
    session_active: bool,
}

impl MicrosoftAdapter {
    pub fn new(config: SdkAdapterConfig) -> Self {
        Self {
            config,
            client: None,
            tools: Vec::new(),
            system_prompt: String::new(),
            session_active: false,
        }
    }

    /// Attach an SDK client implementation.
    pub fn with_client(mut self, client: Box<dyn SdkClient>) -> Self {
        self.client = Some(client);
        self
    }

    /// Whether a session is currently active.
    pub fn session_active(&self) -> bool {
        self.session_active
    }

    /// Returns the registered custom tools.
    pub fn tools(&self) -> &[AgentTool] {
        &self.tools
    }

    /// Reset the session (create a new one for the next run).
    pub fn reset_session(&mut self) {
        self.session_active = false;
        debug!(name = %self.config.name, "Microsoft adapter session reset");
    }
}

#[async_trait]
impl SdkAdapter for MicrosoftAdapter {
    fn sdk_type(&self) -> SdkType {
        SdkType::Microsoft
    }

    fn create_agent(&mut self) -> Result<()> {
        self.system_prompt = self.build_system_prompt();
        self.session_active = true;
        debug!(
            name = %self.config.name,
            model = %self.config.model,
            tools = self.tools.len(),
            "Microsoft adapter agent created"
        );
        Ok(())
    }

    async fn run_agent(&mut self, task: &str, max_turns: u32) -> Result<AdapterResult> {
        if !self.session_active {
            return Err(AgentError::ConfigError(
                "Microsoft adapter: agent not initialized (call create_agent first)".into(),
            ));
        }

        let client = self.client.as_ref().ok_or_else(|| {
            AgentError::ConfigError(
                "Microsoft SDK client not configured".into(),
            )
        })?;

        debug!(task_len = task.len(), max_turns, "Microsoft adapter running");

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
                warn!(error = %e, "Microsoft adapter run failed");
                Err(AgentError::TaskFailed(format!(
                    "Microsoft agent error: {e}"
                )))
            }
        }
    }

    fn native_tools(&self) -> Vec<String> {
        self.tools.iter().map(|t| t.name.clone()).collect()
    }

    fn register_tool(&mut self, tool: AgentTool) -> Result<()> {
        debug!(tool = %tool.name, "Registering tool with Microsoft adapter");
        self.tools.push(tool);
        // Rebuild agent with the new tool set (mirrors Python behavior).
        self.create_agent()
    }

    fn build_system_prompt(&self) -> String {
        let mut prompt = String::from(SYSTEM_TEMPLATE);

        if !self.tools.is_empty() {
            prompt.push_str("\n## Available Tools\n");
            for tool in &self.tools {
                prompt.push_str(&format!("- **{}**: {}\n", tool.name, tool.description));
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
        self.session_active = false;
        debug!(name = %self.config.name, "Microsoft adapter closed");
        Ok(())
    }
}

impl std::fmt::Display for MicrosoftAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "MicrosoftAdapter(name={}, model={})",
            self.config.name, self.config.model
        )
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
        should_fail: bool,
    }

    impl MockClient {
        fn ok(response: &str) -> Box<dyn SdkClient> {
            Box::new(Self {
                response: response.to_string(),
                tool_calls: vec![],
                should_fail: false,
            })
        }

        fn with_tools(response: &str, tools: Vec<String>) -> Box<dyn SdkClient> {
            Box::new(Self {
                response: response.to_string(),
                tool_calls: tools,
                should_fail: false,
            })
        }

        fn failing() -> Box<dyn SdkClient> {
            Box::new(Self {
                response: String::new(),
                tool_calls: vec![],
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
                Ok(SdkClientResponse::new(&self.response)
                    .with_tool_calls(self.tool_calls.clone()))
            }
        }
    }

    fn test_config() -> SdkAdapterConfig {
        SdkAdapterConfig::new("test-ms", SdkType::Microsoft)
            .with_model("gpt-4")
    }

    #[test]
    fn sdk_type_is_microsoft() {
        let adapter = MicrosoftAdapter::new(test_config());
        assert_eq!(adapter.sdk_type(), SdkType::Microsoft);
    }

    #[test]
    fn display_format() {
        let adapter = MicrosoftAdapter::new(test_config());
        let s = adapter.to_string();
        assert!(s.contains("test-ms"));
        assert!(s.contains("gpt-4"));
    }

    #[test]
    fn native_tools_from_registered() {
        let mut adapter = MicrosoftAdapter::new(test_config());
        assert!(adapter.native_tools().is_empty());

        adapter.tools.push(AgentTool::new("search", "Search"));
        adapter.tools.push(AgentTool::new("calc", "Calculate"));
        let tools = adapter.native_tools();
        assert_eq!(tools, vec!["search", "calc"]);
    }

    #[test]
    fn create_agent_activates_session() {
        let mut adapter = MicrosoftAdapter::new(test_config());
        assert!(!adapter.session_active());
        adapter.create_agent().unwrap();
        assert!(adapter.session_active());
    }

    #[test]
    fn reset_session_deactivates() {
        let mut adapter = MicrosoftAdapter::new(test_config());
        adapter.create_agent().unwrap();
        assert!(adapter.session_active());
        adapter.reset_session();
        assert!(!adapter.session_active());
    }

    #[test]
    fn register_tool_rebuilds_agent() {
        let mut adapter = MicrosoftAdapter::new(test_config());
        adapter.create_agent().unwrap();

        let tool = AgentTool::new("fact_check", "Verify facts");
        adapter.register_tool(tool).unwrap();
        assert_eq!(adapter.tools().len(), 1);
        assert!(adapter.system_prompt.contains("fact_check"));
        assert!(adapter.session_active());
    }

    #[test]
    fn system_prompt_includes_template() {
        let adapter = MicrosoftAdapter::new(test_config());
        let prompt = adapter.build_system_prompt();
        assert!(prompt.contains("Microsoft Agent Framework"));
        assert!(prompt.contains("Analyze the task carefully"));
    }

    #[test]
    fn system_prompt_with_tools_and_instructions() {
        let mut adapter = MicrosoftAdapter::new(
            test_config().with_instructions("Focus on accuracy"),
        );
        adapter.tools.push(AgentTool::new("memory", "Memory tool"));
        let prompt = adapter.build_system_prompt();
        assert!(prompt.contains("**memory**"));
        assert!(prompt.contains("Focus on accuracy"));
    }

    #[tokio::test]
    async fn run_agent_success() {
        let mut adapter = MicrosoftAdapter::new(test_config())
            .with_client(MockClient::ok("Microsoft response"));
        adapter.create_agent().unwrap();

        let result = adapter.run_agent("test task", 10).await.unwrap();
        assert!(result.goal_achieved);
        assert_eq!(result.response, "Microsoft response");
    }

    #[tokio::test]
    async fn run_agent_with_tool_calls() {
        let tools = vec!["search".into(), "calc".into()];
        let mut adapter = MicrosoftAdapter::new(test_config())
            .with_client(MockClient::with_tools("result", tools));
        adapter.create_agent().unwrap();

        let result = adapter.run_agent("task", 5).await.unwrap();
        assert_eq!(result.tools_used, vec!["search", "calc"]);
    }

    #[tokio::test]
    async fn run_agent_not_initialized() {
        let mut adapter = MicrosoftAdapter::new(test_config())
            .with_client(MockClient::ok("x"));
        // Don't call create_agent!
        let err = adapter.run_agent("task", 5).await.unwrap_err();
        assert!(matches!(err, AgentError::ConfigError(_)));
    }

    #[tokio::test]
    async fn run_agent_no_client() {
        let mut adapter = MicrosoftAdapter::new(test_config());
        adapter.create_agent().unwrap();

        let err = adapter.run_agent("task", 5).await.unwrap_err();
        assert!(matches!(err, AgentError::ConfigError(_)));
    }

    #[tokio::test]
    async fn run_agent_propagates_error() {
        let mut adapter = MicrosoftAdapter::new(test_config())
            .with_client(MockClient::failing());
        adapter.create_agent().unwrap();

        let err = adapter.run_agent("task", 5).await.unwrap_err();
        assert!(matches!(err, AgentError::TaskFailed(_)));
    }

    #[tokio::test]
    async fn close_clears_state() {
        let mut adapter = MicrosoftAdapter::new(test_config())
            .with_client(MockClient::ok("x"));
        adapter.create_agent().unwrap();
        adapter.close().await.unwrap();
        assert!(adapter.client.is_none());
        assert!(!adapter.session_active());
    }

    #[test]
    fn config_accessor() {
        let adapter = MicrosoftAdapter::new(test_config());
        assert_eq!(adapter.config().name, "test-ms");
        assert_eq!(adapter.config().sdk_type, SdkType::Microsoft);
    }
}
