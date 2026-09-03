//! Launch commands for Claude, Copilot, Codex, and Amplifier.
//!
//! Builds the environment, finds the binary, checks nesting, and spawns
//! a `ManagedChild` with signal forwarding.

mod blarify;
mod checkout;
mod command;
mod context;
mod power_steering;
/// Issue #1265 Option 3 — `--append-system-prompt` delivery of amplihack's
/// routing contract.
mod system_prompt_append;

#[cfg(test)]
mod tests_blarify;
#[cfg(test)]
mod tests_command;
#[cfg(test)]
mod tests_env;
#[cfg(test)]
mod tests_launch;
#[cfg(test)]
mod tests_subprocess_safe;
#[cfg(test)]
mod tests_system_prompt_append;

// Re-exports — public API of the launch module.
pub(crate) use checkout::resolve_checkout_repo;
pub(crate) use command::{
    resolve_no_reflection, resolve_subprocess_safe, validate_routed_copilot_workspace,
};
pub(crate) use power_steering::maybe_prompt_re_enable_power_steering;

// Internal imports from submodules used by run_launch.
use blarify::maybe_run_blarify_indexing_prompt;
use command::{
    augment_claude_launch_env, build_command_for_dir, build_docker_launcher_args,
    isolate_routed_copilot_home,
};
use context::persist_launcher_context;

// Test-visible re-imports from submodules. These become available to
// `#[cfg(test)] mod tests_*` children via `use super::*`.
#[cfg(test)]
use blarify::{
    BlarifyIndexAction, BlarifyMode, BlarifyPromptChoice, blarify_mode, consent_cache_path,
    has_blarify_consent, maybe_run_blarify_indexing_prompt_with, parse_blarify_prompt_choice,
    resolve_blarify_index_action, save_blarify_consent, should_prompt_blarify_indexing,
};
#[cfg(test)]
use checkout::{parse_github_repo_uri, resolve_checkout_repo_in};
#[cfg(test)]
use command::{COPILOT_HOME_ENV, build_command};
#[cfg(test)]
use context::render_launcher_command;
#[cfg(test)]
use power_steering::maybe_prompt_re_enable_power_steering_with;

use crate::binary_finder::BinaryInfo;
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

use amplihack_launcher::flag_matrix::AgentBinary;
use amplihack_launcher::prompt_delivery::validate_prompt_delivery_env_for;
use amplihack_utils::launch_target::OverrideOrigin;
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

const POWER_STEERING_PROMPT_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const RUSTYCLAWD_PACKAGE: &str = "rustyclawd-cli";
pub(crate) const RUSTYCLAWD_VERSION: &str = "0.1.1";
pub(crate) const RUSTYCLAWD_SOURCE: &str = "https://github.com/rysweet/RustyClawd";
pub(crate) const RUSTYCLAWD_REVISION: &str = "e03613a731ec2243590ccbb2b50db4dcf83ca69b";

fn apply_launch_environment(
    command: &mut std::process::Command,
    env_builder: EnvBuilder,
    proxy: Option<(
        &amplihack_utils::litellm_proxy::ProxyConfig,
        amplihack_utils::litellm_proxy::CliTarget,
    )>,
) {
    env_builder.apply_to_command(command);
    if let Some((config, target)) = proxy {
        config.apply_to_command(command, target);
    }
}

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
    override_origin: OverrideOrigin,
    proxy_target: Option<amplihack_utils::litellm_proxy::CliTarget>,
) -> Result<()> {
    validate_launch_prompt_delivery(tool)?;
    let proxy_config = amplihack_utils::litellm_proxy::ProxyConfig::from_env()
        .context("invalid external LiteLLM proxy configuration")?;
    let proxy_enabled = proxy_config.is_some();
    if proxy_enabled {
        let target = proxy_target.ok_or_else(|| {
            anyhow::anyhow!(
                "the external LiteLLM gateway supports only Claude, GitHub Copilot CLI, and rustyclawd; unset AMPLIHACK_LITELLM_ENDPOINT to launch {tool}"
            )
        })?;
        validate_proxy_launch_args(target, resume, continue_session, &extra_args)?;
    }

    if proxy_enabled && DockerDetector.requested_outside_container(docker) {
        match proxy_target {
            Some(amplihack_utils::litellm_proxy::CliTarget::Claude) => {
                anyhow::bail!(
                    "external LiteLLM routing cannot launch Claude Code through Docker because the container executable cannot be verified before Docker image or container operations; launch outside Docker or start inside a trusted container"
                );
            }
            Some(amplihack_utils::litellm_proxy::CliTarget::CopilotCli) => {
                anyhow::bail!(
                    "external LiteLLM routing cannot launch GitHub Copilot CLI through Docker because the container executable cannot be verified before Docker image or container operations; launch outside Docker or start inside a trusted container"
                );
            }
            _ => {}
        }
    }

    let prevalidated_binary = if proxy_enabled {
        match proxy_target {
            Some(amplihack_utils::litellm_proxy::CliTarget::Claude)
            | Some(amplihack_utils::litellm_proxy::CliTarget::CopilotCli) => {
                Some(preflight_proxy_binary(
                    tool,
                    override_origin,
                    proxy_target.expect("matched routed target"),
                )?)
            }
            _ => None,
        }
    } else {
        None
    };

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
    maybe_print_npm_update_notice(tool, skip_update_check, override_origin);

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
    let (binary, preflight_identity) = match prevalidated_binary {
        Some(preflight) => (preflight.binary, Some(preflight.identity)),
        None => (
            bootstrap::ensure_tool_available(tool, override_origin)
                .with_context(|| missing_binary_context(tool))?,
            None,
        ),
    };

    if let Some(proxy_target) = proxy_target {
        validate_proxy_launch_args(proxy_target, resume, continue_session, &extra_args)?;
        if proxy_enabled {
            validate_proxy_binary_capability(proxy_target, &binary)?;
        }
    }

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
        let env_builder = augment_claude_launch_env(env_builder, tool, Some(binary.path.as_path()))
            .set_if(is_noninteractive(), "AMPLIHACK_NONINTERACTIVE", "1")
            .set_if(no_reflection, "AMPLIHACK_SKIP_REFLECTION", "1"); // WS2: propagate flags

        maybe_run_blarify_indexing_prompt(tool, is_noninteractive(), Some(&execution_dir))?;

        // Register before the final executable check so revalidation, command
        // construction, credential projection, and spawn remain adjacent.
        let shutdown = signals::register_handlers()?;

        if let (Some(identity), Some(target)) = (&preflight_identity, proxy_target) {
            revalidate_proxy_binary(tool, override_origin, target, &binary, identity)?;
        }

        // This final check catches ordinary package and symlink replacement
        // after preflight and narrows same-user races without claiming the
        // stronger guarantees of descriptor-based execution.
        let mut cmd = build_command_for_dir(
            &binary,
            resume,
            continue_session,
            skip_permissions,
            &extra_args,
            Some(&execution_dir),
            subprocess_safe,
        );
        cmd.current_dir(&execution_dir);
        let routed_copilot = proxy_config.is_some()
            && proxy_target == Some(amplihack_utils::litellm_proxy::CliTarget::CopilotCli);
        validate_routed_copilot_workspace(&execution_dir, routed_copilot)?;
        let _isolated_copilot_home = isolate_routed_copilot_home(&mut cmd, routed_copilot)
            .context("failed to create isolated Copilot home for external LiteLLM launch")?;
        apply_launch_environment(
            &mut cmd,
            env_builder,
            proxy_config.as_ref().zip(proxy_target),
        );

        // Spawn child in its own process group.
        //
        // Issue #1266, Defect 3: a raw spawn failure here used to surface as
        // `failed to spawn child process: Exec format error (os error 8)`,
        // which named nothing real and sent the user hunting for a CPU
        // problem that did not exist. Resolution already knows what it tried
        // and why each candidate was rejected — say that instead.
        let mut child = match ManagedChild::spawn(cmd) {
            Ok(child) => child,
            Err(err) => {
                let raw_os_error = err
                    .downcast_ref::<std::io::Error>()
                    .and_then(std::io::Error::raw_os_error);
                // The tool and its package, not claude's: this path runs for
                // copilot and codex too.
                let package = crate::bootstrap::npm_package_for_install(tool).unwrap_or(tool);
                anyhow::bail!(
                    "{}",
                    crate::launcher::enrich_spawn_error(
                        raw_os_error,
                        &binary.path,
                        package,
                        &amplihack_utils::launch_target::resolve(tool, override_origin)
                            .rejection_report(tool, package),
                    )
                );
            }
        };

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

pub(crate) fn preflight_claude_proxy_binary(
    tool: &str,
    override_origin: OverrideOrigin,
) -> Result<BinaryInfo> {
    preflight_proxy_binary(
        tool,
        override_origin,
        amplihack_utils::litellm_proxy::CliTarget::Claude,
    )
    .map(|preflight| preflight.binary)
}

pub(crate) fn preflight_copilot_proxy_binary(
    tool: &str,
    override_origin: OverrideOrigin,
) -> Result<BinaryInfo> {
    preflight_proxy_binary(
        tool,
        override_origin,
        amplihack_utils::litellm_proxy::CliTarget::CopilotCli,
    )
    .map(|preflight| preflight.binary)
}

#[derive(Debug)]
struct ProxyBinaryPreflight {
    binary: BinaryInfo,
    identity: ExecutableIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExecutableIdentity {
    canonical_path: PathBuf,
    metadata: StableExecutableMetadata,
    digest: [u8; 32],
}

#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct StableExecutableMetadata {
    device: u64,
    inode: u64,
    length: u64,
    mode: u32,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
}

#[cfg(not(unix))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct StableExecutableMetadata {
    length: u64,
    modified: std::time::SystemTime,
}

fn capture_executable_identity(path: &Path) -> Result<ExecutableIdentity> {
    let canonical_path = path
        .canonicalize()
        .with_context(|| format!("failed to canonicalize executable {}", path.display()))?;
    let mut executable = std::fs::File::open(&canonical_path).with_context(|| {
        format!(
            "failed to open executable {} for identity capture",
            canonical_path.display()
        )
    })?;
    let metadata = executable.metadata().with_context(|| {
        format!(
            "failed to read executable metadata for {}",
            canonical_path.display()
        )
    })?;
    #[cfg(unix)]
    let metadata = {
        use std::os::unix::fs::MetadataExt;
        StableExecutableMetadata {
            device: metadata.dev(),
            inode: metadata.ino(),
            length: metadata.len(),
            mode: metadata.mode(),
            modified_seconds: metadata.mtime(),
            modified_nanoseconds: metadata.mtime_nsec(),
            changed_seconds: metadata.ctime(),
            changed_nanoseconds: metadata.ctime_nsec(),
        }
    };
    #[cfg(not(unix))]
    let metadata = StableExecutableMetadata {
        length: metadata.len(),
        modified: metadata.modified().with_context(|| {
            format!(
                "failed to read executable modification time for {}",
                canonical_path.display()
            )
        })?,
    };
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let bytes_read = executable.read(&mut buffer).with_context(|| {
            format!(
                "failed to hash executable contents for {}",
                canonical_path.display()
            )
        })?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }
    Ok(ExecutableIdentity {
        canonical_path,
        metadata,
        digest: hasher.finalize().into(),
    })
}

fn preflight_proxy_binary(
    tool: &str,
    override_origin: OverrideOrigin,
    target: amplihack_utils::litellm_proxy::CliTarget,
) -> Result<ProxyBinaryPreflight> {
    let resolution = amplihack_utils::launch_target::resolve(tool, override_origin);
    let (display_name, package) = match target {
        amplihack_utils::litellm_proxy::CliTarget::Claude => {
            ("Claude Code", "@anthropic-ai/claude-code")
        }
        amplihack_utils::litellm_proxy::CliTarget::CopilotCli => {
            ("GitHub Copilot CLI", "@github/copilot")
        }
        amplihack_utils::litellm_proxy::CliTarget::RustyClawd => ("RustyClawd", "rustyclawd"),
    };
    let rejection_report = resolution.rejection_report(tool, package);
    let resolved = resolution.target.ok_or_else(|| {
        anyhow::anyhow!(
            "{display_name} capability could not be verified before external LiteLLM launch:\n{}",
            rejection_report
        )
    })?;
    let binary = BinaryInfo {
        name: tool.to_string(),
        path: resolved.path,
        version: Some(resolved.version),
    };
    validate_proxy_binary_capability(target, &binary)?;
    let identity = capture_executable_identity(&binary.path)
        .context("failed to capture routed executable identity during preflight")?;
    Ok(ProxyBinaryPreflight { binary, identity })
}

fn revalidate_proxy_binary(
    tool: &str,
    override_origin: OverrideOrigin,
    target: amplihack_utils::litellm_proxy::CliTarget,
    expected: &BinaryInfo,
    expected_identity: &ExecutableIdentity,
) -> Result<()> {
    let resolution = amplihack_utils::launch_target::resolve_uncached(tool, override_origin);
    let rejection_report = resolution.rejection_report(tool, tool);
    let resolved = resolution.target.ok_or_else(|| {
        anyhow::anyhow!(
            "routed executable changed after preflight and no longer passes verification:\n{}",
            rejection_report
        )
    })?;
    let actual = BinaryInfo {
        name: tool.to_string(),
        path: resolved.path,
        version: Some(resolved.version),
    };
    validate_proxy_binary_capability(target, &actual)?;
    let actual_identity = capture_executable_identity(&actual.path)
        .context("failed to revalidate routed executable identity immediately before spawn")?;

    if actual.version != expected.version || actual_identity != *expected_identity {
        anyhow::bail!(
            "routed executable changed after preflight; refusing to spawn (expected {} version {}, resolved {} version {})",
            expected_identity.canonical_path.display(),
            expected.version.as_deref().unwrap_or("unknown"),
            actual_identity.canonical_path.display(),
            actual.version.as_deref().unwrap_or("unknown"),
        );
    }
    Ok(())
}

fn validate_proxy_binary_capability(
    target: amplihack_utils::litellm_proxy::CliTarget,
    binary: &BinaryInfo,
) -> Result<()> {
    match target {
        amplihack_utils::litellm_proxy::CliTarget::Claude => {
            amplihack_utils::litellm_proxy::require_claude_code_capability(
                binary.version.as_deref(),
            )
            .context("Claude Code external LiteLLM capability check failed")?;
        }
        amplihack_utils::litellm_proxy::CliTarget::CopilotCli => {
            amplihack_utils::litellm_proxy::require_copilot_cli_capability(
                binary.version.as_deref(),
            )
            .context("GitHub Copilot CLI external LiteLLM capability check failed")?;
        }
        amplihack_utils::litellm_proxy::CliTarget::RustyClawd => {
            require_rustyclawd_capability(&binary.path)
                .context("RustyClawd external LiteLLM capability check failed")?;
        }
    }
    Ok(())
}

pub(crate) fn require_rustyclawd_capability(executable: &Path) -> Result<()> {
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo")))
        .context("cannot locate Cargo installation receipt")?;
    let receipt = cargo_home.join(".crates2.json");
    let metadata = fs::symlink_metadata(&receipt)
        .with_context(|| format!("cannot inspect Cargo receipt {}", receipt.display()))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        anyhow::bail!("Cargo receipt must be a regular non-symlink file");
    }
    let document: serde_json::Value = serde_json::from_slice(
        &fs::read(&receipt)
            .with_context(|| format!("cannot read Cargo receipt {}", receipt.display()))?,
    )
    .context("Cargo receipt is malformed")?;
    let installs = document
        .get("installs")
        .and_then(serde_json::Value::as_object)
        .context("Cargo receipt has no installs object")?;
    let expected_receipt = format!(
        "{RUSTYCLAWD_PACKAGE} {RUSTYCLAWD_VERSION} (git+{RUSTYCLAWD_SOURCE}?rev={RUSTYCLAWD_REVISION}#{RUSTYCLAWD_REVISION})"
    );
    let candidates = installs
        .iter()
        .filter(|(package, value)| {
            package.starts_with(&format!("{RUSTYCLAWD_PACKAGE} "))
                && value
                    .get("bins")
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|bins| {
                        bins.iter()
                            .any(|bin| bin.as_str().is_some_and(|name| name == "rusty"))
                    })
        })
        .collect::<Vec<_>>();
    let authoritative = if let [(package, value)] = candidates.as_slice() {
        package.as_str() == expected_receipt
            && value
                .get("bins")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|bins| {
                    bins.as_slice() == [serde_json::Value::String("rusty".to_string())]
                })
    } else {
        false
    };
    if !authoritative {
        anyhow::bail!(
            "RustyClawd must have one authoritative git receipt for {RUSTYCLAWD_SOURCE} at revision {RUSTYCLAWD_REVISION}; registry, path, tag-only, branch-only, and ambiguous receipts are rejected"
        );
    }
    let expected = fs::canonicalize(cargo_home.join("bin/rusty"))
        .context("cannot resolve Cargo-installed RustyClawd binary")?;
    let actual =
        fs::canonicalize(executable).context("cannot resolve selected RustyClawd binary")?;
    if actual != expected {
        anyhow::bail!("RustyClawd executable is not the canonical Cargo-installed binary");
    }
    Ok(())
}

fn validate_proxy_launch_args(
    target: amplihack_utils::litellm_proxy::CliTarget,
    resume: bool,
    continue_session: bool,
    extra_args: &[String],
) -> Result<()> {
    let mut effective_args = extra_args.to_vec();
    if resume {
        effective_args.push("--resume".to_string());
    }
    if continue_session {
        effective_args.push("--continue".to_string());
    }
    amplihack_utils::litellm_proxy::validate_launch_args(target, &effective_args)
        .context("invalid external LiteLLM proxy launch mode")
}

fn validate_launch_prompt_delivery(tool: &str) -> Result<()> {
    let Some(binary) = agent_binary_for_tool(tool) else {
        return Ok(());
    };
    validate_prompt_delivery_env_for(binary).with_context(|| {
        format!(
            "invalid prompt delivery configuration for {}",
            binary.env_value()
        )
    })?;
    Ok(())
}

fn agent_binary_for_tool(tool: &str) -> Option<AgentBinary> {
    match tool {
        "claude" | "rusty" | "rustyclawd" => Some(AgentBinary::Claude),
        "copilot" => Some(AgentBinary::Copilot),
        "codex" => Some(AgentBinary::Codex),
        "amplifier" => Some(AgentBinary::Amplifier),
        _ => None,
    }
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
            child.terminate();
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

/// Error context for "the tool could not be found", with the one explanation
/// the path-based search cannot give (issue #1317).
///
/// A `copilot` process launched from an npm package keeps running after a
/// reinstall unlinks its executable: the kernel holds the inode open, but the
/// path is gone. Every candidate then fails the health gate as `Missing`,
/// including `COPILOT_BINARY_PATH` set to the exact path the live parent
/// reports — which reads as "amplihack cannot find a binary that is obviously
/// right there" and sends the reader looking for a `$PATH` problem.
///
/// So when the search fails, say whether the calling process is itself running
/// a deleted executable. That is a fact about the machine that the search
/// cannot discover by looking at paths, because the file no longer has one.
fn missing_binary_context(tool: &str) -> String {
    let base = format!("could not find '{tool}' binary in PATH");
    match deleted_running_executable(tool) {
        Some(path) => format!(
            "{base}\n\
             A running ancestor process is executing a DELETED '{tool}' \
             executable ({}). The file was unlinked -- typically by a package \
             reinstall or upgrade -- while the process kept running, so no path \
             points at it any more and every candidate correctly fails as \
             missing. Reinstall '{tool}' so a real path exists, then retry.",
            path.display()
        ),
        None => base,
    }
}

/// Does this `/proc/<pid>/exe` link target name a deleted executable for `tool`?
///
/// Pure so the two decisions can be tested without a live process tree: that
/// the " (deleted)" suffix is what marks an unlinked image, and that a path is
/// only claimed when its file name is actually the tool. Matching loosely here
/// would put a confident, wrong sentence into an error message — worse than the
/// bare "not found" it replaces.
fn deleted_target_for_tool(raw: &str, tool: &str) -> Option<std::path::PathBuf> {
    let stripped = raw.strip_suffix(" (deleted)")?;
    let path = std::path::PathBuf::from(stripped);
    let name = path.file_name()?.to_string_lossy().into_owned();
    // `copilot` and `copilot.exe`, but never `copilot-shim` or `mycopilot`.
    if name == tool || name.strip_prefix(tool).is_some_and(|r| r.starts_with('.')) {
        Some(path)
    } else {
        None
    }
}

/// Is some ancestor process running a deleted executable whose name matches
/// `tool`? Returns the stale path as Linux reports it.
///
/// Linux appends " (deleted)" to the `/proc/<pid>/exe` link target once the
/// file is unlinked, which is the only signal available: `stat` on the path
/// fails, so nothing path-based can tell this case apart from "never installed".
///
/// Walks a bounded number of ancestors and stops at pid 1. Best-effort by
/// construction: it only ever produces a sentence in an error message that is
/// already being returned, so every failure to read simply yields `None`.
#[cfg(target_os = "linux")]
fn deleted_running_executable(tool: &str) -> Option<std::path::PathBuf> {
    const MAX_ANCESTORS: usize = 8;

    let mut pid = std::process::id();
    for _ in 0..MAX_ANCESTORS {
        if pid <= 1 {
            break;
        }
        if let Ok(target) = std::fs::read_link(format!("/proc/{pid}/exe"))
            && let Some(path) = deleted_target_for_tool(&target.to_string_lossy(), tool)
        {
            return Some(path);
        }
        pid = parent_pid(pid)?;
    }
    None
}

#[cfg(not(target_os = "linux"))]
fn deleted_running_executable(_tool: &str) -> Option<std::path::PathBuf> {
    None
}

/// Parent pid from `/proc/<pid>/stat`, read from the end so a process name
/// containing spaces or parentheses cannot shift the field index.
#[cfg(target_os = "linux")]
fn parent_pid(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_comm = stat.rfind(')')?;
    let mut fields = stat.get(after_comm + 1..)?.split_whitespace();
    let _state = fields.next()?;
    fields.next()?.parse().ok()
}

#[cfg(test)]
mod issue_1317_deleted_executable_tests {
    use super::*;

    #[test]
    fn a_live_path_is_not_claimed_as_deleted() {
        // The overwhelmingly common case: nothing was unlinked, so the message
        // must stay the plain "not found" and not invent an explanation.
        assert_eq!(deleted_target_for_tool("/usr/bin/copilot", "copilot"), None);
    }

    #[test]
    fn a_deleted_target_for_the_tool_is_claimed() {
        let got = deleted_target_for_tool(
            "/home/u/.npm/_npx/abc/node_modules/.bin/copilot (deleted)",
            "copilot",
        );
        assert_eq!(
            got.as_deref(),
            Some(std::path::Path::new(
                "/home/u/.npm/_npx/abc/node_modules/.bin/copilot"
            ))
        );
    }

    #[test]
    fn a_windows_style_suffix_still_matches() {
        assert!(deleted_target_for_tool("/opt/x/copilot.exe (deleted)", "copilot").is_some());
    }

    #[test]
    fn a_different_deleted_binary_is_not_claimed() {
        // Claiming this would put a confident, wrong sentence in the error --
        // worse than the bare "not found" it replaces.
        assert_eq!(
            deleted_target_for_tool("/usr/bin/node (deleted)", "copilot"),
            None
        );
    }

    #[test]
    fn a_name_that_merely_contains_the_tool_is_not_claimed() {
        assert_eq!(
            deleted_target_for_tool("/usr/bin/copilot-shim (deleted)", "copilot"),
            None
        );
        assert_eq!(
            deleted_target_for_tool("/usr/bin/mycopilot (deleted)", "copilot"),
            None
        );
    }

    #[test]
    fn context_always_contains_the_plain_message() {
        // Whatever the machine looks like, the original sentence survives.
        let msg = missing_binary_context("copilot");
        assert!(
            msg.contains("could not find 'copilot' binary in PATH"),
            "context lost the base message: {msg}"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn parent_pid_of_this_process_is_readable_and_nonzero() {
        let ppid = parent_pid(std::process::id()).expect("this process has a parent");
        assert!(ppid > 0, "parent pid must be positive, got {ppid}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_really_does_append_deleted_to_an_unlinked_exe() {
        // The whole diagnostic rests on this kernel behaviour, so assert it
        // against a real unlinked process image rather than trusting the docs.
        use std::io::Write;

        let dir = std::env::temp_dir().join(format!("amplihack-1317-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let script = dir.join("copilot");
        {
            let Ok(mut f) = std::fs::File::create(&script) else {
                return; // unwritable temp dir; nothing to assert
            };
            let _ = f.write_all(b"#!/bin/sh\nsleep 30\n");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755));
        }

        let Ok(mut child) = std::process::Command::new(&script).spawn() else {
            let _ = std::fs::remove_dir_all(&dir);
            return; // cannot spawn here; skip rather than fail spuriously
        };
        let _ = std::fs::remove_file(&script);

        let link = std::fs::read_link(format!("/proc/{}/exe", child.id()));
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&dir);

        // A shell script execs /bin/sh, so /proc/<pid>/exe points at the
        // interpreter, not the unlinked script. Only assert when the kernel
        // actually reports a deleted image.
        if let Ok(target) = link {
            let raw = target.to_string_lossy().into_owned();
            if raw.ends_with(" (deleted)") {
                assert!(
                    deleted_target_for_tool(&raw, "sh").is_some()
                        || deleted_target_for_tool(&raw, "copilot").is_some(),
                    "a deleted image was reported but not recognised: {raw}"
                );
            }
        }
    }
}
