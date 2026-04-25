use super::*;
use crate::env_builder::{EnvBuilder, active_agent_binary};
use std::io::{BufRead, BufReader, Write as IoWrite};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::thread;

const STDERR_TAIL_LINES: usize = 5;

/// Threshold in bytes for total `--set` argument size before we switch
/// to passing context via a temp file. Well under the typical Linux
/// ARG_MAX (~2MB) to leave room for env vars and other args.
const CONTEXT_ARG_SIZE_THRESHOLD: usize = 128 * 1024;

pub(super) fn execute_recipe_via_rust(
    recipe_path: &Path,
    context: &BTreeMap<String, String>,
    dry_run: bool,
    verbose: bool,
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

    // Issue #357: propagate --progress so step transitions are visible on
    // stderr in real time. Without this, the only signal the user sees
    // before the recipe finishes is the single "Executing recipe: …" line.
    if verbose {
        command.arg("--progress");
    }

    // Pass context as a file when the total size would risk E2BIG (os error 7).
    // The temp file is kept alive until command.output() completes.
    let _context_file = pass_context(&mut command, context)?;

    EnvBuilder::new()
        .with_agent_binary(active_agent_binary())
        .with_session_tree_context()
        .with_amplihack_home()
        .with_asset_resolver()
        .with_python_sanitization()
        .with_project_graph_db(working_dir)?
        .apply_to_command(&mut command);

    if verbose {
        spawn_with_streaming_stderr(command)
    } else {
        let output = command
            .output()
            .context("failed to spawn recipe-runner-rs")?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        parse_recipe_output(&stdout, &stderr, output.status.success()).with_context(|| {
            format!(
                "recipe-runner-rs exited with {}",
                exit_status_label(&output.status)
            )
        })
    }
}

/// Spawn the runner with stdout captured (we need to parse JSON from it)
/// and stderr "teed": each line is forwarded live to our own stderr AND
/// captured in a buffer so the error path can still surface a meaningful
/// stderr tail. (#357)
fn spawn_with_streaming_stderr(mut command: Command) -> Result<RecipeRunResult> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().context("failed to spawn recipe-runner-rs")?;

    let captured_stderr: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let stderr_handle = child.stderr.take().expect("piped stderr");
    let captured_clone = Arc::clone(&captured_stderr);
    let pump = thread::spawn(move || {
        let reader = BufReader::new(stderr_handle);
        let stderr = io::stderr();
        for line in reader.lines().map_while(Result::ok) {
            // Forward live so the user sees progress in real time.
            let _ = writeln!(stderr.lock(), "{line}");
            captured_clone.lock().expect("stderr mutex").push(line);
        }
    });

    let mut stdout_buf = String::new();
    let mut stdout_handle = child.stdout.take().expect("piped stdout");
    use std::io::Read;
    stdout_handle
        .read_to_string(&mut stdout_buf)
        .context("failed to read recipe-runner-rs stdout")?;

    let status = child
        .wait()
        .context("failed to wait for recipe-runner-rs")?;
    pump.join().expect("stderr pump thread panicked");

    let captured = captured_stderr.lock().expect("stderr mutex");
    let stderr_joined = captured.join("\n");
    parse_recipe_output(&stdout_buf, &stderr_joined, status.success()).with_context(|| {
        format!(
            "recipe-runner-rs exited with {}",
            exit_status_label(&status)
        )
    })
}

/// Pure parser for recipe-runner-rs subprocess output.
///
/// Behavior (issue #332):
/// - Empty/whitespace-only stdout + success: returns a default `RecipeRunResult`
///   with `success = true` (treats "ran but produced no JSON" as a no-op success).
/// - Empty/whitespace-only stdout + failure: errors with the meaningful stderr
///   tail surfaced so callers see the upstream cause.
/// - Non-empty stdout: parses as JSON; on failure, errors with a bounded stdout
///   preview (first 200 chars) and stderr tail in the `anyhow::Context`.
///
/// `RecipeRunResult` does not use `deny_unknown_fields`, so future
/// recipe-runner-rs versions may add fields without breaking us.
pub(super) fn parse_recipe_output(
    stdout: &str,
    stderr: &str,
    exit_success: bool,
) -> Result<RecipeRunResult> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        if exit_success {
            return Ok(RecipeRunResult {
                success: true,
                ..RecipeRunResult::default()
            });
        }
        anyhow::bail!(
            "recipe-runner-rs produced no output and exited with failure\nstderr tail:\n{}",
            meaningful_stderr_tail(stderr)
        );
    }

    serde_json::from_str::<RecipeRunResult>(trimmed).with_context(|| {
        let preview: String = trimmed.chars().take(200).collect();
        format!(
            "recipe-runner-rs produced non-JSON stdout (first 200 chars): {}\nstderr tail:\n{}",
            preview,
            meaningful_stderr_tail(stderr)
        )
    })
}

/// Pass context key-value pairs to the command. When total serialised size
/// is small, uses `--set key=value` CLI args. When large, writes a JSON
/// file and passes `--context-file <path>` to avoid E2BIG (issues #209, #211).
///
/// Returns an `Option<tempfile::NamedTempFile>` that must be kept alive
/// until the child process has finished reading the file.
pub(super) fn pass_context(
    command: &mut Command,
    context: &BTreeMap<String, String>,
) -> Result<Option<tempfile::NamedTempFile>> {
    if context.is_empty() {
        return Ok(None);
    }

    let total_bytes: usize = context
        .iter()
        .map(|(k, v)| "--set".len() + k.len() + 1 + v.len())
        .sum();

    if total_bytes <= CONTEXT_ARG_SIZE_THRESHOLD {
        for (key, value) in context {
            command.arg("--set").arg(format!("{key}={value}"));
        }
        return Ok(None);
    }

    // Write context as JSON to a temp file.
    let mut tmp =
        tempfile::NamedTempFile::new().context("failed to create temp file for recipe context")?;
    serde_json::to_writer(&mut tmp, context)
        .context("failed to serialize recipe context to temp file")?;
    tmp.flush()
        .context("failed to flush recipe context temp file")?;

    command.arg("--context-file").arg(tmp.path());

    Ok(Some(tmp))
}

fn exit_status_label(status: &std::process::ExitStatus) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return format!("signal {} ({})", signal_name(signal), signal);
        }
    }

    status
        .code()
        .map(|code| code.to_string())
        .unwrap_or_else(|| "unknown".to_string())
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
