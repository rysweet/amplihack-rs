# Idle Watchdog Concept

> [Home](../index.md) > Concepts > Idle Watchdog

> This page explains why amplihack supervises agentic child processes with idle
> detection instead of wall-clock timeouts. It is the design rationale for
> GitHub issue #867.

**Type**: Explanation (Understanding-Oriented)

## The Owner Rule

> **Never impose a wall-clock timeout that kills a working agentic step.**
> Use idle/liveness detection instead — kill only a genuinely hung child (no
> stdout/stderr progress for an idle window), never a child that is still
> producing output.

Everything below follows from this rule.

## The Problem With Wall-Clock Timeouts

A fixed deadline (for example "kill after 300 seconds") cannot tell the
difference between two very different children:

- A **healthy agent** streaming tokens, editing files, and running tests. It may
  legitimately take 20 minutes on a hard task.
- A **hung child** that deadlocked, lost its connection, or is waiting on input
  that will never arrive. It produces nothing.

A wall-clock timeout kills both. Killing the first is a correctness bug: it
discards good work, corrupts multi-turn agent state, and makes long tasks
impossible. This is exactly what issue #867 fixes.

## The Fix: Idle Detection

The idle watchdog watches **output**, not the clock.

```text
byte on stdout/stderr ──► reset "last progress" timer
                          │
        every poll tick ──┤
                          ▼
     now - last_progress > idle_timeout ?  ──yes──► kill child
                          │
                          no ──► keep waiting
```

- Any new byte on `stdout` or `stderr` proves the child is alive and resets the
  timer.
- The child is killed only after the idle threshold passes with **zero** output.
- There is **no absolute cap**: an agent that keeps talking runs as long as it
  needs.

For call sites whose output is redirected to a log file (so no live pipe is
readable), "progress" is the log file's growing modification time instead of a
byte on a pipe. Same principle, different signal.

## One Helper, Consistent Behavior

A single module — `amplihack-utils::idle_watchdog` — implements the mechanism
once and is reused everywhere. It lives in a foundational crate with no
amplihack dependencies, so any crate can use it without creating dependency
cycles. It offers three entry points for three runtime shapes:

| Entry point                     | Runtime         | Progress signal        |
| ------------------------------- | --------------- | ---------------------- |
| `wait_with_idle_watchdog`       | tokio           | bytes on child pipes   |
| `wait_with_idle_watchdog_sync`  | blocking std    | bytes on child pipes   |
| `file_idle_since`               | any             | log file mtime growth  |

See the [Idle Watchdog Reference](../reference/idle-watchdog.md) for exact
signatures and configuration.

## Where It Applies

Six subprocess call sites previously used wall-clock kills. All now defer to
idle detection:

1. **Cascade steps** — advance on the child's real exit status, not elapsed time.
2. **Multitask runs** — the default policy `continue-preserve` never kills on
   elapsed time; `interrupt-preserve` kills only when the log file goes idle.
3. **Copilot CLI client** — an idle watchdog replaces the fixed CLI deadline.
4. **Remote executor** — streams output and applies idle detection instead of a
   buffered wall-clock cap.
5. **Copilot adapter** — the 600 s wall-clock clamp is removed; the adapter's
   configured timeout is reinterpreted as an idle-window override and handed to
   the site-3 client, so completion is driven by turn/goal completion plus idle
   detection rather than a fixed deadline.
6. **Foundational process wait** — the shared idle-watchdog wrapper replaces the
   `timeout(child.wait())` pattern; `kill_on_drop(true)` stays for drop-safety.

Site 1 (cascade) drives its child through this foundational wait (site 6), so
fixing the foundation fixes cascade kill behavior too. Site 5 (Copilot adapter)
does **not** flow through it — `run_agent` delegates to the SDK client, whose
subprocess is supervised by the site-3 sync watchdog in the Copilot CLI client.

## What Stays a Timeout

Idle detection is right for **agentic** work whose duration is unpredictable. It
is wrong for bounded network operations, which should fail fast. These keep
their genuine timeouts:

- Git bundle transfer / download
- `azlin` list / kill / provision / connect
- `tmux` control operations

A stalled network call should time out; a thinking agent should not.

## Trade-Offs

- **Latency to detect a hang**: a truly hung child survives up to one idle window
  (default 300 s) before it is reaped. This is deliberate — the cost of a rare,
  slightly-late kill is far lower than the cost of killing healthy agents.
- **Noisy-but-stuck children**: a child that keeps printing while making no real
  progress is not detected, by design. Idle detection tracks liveness, not
  usefulness; goal/turn completion is the agent's job to signal.

## Related Docs

- [Idle Watchdog Reference](../reference/idle-watchdog.md) — API and configuration.
- [Use the Idle Watchdog](../howto/use-idle-watchdog.md) — wire up a call site.
- [Automode Safety](automode-safety.md) — related agent-safety guardrails.
