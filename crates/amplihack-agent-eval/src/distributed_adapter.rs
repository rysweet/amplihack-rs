//! Distributed evaluation coordination.
//!
//! Ports Python `amplihack/evaluation/distributed_adapter.py`.
//!
//! Provides a [`RemoteAgentAdapter`] that delegates agent calls to a remote
//! evaluation service (e.g. an Azure-hosted endpoint). This keeps the
//! evaluator process light while heavy agent inference runs elsewhere.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::debug;

use crate::agent_adapter::{AgentAdapter, AgentResponse};
use crate::error::EvalError;

/// Configuration for a remote evaluation endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteEndpointConfig {
    /// Base URL of the remote agent service.
    pub base_url: String,
    /// Optional bearer token for authentication.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_token: Option<String>,
    /// Request timeout in seconds.
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
    /// Extra headers sent with every request.
    #[serde(default)]
    pub headers: HashMap<String, String>,
}

fn default_timeout() -> u64 {
    120
}

impl RemoteEndpointConfig {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            auth_token: None,
            timeout_seconds: default_timeout(),
            headers: HashMap::new(),
        }
    }

    pub fn with_auth(mut self, token: impl Into<String>) -> Self {
        self.auth_token = Some(token.into());
        self
    }

    pub fn with_timeout(mut self, seconds: u64) -> Self {
        self.timeout_seconds = seconds;
        self
    }

    pub fn with_header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(key.into(), value.into());
        self
    }

    pub fn validate(&self) -> Result<(), EvalError> {
        if self.base_url.is_empty() {
            return Err(EvalError::config("base_url must not be empty"));
        }
        if self.timeout_seconds == 0 {
            return Err(EvalError::config("timeout must be > 0"));
        }
        Ok(())
    }
}

/// Agent adapter that delegates to a remote evaluation service.
///
/// In a full implementation, each method would make HTTP calls to the remote
/// endpoint. Here we validate the config and record intent; the HTTP layer is
/// deferred to the runtime integration crate.
pub struct RemoteAgentAdapter {
    name: String,
    config: RemoteEndpointConfig,
    session_id: Option<String>,
}

impl RemoteAgentAdapter {
    pub fn new(name: impl Into<String>, config: RemoteEndpointConfig) -> Result<Self, EvalError> {
        config.validate()?;
        Ok(Self {
            name: name.into(),
            config,
            session_id: None,
        })
    }

    pub fn config(&self) -> &RemoteEndpointConfig {
        &self.config
    }

    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// Set the remote session identifier (assigned by the remote service).
    pub fn set_session_id(&mut self, id: impl Into<String>) {
        self.session_id = Some(id.into());
    }
}

impl AgentAdapter for RemoteAgentAdapter {
    fn learn(&mut self, content: &str) -> Result<(), EvalError> {
        debug!(
            agent = %self.name,
            endpoint = %self.config.base_url,
            content_len = content.len(),
            "Remote learn request"
        );
        // HTTP POST would go here
        Ok(())
    }

    fn answer(&mut self, question: &str) -> Result<AgentResponse, EvalError> {
        debug!(
            agent = %self.name,
            endpoint = %self.config.base_url,
            question_len = question.len(),
            "Remote answer request"
        );
        // HTTP POST would go here
        Ok(AgentResponse::new("remote placeholder"))
    }

    fn reset(&mut self) -> Result<(), EvalError> {
        debug!(agent = %self.name, "Remote reset");
        self.session_id = None;
        Ok(())
    }

    fn close(&mut self) -> Result<(), EvalError> {
        debug!(agent = %self.name, "Remote close");
        self.session_id = None;
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
    fn config_builder() {
        let c = RemoteEndpointConfig::new("https://eval.example.com")
            .with_auth("tok123")
            .with_timeout(60)
            .with_header("X-Custom", "val");
        assert_eq!(c.base_url, "https://eval.example.com");
        assert_eq!(c.auth_token.as_deref(), Some("tok123"));
        assert_eq!(c.timeout_seconds, 60);
        assert_eq!(c.headers["X-Custom"], "val");
    }

    #[test]
    fn config_validate_empty_url() {
        let c = RemoteEndpointConfig::new("");
        assert!(c.validate().is_err());
    }

    #[test]
    fn config_validate_zero_timeout() {
        let c = RemoteEndpointConfig::new("http://x").with_timeout(0);
        assert!(c.validate().is_err());
    }

    #[test]
    fn config_validate_ok() {
        let c = RemoteEndpointConfig::new("http://x");
        assert!(c.validate().is_ok());
    }

    #[test]
    fn adapter_creation_validates() {
        let c = RemoteEndpointConfig::new("");
        assert!(RemoteAgentAdapter::new("a", c).is_err());

        let c = RemoteEndpointConfig::new("http://x");
        assert!(RemoteAgentAdapter::new("a", c).is_ok());
    }

    #[test]
    fn adapter_learn_answer_reset_close() {
        let c = RemoteEndpointConfig::new("http://x");
        let mut adapter = RemoteAgentAdapter::new("test", c).unwrap();
        assert!(adapter.learn("content").is_ok());
        let resp = adapter.answer("question").unwrap();
        assert!(!resp.answer.is_empty());
        assert!(adapter.reset().is_ok());
        assert!(adapter.close().is_ok());
    }

    #[test]
    fn adapter_name() {
        let c = RemoteEndpointConfig::new("http://x");
        let adapter = RemoteAgentAdapter::new("my-agent", c).unwrap();
        assert_eq!(adapter.name(), "my-agent");
    }

    #[test]
    fn adapter_session_id() {
        let c = RemoteEndpointConfig::new("http://x");
        let mut adapter = RemoteAgentAdapter::new("a", c).unwrap();
        assert!(adapter.session_id().is_none());
        adapter.set_session_id("sess-1");
        assert_eq!(adapter.session_id(), Some("sess-1"));
        adapter.reset().unwrap();
        assert!(adapter.session_id().is_none());
    }

    #[test]
    fn config_serde_roundtrip() {
        let c = RemoteEndpointConfig::new("http://x")
            .with_auth("tok")
            .with_timeout(30);
        let json = serde_json::to_string(&c).unwrap();
        let restored: RemoteEndpointConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.base_url, "http://x");
        assert_eq!(restored.timeout_seconds, 30);
    }

    #[test]
    fn config_serde_no_auth() {
        let c = RemoteEndpointConfig::new("http://x");
        let json = serde_json::to_string(&c).unwrap();
        assert!(!json.contains("auth_token"));
    }
}
