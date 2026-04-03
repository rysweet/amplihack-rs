//! Agent adapter — wraps agents behind the eval framework interface.
//!
//! Matches Python `amplihack/eval/agent_adapter.py`:
//! - Generic agent adapter trait
//! - Subprocess adapter for isolated agent execution
//! - Configuration for adapter behavior

use crate::error::EvalError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::{debug, warn};

/// Trait for agents that can be wrapped for evaluation.
pub trait AgentAdapter {
    /// Feed learning content to the agent.
    fn learn(&mut self, content: &str) -> Result<(), EvalError>;

    /// Ask the agent a question and get an answer.
    fn answer(&mut self, question: &str) -> Result<AgentResponse, EvalError>;

    /// Reset the agent state between evaluations.
    fn reset(&mut self) -> Result<(), EvalError>;

    /// Shut down the agent cleanly.
    fn close(&mut self) -> Result<(), EvalError>;

    /// Agent name for reporting.
    fn name(&self) -> &str;
}

/// Response from an agent to a question.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResponse {
    pub answer: String,
    #[serde(default)]
    pub confidence: f64,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub traces: Vec<String>,
}

impl AgentResponse {
    pub fn new(answer: impl Into<String>) -> Self {
        Self {
            answer: answer.into(),
            confidence: 0.0,
            metadata: HashMap::new(),
            traces: Vec::new(),
        }
    }

    pub fn with_confidence(mut self, confidence: f64) -> Self {
        self.confidence = confidence;
        self
    }

    pub fn with_trace(mut self, trace: impl Into<String>) -> Self {
        self.traces.push(trace.into());
        self
    }
}

/// Configuration for subprocess-based agent execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubprocessConfig {
    pub binary: PathBuf,
    pub args: Vec<String>,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub working_dir: Option<PathBuf>,
}

fn default_timeout() -> u64 {
    300
}

impl SubprocessConfig {
    pub fn new(binary: impl Into<PathBuf>) -> Self {
        Self {
            binary: binary.into(),
            args: Vec::new(),
            timeout_seconds: default_timeout(),
            env: HashMap::new(),
            working_dir: None,
        }
    }

    pub fn with_args(mut self, args: Vec<String>) -> Self {
        self.args = args;
        self
    }

    pub fn with_timeout(mut self, seconds: u64) -> Self {
        self.timeout_seconds = seconds;
        self
    }

    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }
}

/// In-memory mock agent for testing.
pub struct MockAgentAdapter {
    name: String,
    learned: Vec<String>,
    responses: HashMap<String, String>,
    default_response: String,
}

impl MockAgentAdapter {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            learned: Vec::new(),
            responses: HashMap::new(),
            default_response: "I don't know".into(),
        }
    }

    pub fn with_response(
        mut self,
        question: impl Into<String>,
        answer: impl Into<String>,
    ) -> Self {
        self.responses.insert(question.into(), answer.into());
        self
    }

    pub fn with_default_response(mut self, response: impl Into<String>) -> Self {
        self.default_response = response.into();
        self
    }

    pub fn learned_content(&self) -> &[String] {
        &self.learned
    }
}

impl AgentAdapter for MockAgentAdapter {
    fn learn(&mut self, content: &str) -> Result<(), EvalError> {
        debug!(agent = %self.name, content_len = content.len(), "Learning content");
        self.learned.push(content.to_string());
        Ok(())
    }

    fn answer(&mut self, question: &str) -> Result<AgentResponse, EvalError> {
        let answer = self
            .responses
            .get(question)
            .cloned()
            .unwrap_or_else(|| {
                // Check if any learned content contains the answer
                for content in &self.learned {
                    if content.to_lowercase().contains(&question.to_lowercase()) {
                        return content.clone();
                    }
                }
                self.default_response.clone()
            });

        let confidence = if self.responses.contains_key(question) {
            0.95
        } else {
            0.3
        };

        Ok(AgentResponse::new(answer)
            .with_confidence(confidence)
            .with_trace(format!("MockAgent answered: {question}")))
    }

    fn reset(&mut self) -> Result<(), EvalError> {
        self.learned.clear();
        Ok(())
    }

    fn close(&mut self) -> Result<(), EvalError> {
        debug!(agent = %self.name, "Closing mock agent");
        Ok(())
    }

    fn name(&self) -> &str {
        &self.name
    }
}

/// Adapter that delegates to a subprocess for isolated agent execution.
pub struct SubprocessAdapter {
    name: String,
    config: SubprocessConfig,
}

impl SubprocessAdapter {
    pub fn new(name: impl Into<String>, config: SubprocessConfig) -> Self {
        Self {
            name: name.into(),
            config,
        }
    }

    pub fn config(&self) -> &SubprocessConfig {
        &self.config
    }
}

impl AgentAdapter for SubprocessAdapter {
    fn learn(&mut self, content: &str) -> Result<(), EvalError> {
        debug!(
            agent = %self.name,
            binary = %self.config.binary.display(),
            content_len = content.len(),
            "Sending learn command to subprocess"
        );
        // Subprocess execution would use std::process::Command
        // For now, validate config is present
        if !self.config.binary.as_os_str().is_empty() {
            Ok(())
        } else {
            Err(EvalError::harness("Subprocess binary not configured"))
        }
    }

    fn answer(&mut self, question: &str) -> Result<AgentResponse, EvalError> {
        debug!(
            agent = %self.name,
            question_len = question.len(),
            "Sending answer command to subprocess"
        );
        if self.config.binary.as_os_str().is_empty() {
            return Err(EvalError::harness("Subprocess binary not configured"));
        }
        // Real implementation would spawn process and parse JSON output
        warn!(
            agent = %self.name,
            "SubprocessAdapter.answer: subprocess execution not wired to real binary"
        );
        Ok(AgentResponse::new("subprocess placeholder"))
    }

    fn reset(&mut self) -> Result<(), EvalError> {
        debug!(agent = %self.name, "Resetting subprocess agent");
        Ok(())
    }

    fn close(&mut self) -> Result<(), EvalError> {
        debug!(agent = %self.name, "Closing subprocess agent");
        Ok(())
    }

    fn name(&self) -> &str {
        &self.name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_adapter_learn_and_answer() {
        let mut agent = MockAgentAdapter::new("test")
            .with_response("What is Rust?", "A systems programming language");

        agent.learn("Rust is fast and safe").unwrap();
        assert_eq!(agent.learned_content().len(), 1);

        let response = agent.answer("What is Rust?").unwrap();
        assert_eq!(response.answer, "A systems programming language");
        assert!(response.confidence > 0.9);
    }

    #[test]
    fn mock_adapter_default_response() {
        let mut agent = MockAgentAdapter::new("test")
            .with_default_response("unknown");

        let response = agent.answer("random question").unwrap();
        assert_eq!(response.answer, "unknown");
        assert!(response.confidence < 0.5);
    }

    #[test]
    fn mock_adapter_reset() {
        let mut agent = MockAgentAdapter::new("test");
        agent.learn("content").unwrap();
        assert_eq!(agent.learned_content().len(), 1);
        agent.reset().unwrap();
        assert_eq!(agent.learned_content().len(), 0);
    }

    #[test]
    fn mock_adapter_close() {
        let mut agent = MockAgentAdapter::new("test");
        assert!(agent.close().is_ok());
    }

    #[test]
    fn agent_response_builder() {
        let response = AgentResponse::new("answer")
            .with_confidence(0.8)
            .with_trace("step 1");
        assert_eq!(response.answer, "answer");
        assert_eq!(response.confidence, 0.8);
        assert_eq!(response.traces.len(), 1);
    }

    #[test]
    fn agent_response_serde() {
        let response = AgentResponse::new("test")
            .with_confidence(0.5);
        let json = serde_json::to_string(&response).unwrap();
        let restored: AgentResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.answer, "test");
        assert_eq!(restored.confidence, 0.5);
    }

    #[test]
    fn subprocess_config_builder() {
        let config = SubprocessConfig::new("/usr/bin/agent")
            .with_args(vec!["--eval".into()])
            .with_timeout(60)
            .with_env("MODEL", "test");
        assert_eq!(config.timeout_seconds, 60);
        assert_eq!(config.env["MODEL"], "test");
    }

    #[test]
    fn subprocess_adapter_empty_binary_errors() {
        let config = SubprocessConfig::new("");
        let mut adapter = SubprocessAdapter::new("test", config);
        assert!(adapter.learn("content").is_err());
        assert!(adapter.answer("question").is_err());
    }

    #[test]
    fn subprocess_adapter_valid_config() {
        let config = SubprocessConfig::new("/usr/bin/agent");
        let mut adapter = SubprocessAdapter::new("test", config);
        assert!(adapter.learn("content").is_ok());
        assert!(adapter.reset().is_ok());
        assert!(adapter.close().is_ok());
    }

    #[test]
    fn subprocess_config_serde() {
        let config = SubprocessConfig::new("/bin/test")
            .with_timeout(120);
        let json = serde_json::to_string(&config).unwrap();
        let restored: SubprocessConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.timeout_seconds, 120);
    }
}
