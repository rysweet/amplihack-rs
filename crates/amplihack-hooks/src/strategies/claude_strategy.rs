//! Claude Code hook strategy.
//!
//! Injects context via the `hookSpecificOutput.additionalContext` JSON path,
//! which is the mechanism Claude Code uses to read hook-provided context.

use super::base::HookStrategy;
use anyhow::Result;
use serde_json::Value;
use std::collections::HashMap;

/// Hook strategy for the Claude Code launcher.
pub struct ClaudeStrategy;

impl HookStrategy for ClaudeStrategy {
    /// Inject context via `hookSpecificOutput.additionalContext`.
    fn inject_context(&self, context: &str) -> Result<HashMap<String, Value>> {
        let mut map = HashMap::new();
        map.insert(
            "hookSpecificOutput".to_string(),
            serde_json::json!({
                "additionalContext": context
            }),
        );
        Ok(map)
    }

    /// Claude Code does not support power-steering via subprocess.
    ///
    /// # Panics
    ///
    /// Always panics — Claude Code sessions are steered through the hook
    /// protocol, not through an external CLI command.
    fn power_steer(&self, _prompt: &str, _session_id: &str) -> Result<bool> {
        panic!("ClaudeStrategy does not support power_steer");
    }

    fn get_launcher_name(&self) -> &'static str {
        "claude"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inject_context_structure() {
        let strategy = ClaudeStrategy;
        let map = strategy.inject_context("test context").unwrap();

        let output = &map["hookSpecificOutput"];
        assert_eq!(output["additionalContext"], "test context");
    }

    #[test]
    fn inject_context_preserves_content() {
        let strategy = ClaudeStrategy;
        let long_ctx = "a".repeat(5000);
        let map = strategy.inject_context(&long_ctx).unwrap();
        assert_eq!(map["hookSpecificOutput"]["additionalContext"], long_ctx);
    }

    #[test]
    fn launcher_name() {
        let strategy = ClaudeStrategy;
        assert_eq!(strategy.get_launcher_name(), "claude");
    }

    #[test]
    #[should_panic(expected = "ClaudeStrategy does not support power_steer")]
    fn power_steer_panics() {
        let strategy = ClaudeStrategy;
        let _ = strategy.power_steer("prompt", "session-1");
    }

    #[test]
    fn inject_context_empty() {
        let strategy = ClaudeStrategy;
        let map = strategy.inject_context("").unwrap();
        assert_eq!(map["hookSpecificOutput"]["additionalContext"], "");
    }
}
