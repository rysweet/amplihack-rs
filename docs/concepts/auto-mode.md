# Auto Mode

**Type**: Explanation (Understanding-Oriented)

Auto mode enables autonomous agentic loops with Claude Code or GitHub Copilot
CLI, allowing AI to work through multi-turn workflows with minimal human
intervention.

## Overview

Auto mode orchestrates an intelligent loop that:

1. Clarifies objectives with measurable evaluation criteria
2. Creates detailed execution plans identifying parallel opportunities
3. Executes plans autonomously through multiple turns
4. Evaluates progress after each turn
5. Continues until objective achieved or max turns reached
6. Provides comprehensive summary of work completed

## Usage

### With Claude Code

```bash
# Basic auto mode
amplihack claude --auto -- -p "implement user authentication"

# With custom max turns
amplihack claude --auto --max-turns 20 -- -p "refactor the API module"
```

### With GitHub Copilot CLI

```bash
# Basic auto mode
amplihack copilot --auto -- -p "add logging to all services"

# With custom max turns
amplihack copilot --auto --max-turns 15 -- -p "implement feature X"
```

## How It Works

### Turn 1: Objective Clarification

Auto mode transforms your prompt into a clear objective with evaluation criteria.

- **Input**: Your prompt
- **Output**: Clear objective statement + measurable evaluation criteria

### Turn 2: Planning

Creates a detailed execution plan, identifying:

- Sequential steps
- Parallel opportunities
- Dependencies between tasks

### Turns 3-N: Execution

Each turn:

1. Executes the next step in the plan
2. Evaluates progress against criteria
3. Adjusts plan if needed
4. Continues or terminates

### Final Turn: Summary

Provides:

- Work completed
- Files changed
- Evaluation results against criteria
- Remaining work (if any)

## Session Limits

Auto mode is governed by a single, explicit, operator-configured policy —
the **turn budget** — plus two narrowly-scoped, non-negotiable safety bounds
(a subprocess network timeout and an injected-content security bound). There
are **no arbitrary wall-clock, API-call, or output-byte caps**: those limits
are neither enforced nor configurable, because open-ended agentic work should
be bounded by explicit operator policy, not by hidden magic numbers.

| Control                   | Default | Where configured                                          | Kind                          |
| ------------------------- | ------- | --------------------------------------------------------- | ----------------------------- |
| Max turns                 | 10      | `--max-turns` flag **or** `AMPLIHACK_AUTO_MAX_TURNS` env  | Operator policy               |
| Subprocess network timeout| 1800 s  | fixed (`QUERY_TIMEOUT`)                                    | I/O liveness backstop         |
| Injected-content size     | 50 KiB  | fixed (`MAX_INJECTED_CONTENT_SIZE`)                       | Security bound (untrusted in) |

### Max turns (the turn budget)

`max_turns` is the primary control. It is resolved with the precedence
**CLI flag > environment variable > default**:

1. An explicit `--max-turns N` flag always wins.
2. Otherwise, if `AMPLIHACK_AUTO_MAX_TURNS` is set in the environment, its
   value is used. This lets daemons, CI jobs, and non-interactive callers set
   policy once instead of editing every invocation.
3. Otherwise the default of `10` applies.

The value is validated identically for both sources (`1..`, i.e. a positive
integer). A zero, negative, non-numeric, or overflowing value is **rejected**
at parse time — it never silently falls back to a default.

```bash
# Flag (highest precedence)
amplihack claude --auto --max-turns 20 -- -p "refactor the API module"

# Environment (applies when no flag is given)
export AMPLIHACK_AUTO_MAX_TURNS=20
amplihack claude --auto -- -p "refactor the API module"

# Flag overrides env: this runs with 5 turns, not 20
AMPLIHACK_AUTO_MAX_TURNS=20 amplihack claude --auto --max-turns 5 -- -p "quick fix"
```

`AMPLIHACK_AUTO_MAX_TURNS` applies to every auto-mode subcommand
(`claude`, `copilot`, `codex`, `amplifier`, `rustyclawd`, and the top-level
launch form) uniformly.

When the turn budget is reached without a verified-complete objective, auto
mode stops and logs the reason explicitly (`Reached max turns without verified
completion`). It never stops silently.

### Subprocess network timeout (kept)

Each turn waits on the agent subprocess with a fixed `QUERY_TIMEOUT` of 1800
seconds (30 minutes). This is an **I/O liveness backstop** on a single
receive — it bounds how long auto mode waits for the subprocess to respond,
not the total wall-clock duration of a turn or session. It is intentionally
not configurable and is not an arbitrary resource cap.

### Injected-content size (security bound, kept)

Content appended to a running session from the append queue is **untrusted
input**. Before injection it is sanitized: suspicious prompt-injection
patterns are redacted, and the payload is bounded to
`MAX_INJECTED_CONTENT_SIZE` (50 KiB). This is a **security control on
untrusted input**, evaluated separately from resource policy — see
[Injected-content sanitization](#injected-content-sanitization) below.

## Platform Differences

| Feature              | Claude Code          | Copilot CLI         |
| -------------------- | -------------------- | ------------------- |
| Context injection    | Full (hook-based)    | File-based (AGENTS.md) |
| Tool restriction     | `--disallowed-tools` | Prompt constraint   |
| Multi-turn support   | Native               | Via subprocess loop |

## amplihack-rs Integration

In amplihack-rs, auto mode is invoked through the unified CLI:

```bash
amplihack claude --auto --max-turns 10 -- -p "task description"
```

The Rust binary handles argument parsing, agent binary resolution, and
subprocess management. See [Automode Safety](../concepts/automode-safety.md)
for critical safety guidelines.

## Injected-content sanitization

Auto mode supports appending instructions to a running session by dropping
Markdown files into the session's append queue. Because this content can
originate from outside the operator's direct control, it is treated as
**untrusted input** and passed through `sanitize_injected_content` before it
is ever added to the agent's context:

1. **Prompt-injection redaction** — known manipulation patterns (e.g.
   "ignore previous instructions", "you are now", "system prompt:") are
   replaced with `[REDACTED: suspicious pattern]`.
2. **Size bound** — if the content exceeds `MAX_INJECTED_CONTENT_SIZE`
   (50 KiB), it is truncated on a UTF-8 character boundary and a
   `[Content truncated due to size limit]` marker is embedded so the agent
   sees that content was removed.

Truncation is **never silent**: when the bound is exceeded, auto mode emits a
`tracing::warn!` naming the actual byte size and the limit, so operators are
told that an appended instruction was shortened. The warning logs the size and
limit only — never the payload — to avoid leaking injected content into logs.

This bound is a **security control on untrusted input**, not a resource cap.
It is intentionally fixed and always applied.

## Related

- [Automode Safety](../concepts/automode-safety.md) — critical safety guide for auto mode
- [Recipe Resilience](../concepts/recipe-resilience.md) — how recipes self-heal on failure
- [Recipe Execution Flow](../concepts/recipe-execution-flow.md) — how recipes execute
