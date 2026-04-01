//! Launch commands for Claude, Copilot, Codex, and Amplifier.
//!
//! Builds the environment, finds the binary, checks nesting, and spawns
//! a `ManagedChild` with signal forwarding.

mod blarify;
mod checkout;
mod command;
mod context;
mod power_steering;

#[cfg(test)]
mod tests_blarify;
#[cfg(test)]
mod tests_command;
#[cfg(test)]
mod tests_env;
#[cfg(test)]
mod tests_launch;

// Re-exports — public API of the launch module.
pub(crate) use checkout::resolve_checkout_repo;
pub(crate) use power_steering::maybe_prompt_re_enable_power_steering;

// Internal imports from submodules used by run_launch.
use blarify::maybe_run_blarify_indexing_prompt;
use command::{augment_claude_launch_env, build_command_for_dir, build_docker_launcher_args};
use context::persist_launcher_context;

// Test-visible re-imports from submodules. These become available to
// `#[cfg(test)] mod tests_*` children via `use super::*`.
#[cfg(test)]
use blarify::{
    blarify_mode, consent_cache_path, has_blarify_consent,
    maybe_run_blarify_indexing_prompt_with, parse_blarify_prompt_choice,
    resolve_blarify_index_action, save_blarify_consent, should_prompt_blarify_indexing,
    BlarifyIndexAction, BlarifyMode, BlarifyPromptChoice,
};
#[cfg(test)]
use checkout::{parse_github_repo_uri, resolve_checkout_repo_in};
#[cfg(test)]
use command::build_command;
#[cfg(test)]
use context::render_launcher_command;
#[cfg(test)]
use power_steering::maybe_prompt_re_enable_power_steering_with;

use crate::bootstrap;
use crate::docker::{DockerDetector, DockerManager};
use crate::env_builder::EnvBuilder;
use crate::launcher::ManagedChild;
use crate::memory_config::prepare_memory_config;
use crate::nesting::NestingDetector;
use crate::session_tracker::SessionTracker;
use crate::signals;
use crate::tool_update_check::maybe_print_npm_update_notice;
use crate::util::is_noninteractive;

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

const POWER_STEERING_PROMPT_TIMEOUT: Duration = Duration::from_secs(30);

/// Launch a tool binary (claude, copilot, codex, amplifier).
#[allow(clippy::too_many_arguments)]
pub fn run_launch(
    tool: &str,
    launcher_command: &str,
    docker: bool,
    resume: bool,
    continue_session: bool,
    skip_permissions: bool,
    skip_update_check: bool,
    no_reflection: bool,
    subprocess_safe: bool,
    checkout_repo: Option<String>,
    extra_args: Vec<String>,
) -> Result<()> {
    let current_dir = std::env::current_dir()
        .ok()
        .unwrap_or_else(|| PathBuf::from("."));
    if let Some(activation) = DockerDetector.activation_source(docker) {
        println!("{}", activation.message());
        let docker_args = build_docker_launcher_args(
            launcher_command,
            resume,
            continue_session,
            skip_update_check,
            no_reflection,
            subprocess_safe,
            checkout_repo.as_deref(),
            &extra_args,
        );
        let exit_code = DockerManager::default().run_command(&docker_args, &current_dir)?;
        if exit_code != 0 {
            std::process::exit(exit_code);
        }
        return Ok(());
    }

    // Check for npm updates before doing anything else.
    // This is a no-op if skip_update_check is true, AMPLIHACK_NONINTERACTIVE is set,
    // or the tool has no npm package mapping.
    maybe_print_npm_update_notice(tool, skip_update_check);

    if !subprocess_safe {
        bootstrap::prepare_launcher(tool)?;
    }

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

    let execution_dir = resolve_checkout_repo(checkout_repo.as_deref())?
        .or(Some(current_dir.clone()))
        .unwrap_or_else(|| PathBuf::from("."));
    let node_options = resolve_launch_node_options(subprocess_safe)?;
    if !subprocess_safe {
        maybe_prompt_re_enable_power_steering(&execution_dir)?;
    }
    persist_launcher_context(tool, Some(&execution_dir), &extra_args)?;
    let launch_dir = execution_dir.clone();
    let tracker = SessionTracker::new(&launch_dir)?;
    let tracker_args = render_session_argv(
        tool,
        resume,
        continue_session,
        checkout_repo.as_deref(),
        &extra_args,
    );
    let session_id = tracker.start_session(
        std::process::id(),
        &launch_dir,
        &tracker_args,
        false,
        &nesting,
    )?;

    let result = (|| -> Result<()> {
        // Build environment — canonical chain order per design spec.
        // SEC-DATA-01: Never log the full env map (may contain inherited secrets).
        let mut env_builder = EnvBuilder::new()
            .with_amplihack_session_id() // AMPLIHACK_SESSION_ID, AMPLIHACK_DEPTH
            .with_session_tree_context() // preserve orchestration tree vars if present
            .with_amplihack_vars_with_node_options(Some(node_options.as_str())) // AMPLIHACK_RUST_RUNTIME, AMPLIHACK_VERSION, NODE_OPTIONS
            .with_agent_binary(tool) // WS1: AMPLIHACK_AGENT_BINARY
            .with_amplihack_home() // WS3: AMPLIHACK_HOME
            .with_asset_resolver(); // Rust-native bundle asset resolver
        env_builder = env_builder.with_project_graph_db(&execution_dir)?;
        let env_builder = augment_claude_launch_env(env_builder, tool)
            .set_if(is_noninteractive(), "AMPLIHACK_NONINTERACTIVE", "1")
            .set_if(no_reflection, "AMPLIHACK_SKIP_REFLECTION", "1"); // WS2: propagate flags

        maybe_run_blarify_indexing_prompt(tool, is_noninteractive(), Some(&execution_dir))?;

        // Build command
        let mut cmd = build_command_for_dir(
            &binary,
            resume,
            continue_session,
            skip_permissions,
            &extra_args,
            Some(&execution_dir),
        );
        cmd.current_dir(&execution_dir);
        env_builder.apply_to_command(&mut cmd);

        // Register signal handlers
        let shutdown = signals::register_handlers()?;

        // Spawn child in its own process group
        let mut child = ManagedChild::spawn(cmd)?;

        // Wait for child or signal
        let exit_code = wait_for_child_or_signal(&mut child, &shutdown)?;
        tracker.complete_session(&session_id)?;
        if exit_code != 0 {
            std::process::exit(exit_code);
        }
        Ok(())
    })();

    if result.is_err() {
        let _ = tracker.crash_session(&session_id);
    }
    result
}

fn resolve_launch_node_options(_subprocess_safe: bool) -> Result<String> {
    let existing = std::env::var("NODE_OPTIONS").ok();
    Ok(prepare_memory_config(existing.as_deref())?.node_options)
}

fn render_session_argv(
    tool: &str,
    resume: bool,
    continue_session: bool,
    checkout_repo: Option<&str>,
    extra_args: &[String],
) -> Vec<String> {
    let mut argv = vec!["amplihack".to_string(), tool.to_string()];
    if resume {
        argv.push("--resume".to_string());
    }
    if continue_session {
        argv.push("--continue".to_string());
    }
    if let Some(repo) = checkout_repo {
        argv.push("--checkout-repo".to_string());
        argv.push(repo.to_string());
    }
    argv.extend(extra_args.iter().cloned());
    argv
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
                return Ok(status.code().unwrap_or(0)); // SIGINT-killed: no numeric code → 0 (parity with Python signal_handler → sys.exit(0))
            }
            None => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    }
}
