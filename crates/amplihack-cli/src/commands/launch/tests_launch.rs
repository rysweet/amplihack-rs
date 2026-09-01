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

#[test]
fn executable_identity_detects_file_replacement() {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("agent");
    fs::write(&executable, b"original executable").unwrap();
    let original = capture_executable_identity(&executable).unwrap();

    fs::rename(&executable, directory.path().join("original-agent")).unwrap();
    fs::write(&executable, b"replacement executable with different length").unwrap();
    let replacement = capture_executable_identity(&executable).unwrap();

    assert_ne!(original, replacement);
}

#[test]
fn executable_identity_detects_equal_length_content_changes() {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("agent");
    fs::write(&executable, b"first payload").unwrap();
    let original = capture_executable_identity(&executable).unwrap();

    fs::write(&executable, b"other payload").unwrap();
    let replacement = capture_executable_identity(&executable).unwrap();

    assert_eq!(original.metadata.length, replacement.metadata.length);
    assert_ne!(original.digest, replacement.digest);
    assert_ne!(original, replacement);
}

#[cfg(unix)]
#[test]
fn routed_executable_revalidation_rejects_same_version_replacement() {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("claude");
    let body = "#!/bin/sh\n[ \"$1\" = --version ] && { printf '2.1.247\\n'; exit 0; }\n";
    let replacement_body =
        "#!/bin/sh\n[ \"$1\" = --version ] && { printf \"2.1.247\\n\"; exit 0; }\n";
    assert_eq!(body.len(), replacement_body.len());
    write_fake_claude(&executable, body);
    let expected = BinaryInfo {
        name: "claude".to_string(),
        path: executable.clone(),
        version: Some("2.1.247".to_string()),
    };
    let identity = capture_executable_identity(&executable).unwrap();
    let executable_text = executable.to_string_lossy().into_owned();
    let _env = EnvGuard::set([("AMPLIHACK_CLAUDE_BINARY_PATH", executable_text.as_str())]);

    fs::rename(&executable, directory.path().join("original-claude")).unwrap();
    write_fake_claude(&executable, replacement_body);

    let error = revalidate_proxy_binary(
        "claude",
        amplihack_utils::launch_target::OverrideOrigin::User,
        amplihack_utils::litellm_proxy::CliTarget::Claude,
        &expected,
        &identity,
    )
    .expect_err("replacement with the same version must fail closed");

    assert!(format!("{error:#}").contains("changed after preflight"));
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
fn copilot_gateway_capability_gate_uses_the_resolved_binary_version() {
    use amplihack_utils::litellm_proxy::CliTarget;

    for version in ["1.0.83-2", "1.0.83-3"] {
        let supported = BinaryInfo {
            name: "copilot".to_string(),
            path: "/opt/copilot/bin/copilot".into(),
            version: Some(version.to_string()),
        };
        validate_proxy_binary_capability(CliTarget::CopilotCli, &supported)
            .expect("the runtime-verified Copilot CLI version must pass");
    }

    for version in [
        Some("1.0.83"),
        Some("1.0.83-1"),
        Some("1.0.84-1"),
        Some("2.0.0"),
        Some("not-a-version"),
        None,
    ] {
        let binary = BinaryInfo {
            name: "copilot".to_string(),
            path: "/opt/copilot/bin/copilot".into(),
            version: version.map(str::to_string),
        };
        let error = validate_proxy_binary_capability(CliTarget::CopilotCli, &binary)
            .expect_err("unproved subprocess scrubbing must fail closed");
        let message = format!("{error:#}");
        assert!(message.contains("1.0.83-2"), "{message}");
        assert!(message.contains("1.0.83-3"), "{message}");
        assert!(
            !message.contains("gateway-secret"),
            "capability errors must not disclose gateway credentials"
        );
    }
}

#[test]
fn rustyclawd_gateway_capability_requires_the_pinned_cargo_receipt() {
    let _lock = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let cargo_home = tempfile::tempdir().unwrap();
    let bin_dir = cargo_home.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let executable = bin_dir.join("rusty");
    fs::write(&executable, b"synthetic rustyclawd executable").unwrap();
    let receipt_key = format!(
        "{} {} (git+{}?rev={}#{})",
        RUSTYCLAWD_PACKAGE,
        RUSTYCLAWD_VERSION,
        RUSTYCLAWD_SOURCE,
        RUSTYCLAWD_REVISION,
        RUSTYCLAWD_REVISION
    );
    fs::write(
        cargo_home.path().join(".crates2.json"),
        serde_json::json!({
            "installs": {
                (receipt_key): {
                    "bins": ["rusty"]
                }
            }
        })
        .to_string(),
    )
    .unwrap();
    let _cargo_home = EnvGuard::set([("CARGO_HOME", cargo_home.path().to_str().unwrap())]);
    let binary = BinaryInfo {
        name: "rustyclawd".to_string(),
        path: executable,
        version: Some(RUSTYCLAWD_VERSION.to_string()),
    };
    validate_proxy_binary_capability(
        amplihack_utils::litellm_proxy::CliTarget::RustyClawd,
        &binary,
    )
    .expect("the pinned Cargo-installed RustyClawd must pass");

    let pinned_receipt = format!(
        "{} {} (git+{}?rev={}#{})",
        RUSTYCLAWD_PACKAGE,
        RUSTYCLAWD_VERSION,
        RUSTYCLAWD_SOURCE,
        RUSTYCLAWD_REVISION,
        RUSTYCLAWD_REVISION
    );
    fs::write(
        cargo_home.path().join(".crates2.json"),
        serde_json::json!({
            "installs": {
                (pinned_receipt): {
                    "bins": ["rusty"]
                },
                "rustyclawd-cli 0.1.1 (git+https://github.com/rysweet/RustyClawd?rev=obsolete#obsolete)": {
                    "bins": ["rusty"]
                }
            }
        })
        .to_string(),
    )
    .unwrap();
    let error = validate_proxy_binary_capability(
        amplihack_utils::litellm_proxy::CliTarget::RustyClawd,
        &binary,
    )
    .expect_err("ambiguous RustyClawd receipts must fail closed");
    assert!(format!("{error:#}").contains("ambiguous"));

    fs::write(
        cargo_home.path().join(".crates2.json"),
        serde_json::json!({
            "installs": {
                "rustyclawd-cli 0.1.1 (git+https://github.com/rysweet/RustyClawd?rev=obsolete#obsolete)": {
                    "bins": ["rusty"]
                }
            }
        })
        .to_string(),
    )
    .unwrap();
    let error = validate_proxy_binary_capability(
        amplihack_utils::litellm_proxy::CliTarget::RustyClawd,
        &binary,
    )
    .expect_err("an unpinned RustyClawd receipt must fail closed");
    assert!(format!("{error:#}").contains(RUSTYCLAWD_REVISION));
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

// ---------------------------------------------------------------------------
// Issue #1424: a child that dies on a signal is reported, not silently zeroed.
// ---------------------------------------------------------------------------

/// A child killed by SIGABRT — the Copilot native binary in issue #1424, which
/// panicked on a failed thread spawn and aborted — must NOT be reported as a
/// clean exit. `status.code().unwrap_or(0)` said 0 for every signal death, so
/// `amplihack copilot` exited 0 and claimed success.
#[test]
#[cfg(unix)]
fn test_wait_for_child_reports_a_sigabrt_death_instead_of_success() {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    let mut cmd = Command::new("sh");
    cmd.args(["-c", "ulimit -c 0 2>/dev/null; kill -ABRT $$"]);
    let mut child = ManagedChild::spawn(cmd).expect("failed to spawn sh");
    let shutdown = Arc::new(AtomicBool::new(false));

    let exit_code = wait_for_child_or_signal(&mut child, &shutdown)
        .expect("wait_for_child_or_signal returned an error");

    assert_eq!(
        exit_code,
        128 + libc::SIGABRT,
        "a SIGABRT death must be reported as 128+6, not as success. Got {exit_code}."
    );
}

#[test]
#[cfg(unix)]
fn describe_child_signal_says_the_binary_ran() {
    // The one sentence the misdiagnosis contradicted: it was there, and it ran.
    let message = describe_child_signal(libc::SIGABRT);
    assert!(message.contains("SIGABRT"), "{message}");
    assert!(
        !message.to_lowercase().contains("not installed")
            && !message.to_lowercase().contains("no platform package"),
        "must not blame the install: {message}"
    );
    assert!(
        message.contains("ulimit -u") && message.contains("pids.max"),
        "SIGABRT must point at the machine: {message}"
    );
}

#[test]
#[cfg(unix)]
fn describe_child_signal_does_not_invent_a_cause_for_every_signal() {
    // SIGSEGV is not resource exhaustion; naming a likely-but-unverified cause
    // is the mistake this issue is about.
    let message = describe_child_signal(libc::SIGSEGV);
    assert!(message.contains("SIGSEGV"), "{message}");
    assert!(!message.contains("ulimit -u"), "{message}");
}
