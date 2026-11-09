//! Platform CLI abstraction for spawning AI-assistant subprocesses.
//!
//! Provides a trait-based abstraction over different coding-assistant CLIs
//! (Claude Code, GitHub Copilot, Microsoft Amplifier) with platform
//! detection, command construction, prompt formatting, and output parsing.
//!
//! Ported from the Python `platform_cli.py`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, RwLock};

use tempfile::TempDir;

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
///
/// Issue #2030: `command` never holds prompt text. The prompt reaches the
/// child on `stdin`, or through a pointer sentence naming a 0600 file inside
/// the 0700 directory owned by `prompt_guard`. Whoever spawns the child must
/// write `stdin` to it and keep `prompt_guard` alive until the child has been
/// waited on; dropping the last guard removes the file.
#[derive(Clone, Default)]
pub struct SpawnConfig {
    /// The command and its arguments. No prompt text.
    pub command: Vec<String>,
    /// Working directory for the child process.
    pub working_dir: String,
    /// Environment variables to set (merged with inherited env).
    pub environment: HashMap<String, String>,
    /// Text to write to the child's stdin, then close it.
    pub stdin: Option<String>,
    /// Owns the private directory holding the pointer's prompt file.
    pub prompt_guard: Option<Arc<TempDir>>,
}

impl std::fmt::Debug for SpawnConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let stdin = self
            .stdin
            .as_ref()
            .map(|s| format!("Some(<{} bytes>)", s.len()))
            .unwrap_or_else(|| "None".to_string());
        f.debug_struct("SpawnConfig")
            .field("command", &self.command)
            .field("working_dir", &self.working_dir)
            .field(
                "environment_keys",
                &self.environment.keys().collect::<Vec<_>>(),
            )
            .field("stdin", &format_args!("{stdin}"))
            .field(
                "prompt_guard",
                &self.prompt_guard.as_ref().map(|g| g.path().to_path_buf()),
            )
            .finish()
    }
}

/// The pointer sentence, identical to
/// `amplihack_utils::prompt_delivery::FILE_REFERENCE_TEMPLATE` so the `/proc`
/// parsers keep matching.
const FILE_REFERENCE_TEMPLATE: &str = "Read and follow the complete instructions in the file {path}. Its contents are the task prompt.";

/// Write `prompt` to a 0600 file in a fresh 0700 `amplihack-prompt-*`
/// directory and return the pointer sentence and the directory's guard.
fn pointer_to_private_file(prompt: &str) -> Result<(String, Arc<TempDir>)> {
    use std::io::Write;
    #[cfg(unix)]
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let io_err = |e: std::io::Error| {
        DelegationError::Failed(format!("cannot write the prompt to a private file: {e}"))
    };
    let dir = tempfile::Builder::new()
        .prefix("amplihack-prompt-")
        .tempdir()
        .map_err(io_err)?;
    #[cfg(unix)]
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).map_err(io_err)?;
    let path = dir.path().join("prompt.md");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&path).map_err(io_err)?;
    file.write_all(prompt.as_bytes()).map_err(io_err)?;
    file.sync_all().map_err(io_err)?;
    let path_text = path.to_string_lossy();
    if path_text.contains('\n') || path_text.contains('\0') {
        return Err(DelegationError::Failed(
            "prompt file path contains a newline or NUL".into(),
        ));
    }
    Ok((
        FILE_REFERENCE_TEMPLATE.replace("{path}", &path_text),
        Arc::new(dir),
    ))
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
        // Issue #2030: `claude -p` reads the prompt from stdin.
        let mut cmd = vec!["claude".to_string()];
        cmd.extend(extra_args.iter().cloned());
        cmd.push("-p".to_string());
        let mut env = environment.clone();
        env.insert("AMPLIHACK_NONINTERACTIVE".into(), "1".into());
        env.insert("CI".into(), "true".into());
        Ok(SpawnConfig {
            command: cmd,
            working_dir: working_dir.into(),
            environment: env,
            stdin: Some(prompt),
            prompt_guard: None,
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
        _goal: &str,
        _persona: &str,
        working_dir: &str,
        _environment: &HashMap<String, String>,
        extra_args: &[String],
        _context: &str,
    ) -> Result<SpawnConfig> {
        validate_working_dir(working_dir)?;
        validate_extra_args(extra_args)?;
        // Issue #2030: `gh copilot suggest` takes its request only as an
        // argument and only turns it into a shell command. It reads no stdin
        // and no files, so it has no channel for the prompt other than argv,
        // where any local user can read it. Refuse rather than send it a
        // "read this file" sentence it cannot act on.
        Err(DelegationError::Validation(
            "no safe channel for the delegation prompt: `gh copilot suggest` accepts it only \
             on the command line, where other local users can read it (issue #2030). \
             Delegate to claude-code or amplifier instead."
                .into(),
        ))
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
        // Issue #2030: the pointer sentence, never the prompt, in argv.
        let (pointer, guard) = pointer_to_private_file(&prompt)?;
        let mut cmd = vec!["amplifier".into(), "run".into()];
        cmd.extend(extra_args.iter().cloned());
        cmd.push(pointer);
        Ok(SpawnConfig {
            command: cmd,
            working_dir: working_dir.into(),
            environment: environment.clone(),
            stdin: None,
            prompt_guard: Some(guard),
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
    fn file_reference_template_matches_prompt_delivery() {
        // The `/proc` parsers match this exact sentence (#2030).
        assert_eq!(
            FILE_REFERENCE_TEMPLATE,
            amplihack_utils::prompt_delivery::FILE_REFERENCE_TEMPLATE
        );
    }

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
