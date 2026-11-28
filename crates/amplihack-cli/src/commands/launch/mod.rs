//! Launch commands for Claude, Copilot, Codex, and Amplifier.
//!
//! Builds the environment, finds the binary, checks nesting, and spawns
//! a `ManagedChild` with signal forwarding.

mod blarify;
mod checkout;
/// Issue #1985 — sandbox and hook-trust flags for Codex agent steps.
mod codex_agent_step;
mod codex_copilot_rewrite;
mod codex_exec_args;
/// Local-model launches of `amplihack codex`: a small Codex home of
/// amplihack's own and no hook text for the model.
pub(crate) mod codex_local_model;
mod command;
mod context;
/// Issue #2030 — the 0600 contract file and the Claude Code version gate.
mod contract_file;
pub(crate) mod hook_settings;
// Issue #3162: where a launch puts operator guidance, and drop records.
mod guidance_route;
// Issue #3435: whether a Codex step has amplihack's guidance hooks.
mod codex_hook_check;
/// Issue #1619 — a recipe step declares a model tier and amplihack turns it
/// into whatever the harness it launches actually has.
pub(crate) mod model_tiers;
/// Issue #2030 — `amplihack claude` passthrough arguments never carry prompt
/// text to the agent's argv.
mod passthrough_rewrite;
mod power_steering;
/// Issue #1265 Option 3 — delivery of amplihack's routing contract through
/// `--append-system-prompt-file` (issue #2030: a file, never argv text).
mod system_prompt_append;

#[cfg(test)]
mod tests_blarify;
#[cfg(test)]
mod tests_codex_agent_step;
#[cfg(test)]
mod tests_codex_copilot_rewrite;
#[cfg(test)]
mod tests_codex_exec_args;
#[cfg(test)]
mod tests_command;
#[cfg(test)]
mod tests_env;
#[cfg(test)]
mod tests_issue_2030_contract;
#[cfg(all(test, unix))]
mod tests_issue_2279_launch_settings;
#[cfg(test)]
mod tests_issue_2361;
#[cfg(all(test, unix))]
mod tests_issue_2535_guidance;
#[cfg(test)]
mod tests_issue_2589_copilot_deny;
#[cfg(all(test, unix))]
mod tests_issue_3162_route;
#[cfg(test)]
mod tests_launch;
#[cfg(test)]
mod tests_model;
#[cfg(test)]
mod tests_passthrough_rewrite;
#[cfg(test)]
mod tests_profile;
#[cfg(test)]
mod tests_root_sandbox;
#[cfg(test)]
mod tests_subprocess_safe;
#[cfg(test)]
mod tests_system_prompt_append;

// Re-exports — public API of the launch module.
pub(crate) use checkout::resolve_checkout_repo;
/// Issue #1619: the shipped `strong` tier reuses this id rather than naming a
/// second one that can drift away from it, which `model_tiers` pins.
#[cfg(test)]
pub(crate) use command::DEFAULT_MODEL;
pub(crate) use command::{
    pin_profile_for_descendants, resolve_no_reflection, resolve_subprocess_safe,
    validate_routed_copilot_workspace,
};
pub(crate) use power_steering::maybe_prompt_re_enable_power_steering;

// Internal imports from submodules used by run_launch.
use blarify::maybe_run_blarify_indexing_prompt;
use command::{
    augment_claude_launch_env, build_command_for_dir, build_docker_launcher_args,
    isolate_routed_copilot_home,
};
use context::persist_launcher_context;
use guidance_route::{
    GuidanceRoute, agent_binary_for_tool, guidance_route, report_guidance_drop, settle_route,
    step_has_hook, step_label,
};

// Test-visible re-imports from submodules. These become available to
// `#[cfg(test)] mod tests_*` children via `use super::*`.
#[cfg(test)]
use blarify::{
    BlarifyIndexAction, BlarifyMode, BlarifyPromptChoice, blarify_mode, consent_cache_path,
    has_blarify_consent, maybe_run_blarify_indexing_prompt_with, parse_blarify_prompt_choice,
    resolve_blarify_index_action, save_blarify_consent, should_prompt_blarify_indexing,
};
#[cfg(test)]
use checkout::{parse_github_repo_uri, resolve_checkout_repo_in_with};
#[cfg(test)]
use command::{COPILOT_HOME_ENV, build_command};
#[cfg(test)]
use context::{persist_launcher_context_with, render_launcher_command};
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
    root_sandbox: Option<&amplihack_utils::root_sandbox::SkipPermissionsEnv>,
) -> Result<()> {
    env_builder.apply_to_command(command);
    crate::commands::orch::autodrive::trusted::clean_retired_environment(command)?;
    if let Some((config, target)) = proxy {
        config.apply_to_command(command, target);
    }
    // Issue #1482: IS_SANDBOX=1 goes on this child only, never on amplihack,
    // and last, so nothing applied above can undo it.
    if let Some(decision) = root_sandbox {
        decision.apply(command)?;
    }
    Ok(())
}

/// Issue #1482: what a `claude` launch needs for `--dangerously-skip-permissions`
/// as root, or `None` when this launch does not pass the flag to claude.
///
/// Claude Code refuses the flag as root unless IS_SANDBOX=1 is set. `Err` when
/// it would refuse, so the launch stops before anything is spawned; the caller
/// applies the returned decision to the child command only. `detect` is
/// injected so tests can stand in for root and sandbox states.
pub(super) fn root_sandbox_for_launch(
    tool: &str,
    skip_permissions: bool,
    extra_args: &[String],
    detect: impl FnOnce() -> amplihack_utils::root_sandbox::SkipPermissionsEnv,
) -> Result<Option<amplihack_utils::root_sandbox::SkipPermissionsEnv>> {
    let passes_flag =
        skip_permissions || amplihack_utils::root_sandbox::args_skip_permissions(extra_args);
    if tool != "claude" || !passes_flag {
        return Ok(None);
    }
    let decision = detect();
    decision.check()?;
    Ok(Some(decision))
}

/// Launch a tool binary (claude, copilot, codex, amplifier).
///
/// Codex dispatch must call this inside [`codex_local_model::scoped`] with
/// the result of [`codex_local_model::prepare_launch`]. The scope supplies
/// the local selection to model resolution and bootstrap on this thread.
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
    run_launch_profiled(
        tool,
        launcher_command,
        docker,
        resume,
        continue_session,
        skip_permissions,
        skip_update_check,
        no_reflection,
        subprocess_safe,
        checkout_repo,
        extra_args,
        override_origin,
        proxy_target,
        None,
    )
}

/// [`run_launch`] with an optional Claude login-profile directory, exported to
/// the child as `CLAUDE_CONFIG_DIR` (Claude-compatible tools only).
#[allow(clippy::too_many_arguments)]
pub fn run_launch_profiled(
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
    claude_config_dir: Option<PathBuf>,
) -> Result<()> {
    run_launch_core(
        tool,
        launcher_command,
        docker,
        resume,
        continue_session,
        skip_permissions,
        skip_update_check,
        no_reflection,
        subprocess_safe,
        checkout_repo,
        extra_args,
        override_origin,
        proxy_target,
        claude_config_dir,
        amplihack_utils::root_sandbox::detect,
    )
}

/// [`run_launch`] with the issue #1482 root-sandbox decision injected, so
/// tests reach the call in the launch whatever uid they run as.
#[allow(clippy::too_many_arguments)]
#[cfg(test)]
pub(crate) fn run_launch_with(
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
    root_sandbox_decision: fn() -> amplihack_utils::root_sandbox::SkipPermissionsEnv,
) -> Result<()> {
    run_launch_core(
        tool,
        launcher_command,
        docker,
        resume,
        continue_session,
        skip_permissions,
        skip_update_check,
        no_reflection,
        subprocess_safe,
        checkout_repo,
        extra_args,
        override_origin,
        proxy_target,
        None,
        root_sandbox_decision,
    )
}

#[allow(clippy::too_many_arguments)]
fn run_launch_core(
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
    claude_config_dir: Option<PathBuf>,
    root_sandbox_decision: fn() -> amplihack_utils::root_sandbox::SkipPermissionsEnv,
) -> Result<()> {
    validate_launch_prompt_delivery(tool)?;
    // Issue #1619, launch order step 0: a recipe agent step declares a model
    // tier (`fast`, `standard`, `strong`), which the recipe runner puts on
    // this passthrough tail as `--model <word>`. Turn it into a model this
    // harness actually has — or remove it, so amplihack's existing chain
    // decides — before anything else reads the tail. The LiteLLM routing
    // check below refuses a launch whose explicit `--model` is not
    // AMPLIHACK_LITELLM_MODEL, so a tier word left in place would stop every
    // gateway launch before it started.
    //
    // `proxy_target` names which harnesses the gateway routes, and is handed
    // down rather than re-derived: Copilot CLI is a gateway target that
    // `is_claude_compatible_tool` excludes.
    let (extra_args, tier_launch) =
        model_tiers::resolve_passthrough(tool, &extra_args, proxy_target.is_some());
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
        eprintln!("{}", activation.message());
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
        // Issue #1811: return the code rather than exit here, so the caller's
        // governor slot guard is dropped and releases its slot first.
        return child_status(exit_code);
    }

    let local_model = codex_local_model::current(tool);
    codex_local_model::check_in_scope(tool, local_model.is_some())?;
    if local_model.is_none()
        && matches!(
            agent_binary_for_tool(tool),
            Some(AgentBinary::Claude | AgentBinary::Copilot | AgentBinary::Codex)
        )
    {
        codex_local_model::warn_inherited_hook_text(tool);
    }

    // Issue #2030, launch order step 1: rewrite the passthrough arguments of a
    // Claude, Codex or Copilot launch so no prompt text reaches the agent's
    // argv. After the Docker hand-off: the amplihack inside the container
    // rewrites its own. `rewritten` owns the per-session files; it lives
    // until this function returns, after the child has been waited on.
    //
    // Issue #2535: the run's operator guidance is read once, here, as the
    // step starts; nothing waits for or watches it. The block goes after
    // the step's own prompt, in full or not at all.
    let guidance = crate::run_guidance::GuidanceForLaunch::load();
    let rewritten = match agent_binary_for_tool(tool) {
        Some(AgentBinary::Claude) => passthrough_rewrite::rewrite_claude_passthrough(&extra_args)?,
        Some(AgentBinary::Codex) => codex_copilot_rewrite::rewrite_codex_passthrough(&extra_args)?,
        // The external LiteLLM gateway refuses `--add-dir` for Copilot, so
        // there the pointer file is readable only with path access.
        Some(AgentBinary::Copilot) => codex_copilot_rewrite::rewrite_copilot_passthrough(
            &extra_args,
            !proxy_enabled,
            copilot_has_path_access(&extra_args, subprocess_safe),
            guidance.block(),
        )?,
        _ => passthrough_rewrite::Rewritten::unchanged(extra_args.clone()),
    };
    // Issue #3162: a prompt that is already on the caller's stdin (the
    // runner's `claude -- … -p …` agent steps, Codex `exec … -`, Copilot
    // with no prompt flag) takes the block after it. Stdin is looked at only
    // when there is guidance to place. `fstat` says only that stdin may hold
    // a prompt; the forwarder decides by reading it (see below).
    let stdin_may_hold_prompt = guidance.block().is_some() && caller_stdin_may_hold_input();
    let guidance_route = guidance_route(
        tool,
        &guidance,
        &rewritten,
        &extra_args,
        stdin_may_hold_prompt,
    );
    // What a drop record names: never argv text (issue #3162).
    let guidance_label = step_label(tool, &extra_args);
    // Issue #3435: inside a recipe run, every agent launch is a new step,
    // named to its hooks so they can give it guidance appended while it
    // runs. The drop of a launch form no prompt can take is settled once
    // the launch knows whether the step has such a hook (below).
    let guidance_step_id = crate::run_guidance::step::step_id_for_launch();
    let stdin_trailer = guidance_route == GuidanceRoute::StdinTrailer;
    // "\n\n" and the block, written after a prompt that arrives on stdin.
    let guidance_injected = matches!(
        guidance_route,
        GuidanceRoute::Prompt | GuidanceRoute::StdinTrailer
    );
    let guidance_tail = if guidance_injected {
        guidance.trailer().unwrap_or_default()
    } else {
        Vec::new()
    };
    let extra_args = rewritten.args.clone();
    let opt_out = std::env::var(system_prompt_append::OPT_OUT_ENV).ok();

    // Issue #1482: decide now, after the Docker hand-off (the container launch
    // decides for itself), so a root host outside a sandbox fails here with a
    // message naming IS_SANDBOX=1 instead of inside claude.
    let root_sandbox =
        root_sandbox_for_launch(tool, skip_permissions, &extra_args, root_sandbox_decision)?;

    // Check for npm updates before doing anything else.
    // This is a no-op if skip_update_check is true, AMPLIHACK_NONINTERACTIVE is set,
    // or the tool has no npm package mapping.
    maybe_print_npm_update_notice(tool, skip_update_check, override_origin);

    let codex_binary = if tool == "codex" && prevalidated_binary.is_none() {
        Some(
            bootstrap::ensure_tool_available(tool, override_origin)
                .with_context(|| missing_binary_context(tool))?,
        )
    } else {
        None
    };
    // A local-model Codex launch runs the generic bootstrap only, so nothing
    // is written to the user's Codex home.
    if !subprocess_safe {
        if local_model.is_some() {
            codex_local_model::bootstrap()?;
        } else if tool == "codex" {
            let selected = prevalidated_binary
                .as_ref()
                .map(|p| &p.binary)
                .or(codex_binary.as_ref())
                .context("resolved Codex executable missing")?;
            bootstrap::prepare_codex_launcher(&selected.path)?;
        } else {
            bootstrap::prepare_launcher(tool)?;
        }
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
        None => match codex_binary {
            Some(binary) => (binary, None),
            None => match bootstrap::ensure_tool_available(tool, override_origin) {
                Ok(binary) => (binary, None),
                Err(error) => {
                    if let Some(gate) = version_gate_error_for_unresolved(
                        tool,
                        override_origin,
                        &rewritten,
                        opt_out.as_deref(),
                    ) {
                        return Err(gate);
                    }
                    return Err(error.context(missing_binary_context(tool)));
                }
            },
        },
    };

    // An interactive local-model launch needs a Codex whose hook-trust format
    // amplihack knows, or its command check would not run.
    if let Some(local) = local_model.as_deref() {
        local.check_codex_version(binary.version.as_deref(), &extra_args)?;
    }

    // A local-model launch builds, refreshes or verifies its Codex home now,
    // the last step that can stop it before anything is spawned, and adds
    // the local settings before the caller's Codex arguments.
    let local_home = local_model
        .as_deref()
        .map(codex_local_model::Prepared::refresh)
        .transpose()?;
    let extra_args = match &local_home {
        Some(home) => home.codex_args(extra_args),
        None => extra_args,
    };

    // Issue #2916: a Claude effort the tier added is checked against the
    // version already read for the binary that will run, and taken back off
    // when that version is older than the first to accept `--effort` or could
    // not be read. Nothing is spawned for it. The record and the applied line
    // are written here, so they say what reached the harness; a launch that
    // returned earlier wrote them when `tier_launch` was dropped.
    let extra_args = tier_launch.finish(extra_args, binary.version.as_deref());

    if let Some(proxy_target) = proxy_target {
        validate_proxy_launch_args(proxy_target, resume, continue_session, &extra_args)?;
        if proxy_enabled {
            validate_proxy_binary_capability(proxy_target, &binary)?;
        }
    }

    // Issue #2030, launch order steps 2-4: decide injection on the rewritten
    // arguments, run the version gate, then write the contract file. All of
    // it happens before anything is spawned.
    let inject_contract = system_prompt_append::contract_injection_applies(
        &binary.name,
        &binary.path,
        &extra_args,
        opt_out.as_deref(),
    );
    if inject_contract || rewritten.used_file_flags {
        let version = binary
            .version
            .clone()
            .or_else(|| system_prompt_append::probe_claude_version(&binary.path));
        if inject_contract {
            system_prompt_append::require_append_prompt_file_capability(
                &binary.path,
                version.as_deref(),
            )?;
        } else {
            system_prompt_append::require_prompt_file_flag_capability(
                &binary.path,
                version.as_deref(),
            )?;
        }
    }
    let contract_file = if inject_contract {
        // Issue #3247: with `text.routing = off` the contract keeps only its
        // rules on waiting and on stopping processes.
        let part = if crate::user_config::routing_text_on(&current_dir) {
            system_prompt_append::ContractPart::Full
        } else {
            tracing::info!(
                key = "text.routing",
                "routing text off: the launch contract leaves out its router lines"
            );
            system_prompt_append::ContractPart::WithoutRouting
        };
        Some(system_prompt_append::ensure_contract_part_file(
            &system_prompt_append::contract_home()?,
            part,
        )?)
    } else {
        None
    };

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

    let result = (|| -> Result<i32> {
        // Build environment — canonical chain order per design spec.
        // SEC-DATA-01: Never log the full env map (may contain inherited secrets).
        let mut env_builder = EnvBuilder::new()
            .with_amplihack_session_id() // AMPLIHACK_SESSION_ID, AMPLIHACK_DEPTH
            .with_session_tree_context() // preserve orchestration tree vars if present
            .with_amplihack_vars_with_node_options(Some(node_options.as_str())) // AMPLIHACK_RUST_RUNTIME, AMPLIHACK_VERSION, NODE_OPTIONS
            // WS1: AMPLIHACK_AGENT_BINARY, still tagged if it was a guess (#1481)
            .with_launched_agent_binary(tool)
            .with_amplihack_home() // WS3: AMPLIHACK_HOME
            .with_asset_resolver(); // Rust-native bundle asset resolver
        env_builder = env_builder.with_project_graph_db(&execution_dir)?;
        let env_builder = augment_claude_launch_env(env_builder, tool, Some(binary.path.as_path()))
            .set_if(is_noninteractive(), "AMPLIHACK_NONINTERACTIVE", "1")
            .set_if(no_reflection, "AMPLIHACK_SKIP_REFLECTION", "1"); // WS2: propagate flags
        let env_builder = match &local_home {
            Some(home) => home.with_env(env_builder),
            None => env_builder,
        };
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
        let hook_settings = match hook_settings::prepare(&binary.name, &extra_args) {
            hook_settings::Inject::Path(path) => Some(path),
            hook_settings::Inject::NotPrivate => {
                // SR-15: the one skip the user can fix, so say it once.
                let dir = hook_settings::launch_file_path()
                    .and_then(|file| file.parent().map(std::path::Path::to_path_buf));
                eprintln!(
                    "amplihack: warning: hooks settings file not passed (runtime dir not \
                     private): {} and its parent must be owned by you and must not be group- \
                     or other-writable.",
                    dir.as_deref()
                        .map_or_else(|| "the amplihack home".into(), |d| d.display().to_string())
                );
                None
            }
            hook_settings::Inject::Skip(reason) => {
                tracing::debug!("no hooks --settings file: {reason}");
                None
            }
        };
        // Issue #3435: a step with a hook is given new guidance at its next
        // prompt or tool call; one without is recorded, so `recipe guide`
        // can tell the sender while the step runs. A hooked step is recorded
        // too: the judgement is not proof, and until one of its hooks runs
        // `recipe guide` names it as not yet confirmed.
        let step_hooked = step_has_hook(
            tool,
            hook_settings.is_some(),
            matches!(agent_binary_for_tool(tool), Some(AgentBinary::Codex))
                && codex_hook_check::codex_step_has_hook(local_model.is_some(), &extra_args),
        );
        if let Some(step_id) = guidance_step_id.as_deref() {
            crate::run_guidance::step::record_step_start(step_id, &guidance_label, step_hooked);
        }
        if let GuidanceRoute::Undeliverable(reason) = settle_route(guidance_route, step_hooked) {
            report_guidance_drop(&guidance, &guidance_label, reason);
        }
        let mut cmd = build_command_for_dir(
            &binary,
            resume,
            continue_session,
            skip_permissions,
            &extra_args,
            Some(&execution_dir),
            subprocess_safe,
            claude_config_dir.as_deref(),
            contract_file.as_deref(),
            hook_settings.as_deref(),
        )?;
        cmd.current_dir(&execution_dir);
        // Issue #2030, step 6: a rewritten `-p <text>` reaches the child on a
        // piped stdin. Without a payload the child inherits amplihack's stdin.
        // Input piped to amplihack itself is passed on after the prompt.
        // Issue #2535 and #3162: a prompt already on the caller's stdin
        // (Codex `exec … -`, Claude print mode, Copilot with no prompt flag)
        // is always forwarded when guidance follows it, with the block after.
        let forward_caller_stdin =
            (rewritten.stdin_payload.is_some() && caller_stdin_may_hold_input()) || stdin_trailer;
        if rewritten.stdin_payload.is_some() || stdin_trailer {
            cmd.stdin(std::process::Stdio::piped());
        }
        let routed_copilot = proxy_config.is_some()
            && proxy_target == Some(amplihack_utils::litellm_proxy::CliTarget::CopilotCli);
        validate_routed_copilot_workspace(&execution_dir, routed_copilot)?;
        let _isolated_copilot_home = isolate_routed_copilot_home(&mut cmd, routed_copilot)
            .context("failed to create isolated Copilot home for external LiteLLM launch")?;
        apply_launch_environment(
            &mut cmd,
            env_builder,
            proxy_config.as_ref().zip(proxy_target),
            root_sandbox.as_ref(),
        )?;
        // Issue #1569: only the supervising amplihack sees this marker.
        cmd.env_remove(crate::commands::profile::AUTO_SWITCH_SUPERVISED_ENV);
        // Issue #3435: the step's own id, never one inherited from a parent.
        match guidance_step_id.as_deref() {
            Some(step_id) => cmd.env(crate::run_guidance::step::STEP_ID_ENV, step_id),
            None => cmd.env_remove(crate::run_guidance::step::STEP_ID_ENV),
        };

        // Spawn child in its own process group.
        //
        // Issue #1266, Defect 3: a raw spawn failure here used to surface as
        // `failed to spawn child process: Exec format error (os error 8)`,
        // which named nothing real and sent the user hunting for a CPU
        // problem that did not exist. Resolution already knows what it tried
        // and why each candidate was rejected — say that instead.
        // Issue #1569: the hold keeps auto-switch swaps out of the profile
        // until the child exists and liveness scans can see it.
        let launch_hold = claude_config_dir
            .as_deref()
            .map(crate::commands::profile::verify_launch_profile_dir)
            .transpose()?;
        let spawned = ManagedChild::spawn(cmd);
        drop(launch_hold);
        let mut child = match spawned {
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

        // Issue #3435: when the block goes to the child's stdin, the writer
        // records it as soon as every byte is written and before stdin is
        // closed, so the step's own hooks, which run only after it has read
        // its prompt, see the record and do not give the entries again.
        let on_delivered = || -> amplihack_utils::payload_delivery::OnDelivered {
            let guidance = guidance.clone();
            let step_id = guidance_step_id.clone();
            Box::new(move || report_guidance_injection(&guidance, step_id.as_deref()))
        };
        let mut recorded_by_writer = false;
        let stdin_writer = match (rewritten.stdin_payload.as_ref(), child.take_stdin()) {
            (Some(payload), Some(stdin)) if forward_caller_stdin && guidance_injected => {
                recorded_by_writer = true;
                Some(
                    amplihack_utils::payload_delivery::spawn_stdin_forwarder_with_trailer_then(
                        stdin,
                        prompt_then_piped_input_header(payload),
                        Box::new(std::io::stdin()),
                        guidance_tail,
                        on_delivered(),
                    ),
                )
            }
            (Some(payload), Some(stdin)) if forward_caller_stdin => Some(
                amplihack_utils::payload_delivery::spawn_stdin_forwarder_with_trailer(
                    stdin,
                    prompt_then_piped_input_header(payload),
                    Box::new(std::io::stdin()),
                    guidance_tail,
                ),
            ),
            (Some(payload), Some(stdin)) if guidance_route == GuidanceRoute::Prompt => {
                recorded_by_writer = true;
                Some(amplihack_utils::payload_delivery::spawn_stdin_writer_then(
                    stdin,
                    guidance.append_to(payload).into_bytes(),
                    on_delivered(),
                ))
            }
            (Some(payload), Some(stdin)) => {
                Some(amplihack_utils::payload_delivery::spawn_stdin_writer(
                    stdin,
                    payload.clone().into_bytes(),
                ))
            }
            (None, Some(stdin)) if stdin_trailer => {
                recorded_by_writer = true;
                Some(
                    amplihack_utils::payload_delivery::spawn_stdin_forwarder_with_trailer_then(
                        stdin,
                        Vec::new(),
                        Box::new(std::io::stdin()),
                        guidance_tail,
                        on_delivered(),
                    ),
                )
            }
            _ => None,
        };
        // Issue #2535: the agent has started with the block in its prompt
        // (a prompt file); say so once (counts only) and record it in each
        // guidance.log it came from. A block on stdin is recorded by its
        // writer, once every byte is written.
        if guidance_injected && !recorded_by_writer {
            report_guidance_injection(&guidance, guidance_step_id.as_deref());
        }

        // Wait for child or signal
        let exit_code = wait_for_child_or_signal(&mut child, &shutdown);
        // PR #2567 M2: the writer sets its flags before it closes the child's
        // stdin, and the child exits only after that EOF, so they are final
        // here. They are read before the writer is joined or detached.
        if stdin_trailer {
            match stdin_writer.as_ref() {
                // Recorded by the writer when the last byte was written.
                Some(writer) if writer.delivered() => {}
                // PR #3190 review: the pipe gave no text, so there was no
                // prompt to follow and the block was not written. Seen by
                // reading the pipe, not guessed from its file type.
                Some(writer) if !writer.saw_text() => report_guidance_drop(
                    &guidance,
                    &guidance_label,
                    crate::run_guidance::DropReason::NoPromptOnStdin,
                ),
                // Issue #3162: the block was due after the caller's prompt
                // and did not reach the child; the sender is told, not only
                // this log.
                _ => report_guidance_drop(
                    &guidance,
                    &guidance_label,
                    crate::run_guidance::DropReason::TrailerNotWritten,
                ),
            }
        }
        let written = match stdin_writer {
            // The child has exited. A forwarder still blocked on a caller
            // pipe that never closes has no reader left; it is detached.
            Some(writer) if forward_caller_stdin => writer.finish_if_done(),
            Some(writer) => Some(writer.finish()),
            None => None,
        };
        if let Some(Err(error)) = written {
            tracing::warn!(%error, "writing the prompt to the agent's stdin failed");
        }
        // Issue #1569: hand back logins the session borrowed, on success and
        // on error, before this process exits.
        if let Some(dir) = claude_config_dir.as_deref() {
            crate::commands::profile::return_after_session(dir);
        }
        let exit_code = exit_code?;
        tracker.complete_session(&session_id)?;
        Ok(exit_code)
    })();

    if result.is_err() {
        let _ = tracker.crash_session(&session_id);
    }
    child_status(result?)
}

/// Issue #2030: when the binary resolver refused every `claude` candidate
/// because `--version` printed no usable version, and the launch would need
/// the version gate, report the gate's error (naming 2.1.319 and the opt-out)
/// instead of a bare "not found".
fn version_gate_error_for_unresolved(
    tool: &str,
    override_origin: OverrideOrigin,
    rewritten: &passthrough_rewrite::Rewritten,
    opt_out: Option<&str>,
) -> Option<anyhow::Error> {
    if agent_binary_for_tool(tool) != Some(AgentBinary::Claude) {
        return None;
    }
    let inject =
        system_prompt_append::should_inject_system_prompt_append(tool, &rewritten.args, opt_out);
    if !inject && !rewritten.used_file_flags {
        return None;
    }
    let resolution = amplihack_utils::launch_target::resolve(tool, override_origin);
    let (path, _) = resolution.rejected.iter().find(|(_, rejection)| {
        matches!(
            rejection,
            amplihack_utils::launch_target::Rejection::UnparseableVersion
        )
    })?;
    let result = if inject {
        system_prompt_append::require_append_prompt_file_capability(path, None)
    } else {
        system_prompt_append::require_prompt_file_flag_capability(path, None)
    };
    result.err()
}

/// A child's non-zero exit status as a [`crate::command_error::CliExitError`],
/// which `main` turns into the same process exit code. Returning it, instead
/// of calling `std::process::exit` here, lets every guard held by a caller
/// (such as a governor slot, issue #1811) run its `Drop` first.
fn child_status(exit_code: i32) -> Result<()> {
    if exit_code == 0 {
        Ok(())
    } else {
        Err(crate::command_error::exit_error(exit_code))
    }
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

/// The first bytes the agent reads when a rewritten prompt takes its stdin
/// and input was also piped to amplihack: the prompt, then a blank line. The
/// piped input follows unchanged, copied as it arrives, so
/// `git diff | amplihack claude -p "review this"` still gives the agent both.
/// amplihack does not wait for the piped input to end before starting the
/// agent; like `claude -p` reading the pipe itself, the agent sees end of
/// input when the caller's writer closes it.
pub(crate) fn prompt_then_piped_input_header(prompt: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(prompt.len() + 2);
    bytes.extend_from_slice(prompt.as_bytes());
    bytes.extend_from_slice(b"\n\n");
    bytes
}

/// amplihack's stdin is a pipe, a socket or a non-empty regular file, as
/// opposed to a terminal, `/dev/null` or an empty file.
///
/// This says only that stdin *may* hold input: a pipe or socket is reported
/// whether or not anything was or will be written to it, and a non-empty
/// file may hold only white space. Where the answer matters for operator
/// guidance, the forwarder settles it by reading
/// ([`amplihack_utils::payload_delivery::StdinWriter::saw_text`]).
#[cfg(unix)]
fn caller_stdin_may_hold_input() -> bool {
    use std::os::unix::fs::FileTypeExt;
    // `/dev/stdin` names fd 0 on Linux and macOS; metadata follows it to the
    // pipe or file itself.
    match std::fs::metadata("/dev/stdin") {
        Ok(meta) => {
            let kind = meta.file_type();
            kind.is_fifo() || kind.is_socket() || (kind.is_file() && meta.len() > 0)
        }
        Err(_) => false,
    }
}

#[cfg(not(unix))]
fn caller_stdin_may_hold_input() -> bool {
    false
}

/// Copilot will run with access to every path: the caller passed
/// `--allow-all`, `--allow-all-paths` or `--yolo`, or [`build_command`] adds
/// `--allow-all` or the subprocess-safe `--allow-all-paths`.
fn copilot_has_path_access(extra_args: &[String], subprocess_safe: bool) -> bool {
    extra_args
        .iter()
        .take_while(|a| a.as_str() != "--")
        .any(|a| matches!(a.as_str(), "--allow-all" | "--allow-all-paths" | "--yolo"))
        || command::should_inject_copilot_allow_all(extra_args)
        || (subprocess_safe && command::should_inject_subprocess_safe_flags(extra_args).1)
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

/// Issue #2535: prints the counts-only notice that guidance reached the
/// agent and records `guidance_injected` in each guidance.log it came from.
/// Issue #3435: `step_id` names the step that was given it, so its hooks
/// do not give the same entries again.
fn report_guidance_injection(
    guidance: &crate::run_guidance::GuidanceForLaunch,
    step_id: Option<&str>,
) {
    eprintln!("{}", guidance.notice());
    guidance.record_injection(
        &std::env::var("AMPLIHACK_RECIPE_RUN_ID").unwrap_or_default(),
        step_id,
    );
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
                return Ok(exit_code_for_status(&status));
            }
            None => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    }
}

/// The exit code amplihack reports for a child that has finished, and the one
/// place a child's death by signal is put into words.
///
/// Issue #1424. This used to be `status.code().unwrap_or(0)`, which is 0 for
/// **every** signal death, not just the one it was written for. So the exact
/// failure that issue reports — a Copilot binary that started and then aborted
/// on `SIGABRT` — made `amplihack copilot` exit 0 and report success, and the
/// only account of what happened was whatever the child had already printed.
///
/// `SIGINT` keeps its 0: the user pressed Ctrl+C, that is not a failure, and
/// the parity with Python's `signal_handler → sys.exit(0)` is deliberate. Every
/// other signal is a death — reported by name, and exited `128 + N` the way a
/// shell reports it.
#[cfg(unix)]
fn exit_code_for_status(status: &std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    if let Some(code) = status.code() {
        return code;
    }
    let Some(signal) = status.signal() else {
        return 0;
    };
    if signal == libc::SIGINT {
        return 0;
    }
    eprintln!("{}", describe_child_signal(signal));
    128 + signal
}

#[cfg(not(unix))]
fn exit_code_for_status(status: &std::process::ExitStatus) -> i32 {
    status.code().unwrap_or(0)
}

/// What to tell the user about a child that was killed by a signal.
///
/// Pure, so the words can be tested without killing a process. Two rules:
/// it says the binary **ran** — that is the fact "no platform package found"
/// contradicted — and for the two signals that resource exhaustion actually
/// produces it points at the machine WITHOUT asserting the cause. Naming a
/// cause that is merely likely is the mistake this issue is about; naming what
/// to look at is not.
#[cfg(unix)]
fn describe_child_signal(signal: i32) -> String {
    let name = amplihack_utils::launch_target::signal_name(signal);
    let mut message = format!(
        "amplihack: the child process was killed by {name} (signal {signal}). \
         It was found and executed, so this is not a missing or incomplete \
         install."
    );
    if signal == libc::SIGABRT || signal == libc::SIGKILL {
        message.push_str(
            " A death by SIGABRT or SIGKILL on a loaded host is commonly \
             resource exhaustion: if the output above mentions \"Resource \
             temporarily unavailable\" or \"failed to spawn thread\", or the \
             kernel OOM-killed it, check the per-user process/thread limit \
             (`ulimit -u`), the cgroup limit (/sys/fs/cgroup/pids.max), load \
             and free memory before assuming anything is wrong with the \
             install.",
        );
    }
    message
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
#[cfg_attr(not(target_os = "linux"), allow(dead_code))] // /proc/<pid>/exe is Linux only
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
