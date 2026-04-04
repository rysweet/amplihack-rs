//! Canonical runtime factory for benchmark-compatible goal-seeking agents.
//!
//! Port of Python `runtime_factory.py`. Provides:
//! - [`GoalAgentRuntime`] trait — unified runtime surface
//! - [`ConfiguredGoalAgentRuntime`] — wrapper binding answer-mode config
//! - [`create_goal_agent_runtime`] — factory function
//!
//! The Rust port uses trait objects instead of Python Protocols and keeps
//! observability hooks as tracing spans (rather than OpenTelemetry directly).

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tracing::instrument;

use crate::error::{AgentError, Result};

// ── GoalAgentRuntime trait ───────────────────────────────────────────────

/// Unified runtime surface shared across eval and Azure entrypoints.
pub trait GoalAgentRuntime: Send {
    /// Learn from input content.
    fn learn_from_content(&mut self, content: &str) -> Result<HashMap<String, serde_json::Value>>;

    /// Answer a question using the configured answer mode.
    fn answer_question(&mut self, question: &str) -> Result<String>;

    /// Prepare facts from content for batch processing.
    fn prepare_fact_batch(
        &mut self,
        content: &str,
        include_summary: bool,
    ) -> Result<HashMap<String, serde_json::Value>>;

    /// Store a pre-prepared fact batch.
    fn store_fact_batch(
        &mut self,
        batch: &HashMap<String, serde_json::Value>,
    ) -> Result<HashMap<String, serde_json::Value>>;

    /// Get memory statistics.
    fn get_memory_stats(&self) -> HashMap<String, serde_json::Value>;

    /// Flush memory caches.
    fn flush_memory(&mut self);

    /// Close and release resources.
    fn close(&mut self);
}

// ── RuntimeConfig ────────────────────────────────────────────────────────

/// Configuration for [`create_goal_agent_runtime`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeConfig {
    pub agent_name: String,
    #[serde(default = "default_sdk")]
    pub sdk: String,
    pub model: Option<String>,
    pub storage_path: Option<PathBuf>,
    #[serde(default)]
    pub use_hierarchical: bool,
    #[serde(default = "default_memory_type")]
    pub memory_type: String,
    #[serde(default = "default_answer_mode")]
    pub answer_mode: String,
    #[serde(default = "default_true")]
    pub bind_answer_mode: bool,
    #[serde(default = "default_true")]
    pub enable_memory: bool,
    #[serde(default)]
    pub enable_eval: bool,
    pub runtime_kind: Option<String>,
}

fn default_sdk() -> String {
    "mini".into()
}
fn default_memory_type() -> String {
    "auto".into()
}
fn default_answer_mode() -> String {
    "single-shot".into()
}
fn default_true() -> bool {
    true
}

impl RuntimeConfig {
    /// Create a minimal runtime config for the given agent.
    pub fn new(agent_name: impl Into<String>) -> Self {
        Self {
            agent_name: agent_name.into(),
            sdk: default_sdk(),
            model: None,
            storage_path: None,
            use_hierarchical: false,
            memory_type: default_memory_type(),
            answer_mode: default_answer_mode(),
            bind_answer_mode: true,
            enable_memory: true,
            enable_eval: false,
            runtime_kind: None,
        }
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    pub fn with_answer_mode(mut self, mode: impl Into<String>) -> Self {
        self.answer_mode = mode.into();
        self
    }

    pub fn with_storage_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.storage_path = Some(path.into());
        self
    }
}

// ── ConfiguredGoalAgentRuntime ───────────────────────────────────────────

/// Wrapper that binds answer-mode configuration around a runtime backend.
pub struct ConfiguredGoalAgentRuntime {
    runtime: Box<dyn GoalAgentRuntime>,
    answer_mode: String,
}

impl ConfiguredGoalAgentRuntime {
    /// Wrap a runtime with the specified answer mode.
    pub fn new(runtime: Box<dyn GoalAgentRuntime>, answer_mode: impl Into<String>) -> Self {
        Self {
            runtime,
            answer_mode: answer_mode.into(),
        }
    }

    /// The configured answer mode.
    pub fn answer_mode(&self) -> &str {
        &self.answer_mode
    }

    #[instrument(skip(self, content), fields(content_len = content.len()))]
    pub fn learn_from_content(
        &mut self,
        content: &str,
    ) -> Result<HashMap<String, serde_json::Value>> {
        self.runtime.learn_from_content(content)
    }

    #[instrument(skip(self, question), fields(question_len = question.len()))]
    pub fn answer_question(&mut self, question: &str) -> Result<String> {
        self.runtime.answer_question(question)
    }

    #[instrument(skip(self, content), fields(content_len = content.len()))]
    pub fn prepare_fact_batch(
        &mut self,
        content: &str,
        include_summary: bool,
    ) -> Result<HashMap<String, serde_json::Value>> {
        self.runtime.prepare_fact_batch(content, include_summary)
    }

    #[instrument(skip(self, batch))]
    pub fn store_fact_batch(
        &mut self,
        batch: &HashMap<String, serde_json::Value>,
    ) -> Result<HashMap<String, serde_json::Value>> {
        self.runtime.store_fact_batch(batch)
    }

    pub fn get_memory_stats(&self) -> HashMap<String, serde_json::Value> {
        self.runtime.get_memory_stats()
    }

    pub fn flush_memory(&mut self) {
        self.runtime.flush_memory();
    }

    pub fn close(&mut self) {
        self.runtime.close();
    }
}

// ── Factory function ─────────────────────────────────────────────────────

/// Create the canonical runtime used by eval and Azure surfaces.
///
/// This is a stub factory — in production the Python version dispatches to
/// `GoalSeekingAgent` or an SDK adapter. The Rust port returns a
/// [`ConfiguredGoalAgentRuntime`] wrapping a `StubRuntime` (or a provided
/// implementation via [`create_goal_agent_runtime_with`]).
pub fn create_goal_agent_runtime(config: &RuntimeConfig) -> Result<ConfiguredGoalAgentRuntime> {
    if config.agent_name.trim().is_empty() {
        return Err(AgentError::ConfigError("agent_name cannot be empty".into()));
    }
    let runtime = Box::new(StubRuntime {
        agent_name: config.agent_name.clone(),
        sdk: config.sdk.clone(),
    });
    Ok(ConfiguredGoalAgentRuntime::new(
        runtime,
        &config.answer_mode,
    ))
}

/// Create a configured runtime wrapping a caller-provided implementation.
pub fn create_goal_agent_runtime_with(
    runtime: Box<dyn GoalAgentRuntime>,
    answer_mode: &str,
) -> ConfiguredGoalAgentRuntime {
    ConfiguredGoalAgentRuntime::new(runtime, answer_mode)
}

// ── StubRuntime (placeholder) ────────────────────────────────────────────

/// Minimal stub runtime for testing / when no real backend is configured.
struct StubRuntime {
    agent_name: String,
    sdk: String,
}

impl GoalAgentRuntime for StubRuntime {
    fn learn_from_content(&mut self, _content: &str) -> Result<HashMap<String, serde_json::Value>> {
        let mut m = HashMap::new();
        m.insert("status".into(), serde_json::json!("stub"));
        Ok(m)
    }

    fn answer_question(&mut self, _question: &str) -> Result<String> {
        Ok("stub answer".into())
    }

    fn prepare_fact_batch(
        &mut self,
        _content: &str,
        _include_summary: bool,
    ) -> Result<HashMap<String, serde_json::Value>> {
        let mut m = HashMap::new();
        m.insert("facts".into(), serde_json::json!([]));
        Ok(m)
    }

    fn store_fact_batch(
        &mut self,
        _batch: &HashMap<String, serde_json::Value>,
    ) -> Result<HashMap<String, serde_json::Value>> {
        let mut m = HashMap::new();
        m.insert("stored".into(), serde_json::json!(0));
        Ok(m)
    }

    fn get_memory_stats(&self) -> HashMap<String, serde_json::Value> {
        let mut m = HashMap::new();
        m.insert("agent_name".into(), serde_json::json!(self.agent_name));
        m.insert("sdk".into(), serde_json::json!(self.sdk));
        m
    }

    fn flush_memory(&mut self) {}
    fn close(&mut self) {}
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_builder() {
        let cfg = RuntimeConfig::new("test-agent")
            .with_model("gpt-4")
            .with_answer_mode("multi-hop")
            .with_storage_path("/data");
        assert_eq!(cfg.agent_name, "test-agent");
        assert_eq!(cfg.model.as_deref(), Some("gpt-4"));
        assert_eq!(cfg.answer_mode, "multi-hop");
    }

    #[test]
    fn config_defaults() {
        let cfg = RuntimeConfig::new("x");
        assert_eq!(cfg.sdk, "mini");
        assert_eq!(cfg.memory_type, "auto");
        assert_eq!(cfg.answer_mode, "single-shot");
        assert!(cfg.bind_answer_mode);
        assert!(cfg.enable_memory);
        assert!(!cfg.enable_eval);
    }

    #[test]
    fn create_runtime_stub() {
        let cfg = RuntimeConfig::new("test-agent");
        let mut rt = create_goal_agent_runtime(&cfg).unwrap();
        assert_eq!(rt.answer_mode(), "single-shot");

        let answer = rt.answer_question("test?").unwrap();
        assert_eq!(answer, "stub answer");

        let stats = rt.get_memory_stats();
        assert_eq!(stats["agent_name"], serde_json::json!("test-agent"));
    }

    #[test]
    fn create_runtime_empty_name_fails() {
        let cfg = RuntimeConfig::new("");
        assert!(create_goal_agent_runtime(&cfg).is_err());
    }

    #[test]
    fn learn_and_batch() {
        let cfg = RuntimeConfig::new("test-agent");
        let mut rt = create_goal_agent_runtime(&cfg).unwrap();

        let learn = rt.learn_from_content("some content").unwrap();
        assert_eq!(learn["status"], serde_json::json!("stub"));

        let batch = rt.prepare_fact_batch("content", true).unwrap();
        assert!(batch.contains_key("facts"));
    }

    #[test]
    fn custom_runtime() {
        struct Custom;
        impl GoalAgentRuntime for Custom {
            fn learn_from_content(
                &mut self,
                _: &str,
            ) -> Result<HashMap<String, serde_json::Value>> {
                Ok(HashMap::new())
            }
            fn answer_question(&mut self, _: &str) -> Result<String> {
                Ok("custom".into())
            }
            fn prepare_fact_batch(
                &mut self,
                _: &str,
                _: bool,
            ) -> Result<HashMap<String, serde_json::Value>> {
                Ok(HashMap::new())
            }
            fn store_fact_batch(
                &mut self,
                _: &HashMap<String, serde_json::Value>,
            ) -> Result<HashMap<String, serde_json::Value>> {
                Ok(HashMap::new())
            }
            fn get_memory_stats(&self) -> HashMap<String, serde_json::Value> {
                HashMap::new()
            }
            fn flush_memory(&mut self) {}
            fn close(&mut self) {}
        }

        let mut rt = create_goal_agent_runtime_with(Box::new(Custom), "multi-hop");
        assert_eq!(rt.answer_mode(), "multi-hop");
        assert_eq!(rt.answer_question("?").unwrap(), "custom");
    }

    #[test]
    fn config_serde_roundtrip() {
        let cfg = RuntimeConfig::new("agent").with_model("gpt-4");
        let json = serde_json::to_string(&cfg).unwrap();
        let parsed: RuntimeConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.agent_name, "agent");
        assert_eq!(parsed.model.as_deref(), Some("gpt-4"));
    }
}
