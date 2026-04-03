//! Trait abstractions for the agentic loop.
//!
//! Defines the three dependency seams that [`AgenticLoop`](super::AgenticLoop)
//! relies on.  Concrete implementations live elsewhere; tests use simple mocks.

use std::collections::HashMap;

use async_trait::async_trait;
use serde_json::Value;

use crate::error::AgentError;

use super::types::{ActionResult, LlmMessage, MemoryFact};

// ---------------------------------------------------------------------------
// LlmClient
// ---------------------------------------------------------------------------

/// Abstraction over an LLM completion API.
///
/// Callers send a list of messages and receive a text response.
/// Concrete implementations (OpenAI, Anthropic, …) are provided outside
/// this crate.
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// Call the LLM and return the completion text.
    async fn completion(
        &self,
        messages: &[LlmMessage],
        model: &str,
        temperature: f64,
    ) -> Result<String, AgentError>;
}

// ---------------------------------------------------------------------------
// ActionExecutor
// ---------------------------------------------------------------------------

/// Executes named actions on behalf of the agent.
pub trait ActionExecutor: Send + Sync {
    /// List available action names.
    fn available_actions(&self) -> Vec<String>;

    /// Execute `action_name` with the given params.
    fn execute(
        &self,
        action_name: &str,
        params: &HashMap<String, Value>,
    ) -> ActionResult;
}

// ---------------------------------------------------------------------------
// MemoryRetriever
// ---------------------------------------------------------------------------

/// Read/write access to the agent's memory store.
pub trait MemoryRetriever: Send + Sync {
    /// Keyword search returning up to `limit` results.
    fn search(&self, query: &str, limit: usize) -> Vec<MemoryFact>;

    /// Store a fact (from the LEARN phase).
    fn store_fact(
        &self,
        context: &str,
        fact: &str,
        confidence: f64,
        tags: &[String],
    );
}

// ---------------------------------------------------------------------------
// MemoryFacade — optional higher-level memory API
// ---------------------------------------------------------------------------

/// Optional Memory facade (mirrors Python `amplihack.memory.facade`).
///
/// When set on [`AgenticLoop`](super::AgenticLoop) the loop uses `remember` /
/// `recall` instead of the lower-level [`MemoryRetriever`].
pub trait MemoryFacade: Send + Sync {
    /// Store content in memory.
    fn remember(&self, content: &str);

    /// Recall relevant items for `query`, returning up to `limit` strings.
    fn recall(&self, query: &str, limit: usize) -> Vec<String>;

    /// Retrieve facts as [`MemoryFact`] items (for iterative reasoning).
    fn retrieve_facts(&self, query: &str, max_nodes: usize) -> Vec<MemoryFact>;
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verify the traits are object-safe (can be used as `dyn Trait`).

    fn _assert_llm_client_object_safe(_: &dyn LlmClient) {}
    fn _assert_action_executor_object_safe(_: &dyn ActionExecutor) {}
    fn _assert_memory_retriever_object_safe(_: &dyn MemoryRetriever) {}
    fn _assert_memory_facade_object_safe(_: &dyn MemoryFacade) {}
}
