use super::*;
use crate::env_builder::{EnvBuilder, active_agent_binary};

const STDERR_TAIL_LINES: usize = 5;

pub(super) fn execute_recipe_via_rust(
    recipe_path: &Path,
    context: &BTreeMap<String, String>,
    dry_run: bool,
    working_dir: &Path,
) -> Result<RecipeRunResult> {
    let binary = super::binary::find_recipe_runner_binary()?;
    let mut command = Command::new(binary);
    command
        .arg(recipe_path)
        .arg("--output-format")
        .arg("json")
        .arg("-C")
        .arg(working_dir);

    if dry_run {
        command.arg("--dry-run");
    }

    for (key, value) in context {
        command.arg("--set").arg(format!("{key}={value}"));
    }

    EnvBuilder::new()
        .with_agent_binary(active_agent_binary())
        .with_session_tree_context()
        .with_amplihack_home()
        .with_asset_resolver()
        .with_project_graph_db(working_dir)?
        .apply_to_command(&mut command);

    let output = command
        .output()
        .context("failed to spawn recipe-runner-rs")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let parsed: RecipeRunResult = serde_json::from_str(&stdout).map_err(|_| {
        if output.status.success() {
            anyhow::anyhow!(
                "Rust recipe runner returned unparseable output (exit {}): {}",
                exit_status_label(&output.status),
                stdout.chars().take(500).collect::<String>()
            )
        } else {
            anyhow::anyhow!(
                "Rust recipe runner failed (exit {}): {}",
                exit_status_label(&output.status),
                format_runner_failure_detail(&output.status, &stderr)
            )
        }
    })?;

    Ok(parsed)
}

fn format_runner_failure_detail(status: &std::process::ExitStatus, stderr: &str) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;

        if let Some(signal) = status.signal() {
            return format!(
                "killed by signal {} ({}). The process was terminated externally before producing output.",
                signal_name(signal),
                signal
            );
        }
    }

    if stderr.is_empty() {
        return "no stderr".to_string();
    }

    meaningful_stderr_tail(stderr)
}

fn exit_status_label(status: &std::process::ExitStatus) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return format!("signal {signal}");
        }
    }

    status
        .code()
        .map(|code| code.to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

pub(super) fn meaningful_stderr_tail(stderr: &str) -> String {
    let lines = stderr
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    let meaningful = lines
        .iter()
        .copied()
        .filter(|line| {
            !matches!(line.chars().next(), Some('▶' | '✓' | '⊘' | '✗'))
                && !line.starts_with("[agent]")
        })
        .collect::<Vec<_>>();

    let selected = if meaningful.is_empty() {
        lines
            .into_iter()
            .rev()
            .take(STDERR_TAIL_LINES)
            .collect::<Vec<_>>()
    } else {
        meaningful
            .into_iter()
            .rev()
            .take(STDERR_TAIL_LINES)
            .collect::<Vec<_>>()
    };

    selected.into_iter().rev().collect::<Vec<_>>().join("\n")
}

#[cfg(unix)]
fn signal_name(signal: i32) -> &'static str {
    match signal {
        2 => "SIGINT",
        6 => "SIGABRT",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        15 => "SIGTERM",
        _ => "signal",
    }
}
