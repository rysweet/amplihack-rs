//! Launch commands for Claude, Copilot, Codex, and Amplifier.
//!
//! Builds the environment, finds the binary, checks nesting, and spawns
//! a `ManagedChild` with signal forwarding.

use crate::binary_finder::{BinaryFinder, BinaryInfo};
use crate::env_builder::EnvBuilder;
use crate::launcher::ManagedChild;
use crate::nesting::NestingDetector;
use crate::signals;
use anyhow::{Context, Result};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// Launch a tool binary (claude, copilot, codex, amplifier).
pub fn run_launch(
    tool: &str,
    resume: bool,
    continue_session: bool,
    extra_args: Vec<String>,
) -> Result<()> {
    // Check nesting
    let nesting = NestingDetector::detect();
    match &nesting {
        crate::nesting::NestingResult::Nested {
            session_id, depth, ..
        } => {
            tracing::warn!(
                session_id,
                depth,
                "nested amplihack session detected — launching anyway"
            );
        }
        crate::nesting::NestingResult::StaleSession { session_id } => {
            tracing::info!(session_id, "stale session detected, ignoring");
        }
        crate::nesting::NestingResult::NotNested => {}
    }

    // Find binary
    let binary = BinaryFinder::find(tool)
        .with_context(|| format!("could not find '{tool}' binary in PATH"))?;

    tracing::info!(
        binary = %binary.path.display(),
        version = binary.version.as_deref().unwrap_or("unknown"),
        "launching {tool}"
    );

    // Build environment
    let env = EnvBuilder::new()
        .with_amplihack_session_id()
        .with_amplihack_vars()
        .build();

    // Build command
    let mut cmd = build_command(&binary, resume, continue_session, &extra_args);
    cmd.envs(env);

    // Register signal handlers
    let shutdown = signals::register_handlers()?;

    // Spawn child in its own process group
    let mut child = ManagedChild::spawn(cmd)?;

    // Wait for child or signal
    let exit_code = wait_for_child_or_signal(&mut child, &shutdown)?;

    if exit_code != 0 {
        std::process::exit(exit_code);
    }
    Ok(())
}

fn build_command(
    binary: &BinaryInfo,
    resume: bool,
    continue_session: bool,
    extra_args: &[String],
) -> Command {
    let mut cmd = Command::new(&binary.path);
    if resume {
        cmd.arg("--resume");
    }
    if continue_session {
        cmd.arg("--continue");
    }
    cmd.args(extra_args);
    cmd
}

fn wait_for_child_or_signal(
    child: &mut ManagedChild,
    shutdown: &Arc<std::sync::atomic::AtomicBool>,
) -> Result<i32> {
    loop {
        // Check if we received a shutdown signal
        if shutdown.load(Ordering::Relaxed) {
            tracing::info!("shutdown signal received, terminating child process");
            // ManagedChild::drop handles graceful shutdown
            return Ok(130); // standard SIGINT exit code
        }

        // Check if child has exited
        match child.try_wait()? {
            Some(status) => {
                return Ok(status.code().unwrap_or(1));
            }
            None => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn build_command_basic() {
        let binary = BinaryInfo {
            name: "claude".to_string(),
            path: PathBuf::from("/usr/bin/claude"),
            version: Some("1.0.0".to_string()),
        };
        let cmd = build_command(&binary, false, false, &[]);
        assert_eq!(cmd.get_program(), "/usr/bin/claude");
        assert_eq!(cmd.get_args().count(), 0);
    }

    #[test]
    fn build_command_with_flags() {
        let binary = BinaryInfo {
            name: "claude".to_string(),
            path: PathBuf::from("/usr/bin/claude"),
            version: None,
        };
        let extra = vec!["--model".to_string(), "opus".to_string()];
        let cmd = build_command(&binary, true, true, &extra);
        let args: Vec<&std::ffi::OsStr> = cmd.get_args().collect();
        assert_eq!(args, &["--resume", "--continue", "--model", "opus"]);
    }
}
