//! Launcher context persistence and shell quoting utilities.

use crate::launcher_context::{LauncherKind, write_launcher_context};

/// Map a launcher subcommand to its persisted kind. Anything not an agent
/// launcher (`amplihack install`, `amplihack doctor`, ...) returns None and
/// writes nothing.
fn launcher_kind_for(tool: &str) -> Option<LauncherKind> {
    match tool {
        "claude" => Some(LauncherKind::Claude),
        "copilot" => Some(LauncherKind::Copilot),
        "codex" => Some(LauncherKind::Codex),
        "amplifier" => Some(LauncherKind::Amplifier),
        _ => None,
    }
}

use anyhow::Result;
use std::collections::BTreeMap;
use std::path::Path;

/// Invocations that answer a question and exit. They are not sessions, so they
/// must not stamp the repository with a session identity.
///
/// The file that caused issue #1335 was written by `amplihack copilot
/// --version`. It then decided which agent CLI ran for every workflow under
/// /tmp for the next five days.
///
/// Only the FIRST argument is examined. `extra_args` is declared
/// `trailing_var_arg` + `allow_hyphen_values`, so it carries prompt text --
/// scanning all of it would make `amplihack claude help me fix the build`
/// persist nothing, and the session would then fall through to the built-in
/// default. That is the very failure this guard exists to prevent, re-entered
/// through a different door.
///
/// `--help` and `-h` are absent deliberately: clap intercepts both at the
/// subcommand and they never reach here. Listing them would advertise
/// coverage that cannot be exercised.
fn is_non_session_invocation(extra_args: &[String]) -> bool {
    matches!(
        extra_args.first().map(String::as_str),
        Some("--version" | "-V" | "help")
    )
}

pub(super) fn persist_launcher_context(
    tool: &str,
    project_root: Option<&Path>,
    extra_args: &[String],
) -> Result<()> {
    let Some(kind) = launcher_kind_for(tool) else {
        return Ok(());
    };
    if is_non_session_invocation(extra_args) {
        tracing::debug!(
            tool,
            "not persisting launcher context for a non-session invocation"
        );
        return Ok(());
    }
    let Some(project_root) = project_root else {
        tracing::warn!(
            "skipping launcher context persistence because current directory is unavailable"
        );
        return Ok(());
    };

    let mut environment = BTreeMap::new();
    environment.insert("AMPLIHACK_LAUNCHER".to_string(), tool.to_string());
    // Issue #506: nested re-launches (recipe-runner sub-recipes, agent
    // tasks) read AMPLIHACK_AGENT_BINARY from the persisted launcher
    // context to choose the active agent binary. Without this entry the
    // child process inherits no preference and exits 1 with a
    // binary-not-found error.
    //
    // Issue #1335: this used to run only for copilot, which made the
    // persisted layer a one-way voter -- it could only ever say "copilot",
    // and the built-in default says copilot too, so a claude session that
    // lost its environment variable could not resolve back to claude by
    // any path. Recording whichever launcher actually ran makes the layer
    // able to answer for either.
    environment.insert("AMPLIHACK_AGENT_BINARY".to_string(), tool.to_string());
    write_launcher_context(
        project_root,
        kind,
        render_launcher_command(tool, extra_args),
        environment,
    )?;
    Ok(())
}

pub(super) fn render_launcher_command(subcommand: &str, extra_args: &[String]) -> String {
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
