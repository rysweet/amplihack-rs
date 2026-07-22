# Recipe Run Failure Cleanup

**Status:** Shipped — process teardown, the fail-closed recursion-depth guard,
and caller git-state restoration all live in
`crates/amplihack-cli/src/commands/recipe/run/execute.rs`.
· **Issue:** [rysweet/amplihack-rs#964](https://github.com/rysweet/amplihack-rs/issues/964)
· **Crate:** `amplihack-cli` · **Module:** `commands::recipe::run`

## Overview

`amplihack recipe run` launches a `recipe-runner-rs` child that, for
orchestration recipes (`smart-orchestrator`, `dev-orchestrator`), recursively
spawns further agent and recipe subprocesses. When a **terminal recipe step
fails**, the top-level command must exit non-zero *and* leave the machine in a
clean, usable state.

Before this change a terminal failure could:

- leak **hundreds of orphaned descendants** (`recipe-runner-rs`,
  `amplihack copilot`, `node copilot`) that kept recursively spawning and were
  reparented to PID 1, and
- leave the **originating checkout unusable** — the caller's repository was left
  with `core.bare=true`, so `git status` failed with
  `this operation must be run in a work tree`.

The failure-cleanup path now guarantees, on **any** terminal recipe failure:

1. **Descendant teardown** — every process in the runner's session/process
   group is terminated (landed behavior; see [Process teardown](#process-teardown)).
2. **Recursion-depth enforcement** — a fail-closed guard blocks re-entry into
   the orchestrator beyond the configured depth *before* any subprocess is
   spawned (see [Depth guard](#depth-guard)).
3. **Caller git-state restoration** — the originating checkout's pre-run
   `core.bare` setting and worktree registration are restored, while durable
   child commits and worktrees are preserved (see [Caller git-state
   restoration](#caller-git-state-restoration)).
4. **Explicit reporting** — any cleanup failure is surfaced via structured
   `tracing` (never a stray `print!` / `println!`).

All behavior is **additive and non-breaking**: the happy-path orchestration
contract, the PRD, and all public APIs are unchanged. There is no "Bridge"
naming anywhere in the feature.

## Depth guard

The depth guard is a **fail-closed** recursion limiter. It runs *before every
subprocess spawn path* in `execute.rs`, so a failing or misbehaving recipe can
never re-enter the orchestrator and fork-bomb the host.

It **reuses the existing session-tree depth convention** rather than inventing a
new one: the same `AMPLIHACK_SESSION_DEPTH` / `AMPLIHACK_MAX_DEPTH` env vars and
the shared `DEFAULT_MAX_DEPTH` (`3`) and `MAX_DEPTH_CEILING` (`32`) constants
from `commands::session_tree::state` (see `session_tree::tree_context`). The
guard must not fork a second source of truth with a divergent ceiling.

### Environment variables

| Variable                 | Meaning                                  | Default | Hard cap |
| ------------------------ | ---------------------------------------- | ------- | -------- |
| `AMPLIHACK_SESSION_DEPTH`| Current nesting depth of this session    | `0`     | —        |
| `AMPLIHACK_MAX_DEPTH`    | Maximum allowed nesting depth            | `3` (`DEFAULT_MAX_DEPTH`) | `32` (`MAX_DEPTH_CEILING`) |

On each spawn the guard reads `AMPLIHACK_SESSION_DEPTH` and `AMPLIHACK_MAX_DEPTH`
and bails **before** spawning when `depth >= max`.

### Fail-closed parsing rules

- **Absent** `AMPLIHACK_SESSION_DEPTH` → treated as `0` (top-level session).
- **Malformed / non-numeric / overflowing** `AMPLIHACK_SESSION_DEPTH` →
  treated as **at the maximum** (guard bails). It is never silently coerced to
  `0`, which would bypass the guard.
  > **Note:** this is intentionally *stricter* than the existing
  > `session_tree::tree_context`, which parses malformed depth as `0`
  > (fail-open). The recipe-run guard deliberately fails closed; implementers
  > must not "align" it back to the fail-open behavior.
- **Absent / malformed** `AMPLIHACK_MAX_DEPTH` → falls back to the default
  `DEFAULT_MAX_DEPTH` (`3`).
- **Forged large** `AMPLIHACK_MAX_DEPTH` (e.g. `999999`) → clamped to the shared
  ceiling `MAX_DEPTH_CEILING` (`32`, via `.min(MAX_DEPTH_CEILING)`). This
  prevents a forged environment from disabling the limit and turning a recursive
  recipe into a fork bomb / DoS.

### Behavior on limit

When the limit is reached the guard returns a structured error and **no
subprocess is spawned**:

```text
Error: recipe run recursion depth guard exceeded: depth 3 reached configured max 3
```

The error is emitted through `tracing` with numeric fields only:

```
WARN depth=3 max=3 recipe run blocked by recursion depth guard
```

> **Security:** the guard logs depths and counts only — never environment-variable
> *values*, which may carry session tokens or secrets.

### API

```rust
/// Fail-closed recursion guard for `amplihack recipe run`. Runs at the top of
/// `execute_recipe_via_rust`, before binary lookup or any spawn.
///
/// Reads `AMPLIHACK_SESSION_DEPTH` (current depth, default `0`) and
/// `AMPLIHACK_MAX_DEPTH` (limit, default `DEFAULT_MAX_DEPTH = 3`, clamped to the
/// shared `MAX_DEPTH_CEILING = 32`) and returns `Err(..)` when `depth >= max`,
/// BEFORE any subprocess is spawned. A malformed / non-UTF-8
/// `AMPLIHACK_SESSION_DEPTH` is treated as at-max (fail-closed); a malformed
/// `AMPLIHACK_MAX_DEPTH` falls back to `3`.
fn enforce_recursion_depth_guard() -> anyhow::Result<()>;
```

## Caller git-state restoration

Orchestration recipes create child worktrees for their nested tasks. On a
terminal failure the caller's own checkout could be left with `core.bare=true`
and its worktree registration dropped, making `git status` fail even though the
source files were still on disk.

The restoration flow:

1. **Snapshot before spawn** — capture the caller checkout's `core.bare` value
   and worktree registration state into a `GitStateSnapshot`.
2. **Restore after terminal-failure teardown** — once descendants are reaped,
   best-effort restore the snapshotted state on the caller checkout **only**.

### Guarantees

- Runs **only on terminal failure**, after descendant teardown — never on the
  happy path.
- **Best-effort:** a restore failure is logged via `tracing::warn!` and never
  aborts or masks the original failure.
- Scoped to the **single `core.bare` key** and the **caller checkout**. It never
  touches child `.git` registrations, so durable child commits and worktrees are
  preserved.
- A `None` snapshot (nothing captured) is never restored as `false` — absence is
  distinguished from a captured value.
- Paths are canonicalized and `.git` ownership is confirmed before writing;
  symlink / relative-path escapes are rejected. The operation inherits caller
  privileges only (no elevation).

### API

```rust
/// Pre-run snapshot of the caller checkout's `core.bare` state, captured before
/// spawning the recipe runner.
struct CallerGitState { /* dir + was_git_checkout + core.bare */ }

impl CallerGitState {
    /// Capture the caller checkout's `core.bare` before spawning. A no-op
    /// (`was_git_checkout = false`) when `dir` is not a git work tree.
    fn snapshot(dir: &Path) -> Self;

    /// Best-effort restore of the caller checkout to the snapshotted `core.bare`,
    /// run only after a terminal failure. Never bails: logs `tracing::error!` on
    /// failure and preserves durable child worktrees (config-only).
    fn restore_on_failure(&self);
}
```

## Process teardown

Descendant teardown is deterministic and scoped strictly to the runner's own
process group.

- The runner is spawned as a **session leader** (`setsid` in
  `spawn_with_streaming_stderr`), so its PID doubles as the process-group id
  (`pgid`) shared by every descendant.
- Teardown signals target `-pgid` only via `libc::kill(-pgid, sig)` — it never
  uses `pkill` / `killall` / name-based matching, and never touches parent or
  unrelated processes.
- Contract: **SIGTERM → grace window → SIGKILL**.
  - `terminate_recipe_runner` handles the timeout path.
  - `reap_recipe_runner_group` sweeps orphaned descendants left behind on any
    non-timeout early-exit (success *or* failure).
- A missing group (`ESRCH`) is expected and silent; any other signal failure is
  logged via `tracing::warn!`.

### Grace window configuration

| Variable                      | Meaning                                    | Default |
| ----------------------------- | ------------------------------------------ | ------- |
| `AMPLIHACK_TEARDOWN_GRACE_SECS` | Whole-second SIGTERM→SIGKILL grace window. `0` escalates to SIGKILL immediately. | `RECIPE_RUNNER_DEFAULT_TEARDOWN_GRACE` (`5` seconds) |

## End-to-end failure sequence

```text
amplihack recipe run smart-orchestrator ...
  │
  ├─ enforce_recursion_depth_guard()     # fail-closed: bail before spawn if depth >= max
  ├─ CallerGitState::snapshot(repo)      # capture caller core.bare before spawn
  ├─ spawn recipe-runner-rs (setsid)     # session leader; pgid == pid
  │     └─ … nested agents / recipes …
  │
  ├─ TERMINAL FAILURE
  │     ├─ terminate_recipe_runner / reap_recipe_runner_group   # SIGTERM → grace → SIGKILL on -pgid
  │     └─ caller_git.restore_on_failure()                      # best-effort; preserves child worktrees
  │
  └─ exit non-zero, structured tracing report
```

## Constraints

- Additive / non-breaking; the PRD and happy-path orchestration contract are
  preserved.
- Structured `tracing` + OpenTelemetry only — **no** `print!` / `println!` /
  `eprintln!` in the new code.
- No "Bridge" naming.
- Shared OODA-core files (`execute.rs`) are modified under the serialized
  `ooda-core` sequence group to avoid colliding with other core edits.

## Testing

Regression coverage lives in
`crates/amplihack-cli/src/commands/recipe/run/tests_teardown.rs` (uses an RAII
env-guard and a stub spawn harness — no real sockets, no real recursion):

| Test area                         | Asserts                                                                 |
| --------------------------------- | ----------------------------------------------------------------------- |
| Depth-exceeded bail               | `enforce_recursion_depth_guard` bails and no spawn occurs.              |
| Malformed `AMPLIHACK_SESSION_DEPTH` | Fail-closed: treated as at-max, guard bails.                          |
| Forged `AMPLIHACK_MAX_DEPTH`      | Clamped to shared `MAX_DEPTH_CEILING = 32`.                             |
| Git snapshot / restore idempotency | Caller `core.bare` restored so `git status` works again on failure.   |
| Child-worktree preservation       | Durable child worktrees are never removed by restoration.              |
| Descendant teardown (verify-only) | Signals target `-pgid` only; no name-based kill.                       |
