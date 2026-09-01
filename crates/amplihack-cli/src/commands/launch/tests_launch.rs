use super::*;
use crate::env_builder::EnvBuilder;
use crate::launcher::ManagedChild;
use crate::test_support::{CwdGuard, EnvGuard, HomeGuard, home_env_lock, restore_home, set_home};
use std::fs;
use std::process::Command;

#[cfg(unix)]
fn write_fake_claude(path: &std::path::Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;

    fs::write(path, body).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

fn restore_prompt_delivery(previous: Option<std::ffi::OsString>) {
    unsafe {
        match previous {
            Some(value) => std::env::set_var("AMPLIHACK_PROMPT_DELIVERY", value),
            None => std::env::remove_var("AMPLIHACK_PROMPT_DELIVERY"),
        }
    }
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
fn run_launch_rejects_explicit_unsupported_amplifier_prompt_delivery_modes() {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous = std::env::var_os("AMPLIHACK_PROMPT_DELIVERY");

    for mode in ["tempfile", "stdin"] {
        unsafe { std::env::set_var("AMPLIHACK_PROMPT_DELIVERY", mode) };
        let err = run_launch(
            "amplifier",
            "amplifier",
            false,
            false,
            false,
            true,
            true,
            false,
            true,
            None,
            vec!["run".to_string(), "review repo".to_string()],
            amplihack_utils::launch_target::OverrideOrigin::User,
            None,
        )
        .expect_err("unsupported Amplifier delivery must fail before launch");
        let message = format!("{err:#}");
        assert!(
            message.contains("Amplifier prompt delivery mode"),
            "error should identify Amplifier prompt-delivery policy; got: {message}"
        );
        assert!(
            message.contains(mode),
            "error should name rejected mode {mode}; got: {message}"
        );
    }

    restore_prompt_delivery(previous);
}

#[test]
fn amplifier_launch_prompt_delivery_policy_allows_auto_and_argv() {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous = std::env::var_os("AMPLIHACK_PROMPT_DELIVERY");

    for mode in [None, Some("auto"), Some("argv")] {
        unsafe {
            match mode {
                Some(value) => std::env::set_var("AMPLIHACK_PROMPT_DELIVERY", value),
                None => std::env::remove_var("AMPLIHACK_PROMPT_DELIVERY"),
            }
        }
        validate_launch_prompt_delivery("amplifier")
            .expect("Amplifier must allow unset, auto, and argv prompt delivery requests");
    }

    restore_prompt_delivery(previous);
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
fn docker_gateway_launch_rejects_claude_conflicts_before_docker() {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _gateway_env = EnvGuard::set([
        (
            amplihack_utils::litellm_proxy::ENDPOINT_ENV,
            "https://gateway.example.com",
        ),
        (
            amplihack_utils::litellm_proxy::API_KEY_ENV,
            "gateway-secret",
        ),
        (amplihack_utils::litellm_proxy::MODEL_ENV, "gateway-model"),
    ]);

    let error = run_launch(
        "claude",
        "launch",
        true,
        false,
        false,
        false,
        true,
        false,
        true,
        None,
        vec!["--model".to_string(), "bypass-model".to_string()],
        amplihack_utils::launch_target::OverrideOrigin::User,
        Some(amplihack_utils::litellm_proxy::CliTarget::Claude),
    )
    .expect_err("a conflicting model must fail before Docker is probed");
    assert!(
        format!("{error:#}").contains("requested and fallback models must match"),
        "unexpected error: {error:#}"
    );
}

#[test]
fn claude_gateway_capability_gate_uses_the_resolved_binary_version() {
    use amplihack_utils::litellm_proxy::CliTarget;

    let supported = BinaryInfo {
        name: "claude".to_string(),
        path: "/opt/claude/bin/claude".into(),
        version: Some("2.1.247".to_string()),
    };
    validate_proxy_binary_capability(CliTarget::Claude, &supported)
        .expect("the runtime-verified Claude Code version must pass");

    for version in [
        Some("2.1.246"),
        Some("2.1.248"),
        Some("3.0.0"),
        Some("2.1.247-beta.1"),
        None,
    ] {
        let unsupported = BinaryInfo {
            name: "claude".to_string(),
            path: "/opt/claude/bin/claude".into(),
            version: version.map(str::to_string),
        };
        let error = validate_proxy_binary_capability(CliTarget::Claude, &unsupported)
            .expect_err("unproved subprocess scrubbing must fail closed");
        let message = format!("{error:#}");
        assert!(message.contains("2.1.247"), "{message}");
        assert!(
            !message.contains("gateway-secret"),
            "capability errors must not disclose gateway credentials"
        );
    }
}

#[test]
fn capability_gate_does_not_change_non_claude_targets() {
    for (target, name) in [
        (
            amplihack_utils::litellm_proxy::CliTarget::CopilotCli,
            "copilot",
        ),
        (
            amplihack_utils::litellm_proxy::CliTarget::RustyClawd,
            "rustyclawd",
        ),
    ] {
        let binary = BinaryInfo {
            name: name.to_string(),
            path: format!("/opt/{name}").into(),
            version: None,
        };
        validate_proxy_binary_capability(target, &binary)
            .expect("the Claude Code version gate must not apply to other targets");
    }
}

#[cfg(unix)]
#[test]
fn rejected_claude_capabilities_fail_before_launch_setup_or_docker() {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    for (case, script, create_binary, docker) in [
        (
            "unsupported",
            "#!/bin/sh\n[ \"$1\" = --version ] && { printf '2.1.82\\n'; exit 0; }\ntouch \"$HOME/launched\"\n",
            true,
            false,
        ),
        (
            "malformed",
            "#!/bin/sh\n[ \"$1\" = --version ] && { printf 'unknown\\n'; exit 0; }\ntouch \"$HOME/launched\"\n",
            true,
            false,
        ),
        (
            "failed",
            "#!/bin/sh\n[ \"$1\" = --version ] && exit 7\ntouch \"$HOME/launched\"\n",
            true,
            false,
        ),
        ("missing", "", false, false),
    ] {
        let home = tempfile::tempdir().unwrap();
        let binary = if case == "unsupported" {
            home.path().join("old-rustyclawd-wrapper")
        } else {
            home.path().join(format!("claude-{case}"))
        };
        if create_binary {
            write_fake_claude(&binary, script);
        }
        let binary_text = binary.to_string_lossy().into_owned();
        let _home = HomeGuard::set(home.path());
        let _env = EnvGuard::set([
            (
                amplihack_utils::litellm_proxy::ENDPOINT_ENV,
                "https://gateway.example.com",
            ),
            (
                amplihack_utils::litellm_proxy::API_KEY_ENV,
                "gateway-secret",
            ),
            (amplihack_utils::litellm_proxy::MODEL_ENV, "gateway-model"),
            ("AMPLIHACK_CLAUDE_BINARY_PATH", binary_text.as_str()),
            ("AMPLIHACK_NONINTERACTIVE", "1"),
        ]);

        let error = run_launch(
            "claude",
            "launch",
            docker,
            false,
            false,
            false,
            true,
            false,
            false,
            None,
            Vec::new(),
            amplihack_utils::launch_target::OverrideOrigin::User,
            Some(amplihack_utils::litellm_proxy::CliTarget::Claude),
        )
        .expect_err("unproved Claude capability must reject the launch");
        let message = format!("{error:#}");
        assert!(
            message.contains("capability") || message.contains("2.1.247"),
            "{case}: {message}"
        );
        assert!(
            !home.path().join("launched").exists(),
            "{case}: the selected executable must not be launched"
        );
        assert!(
            !home.path().join(".amplihack").exists(),
            "{case}: capability rejection must precede launch filesystem setup"
        );
    }
}

#[cfg(unix)]
#[test]
fn docker_claude_gateway_rejects_before_probing_an_unrelated_host_binary() {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().unwrap();
    let binary = home.path().join("claude");
    write_fake_claude(
        &binary,
        "#!/bin/sh\n\
         [ \"$1\" = --version ] && { touch \"$HOME/probed\"; printf '2.1.247\\n'; exit 0; }\n\
         touch \"$HOME/launched\"\n",
    );
    let binary_text = binary.to_string_lossy().into_owned();
    let _home = HomeGuard::set(home.path());
    let _env = EnvGuard::set([
        (
            amplihack_utils::litellm_proxy::ENDPOINT_ENV,
            "https://gateway.example.com",
        ),
        (
            amplihack_utils::litellm_proxy::API_KEY_ENV,
            "gateway-secret",
        ),
        (amplihack_utils::litellm_proxy::MODEL_ENV, "gateway-model"),
        ("AMPLIHACK_CLAUDE_BINARY_PATH", binary_text.as_str()),
    ]);

    let error = run_launch(
        "claude",
        "launch",
        true,
        false,
        false,
        false,
        true,
        false,
        false,
        None,
        Vec::new(),
        amplihack_utils::launch_target::OverrideOrigin::User,
        Some(amplihack_utils::litellm_proxy::CliTarget::Claude),
    )
    .expect_err("Docker cannot attest its selected Claude executable before creation");

    assert!(format!("{error:#}").contains("container executable"));
    assert!(!home.path().join("probed").exists());
    assert!(!home.path().join("launched").exists());
    assert!(!home.path().join(".amplihack").exists());
}

#[cfg(unix)]
#[test]
fn supported_claude_launch_receives_only_the_scrubbed_gateway_environment() {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().unwrap();
    let binary = home.path().join("claude");
    write_fake_claude(
        &binary,
        "#!/bin/sh\n\
         if [ \"$1\" = --version ]; then printf '2.1.247\\n'; exit 0; fi\n\
         printf '%s|%s|%s' \"$CLAUDE_CODE_SUBPROCESS_ENV_SCRUB\" \
         \"${ANTHROPIC_API_KEY-unset}\" \"$ANTHROPIC_AUTH_TOKEN\" > \"$HOME/child-env\"\n",
    );
    let binary_text = binary.to_string_lossy().into_owned();
    let _home = HomeGuard::set(home.path());
    let _cwd = CwdGuard::set(home.path()).unwrap();
    let _env = EnvGuard::set([
        (
            amplihack_utils::litellm_proxy::ENDPOINT_ENV,
            "https://gateway.example.com",
        ),
        (
            amplihack_utils::litellm_proxy::API_KEY_ENV,
            "gateway-secret",
        ),
        (amplihack_utils::litellm_proxy::MODEL_ENV, "gateway-model"),
        ("AMPLIHACK_CLAUDE_BINARY_PATH", binary_text.as_str()),
        ("AMPLIHACK_NONINTERACTIVE", "1"),
        ("ANTHROPIC_API_KEY", "direct-provider-secret"),
    ]);

    run_launch(
        "claude",
        "launch",
        false,
        false,
        false,
        false,
        true,
        false,
        true,
        None,
        Vec::new(),
        amplihack_utils::launch_target::OverrideOrigin::User,
        Some(amplihack_utils::litellm_proxy::CliTarget::Claude),
    )
    .expect("a supported Claude Code executable must launch");

    assert_eq!(
        fs::read_to_string(home.path().join("child-env")).unwrap(),
        "1|unset|gateway-secret"
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
         got: {node_opts:?}"
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
         got: {node_options:?}"
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

    let exit_code =
        wait_for_child_or_signal(&mut child, &shutdown).expect("wait_for_child_or_signal failed");

    assert_eq!(
        exit_code, 0,
        "Shutdown-flag path must return exit code 0 (matching Python sys.exit(0)). \
         Got: {exit_code}"
    );
}
