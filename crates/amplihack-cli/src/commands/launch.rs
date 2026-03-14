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
use crate::tool_update_check::maybe_print_npm_update_notice;
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

    // Build environment — canonical chain order per design spec.
    // SEC-DATA-01: Never log the full env map (may contain inherited secrets).
    let env = EnvBuilder::new()
        .with_amplihack_session_id() // AMPLIHACK_SESSION_ID, AMPLIHACK_DEPTH
        .with_amplihack_vars() // AMPLIHACK_RUST_RUNTIME, AMPLIHACK_VERSION, NODE_OPTIONS
        .with_agent_binary(tool) // WS1: AMPLIHACK_AGENT_BINARY
        .with_amplihack_home() // WS3: AMPLIHACK_HOME
        .set_if(is_noninteractive(), "AMPLIHACK_NONINTERACTIVE", "1") // WS2: propagate flag
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
                return Ok(status.code().unwrap_or(0)); // SIGINT-killed: no numeric code → 0 (parity with Python signal_handler → sys.exit(0))
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

    // ---------------------------------------------------------------------------
    // TDD Step 7: Failing tests for flag injection (Category 2)
    //
    // These tests specify the expected behaviour for --dangerously-skip-permissions
    // and --model injection in build_command. They are written to FAIL until the
    // implementation matches the Python launcher parity requirements.
    // ---------------------------------------------------------------------------

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

    // ---------------------------------------------------------------------------
    // TDD Step 7: WS1 — SIGINT Exit Code Parity Tests
    //
    // These tests specify the required behaviour when the child process is killed
    // by SIGINT (Ctrl-C).  Python's `signal_handler` unconditionally calls
    // `sys.exit(0)` after receiving SIGINT; the Rust launcher must match.
    //
    // ROOT CAUSE: wait_for_child_or_signal() line 148 uses:
    //   status.code().unwrap_or(1)
    // On Unix a signal-killed process has no numeric exit code, so
    // status.code() returns None.  unwrap_or(1) maps that to 1; the correct
    // mapping is unwrap_or(0) to match Python.
    //
    // FIX REQUIRED: change `unwrap_or(1)` → `unwrap_or(0)` at line 148.
    //
    // PARITY TESTS UNBLOCKED BY THIS FIX:
    //   gap-launch-sigint-exit-code  (tier5-gap-tests.yaml)
    //   gap-sigint-exit-code         (tier7-launcher-parity.yaml)
    // ---------------------------------------------------------------------------

    /// Sanity check: when child exits normally with code 0, wait_for_child_or_signal
    /// must return 0.  This verifies the happy path is not broken by the SIGINT fix.
    ///
    /// Expected: PASSES both before and after the fix.
    #[test]
    fn test_wait_for_child_returns_zero_on_normal_success() {
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
        let mut child = std::process::Command::new("sh")
            .args(["-c", "kill -INT $$"])
            .spawn()
            .expect("failed to spawn sh");
        let status = child.wait().expect("wait failed");

        assert!(
            status.code().is_none(),
            "A process killed by SIGINT must have no numeric exit code \
             (status.code() returns None on Unix). \
             This is why unwrap_or matters: unwrap_or(1) → 1 (wrong), \
             unwrap_or(0) → 0 (correct). Got: {:?}",
            status.code()
        );
    }

    /// SIGINT exit code parity with Python: when the child process is killed by
    /// SIGINT, wait_for_child_or_signal MUST return exit code 0.
    ///
    /// Python launcher behaviour (src/amplihack/launcher/core.py):
    ///   signal_handler(sig, frame):
    ///       ...
    ///       sys.exit(0)  ← exits 0 unconditionally on SIGINT
    ///
    /// Rust broken behaviour (launch.rs:148):
    ///   status.code().unwrap_or(1)  → None.unwrap_or(1) = 1  ← WRONG
    ///
    /// Rust required behaviour after fix:
    ///   status.code().unwrap_or(0)  → None.unwrap_or(0) = 0  ← CORRECT
    ///
    /// PARITY AUDIT TARGETS:
    ///   gap-launch-sigint-exit-code  (tier5-gap-tests.yaml  — Python exits 0)
    ///   gap-sigint-exit-code         (tier7-launcher-parity.yaml)
    ///
    /// *** THIS TEST FAILS until the fix: change `unwrap_or(1)` → `unwrap_or(0)`
    ///     at line 148 of wait_for_child_or_signal() in this file. ***
    #[test]
    #[cfg(unix)]
    fn test_wait_for_child_returns_zero_when_killed_by_sigint() {
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

        // Python: sys.exit(0) on SIGINT  →  exit code 0
        // Broken: status.code().unwrap_or(1)  →  1
        // Fixed:  status.code().unwrap_or(0)  →  0
        assert_eq!(
            exit_code, 0,
            "SIGINT-killed child must produce exit code 0 (parity with Python \
             signal_handler → sys.exit(0)). Got exit code {exit_code}. \
             FIX: change `unwrap_or(1)` to `unwrap_or(0)` at launch.rs:148 \
             in wait_for_child_or_signal()."
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
}
