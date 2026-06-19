use super::correlation::{RecipeRunCorrelation, RecipeRunFinalStatus, known_log_paths};
use super::*;
use crate::env_builder::{EnvBuilder, active_agent_binary};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write as IoWrite};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::thread;

const STDERR_TAIL_LINES: usize = 5;
const CAPTURED_STDERR_LINES: usize = 200;

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
    // The temp file is kept alive until command.output() completes.
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
    command.env("AMPLIHACK_WORKFLOW_ARTIFACT_DIR", &artifact_dir);
    command.env("TMPDIR", &tmp_dir);

    spawn_with_streaming_stderr(command, correlation)
}

/// Spawn the runner with stdout captured (we need to parse JSON from it)
/// and stderr "teed": each line is forwarded live to our own stderr AND
/// captured in a buffer so the error path can still surface a meaningful
/// stderr tail. (#357)
fn spawn_with_streaming_stderr(
    mut command: Command,
    correlation: RecipeRunCorrelation,
) -> Result<RecipeRunResult> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
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
    let stderr_handle = child.stderr.take().expect("piped stderr");
    let captured_clone = Arc::clone(&captured_stderr);
    let pump = thread::spawn(move || {
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
                    push_bounded_stderr_line(&captured_clone, trimmed.to_string());
                }
                // I/O error reading from the pipe: log and stop pumping —
                // we MUST NOT spin or leak the thread, but the child will
                // still close stderr at exit and `wait()` will return.
                Err(_) => break,
            }
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
    let stderr_joined = captured.iter().cloned().collect::<Vec<_>>().join("\n");
    match parse_recipe_output(&stdout_buf, &stderr_joined, status.success()) {
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

fn push_bounded_stderr_line(captured: &Arc<Mutex<VecDeque<String>>>, line: String) {
    let mut captured = captured.lock().expect("stderr mutex");
    if captured.len() == CAPTURED_STDERR_LINES {
        captured.pop_front();
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
