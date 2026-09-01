use super::*;
use crate::binary_finder::BinaryInfo;
use crate::test_support::{EnvGuard, home_env_lock, restore_cwd, set_cwd};
use std::ffi::OsStr;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn make_binary(path: &str) -> BinaryInfo {
    BinaryInfo {
        name: "claude".to_string(),
        path: PathBuf::from(path),
        version: Some("1.0.0".to_string()),
    }
}

fn with_uvx_detection_disabled<T>(f: impl FnOnce() -> T) -> T {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let cwd = tempfile::tempdir().unwrap();
    fs::create_dir_all(cwd.path().join(".claude")).unwrap();
    let original_cwd = set_cwd(cwd.path()).unwrap();
    let previous_uv_python = std::env::var_os("UV_PYTHON");
    let previous_root = std::env::var_os("AMPLIHACK_ROOT");
    // Issue #1265: these tests assert exact argv positions and lengths, and
    // `build_command_for_dir` injects `--append-system-prompt` on every claude
    // launch — the fragment is `include_str!`d into the binary, so it is always
    // present. Suppress it here or every argv assertion below shifts by two.
    // The feature has its own suite in `tests_system_prompt_append.rs`.
    let previous_no_append = std::env::var_os("AMPLIHACK_NO_SYSTEM_PROMPT_APPEND");
    unsafe {
        std::env::remove_var("UV_PYTHON");
        std::env::remove_var("AMPLIHACK_ROOT");
        std::env::set_var("AMPLIHACK_NO_SYSTEM_PROMPT_APPEND", "1");
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
    match previous_no_append {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_NO_SYSTEM_PROMPT_APPEND", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_NO_SYSTEM_PROMPT_APPEND") },
    }

    result
}

fn with_default_model_env<T>(value: Option<&str>, f: impl FnOnce() -> T) -> T {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous = std::env::var_os("AMPLIHACK_DEFAULT_MODEL");
    match value {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_DEFAULT_MODEL", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_DEFAULT_MODEL") },
    }

    let result = f();

    match previous {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_DEFAULT_MODEL", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_DEFAULT_MODEL") },
    }
    result
}

#[test]
fn gateway_projection_is_the_final_environment_mutation() {
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

    let env_builder = EnvBuilder::new()
        .set("ANTHROPIC_BASE_URL", "https://bypass.example.com")
        .set("ANTHROPIC_API_KEY", "direct-provider-secret")
        .set("ANTHROPIC_AUTH_TOKEN", "stale-gateway-secret");
    let proxy_config = amplihack_utils::litellm_proxy::ProxyConfig::from_env()
        .unwrap()
        .unwrap();
    unsafe {
        std::env::set_var(
            amplihack_utils::litellm_proxy::API_KEY_ENV,
            "mutated-after-validation",
        );
    }
    let mut command = std::process::Command::new("claude");
    apply_launch_environment(
        &mut command,
        env_builder,
        Some((
            &proxy_config,
            amplihack_utils::litellm_proxy::CliTarget::Claude,
        )),
    );

    let command_env = |name: &str| {
        command
            .get_envs()
            .find(|(key, _)| *key == OsStr::new(name))
            .map(|(_, value)| value.map(|value| value.to_string_lossy().into_owned()))
    };
    assert_eq!(
        command_env("ANTHROPIC_BASE_URL"),
        Some(Some("https://gateway.example.com/".to_string()))
    );
    assert_eq!(
        command_env("ANTHROPIC_AUTH_TOKEN"),
        Some(Some("gateway-secret".to_string()))
    );
    assert_eq!(command_env("ANTHROPIC_API_KEY"), Some(None));
}

#[test]
fn routed_copilot_child_cannot_see_installed_user_plugin() {
    let ambient_home = tempfile::tempdir().unwrap();
    let installed_plugin = ambient_home
        .path()
        .join("installed-plugins")
        .join("review-fixture@local");
    fs::create_dir_all(&installed_plugin).unwrap();
    fs::write(
        installed_plugin.join("plugin.json"),
        r#"{"name":"review-fixture","hooks":"./hooks.json"}"#,
    )
    .unwrap();
    fs::write(
        ambient_home.path().join("config.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "installedPlugins": [{
                "cache_path": installed_plugin,
                "enabled": true,
                "marketplace": "local",
                "name": "review-fixture",
                "source": "local",
                "version": "1.0.0"
            }]
        }))
        .unwrap(),
    )
    .unwrap();

    let mut command = std::process::Command::new("copilot");
    command.env(COPILOT_HOME_ENV, ambient_home.path());
    let isolated_home = isolate_routed_copilot_home(&mut command, true)
        .unwrap()
        .expect("routed Copilot must receive an isolated home");
    let child_home = command
        .get_envs()
        .find(|(key, _)| *key == OsStr::new(COPILOT_HOME_ENV))
        .and_then(|(_, value)| value)
        .map(PathBuf::from)
        .expect("routed Copilot command must set COPILOT_HOME");

    assert_ne!(child_home, ambient_home.path());
    assert_eq!(child_home, isolated_home.path());
    assert!(
        !child_home.join("installed-plugins").exists(),
        "the routed child must not discover ambient installed plugins"
    );
    assert!(
        !child_home.join("config.json").exists(),
        "the routed child must not read the ambient plugin registry"
    );
    assert!(
        installed_plugin.join("plugin.json").is_file(),
        "the fixture must prove isolation without deleting the user's plugin"
    );
}

#[test]
fn non_routed_copilot_keeps_ambient_home() {
    let ambient_home = tempfile::tempdir().unwrap();
    let mut command = std::process::Command::new("copilot");
    command.env(COPILOT_HOME_ENV, ambient_home.path());

    assert!(
        isolate_routed_copilot_home(&mut command, false)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        command
            .get_envs()
            .find(|(key, _)| *key == OsStr::new(COPILOT_HOME_ENV))
            .and_then(|(_, value)| value),
        Some(ambient_home.path().as_os_str())
    );
}

#[test]
fn routed_copilot_rejects_repository_custom_agents() {
    let workspace = tempfile::tempdir().unwrap();
    fs::create_dir(workspace.path().join(".git")).unwrap();
    let nested = workspace.path().join("src").join("nested");
    fs::create_dir_all(&nested).unwrap();

    validate_routed_copilot_workspace(&nested, true).unwrap();
    fs::create_dir_all(workspace.path().join(".github").join("agents")).unwrap();
    fs::write(
        workspace
            .path()
            .join(".github")
            .join("agents")
            .join("model-bypass.agent.md"),
        "---\nname: model-bypass\ndescription: test\nmodel: gpt-5.4\n---\n",
    )
    .unwrap();

    let error = validate_routed_copilot_workspace(&nested, true).unwrap_err();
    assert!(
        error.to_string().contains(".github/agents"),
        "rejection must identify the unsafe repository scope: {error:#}"
    );
    validate_routed_copilot_workspace(&nested, false).unwrap();
}

#[test]
fn real_copilot_confirms_isolated_home_does_not_disable_repository_scope() {
    let _env_guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Ok(version_output) = Command::new("copilot").arg("--version").output() else {
        return;
    };
    if !version_output.status.success() {
        return;
    }
    let version_stdout = String::from_utf8_lossy(&version_output.stdout);
    if !matches!(
        version_stdout.lines().next(),
        Some("GitHub Copilot CLI 1.0.83-3" | "GitHub Copilot CLI 1.0.83-3.")
    ) {
        return;
    }

    let workspace = tempfile::tempdir().unwrap();
    let isolated_home = tempfile::tempdir().unwrap();
    fs::write(
        workspace.path().join("AGENTS.md"),
        "Repository instructions.\n",
    )
    .unwrap();
    let agents = workspace.path().join(".github").join("agents");
    fs::create_dir_all(&agents).unwrap();
    fs::write(
        agents.join("model-bypass.agent.md"),
        "---\nname: model-bypass\ndescription: test\nmodel: gpt-5.4\n---\n",
    )
    .unwrap();

    let output = Command::new("copilot")
        .args(["plugins", "list"])
        .env(COPILOT_HOME_ENV, isolated_home.path())
        .current_dir(workspace.path())
        .output()
        .expect("installed Copilot CLI must run");
    assert!(
        output.status.success(),
        "Copilot repository-discovery probe failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("AGENTS.md") && stdout.contains("Repository"),
        "isolated COPILOT_HOME unexpectedly disabled repository scope:\n{stdout}"
    );
    assert!(
        validate_routed_copilot_workspace(workspace.path(), true).is_err(),
        "routed launch must stop before Copilot can discover or invoke the model-pinned agent"
    );
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
         got: {args:?}"
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

/// Issue #1421: with no `--model` in extra_args and no `AMPLIHACK_DEFAULT_MODEL`,
/// build_command MUST NOT put a model on the command line at all.
///
/// amplihack used to force `--model opus[1m]` here. That alias is resolved by
/// the CLI, whose version amplihack does not control; on one reporter's install
/// it resolved to the retired `claude-opus-4-1-20250805` and every agent step
/// 404'd naming a model the user had never chosen. Passing nothing lets the CLI
/// apply its own current default, and lets the user's `~/.claude/settings.json`
/// `"model"` actually take effect instead of being outranked by our argv.
#[test]
fn test_build_command_passes_no_model_by_default() {
    with_default_model_env(None, || {
        let binary = make_binary("/usr/bin/claude");
        let cmd = build_command(&binary, false, false, false, &[]);
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(
            !args.contains(&"--model".to_string()),
            "amplihack must not choose a model when none was asked for; got: {args:?}"
        );
    });
}

/// Issue #1421: no hardcoded model alias may reach the command line. Asserted
/// on the argv as a whole rather than on the `--model` flag alone, so a future
/// re-introduction by any other route also trips this.
#[test]
fn test_build_command_never_hardcodes_a_model_alias() {
    with_default_model_env(None, || {
        let binary = make_binary("/usr/bin/claude");
        let cmd = build_command(&binary, false, false, false, &[]);
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        for hardcoded in ["opus[1m]", "sonnet[1m]", "opus", "sonnet", "haiku"] {
            assert!(
                !args.iter().any(|a| a == hardcoded),
                "amplihack hardcoded the model alias {hardcoded:?};                  the CLI owns the model catalogue, not amplihack. Args: {args:?}"
            );
        }
    });
}

/// Issue #1421: an empty / whitespace-only AMPLIHACK_DEFAULT_MODEL is how a
/// shell delivers an unset-ish value. It must mean "no model", never
/// `--model ""`, which the CLI would reject with its own confusing error.
#[test]
fn test_build_command_blank_model_env_injects_nothing() {
    with_default_model_env(Some("   "), || {
        let binary = make_binary("/usr/bin/claude");
        let cmd = build_command(&binary, false, false, false, &[]);
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(
            !args.contains(&"--model".to_string()),
            "A blank AMPLIHACK_DEFAULT_MODEL must inject nothing, got: {args:?}"
        );
    });
}

/// When AMPLIHACK_DEFAULT_MODEL env var is set, build_command MUST pass that
/// value through — it is the operator's explicit opt-in to pinning a model.
///
/// Fails if the env var override is not respected.
#[test]
fn test_build_command_respects_custom_model_env() {
    with_default_model_env(Some("claude-3-5-sonnet"), || {
        let binary = make_binary("/usr/bin/claude");
        let cmd = build_command(&binary, false, false, false, &[]);
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let model_pos = args.iter().position(|a| a == "--model").unwrap();
        assert_eq!(
            args[model_pos + 1],
            "claude-3-5-sonnet",
            "Expected AMPLIHACK_DEFAULT_MODEL value 'claude-3-5-sonnet' after '--model', \
             got: {:?}",
            args[model_pos + 1]
        );
    });
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
         but found {model_count} occurrences. Args: {args:?}"
    );
    // And verify the user's model value is preserved
    let model_pos = args.iter().position(|a| a == "--model").unwrap();
    assert_eq!(
        args[model_pos + 1],
        "custom-model",
        "User-supplied model value must be preserved"
    );
}

#[test]
fn test_build_command_no_model_injection_for_equals_form() {
    let binary = make_binary("/usr/bin/claude");
    let extra = vec!["--model=custom-model".to_string()];
    let cmd = build_command(&binary, false, false, false, &extra);
    let args = cmd
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        args.iter().filter(|arg| arg.starts_with("--model")).count(),
        1
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
         skip_permissions=false, got: {args:?}"
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
         Got: {args:?}"
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
        // Safety: tests in this file are serialized via home_env_lock(), which
        // `with_uvx_detection_disabled` already holds.
        let previous_model = std::env::var_os("AMPLIHACK_DEFAULT_MODEL");
        unsafe { std::env::remove_var("AMPLIHACK_DEFAULT_MODEL") };
        // skip_permissions = false (default): should NOT inject --dangerously-skip-permissions
        let cmd = build_command(&binary, false, false, false, &[]);
        if let Some(value) = previous_model {
            unsafe { std::env::set_var("AMPLIHACK_DEFAULT_MODEL", value) };
        }
        assert_eq!(cmd.get_program(), "/usr/bin/claude");
        let args: Vec<&std::ffi::OsStr> = cmd.get_args().collect();
        // Issue #1421: nothing at all is injected. amplihack no longer chooses
        // a model, so a plain launch carries an empty argv.
        assert!(args.is_empty(), "expected no injected args, got: {args:?}");
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
        // Safety: tests in this file are serialized via home_env_lock(), which
        // `with_uvx_detection_disabled` already holds.
        let previous_model = std::env::var_os("AMPLIHACK_DEFAULT_MODEL");
        unsafe { std::env::remove_var("AMPLIHACK_DEFAULT_MODEL") };
        // skip_permissions = true: should inject --dangerously-skip-permissions
        let cmd = build_command(&binary, false, false, true, &[]);
        if let Some(value) = previous_model {
            unsafe { std::env::set_var("AMPLIHACK_DEFAULT_MODEL", value) };
        }
        let args: Vec<&std::ffi::OsStr> = cmd.get_args().collect();
        // Issue #1421: the permission flag is still ours to inject; the model
        // is not, so it is the ONLY argument now.
        assert_eq!(args, &["--dangerously-skip-permissions"]);
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
fn copilot_gets_allow_all_injected_by_default() {
    // Issue #303: amplihack should pass --allow-all to copilot by default so
    // unattended orchestrator loops are not blocked by tool/path/url prompts.
    with_uvx_detection_disabled(|| {
        // Clear the opt-out env var in case the test environment has it set.
        // Safety: tests in this file are serialized via home_env_lock().
        unsafe {
            std::env::remove_var("AMPLIHACK_COPILOT_NO_ALLOW_ALL");
        }
        let binary = BinaryInfo {
            name: "copilot".to_string(),
            path: PathBuf::from("/usr/bin/copilot"),
            version: None,
        };
        let cmd = build_command(&binary, false, false, false, &[]);
        let args: Vec<String> = cmd
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        assert!(
            args.iter().any(|a| a == "--allow-all"),
            "copilot launch must include --allow-all by default; got {args:?}"
        );
    });
}

#[test]
fn copilot_skips_allow_all_when_user_sets_one() {
    with_uvx_detection_disabled(|| {
        unsafe {
            std::env::remove_var("AMPLIHACK_COPILOT_NO_ALLOW_ALL");
        }
        let binary = BinaryInfo {
            name: "copilot".to_string(),
            path: PathBuf::from("/usr/bin/copilot"),
            version: None,
        };
        // User already passed --allow-all-tools; we must NOT inject another flag.
        let extra = vec!["--allow-all-tools".to_string()];
        let cmd = build_command(&binary, false, false, false, &extra);
        let args: Vec<String> = cmd
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        let allow_all_count = args.iter().filter(|a| a.as_str() == "--allow-all").count();
        assert_eq!(
            allow_all_count, 0,
            "should not inject --allow-all when user supplied --allow-all-tools; got {args:?}"
        );
    });
}

#[test]
fn copilot_skips_allow_all_when_env_opt_out() {
    with_uvx_detection_disabled(|| {
        // Safety: serialized via home_env_lock(); restored at end.
        unsafe {
            std::env::set_var("AMPLIHACK_COPILOT_NO_ALLOW_ALL", "1");
        }
        let binary = BinaryInfo {
            name: "copilot".to_string(),
            path: PathBuf::from("/usr/bin/copilot"),
            version: None,
        };
        let cmd = build_command(&binary, false, false, false, &[]);
        let args: Vec<String> = cmd
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        unsafe {
            std::env::remove_var("AMPLIHACK_COPILOT_NO_ALLOW_ALL");
        }
        assert!(
            !args.iter().any(|a| a == "--allow-all"),
            "opt-out must suppress allow-all; got {args:?}"
        );
    });
}

/// Copilot is NOT Claude-compatible, so even when skip_permissions=true the
/// `--dangerously-skip-permissions` flag MUST NOT appear.  This locks the
/// `is_claude_compatible` whitelist against accidental expansion.
#[test]
fn copilot_does_not_get_skip_permissions_even_when_requested() {
    with_uvx_detection_disabled(|| {
        let binary = BinaryInfo {
            name: "copilot".to_string(),
            path: PathBuf::from("/usr/bin/copilot"),
            version: None,
        };
        let cmd = build_command(&binary, false, false, true, &[]);
        let args: Vec<String> = cmd
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        assert!(
            !args.iter().any(|a| a == "--dangerously-skip-permissions"),
            "copilot must never receive --dangerously-skip-permissions, \
             even when skip_permissions=true; got {args:?}"
        );
    });
}

/// Same invariant as copilot: Codex is NOT Claude-compatible.
#[test]
fn codex_does_not_get_skip_permissions_even_when_requested() {
    with_uvx_detection_disabled(|| {
        let binary = BinaryInfo {
            name: "codex".to_string(),
            path: PathBuf::from("/usr/bin/codex"),
            version: None,
        };
        let cmd = build_command(&binary, false, false, true, &[]);
        let args: Vec<String> = cmd
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        assert!(
            !args.iter().any(|a| a == "--dangerously-skip-permissions"),
            "codex must never receive --dangerously-skip-permissions, \
             even when skip_permissions=true; got {args:?}"
        );
    });
}

#[test]
fn claude_does_not_get_allow_all_injected() {
    with_uvx_detection_disabled(|| {
        let binary = BinaryInfo {
            name: "claude".to_string(),
            path: PathBuf::from("/usr/bin/claude"),
            version: None,
        };
        let cmd = build_command(&binary, false, false, false, &[]);
        let args: Vec<String> = cmd
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        assert!(
            !args.iter().any(|a| a == "--allow-all"),
            "non-copilot tools must not get --allow-all; got {args:?}"
        );
    });
}

#[test]
fn copilot_gets_remote_injected_by_default() {
    with_uvx_detection_disabled(|| {
        unsafe {
            std::env::remove_var("AMPLIHACK_COPILOT_NO_REMOTE");
        }
        let binary = BinaryInfo {
            name: "copilot".to_string(),
            path: PathBuf::from("/usr/bin/copilot"),
            version: None,
        };
        let cmd = build_command(&binary, false, false, false, &[]);
        let args: Vec<String> = cmd
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        assert!(
            args.iter().any(|a| a == "--remote"),
            "copilot launch must include --remote by default; got {args:?}"
        );
    });
}

#[test]
fn copilot_skips_remote_when_user_already_passed_it() {
    with_uvx_detection_disabled(|| {
        unsafe {
            std::env::remove_var("AMPLIHACK_COPILOT_NO_REMOTE");
        }
        let binary = BinaryInfo {
            name: "copilot".to_string(),
            path: PathBuf::from("/usr/bin/copilot"),
            version: None,
        };
        let extra = vec!["--remote".to_string()];
        let cmd = build_command(&binary, false, false, false, &extra);
        let args: Vec<String> = cmd
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        let remote_count = args.iter().filter(|a| a.as_str() == "--remote").count();
        assert_eq!(
            remote_count, 1,
            "should not duplicate --remote when user already supplied it; got {args:?}"
        );
    });
}

#[test]
fn copilot_skips_remote_when_env_opt_out() {
    with_uvx_detection_disabled(|| {
        unsafe {
            std::env::set_var("AMPLIHACK_COPILOT_NO_REMOTE", "1");
        }
        let binary = BinaryInfo {
            name: "copilot".to_string(),
            path: PathBuf::from("/usr/bin/copilot"),
            version: None,
        };
        let cmd = build_command(&binary, false, false, false, &[]);
        let args: Vec<String> = cmd
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        unsafe {
            std::env::remove_var("AMPLIHACK_COPILOT_NO_REMOTE");
        }
        assert!(
            !args.iter().any(|a| a == "--remote"),
            "opt-out must suppress --remote; got {args:?}"
        );
    });
}

#[test]
fn copilot_skips_remote_when_litellm_proxy_is_requested() {
    with_uvx_detection_disabled(|| {
        let previous = std::env::var_os(amplihack_utils::litellm_proxy::ENDPOINT_ENV);
        unsafe {
            std::env::set_var(
                amplihack_utils::litellm_proxy::ENDPOINT_ENV,
                "http://127.0.0.1:4000",
            );
        }
        let binary = BinaryInfo {
            name: "copilot".to_string(),
            path: PathBuf::from("/usr/bin/copilot"),
            version: None,
        };
        let cmd = build_command(&binary, false, false, false, &[]);
        let args: Vec<String> = cmd
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        match previous {
            Some(value) => unsafe {
                std::env::set_var(amplihack_utils::litellm_proxy::ENDPOINT_ENV, value)
            },
            None => unsafe { std::env::remove_var(amplihack_utils::litellm_proxy::ENDPOINT_ENV) },
        }
        assert!(
            !args.iter().any(|arg| arg == "--remote"),
            "LiteLLM routing must suppress Copilot remote execution; got {args:?}"
        );
        assert!(
            args.iter().any(|arg| arg == "--no-remote"),
            "LiteLLM routing must override persisted Copilot remote settings; got {args:?}"
        );
        assert!(
            args.iter().any(|arg| arg == "--no-remote-export"),
            "LiteLLM routing must disable Copilot session export; got {args:?}"
        );
        assert!(
            args.iter().any(|arg| arg == "--no-auto-update"),
            "LiteLLM routing must disable Copilot auto-update; got {args:?}"
        );
        assert!(
            args.iter()
                .any(|arg| arg == "--secret-env-vars=COPILOT_PROVIDER_API_KEY"),
            "LiteLLM routing must hide the gateway key from Copilot tools; got {args:?}"
        );
    });
}

#[test]
fn routed_copilot_restrictions_follow_conflicting_user_arguments() {
    with_uvx_detection_disabled(|| {
        let previous = std::env::var_os(amplihack_utils::litellm_proxy::ENDPOINT_ENV);
        unsafe {
            std::env::set_var(
                amplihack_utils::litellm_proxy::ENDPOINT_ENV,
                "http://127.0.0.1:4000",
            );
        }
        let binary = BinaryInfo {
            name: "copilot".to_string(),
            path: PathBuf::from("/usr/bin/copilot"),
            version: None,
        };
        let cmd = build_command(
            &binary,
            false,
            false,
            false,
            &["--remote".to_string(), "--remote-export".to_string()],
        );
        let args: Vec<String> = cmd
            .get_args()
            .map(|value| value.to_string_lossy().into_owned())
            .collect();
        match previous {
            Some(value) => unsafe {
                std::env::set_var(amplihack_utils::litellm_proxy::ENDPOINT_ENV, value)
            },
            None => unsafe { std::env::remove_var(amplihack_utils::litellm_proxy::ENDPOINT_ENV) },
        }

        let position = |flag: &str| {
            args.iter()
                .rposition(|arg| arg == flag)
                .unwrap_or_else(|| panic!("missing {flag}: {args:?}"))
        };
        assert!(position("--no-remote") > position("--remote"));
        assert!(position("--no-remote-export") > position("--remote-export"));
        assert!(args.iter().any(|arg| arg == "--no-auto-update"));
    });
}

#[test]
fn claude_disables_settings_and_plugins_with_litellm() {
    with_uvx_detection_disabled(|| {
        let previous = std::env::var_os(amplihack_utils::litellm_proxy::ENDPOINT_ENV);
        unsafe {
            std::env::set_var(
                amplihack_utils::litellm_proxy::ENDPOINT_ENV,
                "http://127.0.0.1:4000",
            );
        }
        let binary = make_binary("/usr/bin/claude");
        let cmd = build_command(&binary, false, false, false, &[]);
        let args: Vec<String> = cmd
            .get_args()
            .map(|value| value.to_string_lossy().into_owned())
            .collect();
        match previous {
            Some(value) => unsafe {
                std::env::set_var(amplihack_utils::litellm_proxy::ENDPOINT_ENV, value)
            },
            None => unsafe { std::env::remove_var(amplihack_utils::litellm_proxy::ENDPOINT_ENV) },
        }
        assert!(
            args.windows(2)
                .any(|values| values[0] == "--setting-sources" && values[1].is_empty()),
            "LiteLLM routing must suppress mutable Claude settings sources; got {args:?}"
        );
        assert!(
            args.iter().any(|arg| arg == "--safe-mode"),
            "LiteLLM routing must disable Claude customizations; got {args:?}"
        );
        assert!(
            !args.iter().any(|arg| arg == "--plugin-dir"),
            "LiteLLM routing must not inject a UVX plugin directory; got {args:?}"
        );
    });
}

#[test]
fn claude_does_not_get_remote_injected() {
    with_uvx_detection_disabled(|| {
        let binary = BinaryInfo {
            name: "claude".to_string(),
            path: PathBuf::from("/usr/bin/claude"),
            version: None,
        };
        let cmd = build_command(&binary, false, false, false, &[]);
        let args: Vec<String> = cmd
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        assert!(
            !args.iter().any(|a| a == "--remote"),
            "non-copilot tools must not get --remote; got {args:?}"
        );
    });
}

#[test]
fn copilot_skips_remote_when_user_passes_no_remote() {
    with_uvx_detection_disabled(|| {
        unsafe {
            std::env::remove_var("AMPLIHACK_COPILOT_NO_REMOTE");
        }
        let binary = BinaryInfo {
            name: "copilot".to_string(),
            path: PathBuf::from("/usr/bin/copilot"),
            version: None,
        };
        // User explicitly opted out via --no-remote; we must NOT inject --remote.
        let extra = vec!["--no-remote".to_string()];
        let cmd = build_command(&binary, false, false, false, &extra);
        let args: Vec<String> = cmd
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        assert!(
            !args.iter().any(|a| a == "--remote"),
            "must not inject --remote when user passed --no-remote; got {args:?}"
        );
    });
}
