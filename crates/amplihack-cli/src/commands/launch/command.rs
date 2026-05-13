//! Command building for tool binaries: argument injection, UVX plugin
//! handling, Docker launcher args, and Claude-specific env augmentation.

use crate::binary_finder::BinaryInfo;
use crate::commands::uvx_help::is_uvx_deployment;
use crate::env_builder::EnvBuilder;

use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(test)]
pub(super) fn build_command(
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
        false, // subprocess_safe
    )
}

pub(super) fn build_command_for_dir(
    binary: &BinaryInfo,
    resume: bool,
    continue_session: bool,
    skip_permissions: bool,
    extra_args: &[String],
    add_dir_override: Option<&Path>,
    subprocess_safe: bool,
) -> Command {
    let mut cmd = Command::new(&binary.path);

    // SEC-2: Only inject --dangerously-skip-permissions when the caller has
    // explicitly opted in via `--skip-permissions`.  This flag bypasses
    // Claude's interactive confirmation prompts and must not be on by default.
    // Only inject for Claude-compatible tools — Copilot and Codex don't support it.
    let is_claude_compatible = matches!(
        binary.name.as_str(),
        "claude" | "rusty" | "rustyclawd" | "amplifier"
    );
    if skip_permissions && is_claude_compatible {
        cmd.arg("--dangerously-skip-permissions");
    }

    inject_uvx_plugin_args(&mut cmd, &binary.name, extra_args, add_dir_override);

    // Inject --model unless user already supplied one.
    // Default model depends on the tool — Claude uses opus[1m], Copilot uses its own default.
    let user_has_model = extra_args.iter().any(|a| a == "--model");
    if !user_has_model && is_claude_compatible {
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

    // Inject --allow-all for Copilot by default (issue #303). Copilot's
    // `--allow-all` is shorthand for `--allow-all-tools + --allow-all-paths +
    // --allow-all-urls`. Without it, Copilot prompts for tool/path/url
    // permission on its first action, which blocks unattended orchestrator
    // loops launched by amplihack. Skip injection if the user already set any
    // allow-all-* flag, or if AMPLIHACK_COPILOT_NO_ALLOW_ALL=1.
    if binary.name == "copilot" && should_inject_copilot_allow_all(extra_args) {
        cmd.arg("--allow-all");
    }

    // Issue #621: Subprocess-safe defense-in-depth — granular allow-all flags.
    //
    // When subprocess_safe is active, we ALSO inject the granular
    // `--allow-all-tools` and `--allow-all-paths` flags. This is intentional
    // layering: the broader `--allow-all` (issue #303) covers the same
    // permission space, but the granular flags are the explicit contract
    // callers (Simard engineers, recipe-runner agents) audit for. Both
    // appearing in argv is accepted by the copilot CLI without conflict.
    //
    // Scope: copilot binary only. Other tools (claude, codex, amplifier)
    // are excluded — these flags are copilot-specific.
    if binary.name == "copilot" && subprocess_safe {
        let (inject_tools, inject_paths) =
            should_inject_subprocess_safe_flags(subprocess_safe, extra_args);
        if inject_tools {
            cmd.arg("--allow-all-tools");
        }
        if inject_paths {
            cmd.arg("--allow-all-paths");
        }
    }

    // Inject --remote for Copilot by default. Remote mode offloads compute to
    // GitHub's cloud, which is the preferred mode for amplihack orchestration.
    // Skip injection if the user already passed --remote, or if
    // AMPLIHACK_COPILOT_NO_REMOTE=1.
    if binary.name == "copilot" && should_inject_copilot_remote(extra_args) {
        cmd.arg("--remote");
    }

    cmd.args(extra_args);
    cmd
}

/// Pure decision function (issue #621): is this Copilot invocation
/// subprocess-safe?
///
/// Subprocess-safe is true when ANY of:
///   * the user explicitly passed `--subprocess-safe` (`explicit_flag`)
///   * `AMPLIHACK_AGENT_BINARY` is set to a non-empty value
///     (`env_agent_binary = Some(non_empty)`)
///   * not all three standard streams are TTYs (`all_streams_tty = false`)
///
/// Empty-string `env_agent_binary` is treated as "no delegation" (the
/// documented sentinel value) and does NOT trigger subprocess-safe by itself.
///
/// This function takes the TTY state as a parameter (rather than reading
/// stdio internally) so it can be unit-tested deterministically without
/// depending on the test harness's actual stdio state.
pub(crate) fn resolve_subprocess_safe(
    explicit_flag: bool,
    env_agent_binary: Option<&str>,
    all_streams_tty: bool,
) -> bool {
    if explicit_flag {
        return true;
    }
    if env_agent_binary.is_some_and(|s| !s.is_empty()) {
        return true;
    }
    !all_streams_tty
}

/// Pure decision function (issue #621): which subprocess-safe granular flags
/// should `build_command_for_dir` inject for Copilot?
///
/// Returns `(inject_allow_all_tools, inject_allow_all_paths)`. Callers must
/// have already gated on `subprocess_safe == true` and `binary.name ==
/// "copilot"` — passing `subprocess_safe = false` returns `(false, false)`
/// unconditionally as a defensive invariant.
///
/// Duplicate suppression rules (each flag computed independently):
///   * `--allow-all-tools` already in user args → suppress tools injection.
///   * `--allow-all-paths` already in user args → suppress paths injection.
///   * `--allow-all` (broader superset) in user args → suppress BOTH.
pub(super) fn should_inject_subprocess_safe_flags(
    subprocess_safe: bool,
    user_args: &[String],
) -> (bool, bool) {
    if !subprocess_safe {
        return (false, false);
    }
    let user_has_superset = user_args.iter().any(|a| a == "--allow-all");
    if user_has_superset {
        return (false, false);
    }
    let user_has_tools = user_args.iter().any(|a| a == "--allow-all-tools");
    let user_has_paths = user_args.iter().any(|a| a == "--allow-all-paths");
    (!user_has_tools, !user_has_paths)
}

/// Pure precedence resolver (issue #621): compute the *effective*
/// no-reflection decision from the three input signals.
///
/// Precedence (highest → lowest):
///   1. `explicit_reflection = true` (user passed `--reflection`)  → reflection ON  → return false
///   2. `no_reflection = true`       (user passed `--no-reflection`)→ reflection OFF → return true
///   3. `subprocess_safe = true`     (auto-detected or `--subprocess-safe`) → reflection OFF → return true
///   4. otherwise (default)                                          → reflection ON  → return false
///
/// `--reflection` and `--no-reflection` are mutually exclusive at the clap
/// layer (`conflicts_with`); the resolver itself is defense-in-depth and
/// gives `--reflection` priority if both somehow arrive as `true`.
pub(crate) fn resolve_no_reflection(
    explicit_reflection: bool,
    no_reflection: bool,
    subprocess_safe: bool,
) -> bool {
    if explicit_reflection {
        return false;
    }
    if no_reflection {
        return true;
    }
    subprocess_safe
}

/// Decide whether `amplihack` should inject `--allow-all` into a Copilot
/// invocation. Returns false if the user already supplied any allow-all-*
/// flag, or if the `AMPLIHACK_COPILOT_NO_ALLOW_ALL=1` env var is set.
pub(crate) fn should_inject_copilot_allow_all(extra_args: &[String]) -> bool {
    if std::env::var("AMPLIHACK_COPILOT_NO_ALLOW_ALL").as_deref() == Ok("1") {
        return false;
    }
    let already_present = extra_args.iter().any(|a| {
        a == "--allow-all"
            || a == "--allow-all-tools"
            || a == "--allow-all-paths"
            || a == "--allow-all-urls"
    });
    !already_present
}

/// Decide whether `amplihack` should inject `--remote` into a Copilot
/// invocation. Returns false if the user already passed `--remote` or
/// `--no-remote`, or if `AMPLIHACK_COPILOT_NO_REMOTE=1` is set.
pub(crate) fn should_inject_copilot_remote(extra_args: &[String]) -> bool {
    if std::env::var("AMPLIHACK_COPILOT_NO_REMOTE").as_deref() == Ok("1") {
        return false;
    }
    !extra_args
        .iter()
        .any(|a| a == "--remote" || a == "--no-remote")
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

pub(super) fn augment_claude_launch_env(env_builder: EnvBuilder, tool: &str) -> EnvBuilder {
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

#[allow(clippy::too_many_arguments)]
pub(super) fn build_docker_launcher_args(
    launcher_command: &str,
    resume: bool,
    continue_session: bool,
    skip_update_check: bool,
    no_reflection: bool,
    subprocess_safe: bool,
    checkout_repo: Option<&str>,
    extra_args: &[String],
) -> Vec<String> {
    let is_launch_surface = launcher_command == "launch";
    let mut args = vec![launcher_command.to_string()];
    if resume {
        args.push("--resume".to_string());
    }
    if continue_session {
        args.push("--continue".to_string());
    }
    if skip_update_check && is_launch_surface {
        args.push("--skip-update-check".to_string());
    }
    if no_reflection {
        args.push("--no-reflection".to_string());
    }
    if subprocess_safe {
        args.push("--subprocess-safe".to_string());
    }
    if let Some(repo) = checkout_repo {
        args.push("--checkout-repo".to_string());
        args.push(repo.to_string());
    }
    if !extra_args.is_empty() {
        args.push("--".to_string());
        args.extend(extra_args.iter().cloned());
    }
    args
}
