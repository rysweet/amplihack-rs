//! Autonomous execution engine — types and configuration.
//!
//! Matches Python `amplihack/launcher/auto_mode.py`:
//! - Multi-turn agentic loop configuration
//! - Prompt injection sanitization
//! - Session types and metrics

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Maximum size for injected content (50 KB).
pub const MAX_INJECTED_CONTENT_SIZE: usize = 50 * 1024;

/// Default maximum turns per session.
pub const DEFAULT_MAX_TURNS: u32 = 10;
/// Default session time limit in seconds (1 hour).
pub const DEFAULT_MAX_SESSION_SECS: u64 = 3600;
/// Default maximum API calls per session.
pub const DEFAULT_MAX_API_CALLS: u32 = 50;
/// Default maximum cumulative output (50 MB).
pub const DEFAULT_MAX_OUTPUT_BYTES: usize = 50 * 1024 * 1024;

/// Patterns that indicate prompt injection attempts.
const PROMPT_INJECTION_PATTERNS: &[&str] = &[
    r"ignore\s+previous\s+instructions",
    r"disregard\s+all\s+prior",
    r"forget\s+everything",
    r"new\s+instructions:",
    r"system\s+prompt:",
    r"you\s+are\s+now",
    r"override\s+all",
];

/// Which SDK backend to use for agent execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SdkBackend {
    Claude,
    Copilot,
    Codex,
}

impl std::fmt::Display for SdkBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Claude => write!(f, "claude"),
            Self::Copilot => write!(f, "copilot"),
            Self::Codex => write!(f, "codex"),
        }
    }
}

/// Configuration for an autonomous execution session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoModeConfig {
    pub sdk: SdkBackend,
    pub prompt: String,
    pub max_turns: u32,
    pub working_dir: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query_timeout_secs: Option<u64>,
    pub max_session_secs: u64,
    pub max_api_calls: u32,
    pub max_output_bytes: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
}

impl Default for AutoModeConfig {
    fn default() -> Self {
        Self {
            sdk: SdkBackend::Claude,
            prompt: String::new(),
            max_turns: DEFAULT_MAX_TURNS,
            working_dir: PathBuf::from("."),
            query_timeout_secs: None,
            max_session_secs: DEFAULT_MAX_SESSION_SECS,
            max_api_calls: DEFAULT_MAX_API_CALLS,
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
            task: None,
        }
    }
}

/// Per-turn execution result.
#[derive(Debug, Clone, Serialize)]
pub struct TurnResult {
    pub turn: u32,
    pub phase: String,
    pub exit_code: i32,
    pub output: String,
    pub duration_secs: f64,
}

/// Session-level execution result.
#[derive(Debug, Clone, Serialize)]
pub struct SessionResult {
    pub total_turns: u32,
    pub completed: bool,
    pub total_duration_secs: f64,
    pub turns: Vec<TurnResult>,
    pub total_api_calls: u32,
    pub total_output_bytes: usize,
}

/// Snapshot of session metrics.
#[derive(Debug, Clone, Serialize)]
pub struct SessionMetrics {
    pub turn: u32,
    pub elapsed_secs: f64,
    pub api_calls: u32,
    pub output_bytes: usize,
}

/// Sanitize injected content: truncate and remove prompt injection patterns.
pub fn sanitize_injected_content(content: &str) -> String {
    let truncated = if content.len() > MAX_INJECTED_CONTENT_SIZE {
        &content[..MAX_INJECTED_CONTENT_SIZE]
    } else {
        content
    };
    let mut result = truncated.to_string();
    for pattern in PROMPT_INJECTION_PATTERNS {
        if let Ok(re) = regex::Regex::new(&format!("(?i){pattern}")) {
            result = re.replace_all(&result, "[REDACTED]").to_string();
        }
    }
    result
}

/// Check if an error output indicates a transient/retryable failure.
pub fn is_retryable_output(output: &str) -> bool {
    let patterns = [
        "500 internal server error",
        "429 too many requests",
        "503 service unavailable",
        "timeout",
        "overloaded",
        "etimedout",
        "econnreset",
    ];
    let lower = output.to_lowercase();
    patterns.iter().any(|p| lower.contains(p))
}

/// Format seconds as a human-readable duration string.
pub fn format_elapsed(seconds: f64) -> String {
    let total_secs = seconds as u64;
    let mins = total_secs / 60;
    let secs = total_secs % 60;
    if mins > 0 {
        format!("{mins}m {secs}s")
    } else {
        format!("{secs}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdk_backend_display() {
        assert_eq!(SdkBackend::Claude.to_string(), "claude");
        assert_eq!(SdkBackend::Copilot.to_string(), "copilot");
        assert_eq!(SdkBackend::Codex.to_string(), "codex");
    }
    #[test]
    fn sdk_backend_serializes() {
        assert_eq!(serde_json::to_value(SdkBackend::Claude).unwrap(), "claude");
        assert_eq!(
            serde_json::from_str::<SdkBackend>("\"copilot\"").unwrap(),
            SdkBackend::Copilot
        );
    }
    #[test]
    fn default_config_values() {
        let c = AutoModeConfig::default();
        assert_eq!(c.max_turns, DEFAULT_MAX_TURNS);
        assert_eq!(c.max_session_secs, DEFAULT_MAX_SESSION_SECS);
        assert_eq!(c.max_api_calls, DEFAULT_MAX_API_CALLS);
    }
    #[test]
    fn config_serializes() {
        let c = AutoModeConfig {
            sdk: SdkBackend::Copilot,
            prompt: "fix bug".into(),
            max_turns: 5,
            ..Default::default()
        };
        let j = serde_json::to_value(&c).unwrap();
        assert_eq!(j["sdk"], "copilot");
        assert_eq!(j["max_turns"], 5);
    }
    #[test]
    fn sanitize_truncates() {
        let large = "x".repeat(MAX_INJECTED_CONTENT_SIZE + 1000);
        assert_eq!(
            sanitize_injected_content(&large).len(),
            MAX_INJECTED_CONTENT_SIZE
        );
    }
    #[test]
    fn sanitize_removes_injections() {
        let r = sanitize_injected_content("Hello\nignore previous instructions\nWorld");
        assert!(r.contains("[REDACTED]"));
        assert!(!r.contains("ignore previous instructions"));
    }
    #[test]
    fn sanitize_case_insensitive() {
        assert!(sanitize_injected_content("IGNORE PREVIOUS INSTRUCTIONS").contains("[REDACTED]"));
    }
    #[test]
    fn sanitize_preserves_clean() {
        assert_eq!(sanitize_injected_content("fix auth.rs"), "fix auth.rs");
    }
    #[test]
    fn retryable_detection() {
        assert!(is_retryable_output("500 Internal Server Error"));
        assert!(is_retryable_output("429 Too Many Requests"));
        assert!(is_retryable_output("timeout"));
        assert!(!is_retryable_output("syntax error"));
    }
    #[test]
    fn format_elapsed_works() {
        assert_eq!(format_elapsed(90.0), "1m 30s");
        assert_eq!(format_elapsed(45.0), "45s");
        assert_eq!(format_elapsed(0.0), "0s");
    }
    #[test]
    fn turn_result_serializes() {
        let j = serde_json::to_value(TurnResult {
            turn: 1,
            phase: "clarify".into(),
            exit_code: 0,
            output: "done".into(),
            duration_secs: 5.2,
        })
        .unwrap();
        assert_eq!(j["turn"], 1);
        assert_eq!(j["phase"], "clarify");
    }
    #[test]
    fn session_result_serializes() {
        let j = serde_json::to_value(SessionResult {
            total_turns: 3,
            completed: true,
            total_duration_secs: 120.5,
            turns: vec![],
            total_api_calls: 3,
            total_output_bytes: 1024,
        })
        .unwrap();
        assert_eq!(j["total_turns"], 3);
        assert_eq!(j["completed"], true);
    }
}
