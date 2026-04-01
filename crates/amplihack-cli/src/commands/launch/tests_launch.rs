use super::*;
use crate::env_builder::EnvBuilder;
use crate::launcher::ManagedChild;
use crate::test_support::{home_env_lock, restore_home, set_home};
use std::fs;
use std::process::Command;

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
fn build_docker_launcher_args_preserves_shared_launcher_flags() {
    assert_eq!(
        build_docker_launcher_args(
            "launch",
            true,
            true,
            true,
            true,
            true,
            Some("owner/repo"),
            &["-p".to_string(), "audit parity".to_string()]
        ),
        vec![
            "launch",
            "--resume",
            "--continue",
            "--skip-update-check",
            "--no-reflection",
            "--subprocess-safe",
            "--checkout-repo",
            "owner/repo",
            "--",
            "-p",
            "audit parity",
        ]
    );
}

#[test]
fn build_docker_launcher_args_preserves_non_launch_surface_and_omits_launch_only_flags() {
    assert_eq!(
        build_docker_launcher_args("copilot", false, false, true, false, false, None, &[]),
        vec!["copilot"]
    );
}

#[test]
fn build_docker_launcher_args_preserves_each_non_launch_surface() {
    for surface in ["copilot", "codex", "amplifier"] {
        let args =
            build_docker_launcher_args(surface, false, false, false, false, false, None, &[]);
        assert_eq!(
            args.first().map(String::as_str),
            Some(surface),
            "surface '{}' produced first arg {:?}",
            surface,
            args.first()
        );
    }
}

#[test]
fn resolve_launch_node_options_keeps_memory_config_for_subprocess_safe_launches() {
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

    assert_eq!(subprocess_safe, top_level);
    assert!(subprocess_safe.contains("--trace-warnings"));
    assert!(subprocess_safe.contains("--max-old-space-size="));
    assert!(top_level.contains("--max-old-space-size="));
}

#[test]
fn test_subprocess_safe_preserves_existing_node_options() {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().unwrap();
    let original_home = set_home(home.path());
    fs::create_dir_all(home.path().join(".amplihack")).unwrap();
    fs::write(
        home.path().join(".amplihack/config"),
        r#"{"node_options_consent":true,"node_options_limit_mb":32768}"#,
    )
    .unwrap();
    let previous_node_options = std::env::var_os("NODE_OPTIONS");
    unsafe { std::env::set_var("NODE_OPTIONS", "--trace-warnings") };

    let result = resolve_launch_node_options(true).unwrap();

    restore_home(original_home);
    match previous_node_options {
        Some(value) => unsafe { std::env::set_var("NODE_OPTIONS", value) },
        None => unsafe { std::env::remove_var("NODE_OPTIONS") },
    }

    let env = EnvBuilder::new()
        .with_amplihack_vars_with_node_options(Some(result.as_str()))
        .build();

    let node_options = env.get("NODE_OPTIONS").map(String::as_str).unwrap_or("");
    assert!(node_options.contains("--trace-warnings"));
    assert!(node_options.contains("--max-old-space-size="));
}

#[test]
fn test_subprocess_safe_without_parent_still_applies_memory_config() {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().unwrap();
    let original_home = set_home(home.path());
    fs::create_dir_all(home.path().join(".amplihack")).unwrap();
    fs::write(
        home.path().join(".amplihack/config"),
        r#"{"node_options_consent":true,"node_options_limit_mb":32768}"#,
    )
    .unwrap();
    let previous_node_options = std::env::var_os("NODE_OPTIONS");
    unsafe { std::env::remove_var("NODE_OPTIONS") };

    let result = resolve_launch_node_options(true).unwrap();

    restore_home(original_home);
    match previous_node_options {
        Some(value) => unsafe { std::env::set_var("NODE_OPTIONS", value) },
        None => unsafe { std::env::remove_var("NODE_OPTIONS") },
    }

    let env = EnvBuilder::new()
        .with_amplihack_vars_with_node_options(Some(result.as_str()))
        .build();

    let node_opts = env.get("NODE_OPTIONS").map(String::as_str).unwrap_or("");
    assert!(
        node_opts.contains("--max-old-space-size="),
        "subprocess-safe launch must still inject smart NODE_OPTIONS when parent is unset; \
         got: {:?}",
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
    fs::write(
        home.path().join(".amplihack/config"),
        r#"{"node_options_consent":true,"node_options_limit_mb":32768}"#,
    )
    .unwrap();
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
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

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
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

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
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

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
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

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
