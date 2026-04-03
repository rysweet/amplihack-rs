//! Platform CLI abstraction for spawning AI-assistant subprocesses.
//!
//! Provides a trait-based abstraction over different coding-assistant CLIs
//! (Claude Code, GitHub Copilot, Microsoft Amplifier) with platform
//! detection, command construction, prompt formatting, and output parsing.
//!
//! Ported from the Python `platform_cli.py`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::RwLock;

use crate::error::{DelegationError, Result};

/// Platform-specific prompt formatters.
pub mod parsers;

/// Whitelisted CLI flags that may be passed as extra arguments.
pub const ALLOWED_EXTRA_ARGS: &[&str] = &[
    "--debug",
    "--verbose",
    "-v",
    "--quiet",
    "-q",
    "--help",
    "-h",
    "--version",
    "--no-color",
    "--json",
];

/// Validate that every item in `args` is in the whitelist.
pub fn validate_extra_args(args: &[String]) -> Result<()> {
    for arg in args {
        if !ALLOWED_EXTRA_ARGS.contains(&arg.as_str()) {
            return Err(DelegationError::Validation(format!(
                "Argument '{}' is not allowed. Allowed: {}",
                arg,
                ALLOWED_EXTRA_ARGS.join(", ")
            )));
        }
    }
    Ok(())
}

/// Validate that `working_dir` exists, is a directory, and contains no `..` components.
pub fn validate_working_dir(working_dir: &str) -> Result<()> {
    if working_dir.is_empty() {
        return Err(DelegationError::Validation(
            "working_dir cannot be empty".into(),
        ));
    }
    let path = Path::new(working_dir);
    for component in path.components() {
        if let std::path::Component::ParentDir = component {
            return Err(DelegationError::Validation(format!(
                "Path traversal detected in working_dir: {working_dir}"
            )));
        }
    }
    if !path.exists() {
        return Err(DelegationError::Validation(format!(
            "working_dir does not exist: {working_dir}"
        )));
    }
    if !path.is_dir() {
        return Err(DelegationError::Validation(format!(
            "working_dir is not a directory: {working_dir}"
        )));
    }
    Ok(())
}

/// Configuration for spawning a platform subprocess.
#[derive(Debug, Clone)]
pub struct SpawnConfig {
    /// The command and its arguments.
    pub command: Vec<String>,
    /// Working directory for the child process.
    pub working_dir: String,
    /// Environment variables to set (merged with inherited env).
    pub environment: HashMap<String, String>,
}

/// Abstraction over a coding-assistant CLI.
pub trait PlatformCli: Send + Sync {
    /// Human-readable platform name.
    fn platform_name(&self) -> &str;
    /// Build a [`SpawnConfig`] describing how to launch the subprocess.
    fn build_spawn_config(
        &self,
        goal: &str,
        persona: &str,
        working_dir: &str,
        environment: &HashMap<String, String>,
        extra_args: &[String],
        context: &str,
    ) -> Result<SpawnConfig>;
    /// Format a prompt tailored to this platform and the given persona.
    fn format_prompt(&self, goal: &str, persona: &str, context: &str) -> String;
    /// Parse raw subprocess output into structured key-value pairs.
    fn parse_output(&self, output: &str) -> HashMap<String, String>;
    /// Return `true` when the platform CLI binary is available on `$PATH`.
    fn validate_installation(&self) -> bool;
    /// Return the CLI version string, or `"unknown"`.
    fn get_version(&self) -> String;
}

fn stdout_map(output: &str) -> HashMap<String, String> {
    HashMap::from([("stdout".into(), output.to_string())])
}

/// Claude Code CLI implementation.
#[derive(Debug, Default)]
pub struct ClaudeCodeCli;

impl PlatformCli for ClaudeCodeCli {
    fn platform_name(&self) -> &str {
        "claude-code"
    }

    fn format_prompt(&self, goal: &str, persona: &str, context: &str) -> String {
        let ctx = if context.is_empty() {
            "No additional context provided."
        } else {
            context
        };
        parsers::format_claude_prompt(goal, persona, ctx)
    }

    fn build_spawn_config(
        &self,
        goal: &str,
        persona: &str,
        working_dir: &str,
        environment: &HashMap<String, String>,
        extra_args: &[String],
        context: &str,
    ) -> Result<SpawnConfig> {
        validate_working_dir(working_dir)?;
        validate_extra_args(extra_args)?;
        let prompt = self.format_prompt(goal, persona, context);
        let mut cmd = vec!["claude".to_string(), "-p".to_string(), prompt];
        for (i, a) in extra_args.iter().enumerate() {
            cmd.insert(1 + i, a.clone());
        }
        let mut env = environment.clone();
        env.insert("AMPLIHACK_NONINTERACTIVE".into(), "1".into());
        env.insert("CI".into(), "true".into());
        Ok(SpawnConfig {
            command: cmd,
            working_dir: working_dir.into(),
            environment: env,
        })
    }

    fn parse_output(&self, output: &str) -> HashMap<String, String> {
        stdout_map(output)
    }
    fn validate_installation(&self) -> bool {
        which("claude")
    }
    fn get_version(&self) -> String {
        version_from_binary(&["claude", "--version"])
    }
}

/// GitHub Copilot CLI implementation.
#[derive(Debug, Default)]
pub struct CopilotCli;

impl PlatformCli for CopilotCli {
    fn platform_name(&self) -> &str {
        "copilot"
    }

    fn format_prompt(&self, goal: &str, persona: &str, context: &str) -> String {
        parsers::format_copilot_prompt(goal, persona, context)
    }

    fn build_spawn_config(
        &self,
        goal: &str,
        persona: &str,
        working_dir: &str,
        environment: &HashMap<String, String>,
        extra_args: &[String],
        context: &str,
    ) -> Result<SpawnConfig> {
        validate_working_dir(working_dir)?;
        validate_extra_args(extra_args)?;
        let prompt = self.format_prompt(goal, persona, context);
        let mut cmd = vec!["gh".into(), "copilot".into(), "suggest".into()];
        cmd.extend(extra_args.iter().cloned());
        cmd.push(prompt);
        Ok(SpawnConfig {
            command: cmd,
            working_dir: working_dir.into(),
            environment: environment.clone(),
        })
    }

    fn parse_output(&self, output: &str) -> HashMap<String, String> {
        stdout_map(output)
    }
    fn validate_installation(&self) -> bool {
        which("gh")
    }
    fn get_version(&self) -> String {
        version_from_binary(&["gh", "copilot", "--version"])
    }
}

/// Microsoft Amplifier CLI implementation.
#[derive(Debug, Default)]
pub struct AmplifierCli;

impl PlatformCli for AmplifierCli {
    fn platform_name(&self) -> &str {
        "amplifier"
    }

    fn format_prompt(&self, goal: &str, persona: &str, context: &str) -> String {
        parsers::format_amplifier_prompt(goal, persona, context)
    }

    fn build_spawn_config(
        &self,
        goal: &str,
        persona: &str,
        working_dir: &str,
        environment: &HashMap<String, String>,
        extra_args: &[String],
        context: &str,
    ) -> Result<SpawnConfig> {
        validate_working_dir(working_dir)?;
        validate_extra_args(extra_args)?;
        let prompt = self.format_prompt(goal, persona, context);
        let mut cmd = vec!["amplifier".into(), "run".into()];
        cmd.extend(extra_args.iter().cloned());
        cmd.push(prompt);
        Ok(SpawnConfig {
            command: cmd,
            working_dir: working_dir.into(),
            environment: environment.clone(),
        })
    }

    fn parse_output(&self, output: &str) -> HashMap<String, String> {
        stdout_map(output)
    }
    fn validate_installation(&self) -> bool {
        which("amplifier")
    }
    fn get_version(&self) -> String {
        version_from_binary(&["amplifier", "--version"])
    }
}

/// Thread-safe registry of platform CLI implementations.
static PLATFORM_REGISTRY: std::sync::LazyLock<RwLock<HashMap<String, Box<dyn PlatformCli>>>> =
    std::sync::LazyLock::new(|| {
        let mut m: HashMap<String, Box<dyn PlatformCli>> = HashMap::new();
        m.insert("claude-code".into(), Box::new(ClaudeCodeCli));
        m.insert("copilot".into(), Box::new(CopilotCli));
        m.insert("amplifier".into(), Box::new(AmplifierCli));
        RwLock::new(m)
    });

/// Register a custom platform CLI implementation.
pub fn register_platform(name: impl Into<String>, platform: Box<dyn PlatformCli>) {
    if let Ok(mut reg) = PLATFORM_REGISTRY.write() {
        reg.insert(name.into(), platform);
    }
}

/// Look up a platform by name. Defaults to `"claude-code"` when `name` is `None`.
pub fn get_platform(name: Option<&str>) -> Result<String> {
    let key = name.unwrap_or("claude-code");
    let reg = PLATFORM_REGISTRY
        .read()
        .map_err(|e| DelegationError::Failed(e.to_string()))?;
    if reg.contains_key(key) {
        Ok(key.to_string())
    } else {
        let available: Vec<_> = reg.keys().cloned().collect();
        Err(DelegationError::Validation(format!(
            "Unknown platform: {key}. Available: {}",
            available.join(", ")
        )))
    }
}

/// Return the list of registered platform names.
pub fn available_platforms() -> Vec<String> {
    PLATFORM_REGISTRY
        .read()
        .map(|reg| reg.keys().cloned().collect())
        .unwrap_or_default()
}

fn which(binary: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(binary).is_file()))
        .unwrap_or(false)
}

fn version_from_binary(command: &[&str]) -> String {
    let re = regex::Regex::new(r"\d+\.\d+\.\d+").expect("valid regex");
    std::process::Command::new(command[0])
        .args(&command[1..])
        .output()
        .ok()
        .and_then(|o| {
            let stdout = String::from_utf8_lossy(&o.stdout);
            re.find(&stdout).map(|m| m.as_str().to_string())
        })
        .unwrap_or_else(|| "unknown".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_extra_args_ok() {
        assert!(validate_extra_args(&["--debug".into(), "-v".into()]).is_ok());
    }

    #[test]
    fn validate_extra_args_rejects_unknown() {
        assert!(validate_extra_args(&["--evil".into()]).is_err());
    }

    #[test]
    fn validate_working_dir_rejects_empty() {
        assert!(validate_working_dir("").is_err());
    }

    #[test]
    fn validate_working_dir_rejects_traversal() {
        assert!(validate_working_dir("/foo/../bar").is_err());
    }

    #[test]
    fn default_platform_is_claude_code() {
        assert_eq!(get_platform(None).unwrap(), "claude-code");
    }

    #[test]
    fn unknown_platform_is_error() {
        assert!(get_platform(Some("nonexistent")).is_err());
    }
}
