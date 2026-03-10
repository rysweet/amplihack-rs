//! Hook input/output types — host-agnostic (Claude Code, Amplifier, Copilot).
//!
//! These types model the JSON protocol that hook hosts use to communicate
//! with hook binaries via stdin/stdout.

use serde::Deserialize;
use serde_json::Value;
use std::path::PathBuf;

/// Top-level input from the hook host.
///
/// Uses `#[serde(other)]` for forward-compatibility: unknown hook events
/// deserialize to `Unknown` instead of failing.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "hook_event_name")]
pub enum HookInput {
    /// Pre-tool-use: decide whether to allow/deny a tool invocation.
    #[serde(rename = "PreToolUse")]
    PreToolUse {
        tool_name: String,
        tool_input: Value,
        #[serde(default)]
        session_id: Option<String>,
    },

    /// Post-tool-use: observe tool results for metrics/validation.
    #[serde(rename = "PostToolUse")]
    PostToolUse {
        tool_name: String,
        tool_input: Value,
        #[serde(default)]
        tool_result: Option<Value>,
        #[serde(default)]
        session_id: Option<String>,
    },

    /// Stop: session is ending, decide whether to block or allow.
    #[serde(rename = "Stop")]
    Stop {
        #[serde(default)]
        stop_hook_active: Option<bool>,
        #[serde(default)]
        transcript_path: Option<PathBuf>,
        #[serde(default)]
        session_id: Option<String>,
    },

    /// Session start: initialize session state.
    #[serde(rename = "SessionStart")]
    SessionStart {
        #[serde(default)]
        session_id: Option<String>,
        #[serde(default)]
        cwd: Option<PathBuf>,
        #[serde(flatten)]
        extra: Value,
    },

    /// Session stop: finalize session state.
    #[serde(rename = "SessionStop")]
    SessionStop {
        #[serde(default)]
        session_id: Option<String>,
        #[serde(default)]
        transcript_path: Option<PathBuf>,
        #[serde(flatten)]
        extra: Value,
    },

    /// User prompt submission: process user prompt before LLM call.
    #[serde(rename = "UserPromptSubmit")]
    UserPromptSubmit {
        #[serde(default)]
        user_prompt: Option<String>,
        #[serde(default)]
        session_id: Option<String>,
        #[serde(flatten)]
        extra: Value,
    },

    /// Pre-compact: context window is about to be compacted.
    #[serde(rename = "PreCompact")]
    PreCompact {
        #[serde(default)]
        session_id: Option<String>,
        #[serde(default)]
        transcript_path: Option<PathBuf>,
        #[serde(flatten)]
        extra: Value,
    },

    /// Unknown hook event — forward-compatibility.
    /// New hook events from the host deserialize here instead of failing.
    #[serde(other)]
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_pre_tool_use() {
        let json = r#"{
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": "ls -la"}
        }"#;
        let input: HookInput = serde_json::from_str(json).unwrap();
        assert!(matches!(input, HookInput::PreToolUse { tool_name, .. } if tool_name == "Bash"));
    }

    #[test]
    fn deserialize_unknown_event() {
        let json = r#"{"hook_event_name": "FutureEvent", "data": "test"}"#;
        let input: HookInput = serde_json::from_str(json).unwrap();
        assert!(matches!(input, HookInput::Unknown));
    }

    #[test]
    fn deserialize_stop_with_defaults() {
        let json = r#"{"hook_event_name": "Stop"}"#;
        let input: HookInput = serde_json::from_str(json).unwrap();
        assert!(matches!(
            input,
            HookInput::Stop {
                stop_hook_active: None,
                transcript_path: None,
                session_id: None,
            }
        ));
    }

    #[test]
    fn deserialize_pre_tool_use_with_extra_fields() {
        let json = r#"{
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": "ls"},
            "future_field": "should be ignored"
        }"#;
        let input: HookInput = serde_json::from_str(json).unwrap();
        assert!(matches!(input, HookInput::PreToolUse { .. }));
    }
}
