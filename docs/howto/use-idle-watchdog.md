# Use the Idle Watchdog

> [Home](../index.md) > How-To > Use the Idle Watchdog

> This guide shows how to supervise a child process with idle detection using
> `amplihack-utils::idle_watchdog`. It is the usage contract for GitHub issue
> #867.

**Type**: How-To (Task-Oriented)

Use this guide when you spawn a long-running **agentic** child process and need
to reap it if it hangs — without killing it while it is still producing output.

## Before You Start

Add the dependency to your crate's `Cargo.toml`:

```toml
[dependencies]
amplihack-utils = { workspace = true }
```

Pick the entry point that matches your runtime:

| Your situation                                   | Use                              |
| ------------------------------------------------ | -------------------------------- |
| tokio child, you own the stdout/stderr pipes     | `wait_with_idle_watchdog`        |
| blocking `std::process` child                    | `wait_with_idle_watchdog_sync`   |
| child stdout is redirected to a log file         | `file_idle_since`                |

## Task 1: Supervise a tokio child (async)

Spawn with piped stdout/stderr, take the pipes, and hand them to the watchdog.

```rust
use std::time::Duration;
use tokio::process::Command;
use std::process::Stdio;
use amplihack_utils::idle_watchdog::{wait_with_idle_watchdog, IdleConfig};

let mut child = Command::new("copilot")
    .args(["--prompt", "refactor module"])
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .kill_on_drop(true) // drop-safety, not a wall-clock kill
    .spawn()?;

let stdout = child.stdout.take();
let stderr = child.stderr.take();

// Default idle window (300 s) from the environment.
let outcome = wait_with_idle_watchdog(&mut child, stdout, stderr, IdleConfig::from_env()).await;

if outcome.killed_for_idle {
    anyhow::bail!("agent idle: no output for the idle window");
}
let status = outcome.status?;
println!("exit: {status}\n{}", outcome.stdout);
```

To override the idle window for a slow step, use `with_idle`:

```rust
let cfg = IdleConfig::with_idle(Duration::from_secs(900)); // 15-minute idle tolerance
let outcome = wait_with_idle_watchdog(&mut child, stdout, stderr, cfg).await;
```

## Task 2: Supervise a blocking child (sync)

Same shape without a runtime — used by the Copilot CLI client so no tokio is
introduced.

```rust
use std::process::{Command, Stdio};
use amplihack_agent_core::error::AgentError;
use amplihack_utils::idle_watchdog::{wait_with_idle_watchdog_sync, IdleConfig};

let mut child = Command::new("copilot")
    .arg("--print")
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()?;

let stdout = child.stdout.take();
let stderr = child.stderr.take();

let cfg = IdleConfig::from_env();
let idle_secs = cfg.idle_timeout.as_secs();
let outcome = wait_with_idle_watchdog_sync(&mut child, stdout, stderr, cfg);
if outcome.killed_for_idle {
    // Surface as a timeout: the Copilot adapter matches
    // AgentError::TimeoutError(secs) and maps it to a clean run failure.
    return Err(AgentError::TimeoutError(idle_secs));
}
```

## Task 3: Supervise a child that logs to a file

When stdout is already consumed by a logging thread, poll the log file's mtime.

```rust
use std::time::Duration;
use amplihack_utils::idle_watchdog::file_idle_since;

let idle = Duration::from_secs(300);
if file_idle_since(&workspace.log_file, idle)? {
    // Log stopped growing for the idle window → safe to reap.
    child.kill()?;
}
```

Only kill when `file_idle_since` returns `true`. Do **not** kill on elapsed
runtime alone — mark the run resumable instead and let it continue.

## Task 4: Advance a cascade on exit status, not the clock

When chaining fallback steps, drive progression off the child's real exit
status. Pass `None` for the per-step timeout so the foundational idle watchdog
governs the wait:

```rust
// Pass None instead of Some(step.timeout): the idle watchdog decides when to kill.
let result = create_process(cmd, args, /* timeout */ None).await?;
match result.status.code() {
    Some(0) => break,          // success: stop the cascade
    _ => continue,             // real failure: try the next fallback level
}
```

## Do Not Wrap Network Calls

Idle detection is for agentic work. Leave genuine network timeouts in place:

- Git bundle transfer / download
- `azlin` list / kill / provision / connect
- `tmux` control operations

A stalled network call should fail fast; only agent runs get idle detection.

## Required Tests

Every call site you convert must prove **both** behaviors. Use a child that
prints on an interval to simulate a live agent, and a child that sleeps
silently to simulate a hang.

### Test A: a producing child is NOT killed past the old deadline

```rust
#[tokio::test]
async fn producing_child_survives_past_old_deadline() {
    // Prints once per second for ~6 s; old wall-clock cap was ~5 s.
    let mut child = tokio::process::Command::new("bash")
        .args(["-c", "for i in $(seq 1 6); do echo tick $i; sleep 1; done"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let (o, e) = (child.stdout.take(), child.stderr.take());

    // Idle window (2 s) is shorter than total runtime (6 s) but longer than
    // the gap between outputs (1 s), so the child must NOT be killed.
    let cfg = IdleConfig::with_idle(std::time::Duration::from_secs(2));
    let outcome = wait_with_idle_watchdog(&mut child, o, e, cfg).await;

    assert!(!outcome.killed_for_idle);
    assert_eq!(outcome.status.unwrap().code(), Some(0));
    assert!(outcome.stdout.contains("tick 6"));
}
```

### Test B: an idle child IS killed after the idle window

```rust
#[tokio::test]
async fn idle_child_is_killed_after_window() {
    // Emits nothing for 30 s → genuinely hung.
    let mut child = tokio::process::Command::new("bash")
        .args(["-c", "sleep 30"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let (o, e) = (child.stdout.take(), child.stderr.take());

    let cfg = IdleConfig::with_idle(std::time::Duration::from_secs(1));
    let outcome = wait_with_idle_watchdog(&mut child, o, e, cfg).await;

    assert!(outcome.killed_for_idle);
}
```

Add the sync equivalents for `wait_with_idle_watchdog_sync`, and for
`file_idle_since` assert it returns `false` right after a write and `true` only
after the file's mtime ages past the window.

## Related Docs

- [Idle Watchdog Reference](../reference/idle-watchdog.md) — full API and config.
- [Idle Watchdog Concept](../concepts/idle-watchdog.md) — the owner rule and rationale.
