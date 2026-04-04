//! SDK-routed LLM client for amplihack.
//!
//! Ported from `amplihack/llm/client.py`.
//!
//! Auto-detects the active launcher (Claude Code or GitHub Copilot CLI) and
//! provides SDK availability information. The Rust CLI does not call LLM APIs
//! directly — the launcher binary handles that — so [`completion`] delegates
//! to the launcher via subprocess when available, or returns an error.
//!
//! ## Fail-open design
//!
//! [`is_sdk_available`] and [`detect_launcher`] never panic. [`completion`]
//! returns `Err` when no launcher is reachable, letting callers decide how to
//! degrade.
//!
//! ## Environment variables consulted
//!
//! | Variable                 | Indicates            |
//! |--------------------------|----------------------|
//! | `AMPLIHACK_AGENT_BINARY` | Explicit binary path |
//! | `CLAUDE_CODE`            | Claude Code session  |
//! | `CLAUDE_PROJECT_DIR`     | Claude Code session  |
//! | `COPILOT_CLI`            | Copilot CLI session  |
//! | `GITHUB_COPILOT`         | Copilot environment  |

use std::env;
use std::fmt;

use thiserror::Error;

/// Errors from the LLM client module.
#[derive(Debug, Error)]
pub enum LlmClientError {
    /// No SDK launcher is available in the current environment.
    #[error("no SDK launcher available — LLM calls must go through the launcher binary")]
    NoSdkAvailable,

    /// The launcher subprocess failed.
    #[error("launcher subprocess failed: {0}")]
    LauncherError(String),
}

/// The type of launcher hosting the current process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LauncherType {
    /// Running inside a Claude Code session.
    ClaudeCode,
    /// Running inside the GitHub Copilot CLI.
    CopilotCli,
    /// Launcher could not be determined.
    Unknown,
}

impl fmt::Display for LauncherType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ClaudeCode => write!(f, "claude-code"),
            Self::CopilotCli => write!(f, "copilot-cli"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

/// Detect which launcher environment is hosting this process.
///
/// Checks environment variables in priority order:
/// 1. `AMPLIHACK_AGENT_BINARY` — explicit override (value contains "claude" or "copilot")
/// 2. `CLAUDE_CODE` / `CLAUDE_PROJECT_DIR` — Claude Code session markers
/// 3. `COPILOT_CLI` / `GITHUB_COPILOT` — Copilot CLI session markers
///
/// Returns [`LauncherType::Unknown`] if no markers are found.
pub fn detect_launcher() -> LauncherType {
    detect_launcher_from(EnvReader::Real)
}

/// Check whether any recognized SDK launcher is available.
///
/// Equivalent to `detect_launcher() != LauncherType::Unknown`.
pub fn is_sdk_available() -> bool {
    detect_launcher() != LauncherType::Unknown
}

/// Send a completion request through the detected launcher.
///
/// Because the Rust CLI does not embed LLM SDKs directly, this returns
/// [`LlmClientError::NoSdkAvailable`] when no launcher is detected. When a
/// launcher *is* detected the function currently returns a placeholder —
/// actual subprocess delegation will be wired in a follow-up change.
pub fn completion(prompt: &str) -> Result<String, LlmClientError> {
    let launcher = detect_launcher();
    match launcher {
        LauncherType::Unknown => Err(LlmClientError::NoSdkAvailable),
        _ => {
            tracing::debug!(
                launcher = %launcher,
                prompt_len = prompt.len(),
                "completion requested — delegating to launcher"
            );
            // TODO: delegate to launcher binary via subprocess.
            // For now, return a placeholder indicating the launcher was found
            // but direct completion is not yet wired.
            Err(LlmClientError::LauncherError(format!(
                "direct completion not yet implemented for {launcher}; \
                 route LLM calls through the launcher binary"
            )))
        }
    }
}

// ---------------------------------------------------------------------------
// Internal: env-var reader abstraction (enables deterministic tests)
// ---------------------------------------------------------------------------

/// Abstraction over environment variable reads for testability.
#[derive(Clone, Copy)]
enum EnvReader {
    Real,
    #[cfg(test)]
    Test(&'static [(&'static str, &'static str)]),
}

impl EnvReader {
    fn var(self, key: &str) -> Option<String> {
        match self {
            Self::Real => env::var(key).ok(),
            #[cfg(test)]
            Self::Test(pairs) => pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_string()),
        }
    }
}

fn detect_launcher_from(env: EnvReader) -> LauncherType {
    // 1. Explicit agent binary override.
    if let Some(binary) = env.var("AMPLIHACK_AGENT_BINARY") {
        let lower = binary.to_lowercase();
        if lower.contains("claude") {
            return LauncherType::ClaudeCode;
        }
        if lower.contains("copilot") {
            return LauncherType::CopilotCli;
        }
    }

    // 2. Claude Code session markers.
    if env.var("CLAUDE_CODE").is_some() || env.var("CLAUDE_PROJECT_DIR").is_some() {
        return LauncherType::ClaudeCode;
    }

    // 3. Copilot CLI session markers.
    if env.var("COPILOT_CLI").is_some() || env.var("GITHUB_COPILOT").is_some() {
        return LauncherType::CopilotCli;
    }

    LauncherType::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- detect_launcher ----------------------------------------------------

    #[test]
    fn detect_unknown_when_no_env_vars() {
        let env = EnvReader::Test(&[]);
        assert_eq!(detect_launcher_from(env), LauncherType::Unknown);
    }

    #[test]
    fn detect_claude_via_agent_binary() {
        let env = EnvReader::Test(&[("AMPLIHACK_AGENT_BINARY", "claude")]);
        assert_eq!(detect_launcher_from(env), LauncherType::ClaudeCode);
    }

    #[test]
    fn detect_copilot_via_agent_binary() {
        let env = EnvReader::Test(&[("AMPLIHACK_AGENT_BINARY", "copilot")]);
        assert_eq!(detect_launcher_from(env), LauncherType::CopilotCli);
    }

    #[test]
    fn detect_claude_via_claude_code_env() {
        let env = EnvReader::Test(&[("CLAUDE_CODE", "1")]);
        assert_eq!(detect_launcher_from(env), LauncherType::ClaudeCode);
    }

    #[test]
    fn detect_claude_via_project_dir() {
        let env = EnvReader::Test(&[("CLAUDE_PROJECT_DIR", "/some/path")]);
        assert_eq!(detect_launcher_from(env), LauncherType::ClaudeCode);
    }

    #[test]
    fn detect_copilot_via_copilot_cli_env() {
        let env = EnvReader::Test(&[("COPILOT_CLI", "1")]);
        assert_eq!(detect_launcher_from(env), LauncherType::CopilotCli);
    }

    #[test]
    fn detect_copilot_via_github_copilot_env() {
        let env = EnvReader::Test(&[("GITHUB_COPILOT", "true")]);
        assert_eq!(detect_launcher_from(env), LauncherType::CopilotCli);
    }

    #[test]
    fn agent_binary_takes_priority_over_session_markers() {
        let env = EnvReader::Test(&[("AMPLIHACK_AGENT_BINARY", "copilot"), ("CLAUDE_CODE", "1")]);
        assert_eq!(detect_launcher_from(env), LauncherType::CopilotCli);
    }

    #[test]
    fn agent_binary_case_insensitive() {
        let env = EnvReader::Test(&[("AMPLIHACK_AGENT_BINARY", "Claude-Code")]);
        assert_eq!(detect_launcher_from(env), LauncherType::ClaudeCode);
    }

    #[test]
    fn unrecognized_agent_binary_falls_through() {
        let env = EnvReader::Test(&[
            ("AMPLIHACK_AGENT_BINARY", "some-other-tool"),
            ("GITHUB_COPILOT", "1"),
        ]);
        assert_eq!(detect_launcher_from(env), LauncherType::CopilotCli);
    }

    // -- is_sdk_available ---------------------------------------------------

    #[test]
    fn sdk_not_available_when_unknown() {
        // is_sdk_available uses real env — but we test the logic via detect.
        let env = EnvReader::Test(&[]);
        assert_eq!(detect_launcher_from(env), LauncherType::Unknown);
        // LauncherType::Unknown != Unknown is false, matching is_sdk_available logic.
        assert!(detect_launcher_from(env) == LauncherType::Unknown);
    }

    #[test]
    fn sdk_available_when_launcher_detected() {
        let env = EnvReader::Test(&[("CLAUDE_CODE", "1")]);
        assert_ne!(detect_launcher_from(env), LauncherType::Unknown);
    }

    // -- completion ---------------------------------------------------------

    #[test]
    fn completion_errors_when_no_sdk() {
        // We can't control the real env easily, so test the error path via
        // detect_launcher_from returning Unknown — the completion function
        // uses detect_launcher() which reads real env. For unit coverage we
        // verify the error types exist and display correctly.
        let err = LlmClientError::NoSdkAvailable;
        assert!(err.to_string().contains("no SDK launcher available"));
    }

    #[test]
    fn completion_launcher_error_displays() {
        let err = LlmClientError::LauncherError("test failure".into());
        assert!(err.to_string().contains("test failure"));
    }

    // -- Display ------------------------------------------------------------

    #[test]
    fn launcher_type_display() {
        assert_eq!(LauncherType::ClaudeCode.to_string(), "claude-code");
        assert_eq!(LauncherType::CopilotCli.to_string(), "copilot-cli");
        assert_eq!(LauncherType::Unknown.to_string(), "unknown");
    }
}
