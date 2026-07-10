//! Launcher detection.
//!
//! Detects whether running under Claude Code, Copilot, or Amplifier by
//! inspecting environment variables, falling back to the launcher-context
//! file persisted at session start.
//!
//! Detection only: this module has no filesystem side effects. The former
//! context-injection step (which wrote raw `tool_input` into `AGENTS.md` and
//! re-ingested it as agent instructions) was removed as a prompt-injection
//! fix (issue #862). Security checks (CWD, branch, --no-verify) are
//! independent of the detected launcher.

use amplihack_cli::launcher_context::{
    LauncherKind, is_launcher_context_stale, read_launcher_context,
};
use amplihack_types::paths::{ProjectDirs, framework_roots_from};
use std::path::{Path, PathBuf};

/// Detected launcher type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LauncherType {
    ClaudeCode,
    Copilot,
    Amplifier,
    Unknown,
}

/// Detect which launcher is running by checking environment variables.
pub fn detect_launcher() -> LauncherType {
    detect_launcher_from_env().unwrap_or(LauncherType::Unknown)
}

pub(crate) fn detect_launcher_for_dirs(dirs: &ProjectDirs) -> LauncherType {
    if let Some(launcher) = detect_launcher_from_env() {
        return launcher;
    }

    if let Some((_, context)) = read_launcher_context_from_project_or_ancestors(&dirs.root)
        && !is_launcher_context_stale(&context)
    {
        return match context.launcher {
            LauncherKind::Copilot => LauncherType::Copilot,
            LauncherKind::Amplifier => LauncherType::Amplifier,
            LauncherKind::Claude => LauncherType::ClaudeCode,
            LauncherKind::Codex | LauncherKind::Unknown => LauncherType::Unknown,
        };
    }

    LauncherType::Unknown
}

fn detect_launcher_from_env() -> Option<LauncherType> {
    if std::env::var("GITHUB_COPILOT_AGENT").is_ok() || std::env::var("COPILOT_AGENT").is_ok() {
        return Some(LauncherType::Copilot);
    }
    if std::env::var("AMPLIFIER_SESSION").is_ok() {
        return Some(LauncherType::Amplifier);
    }
    if std::env::var("CLAUDE_CODE_SESSION").is_ok() || std::env::var("CLAUDE_SESSION_ID").is_ok() {
        return Some(LauncherType::ClaudeCode);
    }
    None
}

fn read_launcher_context_from_project_or_ancestors(
    root: &Path,
) -> Option<(PathBuf, amplihack_cli::launcher_context::LauncherContext)> {
    for candidate in framework_roots_from(root) {
        if let Some(context) = read_launcher_context(&candidate) {
            return Some((candidate, context));
        }
    }
    read_launcher_context(root).map(|context| (root.to_path_buf(), context))
}

#[cfg(test)]
fn set_launcher_env(
    copilot: Option<&str>,
    amplifier: Option<&str>,
    claude_code: Option<&str>,
    claude_session: Option<&str>,
) {
    match copilot {
        Some(value) => unsafe { std::env::set_var("GITHUB_COPILOT_AGENT", value) },
        None => unsafe { std::env::remove_var("GITHUB_COPILOT_AGENT") },
    }
    unsafe { std::env::remove_var("COPILOT_AGENT") };
    match amplifier {
        Some(value) => unsafe { std::env::set_var("AMPLIFIER_SESSION", value) },
        None => unsafe { std::env::remove_var("AMPLIFIER_SESSION") },
    }
    match claude_code {
        Some(value) => unsafe { std::env::set_var("CLAUDE_CODE_SESSION", value) },
        None => unsafe { std::env::remove_var("CLAUDE_CODE_SESSION") },
    }
    match claude_session {
        Some(value) => unsafe { std::env::set_var("CLAUDE_SESSION_ID", value) },
        None => unsafe { std::env::remove_var("CLAUDE_SESSION_ID") },
    }
}

#[cfg(test)]
#[path = "tests/launcher_tests.rs"]
mod tests;
