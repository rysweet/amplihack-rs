# Idle Watchdog Reference

> [Home](../index.md) > Reference > Idle Watchdog

> This page specifies the `amplihack-utils::idle_watchdog` module and the
> subprocess call sites that use it. It is the implementation reference for
> GitHub issue #867.

**Type**: Reference (Information-Oriented)

The idle watchdog supervises long-running child processes (agent runs, cascade
steps, remote executions) and kills a child **only** when it stops producing
output for an idle window — never on elapsed wall-clock time while output still
streams.

## Contents

- [Why It Exists](#why-it-exists)
- [Module: `amplihack-utils::idle_watchdog`](#module-amplihack-utilsidle_watchdog)
- [Configuration](#configuration)
- [Call-Site Behavior](#call-site-behavior)
- [Preserved (Non-Idle) Timeouts](#preserved-non-idle-timeouts)
- [Related Docs](#related-docs)

## Why It Exists

Wall-clock, kill-on-expiry timeouts kill healthy agents mid-stream when a task
legitimately runs longer than a fixed budget. The idle watchdog replaces those
kills with **idle/liveness detection**: any new byte on `stdout` or `stderr`
resets a "last progress" timer. The child is terminated only after the idle
threshold passes with no output.

See [Idle Watchdog Concept](../concepts/idle-watchdog.md) for the rationale and
the owner rule that governs it.

## Module: `amplihack-utils::idle_watchdog`

`amplihack-utils` is a foundational crate with no amplihack dependencies, so the
helper is reusable from any crate without introducing dependency cycles.

**Workspace dependency**: `amplihack-utils = { workspace = true }`

### `IdleConfig`

Configures the idle threshold and poll interval.

```rust
pub struct IdleConfig {
    /// No output for this long → the child is considered idle and is killed.
    pub idle_timeout: Duration,
    /// How often the supervising loop checks for progress and process exit.
    pub poll: Duration,
}

impl IdleConfig {
    /// Reads AMPLIHACK_IDLE_TIMEOUT_SECS (default 300) and
    /// AMPLIHACK_IDLE_POLL_MS (default 1000).
    pub fn from_env() -> Self;

    /// Override the idle threshold; poll interval comes from env/default.
    pub fn with_idle(idle: Duration) -> Self;
}
```

### `IdleOutcome`

Result of a supervised wait.

```rust
pub struct IdleOutcome {
    /// Exit status of the child, or the I/O error from waiting on it.
    pub status: io::Result<ExitStatus>,
    /// Full captured stdout.
    pub stdout: String,
    /// Full captured stderr.
    pub stderr: String,
    /// True only when the child was killed for exceeding the idle window.
    pub killed_for_idle: bool,
}
```

When `killed_for_idle` is `true`, callers surface a clear error such as
`idle N s: no output` rather than a generic timeout.

### `wait_with_idle_watchdog` (async)

For `tokio::process::Child`. Used by the orchestration and remote crates.

```rust
pub async fn wait_with_idle_watchdog(
    child: &mut tokio::process::Child,
    stdout: Option<tokio::process::ChildStdout>,
    stderr: Option<tokio::process::ChildStderr>,
    cfg: IdleConfig,
) -> IdleOutcome;
```

Drainer tasks are launched with `tokio::spawn` (requires tokio features
`process`, `time`, `rt`, `io-util`). Each read chunk appends to a shared buffer
and stamps a shared `Instant`. The supervising loop calls `try_wait` and
`sleep(poll)`; when `now - last_progress > idle_timeout` it kills the child and
sets `killed_for_idle = true`. `select!` is intentionally **not** used, so the
tokio `macros` feature is not pulled into `amplihack-utils`.

### `wait_with_idle_watchdog_sync` (blocking)

For `std::process::Child`. Used by the blocking Copilot CLI client so no tokio
runtime is introduced into `amplihack-agent-core`.

```rust
pub fn wait_with_idle_watchdog_sync(
    child: &mut std::process::Child,
    stdout: Option<std::process::ChildStdout>,
    stderr: Option<std::process::ChildStderr>,
    cfg: IdleConfig,
) -> IdleOutcome;
```

Same progress semantics as the async variant, implemented with `std::thread`
drainers and a blocking poll loop.

### Bounded drainer join after the child exits

After the child exits (normally or via idle-kill), both variants drain any
output buffered just before exit, but the join is bounded by a `DRAIN_GRACE`
(5 s) window rather than waiting for pipe EOF unconditionally. A reaped child
may leave a reparented descendant that inherited the stdout/stderr write end;
that pipe never reaches EOF, so an unbounded drainer would block forever and
hang the caller. Once the grace elapses the drainer is abandoned (the async
task is aborted, the sync thread detached) and whatever bytes it already
buffered are returned. On the normal path the pipes close as soon as the child
exits, so the bound is never reached.

### `file_idle_since` (file-mtime probe)

For call sites whose child stdout is already consumed by a logging thread, so no
pipe handle is available. Progress is inferred from the log file's modification
time.

```rust
/// Returns true when `path`'s mtime is older than `idle_timeout`
/// (i.e., no new output has been written for at least that long).
pub fn file_idle_since(path: &Path, idle_timeout: Duration) -> io::Result<bool>;
```

This is a stateless probe: callers poll it on their own cadence and kill the
child only when it returns `true`.

## Configuration

| Variable                      | Default  | Applies To                | Meaning                                                   |
| ----------------------------- | -------- | ------------------------- | --------------------------------------------------------- |
| `AMPLIHACK_IDLE_TIMEOUT_SECS` | `300`    | all watchdog entry points | Kill a child after this many seconds with no output.      |
| `AMPLIHACK_IDLE_POLL_MS`      | `1000`   | async + sync waits        | How often the supervising loop checks progress/exit (ms). |

Notes:

- There is **no absolute wall-clock cap** on agentic runs. A live agent that
  keeps streaming tokens is never killed, regardless of total elapsed time.
- Set a longer idle window for slow tools:

  ```bash
  export AMPLIHACK_IDLE_TIMEOUT_SECS=900   # 15-minute idle tolerance
  ```

- A per-call `Option<Duration>` (e.g. a step's configured timeout) is
  reinterpreted as an **idle-timeout override** via `IdleConfig::with_idle`, not
  as a wall-clock deadline.
- The Copilot adapter's `f64` timeout (site 5) is no longer a wall-clock cap: the
  `MAX_TIMEOUT` clamp is removed so large values pass through unchanged (only a
  1.0 s lower bound remains). The subprocess is supervised by the site-3 client's
  idle watchdog.

## Call-Site Behavior

| Site | File                                                | Runtime        | Watchdog Entry Point               | Kill Condition                                   |
| ---- | --------------------------------------------------- | -------------- | ---------------------------------- | ------------------------------------------------ |
| 6    | `amplihack-orchestration/src/claude_process.rs`     | tokio          | `wait_with_idle_watchdog`          | No stdout/stderr for the idle window.            |
| 1    | `amplihack-orchestration/src/patterns/cascade.rs`   | tokio          | via site 6 (`create_process` uses `None`) | Cascade advances on child **exit status**; kills only via site-6 idle watchdog. |
| 3    | `amplihack-agent-core/src/sdk_adapters/copilot_cli_client.rs` | std::process | `wait_with_idle_watchdog_sync`     | No stdout/stderr for the idle window.            |
| 5    | `amplihack-agent-core/src/sdk_adapters/copilot.rs`  | n/a            | none — subprocess supervised by site 3 | `MAX_TIMEOUT` clamp removed so the adapter timeout is no longer a wall-clock cap; the site-3 client supervises the subprocess with its own idle watchdog. |
| 2    | `amplihack-cli/src/commands/multitask/state.rs`     | std::process   | `file_idle_since(ws.log_file, ..)` | Log file mtime idle **and** policy allows a kill. |
| 4    | `amplihack-remote/src/executor.rs`                  | tokio          | `wait_with_idle_watchdog`          | No stdout/stderr for the idle window.            |

### Site 2: multitask timeout policy

`enforce_timeouts` never kills purely because `elapsed >= max_runtime`.

- Default policy is **`continue-preserve`**: the run continues; elapsed runs are
  marked resumable but are not killed.
- Under **`interrupt-preserve`**, a child is killed **only** when
  `file_idle_since(ws.log_file, idle)` is `true` — i.e., its log file has stopped
  growing for the idle window.
- In **both** policies the run is still marked `timed_out_resumable` once
  `elapsed >= max_runtime`, so state is preserved for resume regardless of
  whether the child is killed.

| Constant                 | Value                | Meaning                                             |
| ------------------------ | -------------------- | --------------------------------------------------- |
| `DEFAULT_MAX_RUNTIME`    | `7200`               | Marks a run resumable after this many seconds.      |
| `DEFAULT_TIMEOUT_POLICY` | `continue-preserve`  | Non-killing default (flipped from `interrupt-preserve`); preserves state, keeps running.|

### Site 5: adapter timeout propagation

The Copilot adapter (`run_agent`) spawns no subprocess itself — it delegates to
the site-3 Copilot CLI client, whose subprocess is supervised by
`wait_with_idle_watchdog_sync`. Its role changes as follows:

- The `MAX_TIMEOUT` (600 s) clamp on the adapter's configured `timeout` is
  removed. Large values pass through unchanged; only a 1.0 s lower bound remains.
  There is no wall-clock cap on a run.
- The subprocess idle window is owned by the site-3 `CopilotCliClient`. It
  defaults to `DEFAULT_CLI_TIMEOUT` (300 s) and can be set explicitly via
  `CopilotCliClient::with_timeout(..)`, which the client feeds into
  `IdleConfig::with_idle(..)` for `wait_with_idle_watchdog_sync` — never as a
  kill deadline.

So removing the clamp is observable: the adapter no longer imposes any upper
wall-clock bound. Site 5 itself performs no wait and imposes no deadline of its
own.

## Preserved (Non-Idle) Timeouts

The watchdog replaces only wall-clock kills of **agentic** child processes.
Genuine network/HTTP timeouts are correct and are left unchanged:

- Git bundle create/verify (`amplihack-remote/src/packager.rs`) and bundle
  download (`amplihack-remote/src/executor.rs::retrieve_git_state`)
- `azlin` list / kill / provision / connect calls
- `tmux` control operations

Do not route these through the idle watchdog. Note that `executor.rs` contains
**both** the agentic path (`execute_remote`, site 4 → idle watchdog) and these
bounded network ops — only `execute_remote` is converted.

## Related Docs

- [Idle Watchdog Concept](../concepts/idle-watchdog.md) — why idle beats
  wall-clock; the owner rule.
- [Use the Idle Watchdog](../howto/use-idle-watchdog.md) — wrap a new call site
  and write the two required tests.
- [amplihack-remote API](amplihack-remote-api.md) — remote execution surface.
