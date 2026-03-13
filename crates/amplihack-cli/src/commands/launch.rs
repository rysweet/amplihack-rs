//! Launch commands for Claude, Copilot, Codex, and Amplifier.
//!
//! Builds the environment, finds the binary, checks nesting, and spawns
//! a `ManagedChild` with signal forwarding.

use crate::binary_finder::BinaryInfo;
use crate::bootstrap;
use crate::env_builder::EnvBuilder;
use crate::launcher::ManagedChild;
use crate::nesting::NestingDetector;
use crate::signals;
use crate::util::is_noninteractive;
use anyhow::{Context, Result};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// Launch a tool binary (claude, copilot, codex, amplifier).
pub fn run_launch(
    tool: &str,
    resume: bool,
    continue_session: bool,
    skip_permissions: bool,
    extra_args: Vec<String>,
) -> Result<()> {
    // Compute once; passed to bootstrap and used when building the child env.
    let noninteractive = is_noninteractive();
    bootstrap::prepare_launcher(tool, noninteractive)?;

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
    let binary = bootstrap::ensure_tool_available(tool)
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
        .with_agent_binary()
        .with_amplihack_home()
        .set_if(noninteractive, "CLAUDE_NONINTERACTIVE", "1")
        .build();

    // Build command
    let mut cmd = build_command(
        &binary,
        resume,
        continue_session,
        skip_permissions,
        &extra_args,
    );
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
    skip_permissions: bool,
    extra_args: &[String],
) -> Command {
    let mut cmd = Command::new(&binary.path);

    // SEC-2: Only inject --dangerously-skip-permissions when the caller has
    // explicitly opted in via `--skip-permissions`.  This flag bypasses
    // Claude's interactive confirmation prompts and must not be on by default.
    if skip_permissions {
        cmd.arg("--dangerously-skip-permissions");
    }

    // Inject --model unless user already supplied one
    let user_has_model = extra_args.iter().any(|a| a == "--model");
    if !user_has_model {
        let default_model =
            std::env::var("AMPLIHACK_DEFAULT_MODEL").unwrap_or_else(|_| "opus[1m]".to_string());
        cmd.arg("--model");
        cmd.arg(default_model);
    }

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
            return Ok(0); // match Python behavior: exit 0 on SIGINT
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
    fn build_command_basic_no_skip_permissions_by_default() {
        let binary = BinaryInfo {
            name: "claude".to_string(),
            path: PathBuf::from("/usr/bin/claude"),
            version: Some("1.0.0".to_string()),
        };
        // skip_permissions = false (default): should NOT inject --dangerously-skip-permissions
        let cmd = build_command(&binary, false, false, false, &[]);
        assert_eq!(cmd.get_program(), "/usr/bin/claude");
        let args: Vec<&std::ffi::OsStr> = cmd.get_args().collect();
        // Should inject --model <default> only
        assert_eq!(args[0], "--model");
        // Default model depends on env; just check we have 2 args
        assert_eq!(args.len(), 2);
    }

    #[test]
    fn build_command_with_skip_permissions_flag() {
        let binary = BinaryInfo {
            name: "claude".to_string(),
            path: PathBuf::from("/usr/bin/claude"),
            version: Some("1.0.0".to_string()),
        };
        // skip_permissions = true: should inject --dangerously-skip-permissions
        let cmd = build_command(&binary, false, false, true, &[]);
        let args: Vec<&std::ffi::OsStr> = cmd.get_args().collect();
        assert_eq!(args[0], "--dangerously-skip-permissions");
        assert_eq!(args[1], "--model");
        assert_eq!(args.len(), 3);
    }

    #[test]
    fn build_command_with_flags() {
        let binary = BinaryInfo {
            name: "claude".to_string(),
            path: PathBuf::from("/usr/bin/claude"),
            version: None,
        };
        // User supplies --model so we should NOT inject a default --model
        let extra = vec!["--model".to_string(), "opus".to_string()];
        let cmd = build_command(&binary, true, true, true, &extra);
        let args: Vec<&std::ffi::OsStr> = cmd.get_args().collect();
        assert_eq!(
            args,
            &[
                "--dangerously-skip-permissions",
                "--resume",
                "--continue",
                "--model",
                "opus"
            ]
        );
    }

    #[test]
    fn build_command_without_skip_permissions_and_with_flags() {
        let binary = BinaryInfo {
            name: "claude".to_string(),
            path: PathBuf::from("/usr/bin/claude"),
            version: None,
        };
        let extra = vec!["--model".to_string(), "opus".to_string()];
        let cmd = build_command(&binary, true, true, false, &extra);
        let args: Vec<&std::ffi::OsStr> = cmd.get_args().collect();
        assert_eq!(args, &["--resume", "--continue", "--model", "opus"]);
    }
}
