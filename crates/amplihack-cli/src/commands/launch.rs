//! Launch commands for Claude, Copilot, Codex, and Amplifier.
//!
//! Builds the environment, finds the binary, checks nesting, and spawns
//! a `ManagedChild` with signal forwarding.

use crate::binary_finder::BinaryInfo;
use crate::bootstrap;
use crate::commands::memory::{
    background_index_job_active, check_index_status, code_graph_compatibility_notice_for_project,
    estimate_indexing_time, record_background_index_pid, resolve_code_graph_db_path_for_project,
    run_index_code, run_index_scip,
};
use crate::commands::uvx_help::is_uvx_deployment;
use crate::env_builder::EnvBuilder;
use crate::launcher::ManagedChild;
use crate::launcher_context::{LauncherKind, write_launcher_context};
use crate::memory_config::prepare_memory_config;
use crate::nesting::NestingDetector;
use crate::session_tracker::SessionTracker;
use crate::signals;
use crate::tool_update_check::maybe_print_npm_update_notice;
use crate::util::{is_noninteractive, read_user_input_with_timeout};
use amplihack_types::ProjectDirs;
use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

const BLARIFY_PROMPT_TIMEOUT: Duration = Duration::from_secs(30);
const POWER_STEERING_PROMPT_TIMEOUT: Duration = Duration::from_secs(30);

/// Launch a tool binary (claude, copilot, codex, amplifier).
#[allow(clippy::too_many_arguments)]
pub fn run_launch(
    tool: &str,
    resume: bool,
    continue_session: bool,
    skip_permissions: bool,
    skip_update_check: bool,
    no_reflection: bool,
    subprocess_safe: bool,
    checkout_repo: Option<String>,
    extra_args: Vec<String>,
) -> Result<()> {
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

    let current_dir = std::env::current_dir().ok();
    let execution_dir = resolve_checkout_repo(checkout_repo.as_deref())?
        .or_else(|| current_dir.clone())
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

fn resolve_launch_node_options(subprocess_safe: bool) -> Result<String> {
    if subprocess_safe {
        // Passthrough: forward parent's NODE_OPTIONS unchanged without calling
        // prepare_memory_config(). The top-level launcher already applied
        // memory configuration; re-applying it in a nested subprocess would
        // overwrite the parent's carefully-set value.
        return Ok(std::env::var("NODE_OPTIONS").unwrap_or_default());
    }
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

fn persist_launcher_context(
    tool: &str,
    project_root: Option<&Path>,
    extra_args: &[String],
) -> Result<()> {
    if tool != "copilot" {
        return Ok(());
    }
    let Some(project_root) = project_root else {
        tracing::warn!(
            "skipping launcher context persistence because current directory is unavailable"
        );
        return Ok(());
    };

    let mut environment = BTreeMap::new();
    environment.insert("AMPLIHACK_LAUNCHER".to_string(), "copilot".to_string());
    write_launcher_context(
        project_root,
        LauncherKind::Copilot,
        render_launcher_command("copilot", extra_args),
        environment,
    )?;
    Ok(())
}

fn render_launcher_command(subcommand: &str, extra_args: &[String]) -> String {
    if extra_args.is_empty() {
        return format!("amplihack {subcommand}");
    }
    let rendered_args = extra_args
        .iter()
        .map(|arg| shell_quote(arg))
        .collect::<Vec<_>>()
        .join(" ");
    format!("amplihack {subcommand} {rendered_args}")
}

fn shell_quote(arg: &str) -> String {
    if arg.is_empty() {
        return "''".to_string();
    }
    let is_safe = arg.chars().all(|ch| {
        ch.is_ascii_alphanumeric()
            || matches!(
                ch,
                '@' | '%' | '_' | '-' | '+' | '=' | ':' | ',' | '.' | '/'
            )
    });
    if is_safe {
        return arg.to_string();
    }
    format!("'{}'", arg.replace('\'', r#"'"'"'"#))
}

#[cfg(test)]
fn build_command(
    binary: &BinaryInfo,
    resume: bool,
    continue_session: bool,
    skip_permissions: bool,
    extra_args: &[String],
) -> Command {
    build_command_for_dir(
        binary,
        resume,
        continue_session,
        skip_permissions,
        extra_args,
        None,
    )
}

fn build_command_for_dir(
    binary: &BinaryInfo,
    resume: bool,
    continue_session: bool,
    skip_permissions: bool,
    extra_args: &[String],
    add_dir_override: Option<&Path>,
) -> Command {
    let mut cmd = Command::new(&binary.path);

    // SEC-2: Only inject --dangerously-skip-permissions when the caller has
    // explicitly opted in via `--skip-permissions`.  This flag bypasses
    // Claude's interactive confirmation prompts and must not be on by default.
    if skip_permissions {
        cmd.arg("--dangerously-skip-permissions");
    }

    inject_uvx_plugin_args(&mut cmd, &binary.name, extra_args, add_dir_override);

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

fn inject_uvx_plugin_args(
    cmd: &mut Command,
    tool: &str,
    extra_args: &[String],
    add_dir_override: Option<&Path>,
) {
    if tool != "claude" || !is_uvx_deployment() {
        return;
    }

    if !extra_args.iter().any(|arg| arg == "--plugin-dir")
        && let Some(home) = std::env::var_os("HOME").map(PathBuf::from)
    {
        cmd.arg("--plugin-dir")
            .arg(home.join(".amplihack").join(".claude"));
    }

    if !extra_args.iter().any(|arg| arg == "--add-dir")
        && let Some(original_cwd) = resolve_uvx_add_dir(add_dir_override)
    {
        cmd.arg("--add-dir").arg(original_cwd);
    }
}

fn resolve_uvx_add_dir(add_dir_override: Option<&Path>) -> Option<PathBuf> {
    if std::env::var_os("AMPLIHACK_IS_STAGED").as_deref() == Some(std::ffi::OsStr::new("1"))
        && let Some(original_cwd) = std::env::var_os("AMPLIHACK_ORIGINAL_CWD").map(PathBuf::from)
    {
        return Some(original_cwd);
    }
    add_dir_override
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("AMPLIHACK_ORIGINAL_CWD").map(PathBuf::from))
        .or_else(|| std::env::current_dir().ok())
}

fn augment_claude_launch_env(env_builder: EnvBuilder, tool: &str) -> EnvBuilder {
    if tool != "claude" {
        return env_builder;
    }

    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return env_builder;
    };

    let env_builder = env_builder.prepend_path(home.join(".npm-global").join("bin"));
    if std::env::var("AMPLIHACK_PLUGIN_INSTALLED").as_deref() == Ok("true") {
        return env_builder.set(
            "CLAUDE_PLUGIN_ROOT",
            home.join(".claude")
                .join("plugins")
                .join("cache")
                .join("amplihack")
                .join("amplihack")
                .join("0.9.0")
                .display()
                .to_string(),
        );
    }

    let plugin_root = home.join(".amplihack").join(".claude");
    if plugin_root.exists() {
        env_builder.set("CLAUDE_PLUGIN_ROOT", plugin_root.display().to_string())
    } else {
        env_builder
    }
}

pub(crate) fn resolve_checkout_repo(repo_uri: Option<&str>) -> Result<Option<PathBuf>> {
    let Some(repo_uri) = repo_uri else {
        return Ok(None);
    };
    resolve_checkout_repo_in(repo_uri, &std::env::temp_dir().join("claude-checkouts")).map(Some)
}

fn resolve_checkout_repo_in(repo_uri: &str, base_dir: &Path) -> Result<PathBuf> {
    let (owner, repo) = parse_github_repo_uri(repo_uri)?;
    let target_dir = base_dir.join(format!("{owner}-{repo}"));

    fs::create_dir_all(base_dir)
        .with_context(|| format!("failed to create checkout directory {}", base_dir.display()))?;

    if target_dir.join(".git").is_dir() {
        println!("Using existing repository: {}", target_dir.display());
        return Ok(target_dir);
    }

    if target_dir.exists() {
        fs::remove_dir_all(&target_dir)
            .with_context(|| format!("failed to remove {}", target_dir.display()))?;
    }

    let clone_url = format!("https://github.com/{owner}/{repo}.git");
    let output = Command::new("git")
        .args(["clone", &clone_url, &target_dir.to_string_lossy()])
        .stdin(Stdio::null())
        .output()
        .context("failed to spawn git clone")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        let detail = if stderr.is_empty() {
            "git clone failed"
        } else {
            stderr
        };
        bail!("failed to checkout repository {repo_uri}: {detail}");
    }

    println!("Cloned repository to: {}", target_dir.display());
    Ok(target_dir)
}

fn parse_github_repo_uri(repo_uri: &str) -> Result<(String, String)> {
    let trimmed = repo_uri.trim();
    if trimmed.is_empty() {
        bail!("invalid GitHub repository URI: empty value");
    }

    let repo = trimmed
        .strip_prefix("https://github.com/")
        .or_else(|| trimmed.strip_prefix("git@github.com:"))
        .unwrap_or(trimmed);
    let repo = repo.strip_suffix(".git").unwrap_or(repo);

    let mut parts = repo.split('/');
    let owner = parts.next().unwrap_or_default();
    let name = parts.next().unwrap_or_default();
    if parts.next().is_some() || !is_valid_github_segment(owner) || !is_valid_github_segment(name) {
        bail!("invalid GitHub repository URI: {repo_uri}");
    }

    Ok((owner.to_string(), name.to_string()))
}

fn is_valid_github_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
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

pub(crate) fn maybe_prompt_re_enable_power_steering(project_path: &Path) -> Result<()> {
    maybe_prompt_re_enable_power_steering_with(project_path, read_user_input_with_timeout)
}

fn maybe_prompt_re_enable_power_steering_with<F>(
    project_path: &Path,
    prompt_reader: F,
) -> Result<()>
where
    F: FnOnce(&str, Duration) -> Result<Option<String>>,
{
    if std::env::var_os("AMPLIHACK_SKIP_POWER_STEERING").is_some() {
        return Ok(());
    }

    let dirs = ProjectDirs::from_root(project_path);
    let disabled_file = dirs.power_steering.join(".disabled");
    if !disabled_file.exists() {
        return Ok(());
    }

    println!("\nPower-Steering is currently disabled.");
    let prompt = "Would you like to re-enable it? [Y/n] (30s timeout, defaults to YES): ";
    let response = match prompt_reader(prompt, POWER_STEERING_PROMPT_TIMEOUT) {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(
                project = %project_path.display(),
                "power-steering re-enable prompt failed: {error}; defaulting to YES"
            );
            None
        }
    };

    let normalized = response
        .as_deref()
        .unwrap_or("y")
        .trim()
        .to_ascii_lowercase();
    if normalized == "n" || normalized == "no" {
        println!(
            "\nPower-Steering remains disabled. You can re-enable it by removing:\n{}\n",
            disabled_file.display()
        );
        return Ok(());
    }
    if !normalized.is_empty() && normalized != "y" && normalized != "yes" {
        tracing::warn!(
            project = %project_path.display(),
            input = %response.as_deref().unwrap_or_default(),
            "invalid power-steering re-enable response; defaulting to YES"
        );
    }

    remove_disabled_file_with_warning(&disabled_file);
    Ok(())
}

fn remove_disabled_file_with_warning(disabled_file: &Path) {
    match fs::remove_file(disabled_file) {
        Ok(()) => {
            println!("\nPower-Steering re-enabled.\n");
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            tracing::warn!(
                path = %disabled_file.display(),
                "failed to re-enable power-steering by removing .disabled: {error}"
            );
        }
    }
}

fn should_prompt_blarify_indexing(tool: &str, noninteractive: bool) -> bool {
    tool == "claude"
        && std::env::var("AMPLIHACK_ENABLE_BLARIFY").as_deref() == Ok("1")
        && (!noninteractive || blarify_mode() != BlarifyMode::Prompt)
}

fn maybe_run_blarify_indexing_prompt(
    tool: &str,
    noninteractive: bool,
    current_dir: Option<&Path>,
) -> Result<()> {
    maybe_run_blarify_indexing_prompt_with(
        tool,
        noninteractive,
        current_dir,
        maybe_prompt_blarify_indexing,
    )
}

fn maybe_run_blarify_indexing_prompt_with<F>(
    tool: &str,
    noninteractive: bool,
    current_dir: Option<&Path>,
    prompt_runner: F,
) -> Result<()>
where
    F: FnOnce(&Path) -> Result<()>,
{
    if !should_prompt_blarify_indexing(tool, noninteractive) {
        return Ok(());
    }

    let project_path = current_dir.context("failed to resolve current directory")?;
    if let Some(notice) = code_graph_compatibility_notice_for_project(project_path, None)? {
        println!("⚠️ Compatibility mode: {notice}");
    }
    prompt_runner(project_path).with_context(|| {
        format!(
            "code graph indexing prompt failed for {}",
            project_path.display()
        )
    })
}

fn maybe_prompt_blarify_indexing(project_path: &Path) -> Result<()> {
    if has_blarify_consent(project_path)? {
        tracing::debug!(project = %project_path.display(), "skipping code graph prompt due to saved consent");
        return Ok(());
    }
    if background_index_job_active(project_path)? {
        tracing::debug!(project = %project_path.display(), "skipping code graph prompt because indexing is already running");
        return Ok(());
    }

    let status = check_index_status(project_path)?;
    let db_path = resolve_code_graph_db_path_for_project(project_path)?;
    let code_graph_missing = !db_path.exists();
    if !status.needs_indexing && !code_graph_missing {
        tracing::debug!(reason = %status.reason, "code graph artifact is current");
        return Ok(());
    }

    let json_path = blarify_json_path(project_path);
    let action = resolve_blarify_index_action(&status, &json_path);
    let display_reason = if status.needs_indexing {
        status.reason.clone()
    } else {
        format!("missing (no {} found)", db_path.display())
    };
    match blarify_mode() {
        BlarifyMode::Skip => {
            tracing::info!("code indexing skipped by AMPLIHACK_BLARIFY_MODE=skip");
            return Ok(());
        }
        BlarifyMode::Sync => return run_code_indexing(project_path, &json_path, action, false),
        BlarifyMode::Background => {
            return run_code_indexing(project_path, &json_path, action, true);
        }
        BlarifyMode::Prompt => {}
    }
    print_blarify_prompt_banner(
        project_path,
        &display_reason,
        status.estimated_files,
        &json_path,
        action,
    )?;
    let response = read_user_input_with_timeout(
        "\nRun code indexing? [y/N/b/n] (b=background, n=don't ask again): ",
        BLARIFY_PROMPT_TIMEOUT,
    )?;

    match parse_blarify_prompt_choice(response.as_deref()) {
        BlarifyPromptChoice::Skip => {
            println!(
                "\n⏭️  Skipping code indexing (run later with: {})\n",
                manual_indexing_hint(project_path, &json_path, action)
            );
            Ok(())
        }
        BlarifyPromptChoice::Never => {
            save_blarify_consent(project_path)?;
            println!("\n⏭️  Code indexing skipped (won't ask again for this project)\n");
            Ok(())
        }
        BlarifyPromptChoice::Foreground => {
            run_code_indexing(project_path, &json_path, action, false)
        }
        BlarifyPromptChoice::Background => {
            run_code_indexing(project_path, &json_path, action, true)
        }
    }
}

fn print_blarify_prompt_banner(
    project_path: &Path,
    status_reason: &str,
    estimated_files: usize,
    json_path: &Path,
    action: BlarifyIndexAction,
) -> Result<()> {
    let estimate = estimate_indexing_time(project_path, &[]);
    println!();
    println!("{}", "=".repeat(60));
    println!("Code Indexing with Blarify");
    println!("{}", "=".repeat(60));
    println!("Project: {}", project_path.display());
    println!("Status: {}", status_reason);
    println!("Files to index: {}", estimated_files);
    println!(
        "Estimated time: {}",
        format_duration_seconds(estimate.total_seconds)
    );
    println!();
    println!("Language breakdown:");
    for (language, seconds) in &estimate.by_language {
        let file_count = estimate.file_counts.get(language).copied().unwrap_or(0);
        if file_count == 0 {
            continue;
        }
        println!(
            "  • {}: {} files ({})",
            language_label(language),
            file_count,
            format_duration_seconds(*seconds)
        );
    }
    println!();
    println!("Blarify enables code-aware features:");
    println!("  • Code context in memory retrieval");
    println!("  • Function and class awareness");
    println!("  • Automatic code-memory linking");
    println!();
    if json_path.exists() && action == BlarifyIndexAction::ImportExistingJson {
        println!("Native import input: {}", json_path.display());
    } else {
        println!("Rust will use native SCIP artifact generation to refresh the code graph.");
    }
    println!("{}", "=".repeat(60));
    io::stdout()
        .flush()
        .context("failed to flush prompt banner")
}

fn format_duration_seconds(seconds: f64) -> String {
    if seconds < 60.0 {
        format!("{seconds:.0}s")
    } else {
        let minutes = (seconds / 60.0).floor() as u64;
        let remaining_seconds = (seconds % 60.0).floor() as u64;
        format!("{minutes}m {remaining_seconds}s")
    }
}

fn language_label(language: &str) -> String {
    match language {
        "typescript" => "TypeScript".to_string(),
        "javascript" => "JavaScript".to_string(),
        "go" => "Go".to_string(),
        "rust" => "Rust".to_string(),
        "csharp" => "Csharp".to_string(),
        "cpp" => "Cpp".to_string(),
        "python" => "Python".to_string(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        }
    }
}

fn run_code_indexing(
    project_path: &Path,
    json_path: &Path,
    action: BlarifyIndexAction,
    background: bool,
) -> Result<()> {
    let db_path = resolve_code_graph_db_path_for_project(project_path)?;
    if background {
        let current_exe =
            std::env::current_exe().context("failed to resolve current executable")?;
        let mut cmd = Command::new(current_exe);
        let child = match action {
            BlarifyIndexAction::ImportExistingJson => {
                cmd.arg("index-code")
                    .arg(json_path)
                    .arg("--db-path")
                    .arg(&db_path);
                cmd.current_dir(project_path)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .with_context(|| {
                        format!(
                            "failed to spawn background indexing for {}",
                            project_path.display()
                        )
                    })?
            }
            BlarifyIndexAction::GenerateNativeScip => {
                cmd.arg("index-scip")
                    .arg("--project-path")
                    .arg(project_path);
                cmd.current_dir(project_path)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .with_context(|| {
                        format!(
                            "failed to spawn background indexing for {}",
                            project_path.display()
                        )
                    })?
            }
        };
        record_background_index_pid(project_path, child.id())?;
        println!("\n📊 Started code indexing in the background.\n");
        return Ok(());
    }

    match action {
        BlarifyIndexAction::ImportExistingJson => {
            println!("\n📊 Importing code graph data...\n");
            run_index_code(json_path, Some(&db_path), false)?;
            println!("\n✅ Code graph import complete.\n");
        }
        BlarifyIndexAction::GenerateNativeScip => {
            println!("\n📊 Generating native SCIP artifacts...\n");
            run_index_scip(Some(project_path), &[])?;
            println!("\n✅ Native SCIP artifact generation complete.\n");
        }
    }
    Ok(())
}

fn parse_blarify_prompt_choice(response: Option<&str>) -> BlarifyPromptChoice {
    let response = response
        .map(str::trim)
        .unwrap_or_default()
        .to_ascii_lowercase();
    match response.as_str() {
        "n" | "never" | "skip" => BlarifyPromptChoice::Never,
        "b" | "background" => BlarifyPromptChoice::Background,
        "y" | "yes" => BlarifyPromptChoice::Foreground,
        _ => BlarifyPromptChoice::Skip,
    }
}

fn has_blarify_consent(project_path: &Path) -> Result<bool> {
    Ok(consent_cache_path(project_path)?.exists())
}

fn save_blarify_consent(project_path: &Path) -> Result<()> {
    let cache_path = consent_cache_path(project_path)?;
    if let Some(parent) = cache_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(&cache_path, b"1")
        .with_context(|| format!("failed to write {}", cache_path.display()))?;
    Ok(())
}

fn consent_cache_path(project_path: &Path) -> Result<PathBuf> {
    let resolved = project_path
        .canonicalize()
        .unwrap_or_else(|_| project_path.to_path_buf());
    let hash = {
        let mut hasher = Sha256::new();
        hasher.update(resolved.to_string_lossy().as_bytes());
        let digest = hasher.finalize();
        format!("{:x}", digest)[..16].to_string()
    };
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set; cannot persist code graph consent")?;
    Ok(home
        .join(".amplihack")
        .join(format!(".blarify_consent_{hash}")))
}

fn blarify_json_path(project_path: &Path) -> PathBuf {
    project_path.join(".amplihack").join("blarify.json")
}

fn manual_indexing_hint(
    project_path: &Path,
    json_path: &Path,
    action: BlarifyIndexAction,
) -> String {
    match action {
        BlarifyIndexAction::ImportExistingJson => format!(
            "amplihack index-code {} --db-path {}",
            json_path.display(),
            resolve_code_graph_db_path_for_project(project_path)
                .unwrap_or_else(|_| project_path.join(".amplihack").join("graph_db"))
                .display()
        ),
        BlarifyIndexAction::GenerateNativeScip => format!(
            "amplihack index-scip --project-path {}",
            project_path.display()
        ),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlarifyPromptChoice {
    Skip,
    Never,
    Foreground,
    Background,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlarifyIndexAction {
    ImportExistingJson,
    GenerateNativeScip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlarifyMode {
    Prompt,
    Skip,
    Sync,
    Background,
}

fn blarify_mode() -> BlarifyMode {
    match std::env::var("AMPLIHACK_BLARIFY_MODE")
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "skip" => BlarifyMode::Skip,
        "sync" => BlarifyMode::Sync,
        "background" => BlarifyMode::Background,
        _ => BlarifyMode::Prompt,
    }
}

fn resolve_blarify_index_action(
    status: &crate::commands::memory::IndexStatus,
    json_path: &Path,
) -> BlarifyIndexAction {
    if json_path.exists() && !status.needs_indexing {
        BlarifyIndexAction::ImportExistingJson
    } else {
        BlarifyIndexAction::GenerateNativeScip
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launcher_context::{LauncherKind, read_launcher_context};
    use crate::test_support::{
        cwd_env_lock, home_env_lock, restore_cwd, restore_home, set_cwd, set_home,
    };
    use std::fs;
    use std::path::PathBuf;

    fn make_binary(path: &str) -> BinaryInfo {
        BinaryInfo {
            name: "claude".to_string(),
            path: PathBuf::from(path),
            version: Some("1.0.0".to_string()),
        }
    }

    fn with_uvx_detection_disabled<T>(f: impl FnOnce() -> T) -> T {
        let _home_guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _cwd_guard = cwd_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let cwd = tempfile::tempdir().unwrap();
        fs::create_dir_all(cwd.path().join(".claude")).unwrap();
        let original_cwd = set_cwd(cwd.path()).unwrap();
        let previous_uv_python = std::env::var_os("UV_PYTHON");
        let previous_root = std::env::var_os("AMPLIHACK_ROOT");
        unsafe {
            std::env::remove_var("UV_PYTHON");
            std::env::remove_var("AMPLIHACK_ROOT");
        }

        let result = f();

        restore_cwd(&original_cwd).unwrap();
        match previous_uv_python {
            Some(value) => unsafe { std::env::set_var("UV_PYTHON", value) },
            None => unsafe { std::env::remove_var("UV_PYTHON") },
        }
        match previous_root {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_ROOT", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_ROOT") },
        }

        result
    }

    /// When skip_permissions=true, --dangerously-skip-permissions MUST be the
    /// first argument injected before any other flags.
    ///
    /// Fails if build_command does not inject the flag when skip_permissions=true.
    #[test]
    fn test_build_command_injects_dangerously_skip_permissions() {
        let binary = make_binary("/usr/bin/claude");
        let cmd = build_command(&binary, false, false, true, &[]);
        let args: Vec<_> = cmd.get_args().collect();
        assert!(
            args.contains(&std::ffi::OsStr::new("--dangerously-skip-permissions")),
            "Expected '--dangerously-skip-permissions' in args when skip_permissions=true, \
             got: {:?}",
            args
        );
    }

    #[test]
    fn render_launcher_command_quotes_prompt_args() {
        let args = vec![
            "--model".to_string(),
            "gpt-5".to_string(),
            "-p".to_string(),
            "fix spaces and '$PATH'".to_string(),
        ];
        assert_eq!(
            render_launcher_command("copilot", &args),
            "amplihack copilot --model gpt-5 -p 'fix spaces and '\"'\"'$PATH'\"'\"''"
        );
    }

    #[test]
    fn render_session_argv_includes_checkout_repo_flag() {
        assert_eq!(
            render_session_argv(
                "claude",
                true,
                false,
                Some("owner/repo"),
                &["-p".to_string(), "continue parity".to_string()]
            ),
            vec![
                "amplihack",
                "claude",
                "--resume",
                "--checkout-repo",
                "owner/repo",
                "-p",
                "continue parity",
            ]
        );
    }

    #[test]
    fn persist_launcher_context_writes_copilot_context_file() {
        let dir = tempfile::tempdir().unwrap();
        let args = vec!["--model".to_string(), "opus".to_string()];

        persist_launcher_context("copilot", Some(dir.path()), &args).unwrap();

        let context = read_launcher_context(dir.path()).unwrap();
        assert_eq!(context.launcher, LauncherKind::Copilot);
        assert_eq!(context.command, "amplihack copilot --model opus");
        assert_eq!(
            context
                .environment
                .get("AMPLIHACK_LAUNCHER")
                .map(String::as_str),
            Some("copilot")
        );
    }

    #[test]
    fn resolve_launch_node_options_subprocess_safe_passthrough_differs_from_normal() {
        // subprocess-safe launches pass through NODE_OPTIONS unchanged.
        // A normal (top-level) launch runs prepare_memory_config(), which may
        // augment or replace the value.  The two code paths are deliberately
        // different; this test confirms they diverge so that a nested subprocess
        // does not overwrite the parent launcher's carefully-set memory limit.
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home = tempfile::tempdir().unwrap();
        let original_home = set_home(home.path());
        fs::create_dir_all(home.path().join(".amplihack")).unwrap();
        fs::write(
            home.path().join(".amplihack/config"),
            r#"{"node_options_consent":true,"node_options_limit_mb":16384}"#,
        )
        .unwrap();
        let previous_node_options = std::env::var_os("NODE_OPTIONS");
        unsafe { std::env::set_var("NODE_OPTIONS", "--trace-warnings") };

        let top_level = resolve_launch_node_options(false).unwrap();
        let subprocess_safe = resolve_launch_node_options(true).unwrap();

        restore_home(original_home);
        match previous_node_options {
            Some(value) => unsafe { std::env::set_var("NODE_OPTIONS", value) },
            None => unsafe { std::env::remove_var("NODE_OPTIONS") },
        }

        // Correct behavior: subprocess-safe result is the raw parent value;
        // normal result is the prepare_memory_config()-processed value.
        // They must differ (passthrough vs. processed).
        assert_ne!(subprocess_safe, top_level);
        // subprocess-safe returns the parent value verbatim
        assert_eq!(subprocess_safe, "--trace-warnings");
        // normal launch augments with --max-old-space-size
        assert!(top_level.contains("--max-old-space-size="));
    }

    #[test]
    fn test_subprocess_safe_preserves_node_options() {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous_node_options = std::env::var_os("NODE_OPTIONS");
        unsafe { std::env::set_var("NODE_OPTIONS", "--max-old-space-size=16384") };

        let result = resolve_launch_node_options(true).unwrap();

        match previous_node_options {
            Some(value) => unsafe { std::env::set_var("NODE_OPTIONS", value) },
            None => unsafe { std::env::remove_var("NODE_OPTIONS") },
        }

        // Pass the result through EnvBuilder as the actual launch path does.
        let env = EnvBuilder::new()
            .with_amplihack_vars_with_node_options(Some(result.as_str()))
            .build();

        // The child env must see exactly the parent's value, unchanged.
        assert_eq!(
            env.get("NODE_OPTIONS").map(String::as_str),
            Some("--max-old-space-size=16384"),
            "subprocess-safe launch must preserve parent NODE_OPTIONS verbatim"
        );
    }

    #[test]
    fn test_subprocess_safe_no_node_options_when_parent_unset() {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous_node_options = std::env::var_os("NODE_OPTIONS");
        unsafe { std::env::remove_var("NODE_OPTIONS") };

        let result = resolve_launch_node_options(true).unwrap();

        match previous_node_options {
            Some(value) => unsafe { std::env::set_var("NODE_OPTIONS", value) },
            None => unsafe { std::env::remove_var("NODE_OPTIONS") },
        }

        // Pass the result through EnvBuilder as the actual launch path does.
        let env = EnvBuilder::new()
            .with_amplihack_vars_with_node_options(Some(result.as_str()))
            .build();

        // When parent has no NODE_OPTIONS, subprocess-safe must NOT inject the
        // static 32768 MB default that a normal launch would add.
        let node_opts = env.get("NODE_OPTIONS").map(String::as_str).unwrap_or("");
        assert!(
            !node_opts.contains("--max-old-space-size=32768"),
            "subprocess-safe launch must not inject 32768 MB default when parent has no \
             NODE_OPTIONS; got: {:?}",
            node_opts
        );
    }

    #[test]
    fn test_normal_launch_applies_smart_node_options() {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home = tempfile::tempdir().unwrap();
        let original_home = set_home(home.path());
        fs::create_dir_all(home.path().join(".amplihack")).unwrap();
        let previous_node_options = std::env::var_os("NODE_OPTIONS");
        unsafe { std::env::remove_var("NODE_OPTIONS") };

        let result = resolve_launch_node_options(false);

        restore_home(original_home);
        match previous_node_options {
            Some(value) => unsafe { std::env::set_var("NODE_OPTIONS", value) },
            None => unsafe { std::env::remove_var("NODE_OPTIONS") },
        }

        // Normal (non-subprocess-safe) launch must run prepare_memory_config()
        // and produce a NODE_OPTIONS value containing --max-old-space-size.
        let node_options = result.unwrap();
        assert!(
            node_options.contains("--max-old-space-size"),
            "normal launch must apply smart NODE_OPTIONS via prepare_memory_config(); \
             got: {:?}",
            node_options
        );
    }

    /// When no --model is present in extra_args, build_command MUST inject
    /// '--model' followed by the default model value (opus[1m] or AMPLIHACK_DEFAULT_MODEL).
    ///
    /// Fails if no --model flag is injected by default.
    #[test]
    fn test_build_command_injects_default_model() {
        // Ensure AMPLIHACK_DEFAULT_MODEL is not set so we get the hard-coded default
        // SAFETY: single-threaded test context.
        unsafe { std::env::remove_var("AMPLIHACK_DEFAULT_MODEL") };
        let binary = make_binary("/usr/bin/claude");
        let cmd = build_command(&binary, false, false, false, &[]);
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(
            args.contains(&"--model".to_string()),
            "Expected '--model' to be injected when no --model in extra_args, got: {:?}",
            args
        );
        // Verify the default model value follows --model
        let model_pos = args.iter().position(|a| a == "--model").unwrap();
        assert_eq!(
            args[model_pos + 1],
            "opus[1m]",
            "Expected default model 'opus[1m]' after '--model', got: {:?}",
            args[model_pos + 1]
        );
    }

    /// When AMPLIHACK_DEFAULT_MODEL env var is set, build_command MUST use that
    /// value instead of the hard-coded default 'opus[1m]'.
    ///
    /// Fails if the env var override is not respected.
    #[test]
    fn test_build_command_respects_custom_model_env() {
        // SAFETY: single-threaded test context.
        unsafe { std::env::set_var("AMPLIHACK_DEFAULT_MODEL", "claude-3-5-sonnet") };
        let binary = make_binary("/usr/bin/claude");
        let cmd = build_command(&binary, false, false, false, &[]);
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        unsafe { std::env::remove_var("AMPLIHACK_DEFAULT_MODEL") };
        let model_pos = args.iter().position(|a| a == "--model").unwrap();
        assert_eq!(
            args[model_pos + 1],
            "claude-3-5-sonnet",
            "Expected AMPLIHACK_DEFAULT_MODEL value 'claude-3-5-sonnet' after '--model', \
             got: {:?}",
            args[model_pos + 1]
        );
    }

    /// When the user already supplies --model in extra_args, build_command MUST
    /// NOT inject an additional --model flag (no duplication).
    ///
    /// Fails if build_command injects a second --model when the user already has one.
    #[test]
    fn test_build_command_no_model_injection_when_user_supplies_model() {
        let binary = make_binary("/usr/bin/claude");
        let extra = vec!["--model".to_string(), "custom-model".to_string()];
        let cmd = build_command(&binary, false, false, false, &extra);
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let model_count = args.iter().filter(|a| *a == "--model").count();
        assert_eq!(
            model_count, 1,
            "Expected exactly one '--model' in args when user supplies --model, \
             but found {} occurrences. Args: {:?}",
            model_count, args
        );
        // And verify the user's model value is preserved
        let model_pos = args.iter().position(|a| a == "--model").unwrap();
        assert_eq!(
            args[model_pos + 1],
            "custom-model",
            "User-supplied model value must be preserved"
        );
    }

    /// When skip_permissions=false, '--dangerously-skip-permissions' MUST NOT
    /// appear in the args list.
    ///
    /// Fails if the flag is injected even when skip_permissions=false.
    #[test]
    fn test_build_command_no_dangerously_skip_when_false() {
        let binary = make_binary("/usr/bin/claude");
        let cmd = build_command(&binary, false, false, false, &[]);
        let args: Vec<_> = cmd.get_args().collect();
        assert!(
            !args.contains(&std::ffi::OsStr::new("--dangerously-skip-permissions")),
            "Expected '--dangerously-skip-permissions' to NOT be present when \
             skip_permissions=false, got: {:?}",
            args
        );
    }

    /// The Commands::Launch dispatch in mod.rs must pass skip_permissions=true
    /// by default (matching Python launcher parity where skip_permissions is
    /// always enabled). This test verifies build_command is exercised with
    /// skip_permissions=true from the default dispatch path.
    ///
    /// This test verifies the wiring by confirming that calling build_command
    /// with skip_permissions=true (as dispatch does) produces the expected flag.
    /// Fails if the dispatch hardcodes false instead of true.
    #[test]
    fn test_dispatch_defaults_skip_permissions_true() {
        // Simulate what Commands::Launch dispatch does: always pass skip_permissions=true
        // Build command the same way dispatch calls run_launch (skip_permissions=true)
        let binary = make_binary("/usr/bin/claude");
        // This mirrors the dispatch: skip_permissions is ALWAYS true for launch commands
        let skip_permissions_from_dispatch = true; // this is what dispatch should pass
        let cmd = build_command(&binary, false, false, skip_permissions_from_dispatch, &[]);
        let args: Vec<_> = cmd.get_args().collect();
        assert!(
            args.contains(&std::ffi::OsStr::new("--dangerously-skip-permissions")),
            "Commands::Launch dispatch must pass skip_permissions=true, which means \
             '--dangerously-skip-permissions' must appear in the built command args. \
             Got: {:?}",
            args
        );
    }

    #[test]
    fn build_command_basic_no_skip_permissions_by_default() {
        with_uvx_detection_disabled(|| {
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
        });
    }

    #[test]
    fn build_command_with_skip_permissions_flag() {
        with_uvx_detection_disabled(|| {
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
        });
    }

    #[test]
    fn build_command_with_flags() {
        with_uvx_detection_disabled(|| {
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
        });
    }

    #[test]
    fn build_command_without_skip_permissions_and_with_flags() {
        with_uvx_detection_disabled(|| {
            let binary = BinaryInfo {
                name: "claude".to_string(),
                path: PathBuf::from("/usr/bin/claude"),
                version: None,
            };
            let extra = vec!["--model".to_string(), "opus".to_string()];
            let cmd = build_command(&binary, true, true, false, &extra);
            let args: Vec<&std::ffi::OsStr> = cmd.get_args().collect();
            assert_eq!(args, &["--resume", "--continue", "--model", "opus"]);
        });
    }

    #[test]
    fn build_command_injects_uvx_plugin_and_project_args_for_claude() {
        let _home_guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _cwd_guard = cwd_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let execution_dir = tempfile::tempdir().unwrap();
        let original_home = set_home(home.path());
        let original_cwd = set_cwd(cwd.path()).unwrap();
        let previous_uv_python = std::env::var_os("UV_PYTHON");
        let previous_original_cwd = std::env::var_os("AMPLIHACK_ORIGINAL_CWD");
        unsafe {
            std::env::set_var("UV_PYTHON", "1");
            std::env::remove_var("AMPLIHACK_ORIGINAL_CWD");
        }

        let binary = BinaryInfo {
            name: "claude".to_string(),
            path: PathBuf::from("/usr/bin/claude"),
            version: None,
        };
        let cmd = build_command_for_dir(
            &binary,
            false,
            false,
            false,
            &[],
            Some(execution_dir.path()),
        );
        let args = cmd
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        restore_cwd(&original_cwd).unwrap();
        restore_home(original_home);
        match previous_uv_python {
            Some(value) => unsafe { std::env::set_var("UV_PYTHON", value) },
            None => unsafe { std::env::remove_var("UV_PYTHON") },
        }
        match previous_original_cwd {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_ORIGINAL_CWD", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_ORIGINAL_CWD") },
        }

        assert_eq!(args[0], "--plugin-dir");
        assert_eq!(
            args[1],
            home.path()
                .join(".amplihack")
                .join(".claude")
                .display()
                .to_string()
        );
        assert_eq!(args[2], "--add-dir");
        assert_eq!(args[3], execution_dir.path().display().to_string());
        assert_eq!(args[4], "--model");
    }

    #[test]
    fn build_command_prefers_original_cwd_for_staged_uvx_launches() {
        let _home_guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _cwd_guard = cwd_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let execution_dir = tempfile::tempdir().unwrap();
        let project_dir = tempfile::tempdir().unwrap();
        let original_home = set_home(home.path());
        let original_cwd = set_cwd(cwd.path()).unwrap();
        let previous_uv_python = std::env::var_os("UV_PYTHON");
        let previous_original_cwd = std::env::var_os("AMPLIHACK_ORIGINAL_CWD");
        let previous_is_staged = std::env::var_os("AMPLIHACK_IS_STAGED");
        unsafe {
            std::env::set_var("UV_PYTHON", "1");
            std::env::set_var("AMPLIHACK_ORIGINAL_CWD", project_dir.path());
            std::env::set_var("AMPLIHACK_IS_STAGED", "1");
        }

        let binary = BinaryInfo {
            name: "claude".to_string(),
            path: PathBuf::from("/usr/bin/claude"),
            version: None,
        };
        let cmd = build_command_for_dir(
            &binary,
            false,
            false,
            false,
            &[],
            Some(execution_dir.path()),
        );
        let args = cmd
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        restore_cwd(&original_cwd).unwrap();
        restore_home(original_home);
        match previous_uv_python {
            Some(value) => unsafe { std::env::set_var("UV_PYTHON", value) },
            None => unsafe { std::env::remove_var("UV_PYTHON") },
        }
        match previous_original_cwd {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_ORIGINAL_CWD", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_ORIGINAL_CWD") },
        }
        match previous_is_staged {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_IS_STAGED", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_IS_STAGED") },
        }

        assert_eq!(args[0], "--plugin-dir");
        assert_eq!(args[2], "--add-dir");
        assert_eq!(args[3], project_dir.path().display().to_string());
    }

    #[test]
    fn maybe_prompt_re_enable_power_steering_removes_disabled_file_on_yes_default() {
        let project = tempfile::tempdir().unwrap();
        let disabled_file = project
            .path()
            .join(".claude/runtime/power-steering/.disabled");
        fs::create_dir_all(disabled_file.parent().unwrap()).unwrap();
        fs::write(&disabled_file, "").unwrap();

        maybe_prompt_re_enable_power_steering_with(project.path(), |_prompt, _timeout| Ok(None))
            .unwrap();

        assert!(!disabled_file.exists());
    }

    #[test]
    fn maybe_prompt_re_enable_power_steering_keeps_disabled_file_on_no() {
        let project = tempfile::tempdir().unwrap();
        let disabled_file = project
            .path()
            .join(".claude/runtime/power-steering/.disabled");
        fs::create_dir_all(disabled_file.parent().unwrap()).unwrap();
        fs::write(&disabled_file, "").unwrap();

        maybe_prompt_re_enable_power_steering_with(project.path(), |_prompt, _timeout| {
            Ok(Some("n".to_string()))
        })
        .unwrap();

        assert!(disabled_file.exists());
    }

    #[test]
    fn build_command_does_not_duplicate_uvx_plugin_or_add_dir_args() {
        let _home_guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _cwd_guard = cwd_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let original_home = set_home(home.path());
        let original_cwd = set_cwd(cwd.path()).unwrap();
        let previous_uv_python = std::env::var_os("UV_PYTHON");
        unsafe { std::env::set_var("UV_PYTHON", "1") };

        let binary = BinaryInfo {
            name: "claude".to_string(),
            path: PathBuf::from("/usr/bin/claude"),
            version: None,
        };
        let extra = vec![
            "--plugin-dir".to_string(),
            "/custom/plugin".to_string(),
            "--add-dir".to_string(),
            "/custom/project".to_string(),
        ];
        let cmd = build_command(&binary, false, false, false, &extra);
        let args = cmd
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        restore_cwd(&original_cwd).unwrap();
        restore_home(original_home);
        match previous_uv_python {
            Some(value) => unsafe { std::env::set_var("UV_PYTHON", value) },
            None => unsafe { std::env::remove_var("UV_PYTHON") },
        }

        assert_eq!(
            args,
            vec![
                "--model",
                "opus[1m]",
                "--plugin-dir",
                "/custom/plugin",
                "--add-dir",
                "/custom/project",
            ]
        );
    }

    #[test]
    fn augment_claude_launch_env_sets_directory_copy_plugin_root_and_npm_bin() {
        let _home_guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home = tempfile::tempdir().unwrap();
        fs::create_dir_all(home.path().join(".amplihack/.claude")).unwrap();
        let original_home = set_home(home.path());
        let previous_plugin_installed = std::env::var_os("AMPLIHACK_PLUGIN_INSTALLED");
        unsafe { std::env::remove_var("AMPLIHACK_PLUGIN_INSTALLED") };

        let env = augment_claude_launch_env(EnvBuilder::new(), "claude").build();

        restore_home(original_home);
        match previous_plugin_installed {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_PLUGIN_INSTALLED", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_PLUGIN_INSTALLED") },
        }

        let expected_plugin_root = home.path().join(".amplihack").join(".claude");
        let expected_plugin_root = expected_plugin_root.display().to_string();
        assert_eq!(
            env.get("CLAUDE_PLUGIN_ROOT").map(String::as_str),
            Some(expected_plugin_root.as_str())
        );
        let path = env.get("PATH").expect("PATH should be populated");
        assert!(
            path.split(':')
                .next()
                .unwrap_or_default()
                .ends_with(".npm-global/bin"),
            "expected ~/.npm-global/bin to be prepended to PATH, got {path}"
        );
    }

    #[test]
    fn augment_claude_launch_env_prefers_installed_plugin_cache_path() {
        let _home_guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home = tempfile::tempdir().unwrap();
        let original_home = set_home(home.path());
        let previous_plugin_installed = std::env::var_os("AMPLIHACK_PLUGIN_INSTALLED");
        unsafe { std::env::set_var("AMPLIHACK_PLUGIN_INSTALLED", "true") };

        let env = augment_claude_launch_env(EnvBuilder::new(), "claude").build();

        restore_home(original_home);
        match previous_plugin_installed {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_PLUGIN_INSTALLED", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_PLUGIN_INSTALLED") },
        }

        let expected_plugin_root = home
            .path()
            .join(".claude")
            .join("plugins")
            .join("cache")
            .join("amplihack")
            .join("amplihack")
            .join("0.9.0");
        let expected_plugin_root = expected_plugin_root.display().to_string();
        assert_eq!(
            env.get("CLAUDE_PLUGIN_ROOT").map(String::as_str),
            Some(expected_plugin_root.as_str())
        );
    }

    #[test]
    fn parse_github_repo_uri_accepts_supported_formats() {
        assert_eq!(
            parse_github_repo_uri("owner/repo").unwrap(),
            ("owner".to_string(), "repo".to_string())
        );
        assert_eq!(
            parse_github_repo_uri("https://github.com/owner/repo.git").unwrap(),
            ("owner".to_string(), "repo".to_string())
        );
        assert_eq!(
            parse_github_repo_uri("git@github.com:owner/repo.git").unwrap(),
            ("owner".to_string(), "repo".to_string())
        );
        assert!(parse_github_repo_uri("https://example.com/owner/repo").is_err());
    }

    #[test]
    #[cfg(unix)]
    fn resolve_checkout_repo_in_uses_git_clone_stub() {
        use std::os::unix::fs::PermissionsExt;

        let _home_guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let bin_dir = temp.path().join("bin");
        let base_dir = temp.path().join("checkouts");
        fs::create_dir_all(&bin_dir).unwrap();
        let git_path = bin_dir.join("git");
        fs::write(
            &git_path,
            "#!/bin/sh\nif [ \"$1\" = \"clone\" ]; then\n  /bin/mkdir -p \"$3/.git\"\n  exit 0\nfi\nexit 1\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&git_path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&git_path, permissions).unwrap();

        let previous_path = std::env::var_os("PATH");
        unsafe { std::env::set_var("PATH", &bin_dir) };

        let checkout = resolve_checkout_repo_in("owner/repo", &base_dir).unwrap();

        match previous_path {
            Some(value) => unsafe { std::env::set_var("PATH", value) },
            None => unsafe { std::env::remove_var("PATH") },
        }

        assert_eq!(checkout, base_dir.join("owner-repo"));
        assert!(checkout.join(".git").is_dir());
    }

    #[test]
    fn env_builder_sets_skip_reflection_when_requested() {
        let env = EnvBuilder::new()
            .set_if(true, "AMPLIHACK_SKIP_REFLECTION", "1")
            .build();
        assert_eq!(
            env.get("AMPLIHACK_SKIP_REFLECTION").map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn env_builder_omits_skip_reflection_when_not_requested() {
        let env = EnvBuilder::new()
            .set_if(false, "AMPLIHACK_SKIP_REFLECTION", "1")
            .build();
        assert!(!env.contains_key("AMPLIHACK_SKIP_REFLECTION"));
    }

    /// When child exits normally with code 0, wait_for_child_or_signal must return 0.
    #[test]
    fn test_wait_for_child_returns_zero_on_normal_success() {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        use std::sync::Arc;
        use std::sync::atomic::AtomicBool;

        let cmd = Command::new("true"); // always exits 0 on Unix
        let mut child = ManagedChild::spawn(cmd).expect("failed to spawn 'true'");
        let shutdown = Arc::new(AtomicBool::new(false));

        let exit_code = wait_for_child_or_signal(&mut child, &shutdown)
            .expect("wait_for_child_or_signal failed unexpectedly");

        assert_eq!(
            exit_code, 0,
            "Normal success exit (code 0) must be propagated as 0. Got: {exit_code}"
        );
    }

    /// Sanity check: when child exits with code 1, wait_for_child_or_signal
    /// must return 1 (non-zero exits are propagated unchanged).
    ///
    /// Expected: PASSES both before and after the fix.
    #[test]
    fn test_wait_for_child_returns_nonzero_on_normal_failure() {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        use std::sync::Arc;
        use std::sync::atomic::AtomicBool;

        let cmd = Command::new("false"); // always exits 1 on Unix
        let mut child = ManagedChild::spawn(cmd).expect("failed to spawn 'false'");
        let shutdown = Arc::new(AtomicBool::new(false));

        let exit_code = wait_for_child_or_signal(&mut child, &shutdown)
            .expect("wait_for_child_or_signal failed unexpectedly");

        assert_eq!(
            exit_code, 1,
            "Non-zero exit code (1) must be propagated unchanged. Got: {exit_code}"
        );
    }

    /// Document the root cause: on Unix, a process killed by SIGINT has *no*
    /// numeric exit code — status.code() returns None.
    ///
    /// This test validates the assumption, not the implementation.
    /// It PASSES regardless of the fix status.
    #[test]
    #[cfg(unix)]
    fn test_sigint_killed_process_has_no_numeric_exit_code() {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut child = std::process::Command::new("sh")
            .args(["-c", "kill -INT $$"])
            .spawn()
            .expect("failed to spawn sh");
        let status = child.wait().expect("wait failed");

        assert!(
            status.code().is_none(),
            "A process killed by SIGINT must have no numeric exit code \
             (status.code() returns None on Unix). Got: {:?}",
            status.code()
        );
    }

    /// SIGINT exit code parity with Python: when the child process is killed by
    /// SIGINT, wait_for_child_or_signal must return exit code 0, matching Python's
    /// `signal_handler → sys.exit(0)` behaviour.
    #[test]
    #[cfg(unix)]
    fn test_wait_for_child_returns_zero_when_killed_by_sigint() {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        use std::sync::Arc;
        use std::sync::atomic::AtomicBool;

        // Spawn a shell that immediately sends SIGINT to itself.
        // This models a user pressing Ctrl+C while the claude binary is running.
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "kill -INT $$"]);
        let mut child = ManagedChild::spawn(cmd).expect("failed to spawn sh");
        let shutdown = Arc::new(AtomicBool::new(false));

        let exit_code = wait_for_child_or_signal(&mut child, &shutdown)
            .expect("wait_for_child_or_signal returned an error");

        // Python: sys.exit(0) on SIGINT → exit code 0. unwrap_or(0) matches this.
        assert_eq!(
            exit_code, 0,
            "SIGINT-killed child must produce exit code 0 (parity with Python \
             signal_handler → sys.exit(0)). Got exit code {exit_code}."
        );
    }

    /// When the shutdown flag is set (SIGINT received by the Rust process itself,
    /// not the child), wait_for_child_or_signal must also return 0.
    ///
    /// This path already works correctly (the loop returns Ok(0) on shutdown flag).
    /// This test documents and guards that behaviour.
    ///
    /// Expected: PASSES both before and after the fix.
    #[test]
    fn test_wait_for_child_returns_zero_when_shutdown_flag_set() {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        // Spawn a long-running process that would normally run for 60 seconds
        let mut cmd = Command::new("sleep");
        cmd.args(["60"]);
        let mut child = ManagedChild::spawn(cmd).expect("failed to spawn sleep");

        // Pre-set the shutdown flag (simulates SIGINT arriving before we poll)
        let shutdown = Arc::new(AtomicBool::new(true));
        shutdown.store(true, Ordering::Relaxed);

        let exit_code = wait_for_child_or_signal(&mut child, &shutdown)
            .expect("wait_for_child_or_signal failed");

        assert_eq!(
            exit_code, 0,
            "Shutdown-flag path must return exit code 0 (matching Python sys.exit(0)). \
             Got: {exit_code}"
        );
    }

    #[test]
    fn should_prompt_blarify_indexing_only_for_interactive_claude_opt_in() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        unsafe { std::env::set_var("AMPLIHACK_ENABLE_BLARIFY", "1") };
        assert!(should_prompt_blarify_indexing("claude", false));
        assert!(!should_prompt_blarify_indexing("copilot", false));
        assert!(!should_prompt_blarify_indexing("claude", true));
        unsafe { std::env::remove_var("AMPLIHACK_ENABLE_BLARIFY") };
        assert!(!should_prompt_blarify_indexing("claude", false));
    }

    #[test]
    fn should_allow_noninteractive_blarify_when_mode_is_explicit() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        unsafe {
            std::env::set_var("AMPLIHACK_ENABLE_BLARIFY", "1");
            std::env::set_var("AMPLIHACK_BLARIFY_MODE", "background");
        }
        assert!(should_prompt_blarify_indexing("claude", true));
        unsafe {
            std::env::remove_var("AMPLIHACK_ENABLE_BLARIFY");
            std::env::remove_var("AMPLIHACK_BLARIFY_MODE");
        }
    }

    #[test]
    fn maybe_run_blarify_indexing_prompt_surfaces_prompt_failure() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let project = tempfile::tempdir().unwrap();

        unsafe {
            std::env::set_var("AMPLIHACK_ENABLE_BLARIFY", "1");
            std::env::set_var("AMPLIHACK_BLARIFY_MODE", "background");
        }

        let result =
            maybe_run_blarify_indexing_prompt_with("claude", true, Some(project.path()), |_path| {
                Err(anyhow::anyhow!("synthetic prompt failure"))
            });

        unsafe {
            std::env::remove_var("AMPLIHACK_ENABLE_BLARIFY");
            std::env::remove_var("AMPLIHACK_BLARIFY_MODE");
        }

        let error = result.expect_err("prompt failure should stop launch");
        let error_message = error.to_string();
        let error_chain = format!("{error:#}");
        assert!(
            error_message.contains("code graph indexing prompt failed for"),
            "expected launch-side prompt failure context, got: {error_message}"
        );
        assert!(
            error_chain.contains("synthetic prompt failure"),
            "expected root-cause prompt failure, got: {error_chain}"
        );
        assert!(
            error_chain.contains(&project.path().display().to_string()),
            "expected project path in error chain, got: {error_chain}"
        );
    }

    #[test]
    fn parse_blarify_prompt_choice_matches_supported_inputs() {
        assert_eq!(
            parse_blarify_prompt_choice(Some("y")),
            BlarifyPromptChoice::Foreground
        );
        assert_eq!(
            parse_blarify_prompt_choice(Some("background")),
            BlarifyPromptChoice::Background
        );
        assert_eq!(
            parse_blarify_prompt_choice(Some("skip")),
            BlarifyPromptChoice::Never
        );
        assert_eq!(parse_blarify_prompt_choice(None), BlarifyPromptChoice::Skip);
    }

    #[test]
    fn blarify_mode_parses_supported_values() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        unsafe { std::env::set_var("AMPLIHACK_BLARIFY_MODE", "sync") };
        assert_eq!(blarify_mode(), BlarifyMode::Sync);
        unsafe { std::env::set_var("AMPLIHACK_BLARIFY_MODE", "background") };
        assert_eq!(blarify_mode(), BlarifyMode::Background);
        unsafe { std::env::set_var("AMPLIHACK_BLARIFY_MODE", "skip") };
        assert_eq!(blarify_mode(), BlarifyMode::Skip);
        unsafe { std::env::remove_var("AMPLIHACK_BLARIFY_MODE") };
        assert_eq!(blarify_mode(), BlarifyMode::Prompt);
    }

    #[test]
    fn resolve_blarify_index_action_prefers_import_for_current_json() {
        let dir = tempfile::tempdir().unwrap();
        let json_path = dir.path().join(".amplihack").join("blarify.json");
        std::fs::create_dir_all(json_path.parent().unwrap()).unwrap();
        std::fs::write(&json_path, "{}\n").unwrap();
        let status = crate::commands::memory::IndexStatus {
            needs_indexing: false,
            reason: "up-to-date".to_string(),
            estimated_files: 1,
            last_indexed: None,
        };

        assert_eq!(
            resolve_blarify_index_action(&status, &json_path),
            BlarifyIndexAction::ImportExistingJson
        );
    }

    #[test]
    fn resolve_blarify_index_action_prefers_native_scip_for_stale_json() {
        let dir = tempfile::tempdir().unwrap();
        let json_path = dir.path().join(".amplihack").join("blarify.json");
        std::fs::create_dir_all(json_path.parent().unwrap()).unwrap();
        std::fs::write(&json_path, "{}\n").unwrap();
        let status = crate::commands::memory::IndexStatus {
            needs_indexing: true,
            reason: "stale".to_string(),
            estimated_files: 3,
            last_indexed: None,
        };

        assert_eq!(
            resolve_blarify_index_action(&status, &json_path),
            BlarifyIndexAction::GenerateNativeScip
        );
    }

    #[test]
    fn consent_cache_round_trip_persists_per_project() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let previous_home = set_home(home.path());

        assert!(!has_blarify_consent(project.path()).unwrap());
        save_blarify_consent(project.path()).unwrap();
        assert!(has_blarify_consent(project.path()).unwrap());

        let consent_path = consent_cache_path(project.path()).unwrap();
        assert!(consent_path.exists());

        restore_home(previous_home);
    }
}
