//! Native auto-mode loop for launcher commands.

use crate::auto_mode_append::process_appended_instructions;
use crate::auto_mode_completion_signals::CompletionSignalDetector;
use crate::auto_mode_completion_verifier::CompletionVerifier;
use crate::auto_mode_state::AutoModeState;
use crate::auto_mode_ui::AutoModeUiHandle;
use crate::auto_mode_work_summary_generator::WorkSummaryGenerator;
use crate::auto_stager::AutoStager;
use crate::env_builder::EnvBuilder;
use crate::memory_config::prepare_memory_config;
use crate::nesting::NestingDetector;
use crate::session_tracker::SessionTracker;
use crate::util::run_output_with_timeout;
use anyhow::{Context, Result, bail};
use chrono::Local;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

mod helpers;
mod run;
mod session;

pub use run::run_auto_mode;

const QUERY_TIMEOUT: Duration = Duration::from_secs(30 * 60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoModeTool {
    Claude,
    Copilot,
    Codex,
    Amplifier,
    RustyClawd,
}

impl AutoModeTool {
    fn slug(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Copilot => "copilot",
            Self::Codex => "codex",
            Self::Amplifier => "amplifier",
            Self::RustyClawd => "claude",
        }
    }

    fn subcommand(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Copilot => "copilot",
            Self::Codex => "codex",
            Self::Amplifier => "amplifier",
            Self::RustyClawd => "RustyClawd",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedPromptArgs {
    prompt: String,
    passthrough_args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExecutionResult {
    exit_code: i32,
    stdout: String,
    stderr: String,
}

trait PromptExecutor {
    fn run_prompt(
        &self,
        tool: AutoModeTool,
        execution_dir: &Path,
        project_dir: &Path,
        passthrough_args: &[String],
        prompt: &str,
    ) -> Result<ExecutionResult>;
}

#[derive(Clone, Debug, Default)]
struct SystemPromptExecutor {
    ui_active: Option<Arc<AtomicBool>>,
    node_options: Option<String>,
}

impl PromptExecutor for SystemPromptExecutor {
    fn run_prompt(
        &self,
        tool: AutoModeTool,
        execution_dir: &Path,
        project_dir: &Path,
        passthrough_args: &[String],
        prompt: &str,
    ) -> Result<ExecutionResult> {
        let command = helpers::build_auto_command(
            tool,
            execution_dir,
            project_dir,
            self.node_options.as_deref(),
            passthrough_args,
            prompt,
        )
        .with_context(|| {
            format!(
                "failed to build auto-mode command for {}",
                tool.subcommand()
            )
        })?;
        let output = run_output_with_timeout(command, QUERY_TIMEOUT)?;

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let ui_is_active = self
            .ui_active
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::Acquire));
        if !ui_is_active && !stdout.is_empty() {
            io::stdout().write_all(stdout.as_bytes())?;
            io::stdout().flush()?;
        }
        if !ui_is_active && !stderr.is_empty() {
            io::stderr().write_all(stderr.as_bytes())?;
            io::stderr().flush()?;
        }

        Ok(ExecutionResult {
            exit_code: output.status.code().unwrap_or(1),
            stdout,
            stderr,
        })
    }
}

#[cfg(test)]
mod tests;
