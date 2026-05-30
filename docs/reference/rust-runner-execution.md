# Rust Runner Execution — API Reference

The `amplihack` Rust CLI binary (`recipe-runner-rs`) handles subprocess management, progress tracking, JSONL event emission, and log I/O for recipe execution.

Subprocess management, progress tracking, JSONL event emission, and log I/O helpers for the Rust recipe runner. Callers should use `amplihack recipe run`.

## Contents

- [Public API](#public-api)
  - [execute_rust_command](#execute_rust_command)
  - [read_progress_file](#read_progress_file)
  - [emit_step_transition](#emit_step_transition)
  - [build_rust_env](#build_rust_env)
- [Data Shapes](#data-shapes)
  - [Progress file JSON](#progress-file-json)
  - [JSONL step-transition event](#jsonl-step-transition-event)
- [Environment Variables](#environment-variables)
- [Security Model](#security-model)
- [Internal Functions](#internal-functions)

---

## Public API

### `execute_rust_command`

The Rust binary handles command execution internally. To run a recipe:

```bash
amplihack recipe run my-recipe --verbose
```

The binary manages:

| Concern       | Description                                                                                                                                    |
| ------------- | ---------------------------------------------------------------------------------------------------------------------------------------------- |
| Command       | The recipe name and context flags. Never passed through a shell.                                                                               |
| Progress      | When `--verbose` is set, creates a per-recipe log file, writes progress JSON after each step marker, and prints the log path to stderr for live `tail -f`. |
| Environment   | Filtered environment with only allowlisted variables (see [build_rust_env](#build_rust_env)).                                                   |

Returns structured JSON output when `--output json` is used. See [`RecipeResult`](./recipe-result.md).

**Example:**

```bash
amplihack recipe run my-recipe --verbose --output json | jq '.success'
```

---

### `read_progress_file`

The Rust binary writes progress JSON files during execution. These can be read by monitoring tools:

```bash
# Progress files are written to the system temp directory
cat /tmp/amplihack-progress-my_recipe-12345.json
```

Returned dict fields (required fields are always present when the function returns non-`None`; optional fields may be absent):

| Field             | Type    | Required | Description                                                                                                                                         |
| ----------------- | ------- | -------- | --------------------------------------------------------------------------------------------------------------------------------------------------- |
| `recipe_name`     | `str`   | ✓        | Sanitized recipe name                                                                                                                               |
| `current_step`    | `int`   | ✓        | 1-based index of the running step                                                                                                                   |
| `status`          | `str`   | ✓        | One of `running`, `completed`, `failed`                                                                                                             |
| `pid`             | `int`   | ✓        | PID of the recipe-runner-rs process                                                                                                                 |
| `total_steps`     | `int`   | optional | Total step count; the Python streaming layer always writes `0` (unknown) — only a Rust binary that reports step totals will supply a non-zero value |
| `step_name`       | `str`   | optional | Human-readable step label                                                                                                                           |
| `elapsed_seconds` | `float` | optional | Seconds since recipe start                                                                                                                          |
| `updated_at`      | `float` | optional | Unix timestamp of last write                                                                                                                        |

---

### `emit_step_transition`

The Rust binary emits machine-readable JSONL step-transition events to **stderr** with immediate flush.

| Field       | Values                                        |
| ----------- | --------------------------------------------- |
| `step_name` | Arbitrary label matching the recipe step name |
| `status`    | `"start"` · `"done"` · `"fail"` · `"skip"`    |

These events are emitted automatically by the recipe runner during execution.

**Output format:**

```json
{ "type": "step_transition", "step": "validate-inputs", "status": "start", "ts": 1743554401.12 }
```

**Filtering:** Parent processes suppress these lines from user-visible output via `_STEP_TRANSITION_PREFIX` detection.

---

### `build_rust_env`

The Rust binary builds a filtered environment for subprocess execution. Only variables on an internal allowlist are included, preventing accidental secret leakage (e.g. `ANTHROPIC_API_KEY`, `GITHUB_TOKEN`).

If `AMPLIHACK_AGENT_BINARY` is `"copilot"`, the binary creates a shim wrapper to intercept nested `copilot` invocations inside the recipe.

**Allowlisted variable families:**

| Family           | Examples                                                         |
| ---------------- | ---------------------------------------------------------------- |
| `AMPLIHACK_*`    | `AMPLIHACK_HOME`, `AMPLIHACK_SESSION_ID`, `AMPLIHACK_RECIPE_LOG` |
| Path & shell     | `PATH`, `HOME`, `SHELL`, `USER`                                  |
| Proxy            | `HTTP_PROXY`, `HTTPS_PROXY`, `NO_PROXY` (case-insensitive)       |
| TLS / CA bundles | `SSL_CERT_FILE`, `CURL_CA_BUNDLE`, `REQUESTS_CA_BUNDLE`          |
| Locale           | `LANG`, `LC_ALL`, `LC_CTYPE`                                     |
| Temp dirs        | `TMPDIR`, `TMP`, `TEMP`                                          |
| Runtime          | `RECIPE_RUNNER_RS_PATH`, `CLAUDE_PROJECT_DIR`                |

---

## Data Shapes

### Progress file JSON

Written to `/tmp/amplihack-progress-<recipe>-<pid>.json` after each step transition.

```json
{
  "recipe_name": "smart-orchestrator",
  "current_step": 3,
  "total_steps": 0,
  "step_name": "Run builder agent",
  "elapsed_seconds": 42.8,
  "status": "running",
  "pid": 98765,
  "updated_at": 1743554401.5
}
```

> **Note:** `total_steps` may be `0` if the recipe does not report step totals upfront. Treat `0` as "unknown".

An optional workstream sidecar at `$AMPLIHACK_WORKSTREAM_PROGRESS_FILE` augments the main file with:

```json
{
  "recipe_name": "smart-orchestrator",
  "current_step": 3,
  "step_name": "Run builder agent",
  "status": "running",
  "pid": 98765,
  "updated_at": 1743554401.5,
  "issue": "1234",
  "checkpoint_id": "cp-abc",
  "worktree_path": "/home/user/repo/worktrees/issue-1234"
}
```

### JSONL step-transition event

Emitted to stderr; one line per step transition.

```
{"type":"step_transition","step":"<step-name>","status":"<start|done|fail|skip>","ts":<float>}
```

Heartbeat pings from long-running steps use a separate type:

```
{"type":"heartbeat","step":"<step-name>","ts":<float>}
```

---

## Environment Variables

| Variable                             | Description                                                                                           |
| ------------------------------------ | ----------------------------------------------------------------------------------------------------- |
| `AMPLIHACK_RECIPE_LOG`               | Set automatically to the log file path when `progress=True`. Child processes can append to this file. |
| `AMPLIHACK_WORKSTREAM_PROGRESS_FILE` | Optional path for the workstream progress sidecar. Set by the workstream orchestrator.                |
| `AMPLIHACK_WORKSTREAM_STATE_FILE`    | Optional path to a workstream state JSON file read for issue/checkpoint context.                      |

---

## Security Model

| Property                   | Mechanism                                                                                                 |
| -------------------------- | --------------------------------------------------------------------------------------------------------- | ---------------------- |
| No shell injection         | `subprocess.Popen` always receives a `list[str]`; `shell=False` is the default                            |
| Symlink-safe file creation | Progress and log files opened with `O_NOFOLLOW                                                            | O_CREAT`; mode `0o600` |
| Atomic writes              | Progress JSON written via `tempfile.NamedTemporaryFile` + `os.replace()`; readers never see partial files |
| Path traversal prevention  | `_validate_path_within_tmpdir()` rejects any resolved path outside `tempfile.gettempdir()`                |
| Minimal env surface        | `build_rust_env()` allowlist excludes all credential variables                                            |
| Recipe name sanitization   | Recipe names reduced to `[a-zA-Z0-9_]`, capped at 64 characters before use in file paths                  |

---

## Internal Functions

These functions are implementation details; do not call them directly.

| Function                                                                    | Purpose                                                                                 |
| --------------------------------------------------------------------------- | --------------------------------------------------------------------------------------- |
| `_validate_path_within_tmpdir(path)`                                        | Raises `ValueError` if `path.resolve()` escapes `tempfile.gettempdir()`                 |
| `_atomic_write_json(path, payload)`                                         | Write JSON payload atomically; falls back to direct write on cross-device rename errors |
| `_progress_file_path(recipe_name, pid)`                                     | Returns `/tmp/amplihack-progress-<sanitized>-<pid>.json`                                |
| `_recipe_log_path(recipe_name, pid)`                                        | Returns `/tmp/amplihack-recipe-<sanitized>-<pid>.log`                                   |
| `_stream_process_output(process)`                                           | Basic stdout/stderr drain (no progress tracking)                                        |
| `_stream_process_output_with_progress(process, recipe_name, log_file_path)` | Thread-based drain with step-marker detection and log tee                               |
| `_run_rust_process(cmd, progress, env, recipe_name)`                        | Spawn process, select streaming mode, return `(stdout, stderr, returncode, log_path)`   |
| `_meaningful_stderr_tail(stderr)`                                           | Last 5 lines of stderr after filtering metadata lines                                   |
| `_is_progress_metadata_line(line)`                                          | Returns `True` for step-transition and heartbeat JSONL lines                            |
| `_raise_process_failure(stderr, returncode)`                                | Raise `RuntimeError` with signal name or trimmed stderr                                 |
| `_parse_rust_response(stdout, stderr, returncode, name)`                    | JSON parse with structured error fallback                                               |
| `_validate_rust_response_payload(data, name)`                               | Assert contract: `success` (bool), `step_results` (list), `context` (dict)              |
| `_build_step_results(step_results_data)`                                    | Convert raw JSON list to `list[StepResult]`                                             |

---

**See also:**

- [Recipe Result data model](./recipe-result.md)
- [Recipe CLI Reference](./recipe-cli-reference.md)
- [Rust Runner Execution Architecture](../concepts/rust-runner-execution-architecture.md)
- [Issue Classifier Workflow](../howto/configure-issue-classifier-workflow.md)
