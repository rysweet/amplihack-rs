use super::correlation::{RecipeRunCorrelation, RecipeRunFinalStatus, known_log_paths};
use super::*;
use crate::env_builder::{EnvBuilder, active_agent_binary};
#[cfg(windows)]
use crate::util::run_with_timeout;
use crate::util::truncate_chars_with_notice;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write as IoWrite};
use std::process::{Child, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

const STDERR_TAIL_LINES: usize = 5;
const CAPTURED_STDERR_LINES: usize = 200;
const RECIPE_RUNNER_DEFAULT_TIMEOUT: Duration = Duration::from_secs(6 * 60 * 60);
const RECIPE_RUNNER_POLL_INTERVAL: Duration = Duration::from_millis(100);
const RECIPE_RUNNER_PIPE_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(windows)]
const RECIPE_RUNNER_TERMINATE_TIMEOUT: Duration = Duration::from_secs(5);
const RECIPE_RUNNER_TIMEOUT_ENV: &str = "AMPLIHACK_RECIPE_RUNNER_TIMEOUT_SECS";

/// Threshold in bytes for total `--set` argument size before we switch
/// to passing context via a temp file. Well under the typical Linux
/// ARG_MAX (~2MB) to leave room for env vars and other args.
const CONTEXT_ARG_SIZE_THRESHOLD: usize = 128 * 1024;

/// Maximum byte length of a context value that we export as a single
/// environment variable. The kernel rejects any individual argv/envp
/// string longer than `MAX_ARG_STRLEN` (PAGE_SIZE * 32 = 131072 on Linux)
/// with `E2BIG`. We cap conservatively below that so a pathologically large
/// value cannot make the spawn fail. Over-limit values are still delivered
/// to the runner via `--set` / `--context-file` for `{{placeholder}}`
/// substitution; only the env mirror is skipped (issue #784, regression
/// guard for the E2BIG / `--context-file` path).
const CONTEXT_ENV_VALUE_MAX_BYTES: usize = 96 * 1024;

/// Reserved / dangerous environment-variable names that must never be set
/// from untrusted recipe context (issue bodies, task descriptions and
/// third-party recipes all flow into the context map). These names are
/// NOT managed by `EnvBuilder`, so without this denylist a pathological
/// context key could clobber a process-critical variable or inject code
/// into a child shell/interpreter. The `AMPLIHACK_` namespace is handled
/// separately (prefix check) because it is owned by `EnvBuilder`.
///
/// Names are compared after uppercasing the context key (see
/// [`context_env_pairs`]). Covers: path/identity, the dynamic linker,
/// shell-startup remote-code-execution vectors, word-splitting and
/// interpreter option-injection vectors.
const RESERVED_ENV_DENYLIST: &[&str] = &[
    // path / identity
    "PATH",
    "HOME",
    "SHELL",
    "PWD",
    "USER",
    "LOGNAME",
    // dynamic linker
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "DYLD_INSERT_LIBRARIES",
    "DYLD_LIBRARY_PATH",
    "GLIBC_TUNABLES",
    // shell-startup remote-code-execution vectors
    "BASH_ENV",
    "ENV",
    "PS4",
    "PROMPT_COMMAND",
    "SHELLOPTS",
    "BASHOPTS",
    // word splitting
    "IFS",
    // interpreter option injection
    "PYTHONPATH",
    "NODE_OPTIONS",
    "PERL5OPT",
    "RUBYOPT",
];

/// `true` when `name` is a valid POSIX environment-variable identifier,
/// i.e. it matches `^[A-Z_][A-Z0-9_]*$`. The transform in
/// [`context_env_pairs`] uppercases the key first, so only uppercase
/// ASCII letters, digits and underscores are expected here; anything else
/// (hyphens, dots, spaces, a leading digit, non-ASCII, empty) is rejected.
fn is_valid_env_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_uppercase() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// Deterministically map recipe context entries to environment variables for
/// the spawned recipe runner (and, by OS inheritance, every bash step and
/// nested sub-recipe it runs). This is the fix for issue #784 / #4583: bash
/// steps under `set -u` reference `$TASK_DESCRIPTION` / `$REPO_PATH`, which
/// must exist in the environment rather than only being substituted into
/// `{{placeholder}}` text.
///
/// Transform (pure, total — invalid entries are skipped, never fatal):
/// 1. Uppercase the key (`task_description` → `TASK_DESCRIPTION`).
/// 2. Drop keys that are not valid env identifiers after uppercasing
///    (empty, leading digit, hyphen/dot/space, non-ASCII).
/// 3. Drop keys in the `AMPLIHACK_` namespace (owned by `EnvBuilder`).
/// 4. Drop reserved/dangerous names ([`RESERVED_ENV_DENYLIST`]).
/// 5. Drop values containing a NUL byte (rejected by the OS for env vars;
///    would otherwise panic at spawn time).
/// 6. Drop values larger than [`CONTEXT_ENV_VALUE_MAX_BYTES`] (would risk
///    `E2BIG`); they remain available via the recipe context file.
///
/// Skipped entries are logged name-only at `warn` level — values may carry
/// sensitive data and are never logged.
pub(super) fn context_env_pairs(context: &BTreeMap<String, String>) -> Vec<(String, String)> {
    let mut pairs = Vec::with_capacity(context.len());
    for (key, value) in context {
        let name = key.to_ascii_uppercase();
        if !is_valid_env_identifier(&name) {
            tracing::warn!(
                name = %key,
                reason = %"invalid_identifier",
                "recipe context key skipped for env export"
            );
            continue;
        }
        if name.starts_with("AMPLIHACK_") {
            tracing::warn!(
                name = %key,
                reason = %"reserved_name",
                "recipe context key skipped for env export"
            );
            continue;
        }
        if RESERVED_ENV_DENYLIST.contains(&name.as_str()) {
            tracing::warn!(
                name = %key,
                reason = %"reserved_name",
                "recipe context key skipped for env export"
            );
            continue;
        }
        if value.contains('\0') {
            tracing::warn!(
                name = %key,
                reason = %"value_contains_nul",
                "recipe context key skipped for env export"
            );
            continue;
        }
        if value.len() > CONTEXT_ENV_VALUE_MAX_BYTES {
            tracing::warn!(
                name = %key,
                reason = %"value_too_large",
                "recipe context key skipped for env export"
            );
            continue;
        }
        pairs.push((name, value.clone()));
    }
    pairs
}

pub(super) fn execute_recipe_via_rust(
    recipe_path: &Path,
    context: &BTreeMap<String, String>,
    dry_run: bool,
    _verbose: bool,
    working_dir: &Path,
    search_dirs: &[PathBuf],
    step_timeout: Option<u64>,
) -> Result<RecipeRunResult> {
    let binary = super::binary::find_recipe_runner_binary()?;
    let recipe_name = recipe_name_for_correlation(recipe_path);
    let correlation =
        RecipeRunCorrelation::new(recipe_name, working_dir, context, binary.as_path());
    let mut command = Command::new(&binary);
    command
        .arg(recipe_path)
        .arg("--output-format")
        .arg("json")
        .arg("-C")
        .arg(working_dir);

    // Issue #494: forward sub-recipe search dirs as -R flags so
    // recipe-runner-rs can resolve sub-recipes the same way amplihack
    // resolves top-level recipes. One -R per non-empty entry, in order.
    for dir in search_dirs {
        if dir.as_os_str().is_empty() {
            continue;
        }
        command.arg("-R").arg(dir);
        tracing::debug!(dir = %dir.display(), "forwarding -R to recipe-runner-rs");
    }

    if dry_run {
        command.arg("--dry-run");
    }

    // Pass context as a file when the total size would risk E2BIG (os error 7).
    // The temp file is kept alive until the recipe runner child completes.
    let _context_file = pass_context(&mut command, context)?;

    // Issue #784 / #4583: export recipe context as environment variables so
    // bash steps (and nested sub-recipes, via OS inheritance) can read
    // $TASK_DESCRIPTION / $REPO_PATH under `set -u`. Applied at the LOWEST
    // precedence — written BEFORE EnvBuilder and the run-id below — so every
    // amplihack-managed/protective variable deterministically wins over any
    // colliding context key. Reserved/dangerous names are dropped upstream in
    // `context_env_pairs` (they are not EnvBuilder-managed).
    command.envs(context_env_pairs(context));

    let runtime_dir = tempfile::Builder::new()
        .prefix("amplihack-workflow-")
        .tempdir()
        .context("failed to create isolated workflow runtime directory")?;
    let artifact_dir = runtime_dir.path().join("artifacts");
    let tmp_dir = runtime_dir.path().join("tmp");
    std::fs::create_dir_all(&artifact_dir)
        .context("failed to create isolated workflow artifact directory")?;
    std::fs::create_dir_all(&tmp_dir)
        .context("failed to create isolated workflow tmp directory")?;

    let env_builder = EnvBuilder::new()
        .with_agent_binary(active_agent_binary())
        .with_session_tree_context()
        .with_amplihack_home_from(working_dir)
        .with_asset_resolver()
        .with_pager_safe_defaults()
        .with_python_sanitization()
        .unset("CLAUDECODE")
        .set("AMPLIHACK_NONINTERACTIVE", "1")
        .with_project_graph_db(working_dir)?;

    // Issue #439: propagate --step-timeout as AMPLIHACK_STEP_TIMEOUT env var.
    // When Some(n), the child process sees AMPLIHACK_STEP_TIMEOUT=n (0 = disable).
    // When None, the env var is not injected (parent-inherited values flow through).
    let env_builder = match step_timeout {
        Some(seconds) => env_builder.set("AMPLIHACK_STEP_TIMEOUT", seconds.to_string()),
        None => env_builder,
    };

    env_builder.apply_to_command(&mut command);
    command.env("AMPLIHACK_RECIPE_RUN_ID", correlation.run_id());
    command.env("AMPLIHACK_WORKFLOW_RUNTIME_DIR", runtime_dir.path());
    command.env("AMPLIHACK_RUNTIME_ROOT", runtime_dir.path());
    command.env("AMPLIHACK_WORKFLOW_ARTIFACT_DIR", &artifact_dir);
    command.env("TMPDIR", &tmp_dir);

    spawn_with_streaming_stderr(command, correlation, recipe_path, recipe_runner_timeout())
}

/// Spawn the runner with stdout captured (we need to parse JSON from it)
/// and stderr "teed": each line is forwarded live to our own stderr AND
/// captured in a buffer so the error path can still surface a meaningful
/// stderr tail. (#357)
fn spawn_with_streaming_stderr(
    mut command: Command,
    correlation: RecipeRunCorrelation,
    recipe_path: &Path,
    timeout: Duration,
) -> Result<RecipeRunResult> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: `pre_exec` runs after fork and before exec. `setsid` is
        // async-signal-safe and lets timeout cleanup terminate the recipe tree.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    correlation.emit_early();
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let _summary = correlation.emit_final(
                RecipeRunFinalStatus::SpawnFailure,
                None,
                None,
                known_log_paths(None),
            );
            return Err(error).context("failed to spawn recipe-runner-rs");
        }
    };
    let child_pid = Some(child.id());

    let captured_stderr: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(VecDeque::new()));
    let dropped_stderr_lines: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
    let stderr_handle = child.stderr.take().expect("piped stderr");
    let captured_clone = Arc::clone(&captured_stderr);
    let dropped_clone = Arc::clone(&dropped_stderr_lines);
    let (stderr_done_tx, stderr_done_rx) = mpsc::channel();
    thread::spawn(move || {
        // Read RAW BYTES, not str-typed lines(): an Err(InvalidData) from
        // non-UTF-8 stderr would otherwise terminate the pump silently and
        // the child can then block on a full pipe (#366 / COE feedback).
        let mut reader = BufReader::new(stderr_handle);
        let stderr = io::stderr();
        let mut buf: Vec<u8> = Vec::with_capacity(4096);
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) => break, // EOF
                Ok(_) => {
                    let line = String::from_utf8_lossy(&buf);
                    let trimmed = line.trim_end_matches(['\r', '\n']);
                    let _ = writeln!(stderr.lock(), "{trimmed}");
                    push_bounded_stderr_line(&captured_clone, &dropped_clone, trimmed.to_string());
                }
                // I/O error reading from the pipe: log and stop pumping —
                // we MUST NOT spin or leak the thread, but the child will
                // still close stderr at exit and `wait()` will return.
                Err(_) => break,
            }
        }
        let _ = stderr_done_tx.send(());
    });

    let mut stdout_handle = child.stdout.take().expect("piped stdout");
    use std::io::Read;
    let (stdout_tx, stdout_rx) = mpsc::channel();
    thread::spawn(move || {
        let mut stdout_buf = String::new();
        let result = stdout_handle
            .read_to_string(&mut stdout_buf)
            .map(|_| stdout_buf);
        let _ = stdout_tx.send(result);
    });

    let status = match wait_for_recipe_runner(&mut child, timeout)
        .context("failed to wait for recipe-runner-rs")?
    {
        Some(status) => status,
        None => {
            let pid = child.id();
            terminate_recipe_runner(&mut child)?;
            let _summary = correlation.emit_final(
                RecipeRunFinalStatus::Failure,
                child_pid,
                None,
                known_log_paths(None),
            );
            anyhow::bail!(
                "recipe-runner-rs timed out after {:?} (pid {}, recipe {}, working dir {})",
                timeout,
                pid,
                recipe_path.display(),
                correlation.cwd()
            );
        }
    };

    let stdout_buf = stdout_rx
        .recv_timeout(RECIPE_RUNNER_PIPE_DRAIN_TIMEOUT)
        .with_context(|| {
            format!(
                "recipe-runner-rs stdout did not close within {:?} after process exit",
                RECIPE_RUNNER_PIPE_DRAIN_TIMEOUT
            )
        })?
        .context("failed to read recipe-runner-rs stdout")?;
    let _ = stderr_done_rx.recv_timeout(RECIPE_RUNNER_PIPE_DRAIN_TIMEOUT);

    let captured = captured_stderr.lock().expect("stderr mutex");
    let dropped = *dropped_stderr_lines
        .lock()
        .expect("stderr drop-count mutex");
    let stderr_joined = captured.iter().cloned().collect::<Vec<_>>().join("\n");
    match parse_recipe_output_with_stderr_drops(
        &stdout_buf,
        &stderr_joined,
        status.success(),
        dropped,
    ) {
        Ok(mut result) => {
            let final_status = if status.success() && result.success {
                RecipeRunFinalStatus::Success
            } else {
                RecipeRunFinalStatus::Failure
            };
            let summary = correlation.emit_final(
                final_status,
                child_pid,
                status.code(),
                known_log_paths(Some(&result)),
            );
            result.run_id = Some(summary.run_id.clone());
            result.log_pointer = Some(summary);
            Ok(result)
        }
        Err(error) => {
            let final_status = if status.success() || !stdout_buf.trim().is_empty() {
                RecipeRunFinalStatus::ParseFailure
            } else {
                RecipeRunFinalStatus::Failure
            };
            let _summary = correlation.emit_final(
                final_status,
                child_pid,
                status.code(),
                known_log_paths(None),
            );
            Err(error).with_context(|| {
                format!(
                    "recipe-runner-rs exited with {}",
                    exit_status_label(&status)
                )
            })
        }
    }
}

fn recipe_name_for_correlation(recipe_path: &Path) -> String {
    std::fs::read_to_string(recipe_path)
        .ok()
        .and_then(|content| serde_yaml::from_str::<serde_yaml::Value>(&content).ok())
        .and_then(|value| {
            value
                .get("name")
                .and_then(serde_yaml::Value::as_str)
                .map(str::to_string)
        })
        .filter(|name| !name.trim().is_empty())
        .or_else(|| {
            recipe_path
                .file_stem()
                .map(|value| value.to_string_lossy().to_string())
                .filter(|name| !name.trim().is_empty())
        })
        .unwrap_or_else(|| recipe_path.display().to_string())
}

fn push_bounded_stderr_line(
    captured: &Arc<Mutex<VecDeque<String>>>,
    dropped: &Arc<Mutex<usize>>,
    line: String,
) {
    let mut captured = captured.lock().expect("stderr mutex");
    if captured.len() == CAPTURED_STDERR_LINES {
        captured.pop_front();
        *dropped.lock().expect("stderr drop-count mutex") += 1;
    }
    captured.push_back(line);
}

/// Pure parser for recipe-runner-rs subprocess output.
///
/// Behavior:
/// - Empty/whitespace-only stdout + success returns an explicit hollow-success
///   terminal failure. A runner that produced no structured result must not
///   become a success-shaped no-op.
/// - Empty/whitespace-only stdout + failure: errors with the meaningful stderr
///   tail surfaced so callers see the upstream cause.
/// - Non-empty stdout: parses as JSON; on failure, errors with a bounded stdout
///   preview that reports discarded chars and stderr tail in the `anyhow::Context`.
///
/// `RecipeRunResult` does not use `deny_unknown_fields`, so future
/// recipe-runner-rs versions may add fields without breaking us.
#[cfg(test)]
pub(super) fn parse_recipe_output(
    stdout: &str,
    stderr: &str,
    exit_success: bool,
) -> Result<RecipeRunResult> {
    parse_recipe_output_with_stderr_drops(stdout, stderr, exit_success, 0)
}

fn parse_recipe_output_with_stderr_drops(
    stdout: &str,
    stderr: &str,
    exit_success: bool,
    prior_discarded_stderr_lines: usize,
) -> Result<RecipeRunResult> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        if exit_success {
            let mut extra = JsonMap::new();
            extra.insert(
                "workflow_result".into(),
                serde_json::json!({
                    "terminal_state": "HOLLOW_SUCCESS",
                    "terminal_success": false,
                    "terminal_reason": "recipe-runner-rs exited successfully but produced no structured workflow output",
                    "required_next_action": "Inspect recipe-runner logs and rerun with structured JSON output."
                }),
            );
            return Ok(RecipeRunResult {
                success: false,
                status: Some("HOLLOW_SUCCESS".into()),
                phase: Some("finalization".into()),
                extra,
                ..RecipeRunResult::default()
            });
        }
        anyhow::bail!(
            "recipe-runner-rs produced no output and exited with failure\nstderr tail:\n{}",
            meaningful_stderr_tail_with_prior_drops(stderr, prior_discarded_stderr_lines)
        );
    }

    serde_json::from_str::<RecipeRunResult>(trimmed).with_context(|| {
        let preview = truncate_chars_with_notice(trimmed, 200);
        format!(
            "recipe-runner-rs produced non-JSON stdout preview:\n{}\nstderr tail:\n{}",
            preview,
            meaningful_stderr_tail_with_prior_drops(stderr, prior_discarded_stderr_lines)
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

#[cfg(test)]
pub(super) fn meaningful_stderr_tail(stderr: &str) -> String {
    meaningful_stderr_tail_with_prior_drops(stderr, 0)
}

pub(super) fn meaningful_stderr_tail_with_prior_drops(
    stderr: &str,
    prior_discarded_stderr_lines: usize,
) -> String {
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

    let (selected, discarded) = if meaningful.is_empty() {
        let discarded = lines.len().saturating_sub(STDERR_TAIL_LINES);
        (
            lines
                .into_iter()
                .rev()
                .take(STDERR_TAIL_LINES)
                .collect::<Vec<_>>(),
            discarded,
        )
    } else {
        let discarded = meaningful.len().saturating_sub(STDERR_TAIL_LINES);
        (
            meaningful
                .into_iter()
                .rev()
                .take(STDERR_TAIL_LINES)
                .collect::<Vec<_>>(),
            discarded,
        )
    };

    let mut tail = selected.into_iter().rev().collect::<Vec<_>>().join("\n");
    let discarded = discarded + prior_discarded_stderr_lines;
    if discarded > 0 {
        if !tail.is_empty() {
            tail.push('\n');
        }
        tail.push_str(&format!("[truncated: discarded {discarded} stderr lines]"));
    }
    tail
}

fn recipe_runner_timeout() -> Duration {
    std::env::var(RECIPE_RUNNER_TIMEOUT_ENV)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
        .map(Duration::from_secs)
        .unwrap_or(RECIPE_RUNNER_DEFAULT_TIMEOUT)
}

fn wait_for_recipe_runner(
    child: &mut Child,
    timeout: Duration,
) -> std::io::Result<Option<ExitStatus>> {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if started.elapsed() >= timeout {
            return Ok(None);
        }
        thread::sleep(RECIPE_RUNNER_POLL_INTERVAL.min(timeout.saturating_sub(started.elapsed())));
    }
}

fn terminate_recipe_runner(child: &mut Child) -> Result<()> {
    let pid = child.id();
    #[cfg(unix)]
    {
        let process_group = -(pid as libc::pid_t);
        let result = unsafe { libc::kill(process_group, libc::SIGKILL) };
        if result != 0 {
            let error = std::io::Error::last_os_error();
            tracing::warn!(pid, %error, "failed to terminate timed-out recipe-runner process group");
        }
    }
    #[cfg(windows)]
    {
        let mut command = Command::new("taskkill");
        command
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let _ = run_with_timeout(command, RECIPE_RUNNER_TERMINATE_TIMEOUT);
    }
    child
        .kill()
        .or_else(|kill_error| match child.try_wait() {
            Ok(Some(_)) => Ok(()),
            Ok(None) => Err(kill_error),
            Err(wait_error) => Err(wait_error),
        })
        .with_context(|| format!("failed to terminate timed-out recipe-runner-rs pid {pid}"))?;
    child
        .wait()
        .with_context(|| format!("failed to wait for timed-out recipe-runner-rs pid {pid}"))?;
    Ok(())
}
