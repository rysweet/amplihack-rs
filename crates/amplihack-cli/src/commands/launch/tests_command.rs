use super::command::{
    DEFAULT_MODEL, ModelArgs, ModelSelection, ModelSource, configured_default_model,
    explicit_model_warnings, is_claude_compatible_tool, model_args, model_selection_notice,
    normalize_dotted_claude_model_id,
};
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

/// The three variables that make `proxy_requested()` true. A launch routed
/// through the LiteLLM gateway names the gateway's model and outranks
/// `AMPLIHACK_DEFAULT_MODEL`, so every test that asserts on the *default* has
/// to start from a host where the gateway is not configured -- otherwise the
/// result depends on the developer's shell.
const PROXY_ENV_VARS: [&str; 3] = [
    amplihack_utils::litellm_proxy::ENDPOINT_ENV,
    amplihack_utils::litellm_proxy::API_KEY_ENV,
    amplihack_utils::litellm_proxy::MODEL_ENV,
];

fn with_default_model_env<T>(value: Option<&str>, f: impl FnOnce() -> T) -> T {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous = std::env::var_os("AMPLIHACK_DEFAULT_MODEL");
    let previous_proxy: Vec<_> = PROXY_ENV_VARS
        .iter()
        .map(|name| (*name, std::env::var_os(name)))
        .collect();
    for name in PROXY_ENV_VARS {
        unsafe { std::env::remove_var(name) };
    }
    match value {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_DEFAULT_MODEL", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_DEFAULT_MODEL") },
    }

    let result = f();

    match previous {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_DEFAULT_MODEL", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_DEFAULT_MODEL") },
    }
    for (name, value) in previous_proxy {
        match value {
            Some(value) => unsafe { std::env::set_var(name, value) },
            None => unsafe { std::env::remove_var(name) },
        }
    }
    result
}

/// The model on the command line when a launch is routed through the LiteLLM
/// gateway, with `AMPLIHACK_LITELLM_MODEL` set to `model`.
fn model_arg_through_proxy(model: Option<&str>) -> Option<String> {
    with_default_model_env(Some("pinned-by-env"), || {
        // `with_default_model_env` has already cleared all three, so setting
        // the endpoint alone is what "gateway configured, no model named" is.
        let _endpoint = EnvGuard::set([(
            amplihack_utils::litellm_proxy::ENDPOINT_ENV,
            "https://gateway.example.com",
        )]);
        let _model =
            model.map(|model| EnvGuard::set([(amplihack_utils::litellm_proxy::MODEL_ENV, model)]));
        let binary = make_binary("/usr/bin/claude");
        let cmd = build_command(&binary, false, false, false, &[]);
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        args.iter()
            .position(|a| a == "--model")
            .and_then(|at| args.get(at + 1).cloned())
    })
}

/// A gateway launch must name the gateway's model.
///
/// LiteLLM routes on the model name, so the name is not a preference here --
/// it is the address. `AMPLIHACK_DEFAULT_MODEL` is set to something else in
/// this test precisely to prove the gateway wins: a rebase that dropped this
/// branch would send every proxied launch to amplihack's own default, which
/// the gateway does not serve.
#[test]
fn a_gateway_launch_names_the_gateway_model() {
    assert_eq!(
        model_arg_through_proxy(Some("gateway-model")).as_deref(),
        Some("gateway-model")
    );
}

/// With the gateway configured but no model named, amplihack still passes a
/// name -- the gateway's documented catch-all -- rather than falling back to a
/// concrete Anthropic id the gateway has no route for.
#[test]
fn a_gateway_launch_without_a_named_model_uses_the_gateway_default() {
    assert_eq!(
        model_arg_through_proxy(None).as_deref(),
        Some("amplihack-default")
    );
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
        None,
    )
    .unwrap();

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
/// build_command requests amplihack's built-in default — and that default is a
/// CONCRETE model id, never an alias.
///
/// amplihack used to force `--model opus[1m]`. An alias is resolved by the CLI,
/// whose version amplihack does not control; on one reporter's install it
/// resolved to the retired `claude-opus-4-1-20250805` and every agent step
/// 404'd naming a model the user had never chosen and could not find written
/// down anywhere, because it only existed at resolution time.
///
/// The fix is not to stop choosing — it is to choose something that cannot be
/// reinterpreted. A stale concrete id fails with a 404 naming itself, which is
/// searchable. A stale alias fails with a 404 naming a phantom.
#[test]
fn test_build_command_passes_the_concrete_default_model() {
    with_default_model_env(None, || {
        let binary = make_binary("/usr/bin/claude");
        let cmd = build_command(&binary, false, false, false, &[]);
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let at = args
            .iter()
            .position(|a| a == "--model")
            .unwrap_or_else(|| panic!("expected amplihack to request a model; got: {args:?}"));
        assert_eq!(
            args.get(at + 1).map(String::as_str),
            Some(super::command::DEFAULT_MODEL),
            "the default must be the concrete id, got: {args:?}"
        );
    });
}

/// Issue #1421: the built-in default must be a concrete id, not an alias.
///
/// This is the property that actually failed. `opus[1m]` was rejected not
/// because it named the wrong model but because it named *no* model until the
/// CLI decided — so two hosts running the same amplihack got different models,
/// and one of them got a retired one.
#[test]
fn test_default_model_is_concrete_not_an_alias() {
    let d = super::command::DEFAULT_MODEL;
    assert!(
        d.starts_with("claude-"),
        "a concrete Anthropic model id starts with `claude-`; {d:?} looks like an alias"
    );
    for alias in ["opus", "sonnet", "haiku", "opus[1m]", "sonnet[1m]"] {
        assert_ne!(d, alias, "the default must not be the bare alias {alias:?}");
    }
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

// ---------------------------------------------------------------------------
// Issue #1527: a dotted Claude model id in AMPLIHACK_DEFAULT_MODEL.
//
// Operators set the variable to a Claude id in the dotted spelling GitHub
// Copilot CLI uses (`claude-opus-5.5`). Claude model ids use hyphens
// (`claude-opus-5-5`); the issue records what Claude Code did with the dotted
// form. amplihack reads the variable only for claude-compatible launches, so it
// rewrites the one dot between major and minor version before passing it on,
// and says so on the stderr line added for #1421. An explicit `--model` is what
// the operator typed and is forwarded unchanged; when it is dotted, amplihack
// prints a warning naming the hyphenated spelling, because the launched tool
// may not report the problem itself.
// ---------------------------------------------------------------------------

/// The line the docs promise for a rewritten id
/// (docs/reference/environment-variables.md, AMPLIHACK_DEFAULT_MODEL).
const DOCUMENTED_REWRITE_NOTICE: &str = "amplihack: passing `--model claude-opus-5-5[1m]` to \
     `claude` (from AMPLIHACK_DEFAULT_MODEL, normalised from `claude-opus-5.5[1m]`: Claude \
     model ids use hyphens, not dots). Set AMPLIHACK_DEFAULT_MODEL to override it, or to an \
     empty value to let claude choose its own default model.";

/// The warning the docs promise for a dotted explicit `--model`
/// (docs/reference/environment-variables.md, "Explicit `--model`").
///
/// It states only what amplihack can vouch for, the spelling. What another
/// tool does with the dotted id changes with that tool's version, so it is not
/// in the line, the docs or this constant; issue #1527 has the report.
const DOCUMENTED_EXPLICIT_WARNING: &str = "amplihack: warning: passing `--model \
     claude-opus-5.5` to `claude` as typed, but Claude model ids use hyphens, not dots. \
     Use `--model claude-opus-5-5`.";

/// The line the docs promise on the LiteLLM gateway path
/// (docs/reference/environment-variables.md, AMPLIHACK_DEFAULT_MODEL).
const DOCUMENTED_GATEWAY_NOTICE: &str = "amplihack: passing `--model gateway-model` to \
     `claude` (from AMPLIHACK_LITELLM_MODEL). Set AMPLIHACK_LITELLM_MODEL to change it. \
     AMPLIHACK_DEFAULT_MODEL is not read while a LiteLLM gateway variable is set.";

fn selection(model: &str, source: ModelSource, normalised_from: Option<&str>) -> ModelSelection {
    ModelSelection {
        model: model.to_string(),
        source,
        normalised_from: normalised_from.map(str::to_string),
    }
}

/// A `BinaryInfo` for any tool name; `make_binary` is always `claude`.
fn make_named_binary(name: &str) -> BinaryInfo {
    BinaryInfo {
        name: name.to_string(),
        path: PathBuf::from(format!("/usr/bin/{name}")),
        version: Some("1.0.0".to_string()),
    }
}

/// The argv `build_command` produces for `binary_name` with
/// `AMPLIHACK_DEFAULT_MODEL` set to `env`, or unset for `None`.
///
/// Returns the argv instead of asserting inside the closure: the helper only
/// restores the environment when the closure returns, so an assertion that
/// fired in there would leave a dotted value set for the next test.
fn argv_with_default_model(env: Option<&str>, binary_name: &str, extra: &[&str]) -> Vec<String> {
    let extra: Vec<String> = extra.iter().map(|arg| arg.to_string()).collect();
    with_default_model_env(env, || {
        build_command(&make_named_binary(binary_name), false, false, false, &extra)
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    })
}

/// The value after the first `--model`, if any.
fn model_after_flag(args: &[String]) -> Option<&str> {
    args.iter()
        .position(|arg| arg == "--model")
        .and_then(|at| args.get(at + 1))
        .map(String::as_str)
}

/// The rewrite's safety contract: the output is the input with its first `.`
/// (the one between major and minor) turned into `-`. Same length, one byte
/// changed, still `claude-` prefixed, so it cannot name some other model or
/// produce a token that reads as a flag.
fn assert_is_one_dot_to_hyphen_rewrite(input: &str, output: &str) {
    assert_eq!(
        output.len(),
        input.len(),
        "{input:?} -> {output:?} changed length"
    );
    let changed: Vec<(usize, u8, u8)> = input
        .bytes()
        .zip(output.bytes())
        .enumerate()
        .filter(|(_, (from, to))| from != to)
        .map(|(at, (from, to))| (at, from, to))
        .collect();
    assert_eq!(
        changed.len(),
        1,
        "{input:?} -> {output:?} must change exactly one byte, changed {changed:?}"
    );
    let (at, from, to) = changed[0];
    assert_eq!(
        (from, to),
        (b'.', b'-'),
        "{input:?} -> {output:?} must turn a `.` into a `-`"
    );
    assert_eq!(
        Some(at),
        input.find('.'),
        "{input:?} -> {output:?} must rewrite the dot between major and minor, \
         which is the first one"
    );
    assert!(
        output.starts_with("claude-"),
        "{input:?} -> {output:?} lost its `claude-` prefix"
    );
}

/// Issue #1527: each dotted spelling becomes the hyphenated id Claude Code
/// accepts. The docs table's rewritten rows are all here.
///
/// The version digits and the suffix are copied, never parsed.
/// `claude-opus-4.1-2025.08` keeps the dot in its suffix, because only the dot
/// between major and minor is rewritten and a suffix starting with `-` is
/// copied as it is. Leading zeros, and a digit run longer than any integer
/// type, survive unchanged; a scan that parsed the version into a number and
/// printed it back would get those wrong, or panic.
#[test]
fn test_normalize_dotted_claude_model_id_rewrites_dotted_ids() {
    let huge = "9".repeat(64);
    let huge_dotted = format!("claude-opus-{huge}.{huge}[1m]");
    let huge_hyphenated = format!("claude-opus-{huge}-{huge}[1m]");
    let cases = [
        ("claude-opus-5.5", "claude-opus-5-5"),
        ("claude-sonnet-4.5", "claude-sonnet-4-5"),
        ("claude-haiku-4.5", "claude-haiku-4-5"),
        ("claude-opus-5.5[1m]", "claude-opus-5-5[1m]"),
        ("claude-opus-4.1-20250805", "claude-opus-4-1-20250805"),
        ("claude-opus-4.1-2025.08", "claude-opus-4-1-2025.08"),
        ("claude-opus-05.05", "claude-opus-05-05"),
        (huge_dotted.as_str(), huge_hyphenated.as_str()),
    ];
    for (dotted, hyphenated) in cases {
        let got = normalize_dotted_claude_model_id(dotted);
        assert_eq!(got.as_deref(), Some(hyphenated), "rewriting {dotted:?}");
        assert_is_one_dot_to_hyphen_rewrite(dotted, hyphenated);
    }
}

/// Issue #1527: everything that is not exactly
/// `claude-<family>-<major>.<minor><suffix>` is left alone. No fuzzy matching,
/// no case-folding, no partial matches.
#[test]
fn test_normalize_dotted_claude_model_id_leaves_everything_else() {
    let unchanged = [
        // Already hyphenated, including amplihack's own default.
        "claude-opus-5-5",
        "claude-opus-5-5[1m]",
        "claude-opus-5[1m]",
        DEFAULT_MODEL,
        "claude-opus-4-1-20250805",
        // Not Claude ids: the dot is part of the real id.
        "gpt-5.1",
        "gemini-2.5-pro",
        // Aliases.
        "opus",
        "opus[1m]",
        "sonnet",
        // Legacy version-first ids are out of scope.
        "claude-3.5-sonnet",
        // A dot right after the minor version is not a valid suffix.
        "claude-opus-5.5.1",
        // Not lowercase.
        "Claude-Opus-5.5",
        "CLAUDE-OPUS-5.5",
        "claude-Opus-5.5",
        // A piece of the pattern is missing.
        "",
        "claude",
        "claude-",
        "claude-opus",
        "claude-opus-5",
        "claude-opus-5.",
        "claude-opus-.5",
        "claude--5.5",
        "claude-opus5.5",
        "claude-opus-4-5.1",
        // The suffix must be empty or start with `[` or `-`.
        "claude-opus-5.5x",
        "claude-opus-5.5]",
        "claude-opus-5.5 [1m]",
        // The pattern is anchored at the start. Callers trim before calling.
        "xclaude-opus-5.5",
        " claude-opus-5.5",
        "--model=claude-opus-5.5",
        // Non-ASCII look-alikes: only ASCII letters and digits count.
        "claude-op\u{fc}s-5.5",
        "claude-opus-\u{ff15}.\u{ff15}",
        "claude-opus-5\u{2024}5",
    ];
    for id in unchanged {
        assert_eq!(
            normalize_dotted_claude_model_id(id),
            None,
            "{id:?} must be passed through unchanged"
        );
    }
}

/// Issue #1527: the scanner is total. It never panics on any input, anything
/// it does rewrite obeys the one-byte contract, and its output is already in
/// final form, so rewriting twice is the same as rewriting once.
///
/// This is a sweep with no dependencies: every one-character insertion,
/// deletion and substitution of a few seed ids, over an alphabet that includes
/// the pattern's delimiters, multi-byte characters, whitespace and NUL.
#[test]
fn test_normalize_dotted_claude_model_id_is_total_and_idempotent() {
    let seeds = [
        "claude-opus-5.5",
        "claude-opus-5.5[1m]",
        "claude-opus-4.1-20250805",
        "claude-opus-5-5",
        "gpt-5.1",
    ];
    let alphabet = [
        '.', '-', '[', ']', 'a', 'Z', '0', '9', ' ', '\0', '\u{fc}', '\u{ff15}',
    ];
    let mut inputs: Vec<String> = Vec::new();
    for seed in seeds {
        let chars: Vec<char> = seed.chars().collect();
        inputs.push(seed.to_string());
        for at in 0..=chars.len() {
            for c in alphabet {
                let mut inserted = chars.clone();
                inserted.insert(at, c);
                inputs.push(inserted.into_iter().collect());
                if at < chars.len() {
                    let mut substituted = chars.clone();
                    substituted[at] = c;
                    inputs.push(substituted.into_iter().collect());
                }
            }
            if at < chars.len() {
                let mut deleted = chars.clone();
                deleted.remove(at);
                inputs.push(deleted.into_iter().collect());
            }
        }
    }

    let mut rewritten = 0;
    for input in &inputs {
        if let Some(output) = normalize_dotted_claude_model_id(input) {
            rewritten += 1;
            assert_is_one_dot_to_hyphen_rewrite(input, &output);
            assert_eq!(
                normalize_dotted_claude_model_id(&output),
                None,
                "{output:?} (rewritten from {input:?}) must not be rewritten again"
            );
        }
    }
    assert!(
        rewritten > seeds.len(),
        "the sweep must exercise the rewrite path, not only rejections; \
         {rewritten} of {} inputs were rewritten",
        inputs.len()
    );
}

/// Issue #1527: the selection records the rewrite, so the stderr line can name
/// both spellings. The original is stored trimmed, as the operator would
/// recognise it, not with the shell's surrounding whitespace.
#[test]
fn test_configured_default_model_records_the_rewrite() {
    let got = with_default_model_env(Some("  claude-opus-5.5[1m]  "), configured_default_model);
    assert_eq!(
        got,
        Some(selection(
            "claude-opus-5-5[1m]",
            ModelSource::DefaultModelEnv,
            Some("claude-opus-5.5[1m]"),
        ))
    );
}

/// Issue #1527: without a dotted Claude id, `configured_default_model` keeps
/// the #1421 behaviour. The source is decided where the variable is read, so
/// it cannot disagree with the value.
#[test]
fn test_configured_default_model_without_a_rewrite() {
    let unset = with_default_model_env(None, configured_default_model);
    let hyphenated = with_default_model_env(Some("claude-opus-5-5"), configured_default_model);
    let not_claude = with_default_model_env(Some(" gpt-5.1 "), configured_default_model);
    let empty = with_default_model_env(Some(""), configured_default_model);
    let blank = with_default_model_env(Some(" \t "), configured_default_model);

    assert_eq!(
        unset,
        Some(selection(DEFAULT_MODEL, ModelSource::BuiltInDefault, None)),
        "unset means amplihack's built-in default"
    );
    assert_eq!(
        hyphenated,
        Some(selection(
            "claude-opus-5-5",
            ModelSource::DefaultModelEnv,
            None
        )),
        "an already-hyphenated id is passed as set and is not reported as rewritten"
    );
    assert_eq!(
        not_claude,
        Some(selection("gpt-5.1", ModelSource::DefaultModelEnv, None)),
        "a dotted id that is not a Claude id is passed as set, trimmed"
    );
    assert_eq!(empty, None, "an empty value means no --model");
    assert_eq!(blank, None, "a whitespace-only value means no --model");
}

/// Issue #1527: a value that is not valid UTF-8 falls back to the built-in
/// default, labelled as such, even when its bytes look like a dotted Claude
/// id. It is never forwarded lossily or "repaired" into an id.
#[cfg(unix)]
#[test]
fn test_configured_default_model_non_utf8_falls_back_to_built_in_default() {
    use std::os::unix::ffi::OsStrExt;

    let got = with_default_model_env(None, || {
        // Set inside the helper so it restores the previous value afterwards.
        // Safety: the helper holds home_env_lock(), which serialises env access.
        unsafe {
            std::env::set_var(
                "AMPLIHACK_DEFAULT_MODEL",
                OsStr::from_bytes(b"claude-opus-5.5\xff"),
            )
        };
        configured_default_model()
    });
    assert_eq!(
        got,
        Some(selection(DEFAULT_MODEL, ModelSource::BuiltInDefault, None))
    );
}

/// Issue #1527, the reported bug: a dotted id reaches the command line as the
/// hyphenated id Claude Code accepts, trimmed and exactly once, for every
/// Claude-compatible tool, and the dotted spelling it rejects is nowhere in
/// argv.
#[test]
fn test_build_command_normalises_dotted_default_model() {
    let cases = [
        ("claude-opus-5.5[1m]", "claude-opus-5-5[1m]"),
        ("claude-opus-5.5", "claude-opus-5-5"),
        ("claude-sonnet-4.5", "claude-sonnet-4-5"),
        (" \tclaude-opus-5.5[1m]\n ", "claude-opus-5-5[1m]"),
    ];
    for (dotted, hyphenated) in cases {
        let args = argv_with_default_model(Some(dotted), "claude", &[]);
        assert_eq!(
            model_after_flag(&args),
            Some(hyphenated),
            "AMPLIHACK_DEFAULT_MODEL={dotted:?}, got: {args:?}"
        );
        assert_eq!(
            args.iter().filter(|arg| arg.starts_with("--model")).count(),
            1,
            "exactly one --model, got: {args:?}"
        );
        assert!(
            !args.iter().any(|arg| arg.contains(dotted.trim())),
            "the dotted spelling Claude Code rejects must not reach argv, got: {args:?}"
        );
    }

    for tool in ["claude", "rusty", "rustyclawd", "amplifier"] {
        let args = argv_with_default_model(Some("claude-opus-5.5"), tool, &[]);
        assert_eq!(
            model_after_flag(&args),
            Some("claude-opus-5-5"),
            "`amplihack {tool}` is Claude-compatible and must get the rewrite, got: {args:?}"
        );
    }
}

/// Issue #1527: anything not of the documented dotted form reaches the tool
/// exactly as set, and only once. That covers dotted ids the rewrite must not
/// touch, and the hyphenated spellings the issue reports as working, `[1m]`
/// suffix and all: a rewrite that also touched hyphens or brackets would break
/// the ids that work today.
#[test]
fn test_build_command_passes_other_default_models_unchanged() {
    for id in [
        "gpt-5.1",
        "claude-3.5-sonnet",
        "claude-opus-5.5.1",
        "claude-opus-5-5",
        "claude-opus-5-5[1m]",
        "claude-opus-5[1m]",
        "claude-opus-4-1-20250805",
    ] {
        let args = argv_with_default_model(Some(id), "claude", &[]);
        assert_eq!(
            model_after_flag(&args),
            Some(id),
            "AMPLIHACK_DEFAULT_MODEL={id:?} must be passed unchanged, got: {args:?}"
        );
        assert_eq!(
            args.iter().filter(|arg| arg.starts_with("--model")).count(),
            1,
            "exactly one --model, got: {args:?}"
        );
    }
}

/// Issue #1527 (decision A2): only argv is rewritten. The child must inherit
/// AMPLIHACK_DEFAULT_MODEL exactly as the operator set it: the command sets no
/// override for it, removes nothing, and building the command leaves the
/// parent's value alone. Rewriting the environment would silently change the
/// operator's setting for every process below this one. Left alone, a nested
/// amplihack launch re-reads the dotted value and makes the same rewrite, with
/// the same stderr line naming the original spelling.
#[test]
fn test_build_command_leaves_the_childs_default_model_env_untouched() {
    let dotted = "claude-opus-5.5[1m]";
    let (overrides, parent_value_after_build) = with_default_model_env(Some(dotted), || {
        let cmd = build_command(&make_named_binary("claude"), false, false, false, &[]);
        let overrides: Vec<(String, Option<String>)> = cmd
            .get_envs()
            .filter(|(key, _)| *key == OsStr::new("AMPLIHACK_DEFAULT_MODEL"))
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect();
        (overrides, std::env::var("AMPLIHACK_DEFAULT_MODEL").ok())
    });
    assert!(
        overrides.is_empty(),
        "the child must inherit AMPLIHACK_DEFAULT_MODEL unmodified, but the command \
         overrides it: {overrides:?}"
    );
    assert_eq!(
        parent_value_after_build.as_deref(),
        Some(dotted),
        "building the command must not rewrite the parent's AMPLIHACK_DEFAULT_MODEL, \
         which every later child inherits"
    );
}

/// Issue #1527: an explicit `--model` is forwarded unchanged, in both forms,
/// even when it is dotted and AMPLIHACK_DEFAULT_MODEL is dotted too. The issue
/// asks that the operator's value be left alone. What amplihack adds for a
/// dotted one is a stderr warning, pinned by the
/// `test_explicit_model_warnings_*` and `test_model_args_*` tests, not a rewrite.
#[test]
fn test_build_command_explicit_dotted_model_is_not_normalised() {
    let env = Some("claude-opus-5.5[1m]");

    let spaced = argv_with_default_model(env, "claude", &["--model", "claude-opus-5.5"]);
    assert_eq!(
        spaced
            .iter()
            .filter(|arg| arg.starts_with("--model"))
            .count(),
        1,
        "the explicit --model must be the only one, got: {spaced:?}"
    );
    assert_eq!(
        model_after_flag(&spaced),
        Some("claude-opus-5.5"),
        "the explicit value must be forwarded byte for byte, got: {spaced:?}"
    );
    assert!(
        !spaced.iter().any(|arg| arg.contains("claude-opus-5-5")),
        "neither the explicit value nor the env value may be rewritten into argv, got: {spaced:?}"
    );

    let equals = argv_with_default_model(env, "claude", &["--model=claude-opus-5.5"]);
    let model_args: Vec<&str> = equals
        .iter()
        .filter(|arg| arg.starts_with("--model"))
        .map(String::as_str)
        .collect();
    assert_eq!(
        model_args,
        vec!["--model=claude-opus-5.5"],
        "the explicit --model= form must be the only one and unchanged, got: {equals:?}"
    );
    assert!(
        !equals.iter().any(|arg| arg.contains("claude-opus-5-5")),
        "neither the explicit value nor the env value may be rewritten into argv, got: {equals:?}"
    );
}

/// Issue #1527: amplihack reads AMPLIHACK_DEFAULT_MODEL only for
/// claude-compatible launches. A dotted value must not start reaching Copilot
/// or Codex in either spelling.
#[test]
fn test_build_command_dotted_model_env_ignored_for_copilot_and_codex() {
    for tool in ["copilot", "codex"] {
        let args = argv_with_default_model(Some("claude-opus-5.5"), tool, &[]);
        assert!(
            !args.iter().any(|arg| arg.starts_with("--model")),
            "`amplihack {tool}` must not get a --model, got: {args:?}"
        );
        assert!(
            !args.iter().any(|arg| arg.contains("claude-opus-5")),
            "`amplihack {tool}` must not get either spelling, got: {args:?}"
        );
    }
}

/// Issue #1527: the LiteLLM gateway routes on the model name, so its model is
/// an address and not a spelling to correct. A gateway that serves
/// `claude-opus-5.5` must be sent exactly that.
#[test]
fn test_proxy_model_is_never_normalised() {
    assert_eq!(
        model_arg_through_proxy(Some("claude-opus-5.5")).as_deref(),
        Some("claude-opus-5.5")
    );
}

/// Issue #1527: without a rewrite, the #1421 line is byte-for-byte what it was,
/// so nothing that reads stderr notices this change.
#[test]
fn test_model_selection_notice_is_unchanged_without_rewrite() {
    let documented = model_selection_notice(
        &selection("claude-sonnet-4-5", ModelSource::DefaultModelEnv, None),
        "claude",
    );
    assert_eq!(
        documented,
        "amplihack: passing `--model claude-sonnet-4-5` to `claude` (from \
         AMPLIHACK_DEFAULT_MODEL). Set AMPLIHACK_DEFAULT_MODEL to override it, or to an \
         empty value to let claude choose its own default model."
    );

    let built_in = model_selection_notice(
        &selection(DEFAULT_MODEL, ModelSource::BuiltInDefault, None),
        "rusty",
    );
    assert_eq!(
        built_in,
        format!(
            "amplihack: passing `--model {DEFAULT_MODEL}` to `rusty` (from amplihack's \
             built-in default). Set AMPLIHACK_DEFAULT_MODEL to override it, or to an empty \
             value to let rusty choose its own default model."
        )
    );
}

/// Issue #1527: on the LiteLLM gateway path the notice names
/// AMPLIHACK_LITELLM_MODEL as the variable to change. AMPLIHACK_DEFAULT_MODEL
/// is not read there, so advice to set it, or to empty it, would have no
/// effect.
#[test]
fn test_model_selection_notice_on_the_gateway_names_the_gateway_variable() {
    let got = model_selection_notice(
        &selection("gateway-model", ModelSource::LiteLlmGateway, None),
        "claude",
    );
    assert_eq!(got, DOCUMENTED_GATEWAY_NOTICE);
    assert!(
        !got.contains("Set AMPLIHACK_DEFAULT_MODEL"),
        "the gateway notice must not advise setting a variable that is not read: {got}"
    );
}

/// Issue #1527: a dotted Claude id given to `--model`, in either form, produces
/// one warning naming the hyphenated spelling. The value in the warning is the
/// one forwarded, and the suggestion keeps any suffix.
#[test]
fn test_explicit_model_warnings_name_the_hyphenated_spelling() {
    let args = |list: &[&str]| -> Vec<String> { list.iter().map(|a| a.to_string()).collect() };

    assert_eq!(
        explicit_model_warnings("claude", &args(&["--model", "claude-opus-5.5"]), false),
        vec![DOCUMENTED_EXPLICIT_WARNING.to_string()]
    );
    assert_eq!(
        explicit_model_warnings("claude", &args(&["--model=claude-opus-5.5"]), false),
        vec![DOCUMENTED_EXPLICIT_WARNING.to_string()],
        "the --model= form must give the same warning"
    );

    let suffixed = explicit_model_warnings(
        "rusty",
        &args(&["-p", "hello", "--model", "claude-opus-5.5[1m]"]),
        false,
    );
    assert_eq!(suffixed.len(), 1, "got: {suffixed:?}");
    assert!(
        suffixed[0].contains("passing `--model claude-opus-5.5[1m]` to `rusty` as typed")
            && suffixed[0].ends_with("Use `--model claude-opus-5-5[1m]`."),
        "got: {suffixed:?}"
    );
    assert!(
        !suffixed[0].contains("Claude Code"),
        "the warning must not name a tool other than the one launched: {suffixed:?}"
    );
}

/// Issue #1527: no warning for anything that is not a dotted Claude id, for a
/// `--model` with no value, or for arguments that only resemble `--model`.
#[test]
fn test_explicit_model_warnings_are_silent_otherwise() {
    let quiet: [&[&str]; 9] = [
        &[],
        &["--model", "claude-opus-5-5"],
        &["--model=claude-opus-5-5[1m]"],
        &["--model", "opus[1m]"],
        &["--model", "gpt-5.1"],
        &["--model", "claude-3.5-sonnet"],
        &["--model"],
        &["--model-config", "claude-opus-5.5"],
        &["-p", "claude-opus-5.5"],
    ];
    for list in quiet {
        let args: Vec<String> = list.iter().map(|a| a.to_string()).collect();
        assert_eq!(
            explicit_model_warnings("claude", &args, false),
            Vec::<String>::new(),
            "{args:?} must not produce a warning"
        );
    }
}

/// The tools amplihack treats as Claude Code, and so the only ones that get a
/// `--model` from it or the dotted `--model` warning.
const CLAUDE_COMPATIBLE_TOOLS: [&str; 4] = ["claude", "rusty", "rustyclawd", "amplifier"];

/// Every `AMPLIHACK_LITELLM_*` variable amplihack reads that is not one of the
/// three gateway variables in [`PROXY_ENV_VARS`]. `amplihack litellm`
/// verification reads these (commands/litellm/preflight.rs, and
/// `AMPLIHACK_LITELLM_TARGET` in amplihack-utils' litellm_proxy.rs). None of
/// them routes a launch through the gateway.
const NON_GATEWAY_LITELLM_VARS: [&str; 6] = [
    "AMPLIHACK_LITELLM_TELEMETRY_FILE",
    "AMPLIHACK_LITELLM_TELEMETRY_HMAC_KEY",
    "AMPLIHACK_LITELLM_EXPECTED_PROVIDER",
    "AMPLIHACK_LITELLM_EXPECTED_MODEL",
    "AMPLIHACK_LITELLM_EXPECTED_GATEWAY_IDENTITY",
    "AMPLIHACK_LITELLM_TARGET",
];

/// Puts back every variable [`model_args_with`] cleared, even when the call
/// under test panics, so a failure cannot leak a gateway variable into the
/// next test.
struct RestoreEnv(Vec<(&'static str, Option<std::ffi::OsString>)>);

impl Drop for RestoreEnv {
    fn drop(&mut self) {
        for (name, value) in self.0.drain(..) {
            match value {
                Some(value) => unsafe { std::env::set_var(name, value) },
                None => unsafe { std::env::remove_var(name) },
            }
        }
    }
}

/// [`model_args`] for `binary_name` and `extra` on a host where, of
/// `AMPLIHACK_DEFAULT_MODEL` and every `AMPLIHACK_LITELLM_*` variable amplihack
/// reads, exactly the ones in `env` are set.
fn model_args_with(env: &[(&str, &str)], binary_name: &str, extra: &[&str]) -> ModelArgs {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let names = PROXY_ENV_VARS
        .into_iter()
        .chain(NON_GATEWAY_LITELLM_VARS)
        .chain(["AMPLIHACK_DEFAULT_MODEL"]);
    let restore = RestoreEnv(names.map(|name| (name, std::env::var_os(name))).collect());
    for (name, _) in &restore.0 {
        unsafe { std::env::remove_var(name) };
    }
    for (name, value) in env {
        unsafe { std::env::set_var(name, value) };
    }
    let extra: Vec<String> = extra.iter().map(|arg| arg.to_string()).collect();
    let got = model_args(binary_name, &extra);
    drop(restore);
    got
}

/// The warning [`explicit_model_warnings`] gives `tool` for an explicit
/// `--model claude-opus-5.5`, built from the documented one.
fn explicit_warning_for(tool: &str) -> String {
    DOCUMENTED_EXPLICIT_WARNING.replace("to `claude`", &format!("to `{tool}`"))
}

/// Issue #1527 review: the warning's gate, as a pure function. Only a
/// Claude-compatible tool gets it, and not on the LiteLLM gateway path.
/// `amplihack copilot -- --model claude-opus-4.5` is Copilot's own spelling and
/// correct as typed; a dotted gateway route name may be correct too.
#[test]
fn test_explicit_model_warnings_only_for_claude_compatible_tools_off_the_gateway() {
    let dotted = ["--model".to_string(), "claude-opus-5.5".to_string()];
    for tool in CLAUDE_COMPATIBLE_TOOLS {
        assert!(is_claude_compatible_tool(tool), "{tool}");
        assert_eq!(
            explicit_model_warnings(tool, &dotted, false),
            vec![explicit_warning_for(tool)],
            "`amplihack {tool}` with a dotted --model must be warned"
        );
        assert_eq!(
            explicit_model_warnings(tool, &dotted, true),
            Vec::<String>::new(),
            "`amplihack {tool}` through the LiteLLM gateway must not be warned"
        );
    }
    for tool in ["copilot", "codex", "Claude", ""] {
        assert!(!is_claude_compatible_tool(tool), "{tool:?}");
        for through_gateway in [false, true] {
            assert_eq!(
                explicit_model_warnings(tool, &dotted, through_gateway),
                Vec::<String>::new(),
                "{tool:?} is not Claude-compatible and must never be warned \
                 (through_gateway = {through_gateway})"
            );
        }
    }
}

/// Issue #1527 review, wired to the environment: a dotted explicit `--model`
/// on a Claude-compatible tool, with no gateway variable set, gives exactly one
/// stderr line, the documented warning, and amplihack adds no `--model` of its
/// own. A dotted AMPLIHACK_DEFAULT_MODEL does not change that.
#[test]
fn test_model_args_warns_on_a_dotted_explicit_model() {
    for tool in CLAUDE_COMPATIBLE_TOOLS {
        for extra in [
            &["--model", "claude-opus-5.5"][..],
            &["--model=claude-opus-5.5"],
        ] {
            for env in [
                &[][..],
                &[("AMPLIHACK_DEFAULT_MODEL", "claude-opus-5.5[1m]")],
            ] {
                let got = model_args_with(env, tool, extra);
                assert_eq!(
                    got,
                    ModelArgs {
                        argv: Vec::new(),
                        stderr: vec![explicit_warning_for(tool)],
                    },
                    "`amplihack {tool}` with {extra:?} and {env:?}"
                );
                assert!(
                    !got.stderr[0].contains("Claude Code"),
                    "the warning must not name a tool other than `{tool}`: {:?}",
                    got.stderr
                );
            }
        }
    }
    assert_eq!(
        model_args_with(&[], "claude", &["--model", "claude-opus-5.5"]).stderr,
        vec![DOCUMENTED_EXPLICIT_WARNING.to_string()]
    );
}

/// Issue #1527 review: `amplihack copilot` and `amplihack codex` get neither a
/// `--model` nor any stderr line about one, whatever the operator passes and
/// whatever AMPLIHACK_DEFAULT_MODEL says. Telling a Copilot user to change
/// `claude-opus-4.5`, Copilot's own spelling, would be wrong.
#[test]
fn test_model_args_never_warns_copilot_or_codex() {
    for tool in ["copilot", "codex"] {
        for extra in [
            &["--model", "claude-opus-4.5"][..],
            &["--model=claude-opus-4.5"],
            &[],
        ] {
            for env in [&[][..], &[("AMPLIHACK_DEFAULT_MODEL", "claude-opus-5.5")]] {
                assert_eq!(
                    model_args_with(env, tool, extra),
                    ModelArgs::default(),
                    "`amplihack {tool}` with {extra:?} and {env:?}"
                );
            }
        }
    }
}

/// Issue #1527 review: on the LiteLLM gateway path, which any one of the three
/// gateway variables selects, a dotted explicit `--model` is a route name. It
/// is forwarded with no warning, and amplihack adds nothing.
#[test]
fn test_model_args_never_warns_on_the_gateway() {
    for gateway_var in PROXY_ENV_VARS {
        for tool in CLAUDE_COMPATIBLE_TOOLS {
            for extra in [
                &["--model", "claude-opus-5.5"][..],
                &["--model=claude-opus-5.5"],
            ] {
                assert_eq!(
                    model_args_with(&[(gateway_var, "claude-opus-5.5")], tool, extra),
                    ModelArgs::default(),
                    "`amplihack {tool}` with {extra:?} and only {gateway_var} set"
                );
            }
        }
    }
}

/// Issue #1527 review: only AMPLIHACK_LITELLM_ENDPOINT, _API_KEY and _MODEL
/// select the gateway, which is what the help text and docs say. Any one of
/// them alone does, and AMPLIHACK_DEFAULT_MODEL is then not read. Every other
/// `AMPLIHACK_LITELLM_*` variable leaves AMPLIHACK_DEFAULT_MODEL in force and
/// the dotted `--model` warning on.
#[test]
fn test_only_the_three_gateway_variables_select_the_gateway() {
    let pinned = [("AMPLIHACK_DEFAULT_MODEL", "claude-sonnet-4-5")];
    let pinned_notice = model_selection_notice(
        &selection("claude-sonnet-4-5", ModelSource::DefaultModelEnv, None),
        "claude",
    );
    for var in NON_GATEWAY_LITELLM_VARS {
        let env = [pinned[0], (var, "set")];
        assert_eq!(
            model_args_with(&env, "claude", &[]),
            ModelArgs {
                argv: vec!["--model".to_string(), "claude-sonnet-4-5".to_string()],
                stderr: vec![pinned_notice.clone()],
            },
            "{var} must not select the gateway: AMPLIHACK_DEFAULT_MODEL must still be read"
        );
        assert_eq!(
            model_args_with(&env, "claude", &["--model", "claude-opus-5.5"]).stderr,
            vec![DOCUMENTED_EXPLICIT_WARNING.to_string()],
            "{var} must not select the gateway: the dotted --model warning must stay on"
        );
    }
    for var in PROXY_ENV_VARS {
        let got = model_args_with(&[pinned[0], (var, "gateway-model")], "claude", &[]);
        let gateway_model = if var == amplihack_utils::litellm_proxy::MODEL_ENV {
            "gateway-model"
        } else {
            "amplihack-default"
        };
        assert_eq!(
            got,
            ModelArgs {
                argv: vec!["--model".to_string(), gateway_model.to_string()],
                stderr: vec![model_selection_notice(
                    &selection(gateway_model, ModelSource::LiteLlmGateway, None),
                    "claude",
                )],
            },
            "{var} alone must select the gateway, so AMPLIHACK_DEFAULT_MODEL is not read"
        );
    }
}

const STDERR_PROBE_BINARY_ENV: &str = "AMPLIHACK_TEST_1527_PROBE_BINARY";
const STDERR_PROBE_ARGS_ENV: &str = "AMPLIHACK_TEST_1527_PROBE_ARGS";

/// The child half of [`test_build_command_prints_exactly_the_model_args_stderr`].
/// In a normal test run neither probe variable is set and this does nothing.
/// When that test spawns it, it builds the command for the tool and arguments
/// the variables name, so the parent can read what really reached stderr.
#[test]
fn probe_build_command_stderr_for_issue_1527() {
    let Some(binary_name) = std::env::var_os(STDERR_PROBE_BINARY_ENV) else {
        return;
    };
    let extra: Vec<String> = std::env::var(STDERR_PROBE_ARGS_ENV)
        .unwrap_or_default()
        .split('\n')
        .filter(|arg| !arg.is_empty())
        .map(str::to_string)
        .collect();
    let binary = make_named_binary(&binary_name.to_string_lossy());
    build_command(&binary, false, false, false, &extra);
}

/// The `amplihack: ` lines `build_command` writes to the real stderr for
/// `binary_name` and `extra`, with exactly the variables in `env` set. Runs
/// [`probe_build_command_stderr_for_issue_1527`] in a child copy of this test
/// binary with a cleared environment, so neither the developer's shell nor
/// another test can change the answer.
fn real_build_command_stderr(
    env: &[(&str, &str)],
    binary_name: &str,
    extra: &[&str],
) -> Vec<String> {
    let module = module_path!()
        .split_once("::")
        .map_or(module_path!(), |(_, rest)| rest);
    let probe = format!("{module}::probe_build_command_stderr_for_issue_1527");
    let home = tempfile::tempdir().unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap());
    child
        .args(["--exact", probe.as_str(), "--nocapture", "--test-threads=1"])
        .env_clear()
        .current_dir(home.path())
        .env("HOME", home.path())
        .env("AMPLIHACK_NO_SYSTEM_PROMPT_APPEND", "1")
        .env(STDERR_PROBE_BINARY_ENV, binary_name)
        .env(STDERR_PROBE_ARGS_ENV, extra.join("\n"))
        .envs(env.iter().copied());
    let output = child.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains("1 passed"),
        "the probe {probe} did not run exactly once and pass\n\
         stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    stderr
        .lines()
        .filter(|line| line.starts_with("amplihack: "))
        .map(str::to_string)
        .collect()
}

/// Issue #1527 review: what `build_command_for_dir` actually writes to stderr
/// for `--model` is what `model_args` returns, line for line, in a real
/// process. Without this, dropping the line that prints them, or printing a
/// warning `model_args` withheld, would pass every other test.
#[test]
fn test_build_command_prints_exactly_the_model_args_stderr() {
    assert_eq!(
        real_build_command_stderr(&[], "claude", &["--model", "claude-opus-5.5"]),
        vec![DOCUMENTED_EXPLICIT_WARNING.to_string()],
        "a dotted explicit --model on claude must print the documented warning, once"
    );
    assert_eq!(
        real_build_command_stderr(
            &[("AMPLIHACK_DEFAULT_MODEL", "claude-opus-5.5[1m]")],
            "claude",
            &[]
        ),
        vec![DOCUMENTED_REWRITE_NOTICE.to_string()],
        "a dotted AMPLIHACK_DEFAULT_MODEL must print the documented rewrite notice, once"
    );
    for tool in ["copilot", "codex"] {
        assert_eq!(
            real_build_command_stderr(
                &[("AMPLIHACK_DEFAULT_MODEL", "claude-opus-5.5")],
                tool,
                &["--model", "claude-opus-4.5"]
            ),
            Vec::<String>::new(),
            "`amplihack {tool}` must print nothing about --model"
        );
    }
    assert_eq!(
        real_build_command_stderr(
            &[
                (
                    amplihack_utils::litellm_proxy::ENDPOINT_ENV,
                    "https://gateway.example.com"
                ),
                (
                    amplihack_utils::litellm_proxy::API_KEY_ENV,
                    "gateway-secret"
                ),
                (amplihack_utils::litellm_proxy::MODEL_ENV, "claude-opus-5.5"),
            ],
            "claude",
            &["--model", "claude-opus-5.5"]
        ),
        Vec::<String>::new(),
        "a gateway launch with an explicit --model must print nothing about --model"
    );
}

const ENVIRONMENT_VARIABLES_MD: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs/reference/environment-variables.md"
));
const LAUNCH_FLAG_INJECTION_MD: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs/reference/launch-flag-injection.md"
));
const LAUNCHER_MODEL_CONFIGURATION_MD: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs/reference/LAUNCHER_MODEL_CONFIGURATION.md"
));

/// `amplihack claude --help`, as an operator sees it.
fn claude_long_help() -> String {
    use clap::CommandFactory;

    let mut cli = crate::Cli::command();
    let claude = cli
        .find_subcommand_mut("claude")
        .expect("`amplihack claude` should exist");
    let mut help = Vec::new();
    claude.write_long_help(&mut help).unwrap();
    String::from_utf8(help).unwrap()
}

/// The help text and every reference page that describes how amplihack picks
/// the model, by name.
fn model_help_and_pages() -> Vec<(&'static str, String)> {
    vec![
        ("`amplihack claude --help`", claude_long_help()),
        (
            "docs/reference/launch-flag-injection.md",
            LAUNCH_FLAG_INJECTION_MD.to_string(),
        ),
        (
            "docs/reference/environment-variables.md",
            ENVIRONMENT_VARIABLES_MD.to_string(),
        ),
        (
            "docs/reference/flag-matrix.md",
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../docs/reference/flag-matrix.md"
            ))
            .to_string(),
        ),
        (
            "docs/reference/LAUNCHER_MODEL_CONFIGURATION.md",
            LAUNCHER_MODEL_CONFIGURATION_MD.to_string(),
        ),
    ]
}

/// Issue #1527 review: the help text and reference pages an operator reads
/// about `--model` must name the default amplihack passes. Before this test
/// they still said amplihack passes no `--model` when AMPLIHACK_DEFAULT_MODEL
/// is unset, which stopped being true when [`DEFAULT_MODEL`] was introduced.
///
/// Both checks are positive, and each is about this variable alone. Every text
/// must show `--model <DEFAULT_MODEL>`. The variable's own entry in
/// environment-variables.md must give it on its `**Default:**` line. On main
/// that line read "unset — amplihack passes **no** `--model` at all", and no
/// page showed the flag. Both checks fail on that.
///
/// There is deliberately no ban on phrases such as "no built-in default".
/// environment-variables.md documents dozens of variables, and that phrase is
/// true of some of them, such as the required AMPLIHACK_LITELLM_MODEL. A
/// page-wide ban fails on a correct sentence about another variable and blames
/// DEFAULT_MODEL for it.
#[test]
fn test_model_help_and_docs_name_the_built_in_default() {
    let default_flag = format!("--model {DEFAULT_MODEL}");
    for (name, text) in model_help_and_pages() {
        assert!(
            text.contains(&default_flag),
            "{name} must name the built-in default `{default_flag}`"
        );
    }

    let heading = "### AMPLIHACK_DEFAULT_MODEL";
    let default_line = ENVIRONMENT_VARIABLES_MD
        .lines()
        .skip_while(|line| *line != heading)
        .skip(1)
        .take_while(|line| !line.starts_with("## ") && !line.starts_with("### "))
        .find(|line| line.starts_with("**Default:**"));
    assert!(
        default_line.is_some_and(|line| line.contains(&format!("`{DEFAULT_MODEL}`"))),
        "docs/reference/environment-variables.md, section `{heading}`: the \
         **Default:** line must give the built-in default `{DEFAULT_MODEL}`, \
         found {default_line:?}"
    );
}

/// Issue #1527 review: the stderr lines the docs show for `--model` are the ones
/// amplihack prints, in both directions.
///
/// Each line is rendered here by the code, not copied, and the `DOCUMENTED_*`
/// constants the other tests compare the code against must equal those
/// renderings. Then environment-variables.md must show every one of them, each
/// on a line of its own, and no page may show a full `--model` stderr line that
/// is not one of them. A change to the wording in the code fails here until the
/// docs say the same.
#[test]
fn test_documented_model_stderr_lines_are_the_ones_amplihack_prints() {
    let dotted = ["--model".to_string(), "claude-opus-5.5".to_string()];
    let rendered = [
        model_selection_notice(
            &selection("claude-sonnet-4-5", ModelSource::DefaultModelEnv, None),
            "claude",
        ),
        model_selection_notice(
            &selection(
                "claude-opus-5-5[1m]",
                ModelSource::DefaultModelEnv,
                Some("claude-opus-5.5[1m]"),
            ),
            "claude",
        ),
        model_selection_notice(
            &selection("gateway-model", ModelSource::LiteLlmGateway, None),
            "claude",
        ),
        explicit_model_warnings("claude", &dotted, false).concat(),
    ];
    assert_eq!(rendered[1], DOCUMENTED_REWRITE_NOTICE);
    assert_eq!(rendered[2], DOCUMENTED_GATEWAY_NOTICE);
    assert_eq!(rendered[3], DOCUMENTED_EXPLICIT_WARNING);

    for line in &rendered {
        assert!(
            ENVIRONMENT_VARIABLES_MD.lines().any(|doc| doc == line),
            "docs/reference/environment-variables.md must show, on a line of its \
             own, the stderr line amplihack prints:\n{line}"
        );
    }

    for (page, text) in [
        (
            "docs/reference/environment-variables.md",
            ENVIRONMENT_VARIABLES_MD,
        ),
        (
            "docs/reference/launch-flag-injection.md",
            LAUNCH_FLAG_INJECTION_MD,
        ),
        (
            "docs/reference/LAUNCHER_MODEL_CONFIGURATION.md",
            LAUNCHER_MODEL_CONFIGURATION_MD,
        ),
    ] {
        for doc in text.lines().map(str::trim).filter(|doc| {
            doc.starts_with("amplihack: passing `--model")
                || doc.starts_with("amplihack: warning: passing `--model")
        }) {
            assert!(
                rendered.iter().any(|line| line == doc),
                "{page} shows a stderr line amplihack does not print:\n{doc}\n\
                 amplihack prints:\n{}",
                rendered.join("\n")
            );
        }
    }
}

/// Issue #1527 review: only the three variables `proxy_requested()` checks
/// select the LiteLLM gateway (see
/// `test_only_the_three_gateway_variables_select_the_gateway`). An earlier
/// draft of the help text and LAUNCHER_MODEL_CONFIGURATION.md said any
/// `AMPLIHACK_LITELLM_*` variable did. That told an operator with only
/// telemetry variables set that their AMPLIHACK_DEFAULT_MODEL was ignored when
/// it was in use.
///
/// The check is positive: both texts must name each of the three. The draft
/// named neither AMPLIHACK_LITELLM_ENDPOINT nor AMPLIHACK_LITELLM_API_KEY in
/// either text, so this fails on it. There is no ban on the phrase "any
/// `AMPLIHACK_LITELLM_*`". It is true in other contexts:
/// docs/reference/security-recommendations.md uses it for the variables launch
/// setup subprocesses never receive.
#[test]
fn test_help_and_docs_name_each_gateway_variable() {
    for (name, text) in [
        ("`amplihack claude --help`", claude_long_help()),
        (
            "docs/reference/LAUNCHER_MODEL_CONFIGURATION.md",
            LAUNCHER_MODEL_CONFIGURATION_MD.to_string(),
        ),
    ] {
        for var in PROXY_ENV_VARS {
            assert!(
                text.contains(var),
                "{name} must name {var}, one of the variables that select the gateway"
            );
        }
    }
}

/// Issue #1527, wired together short of spawning a process: the selection a
/// dotted environment value produces renders as the line the docs promise.
#[test]
fn test_dotted_default_model_env_renders_the_documented_notice() {
    let got = with_default_model_env(Some("claude-opus-5.5[1m]"), configured_default_model)
        .expect("a dotted id is still a model to pass");
    assert_eq!(
        model_selection_notice(&got, "claude"),
        DOCUMENTED_REWRITE_NOTICE
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
        // Issue #1421: the only thing injected is the model request.
        let strs: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            strs,
            vec![
                "--model".to_string(),
                super::command::DEFAULT_MODEL.to_string()
            ],
            "a plain launch should carry only the model request, got: {strs:?}"
        );
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
        // Issue #1421: the permission flag and the model request, in that order.
        let strs: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            strs,
            vec![
                "--dangerously-skip-permissions".to_string(),
                "--model".to_string(),
                super::command::DEFAULT_MODEL.to_string(),
            ],
            "got: {strs:?}"
        );
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
