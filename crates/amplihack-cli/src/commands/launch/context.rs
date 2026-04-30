//! Launcher context persistence and shell quoting utilities.

use crate::launcher_context::{LauncherKind, write_launcher_context};

use anyhow::Result;
use std::collections::BTreeMap;
use std::path::Path;

pub(super) fn persist_launcher_context(
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
    // Issue #506: nested re-launches (recipe-runner sub-recipes, agent
    // tasks) read AMPLIHACK_AGENT_BINARY from the persisted launcher
    // context to choose the active agent binary. Without this entry the
    // child process inherits no preference, falls back to claude, and
    // exits 1 with claude-not-found. The value is hardcoded here because
    // this branch is gated by `tool == "copilot"` above — reading from
    // std::env would be wrong (the parent may not have it set even when
    // we explicitly know we are launching copilot).
    environment.insert("AMPLIHACK_AGENT_BINARY".to_string(), "copilot".to_string());
    write_launcher_context(
        project_root,
        LauncherKind::Copilot,
        render_launcher_command("copilot", extra_args),
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
