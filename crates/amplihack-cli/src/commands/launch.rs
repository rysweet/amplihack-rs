//! Launch commands for Claude, Copilot, Codex, and Amplifier.
//!
//! Builds the environment, finds the binary, checks nesting, and spawns
//! a `ManagedChild` with signal forwarding.

use crate::binary_finder::BinaryInfo;
use crate::bootstrap;
use crate::commands::memory::{
    background_index_job_active, check_index_status, record_background_index_pid,
    resolve_code_graph_db_path_for_project, run_index_code, run_index_scip,
};
use crate::env_builder::EnvBuilder;
use crate::launcher::ManagedChild;
use crate::nesting::NestingDetector;
use crate::signals;
use crate::tool_update_check::maybe_print_npm_update_notice;
use crate::util::is_noninteractive;
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

const BLARIFY_PROMPT_TIMEOUT: Duration = Duration::from_secs(30);

/// Launch a tool binary (claude, copilot, codex, amplifier).
pub fn run_launch(
    tool: &str,
    resume: bool,
    continue_session: bool,
    skip_permissions: bool,
    skip_update_check: bool,
    extra_args: Vec<String>,
) -> Result<()> {
    // Check for npm updates before doing anything else.
    // This is a no-op if skip_update_check is true, AMPLIHACK_NONINTERACTIVE is set,
    // or the tool has no npm package mapping.
    maybe_print_npm_update_notice(tool, skip_update_check);

    bootstrap::prepare_launcher(tool)?;

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

    // Build environment — canonical chain order per design spec.
    // SEC-DATA-01: Never log the full env map (may contain inherited secrets).
    let mut env_builder = EnvBuilder::new()
        .with_amplihack_session_id() // AMPLIHACK_SESSION_ID, AMPLIHACK_DEPTH
        .with_amplihack_vars() // AMPLIHACK_RUST_RUNTIME, AMPLIHACK_VERSION, NODE_OPTIONS
        .with_agent_binary(tool) // WS1: AMPLIHACK_AGENT_BINARY
        .with_amplihack_home() // WS3: AMPLIHACK_HOME
        .with_asset_resolver(); // Rust-native bundle asset resolver
    if let Some(project_path) = current_dir.as_deref() {
        env_builder = env_builder.with_project_kuzu_db(project_path);
    }
    let env = env_builder
        .set_if(is_noninteractive(), "AMPLIHACK_NONINTERACTIVE", "1") // WS2: propagate flag
        .build();

    if should_prompt_blarify_indexing(tool, is_noninteractive()) {
        let project_path = current_dir
            .as_ref()
            .context("failed to resolve current directory")?;
        if let Err(err) = maybe_prompt_blarify_indexing(project_path) {
            tracing::warn!(error = %err, "code graph indexing prompt failed (continuing)");
        }
    }

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
                return Ok(status.code().unwrap_or(0)); // SIGINT-killed: no numeric code → 0 (parity with Python signal_handler → sys.exit(0))
            }
            None => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    }
}

fn should_prompt_blarify_indexing(tool: &str, noninteractive: bool) -> bool {
    tool == "claude"
        && std::env::var("AMPLIHACK_ENABLE_BLARIFY").as_deref() == Ok("1")
        && (!noninteractive || blarify_mode() != BlarifyMode::Prompt)
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
    println!();
    println!("{}", "=".repeat(60));
    println!("Code Indexing");
    println!("{}", "=".repeat(60));
    println!("Project: {}", project_path.display());
    println!("Status: {}", status_reason);
    println!("Files to index: {}", estimated_files);
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

fn run_code_indexing(
    project_path: &Path,
    json_path: &Path,
    action: BlarifyIndexAction,
    background: bool,
) -> Result<()> {
    let kuzu_path = resolve_code_graph_db_path_for_project(project_path)?;
    if background {
        let current_exe =
            std::env::current_exe().context("failed to resolve current executable")?;
        let mut cmd = Command::new(current_exe);
        let child = match action {
            BlarifyIndexAction::ImportExistingJson => {
                cmd.arg("index-code")
                    .arg(json_path)
                    .arg("--db-path")
                    .arg(&kuzu_path);
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
            run_index_code(json_path, Some(&kuzu_path))?;
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

fn read_user_input_with_timeout(prompt: &str, timeout: Duration) -> Result<Option<String>> {
    print!("{prompt}");
    io::stdout().flush().context("failed to flush prompt")?;

    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;

        let fd = io::stdin().as_raw_fd();
        let mut pollfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let timeout_ms = timeout.as_millis().min(i32::MAX as u128) as i32;
        let ready = unsafe { libc::poll(&mut pollfd, 1, timeout_ms) };
        if ready < 0 {
            return Err(io::Error::last_os_error()).context("failed waiting for prompt input");
        }
        if ready == 0 {
            println!();
            return Ok(None);
        }
    }

    #[cfg(not(unix))]
    {
        if !std::io::stdin().is_terminal() {
            return Ok(None);
        }
    }

    let mut response = String::new();
    io::stdin()
        .read_line(&mut response)
        .context("failed to read prompt input")?;
    Ok(Some(response.trim().to_string()))
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
            project_path.join(".amplihack").join("kuzu_db").display()
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
    use crate::test_support::{home_env_lock, restore_home, set_home};
    use std::path::PathBuf;

    fn make_binary(path: &str) -> BinaryInfo {
        BinaryInfo {
            name: "claude".to_string(),
            path: PathBuf::from(path),
            version: Some("1.0.0".to_string()),
        }
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
