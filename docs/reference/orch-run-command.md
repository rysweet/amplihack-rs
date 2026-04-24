# `amplihack orch run` — Native Workstream Orchestrator

## Overview

`amplihack orch run` executes a parallel workstreams plan described by a JSON
file. It is the native Rust replacement for the legacy
`python3 -u multitask/orchestrator.py <ws_file>` invocation used by
`smart-orchestrator.yaml`.

The subcommand is a thin alias over `amplihack multitask run`, providing the
exact CLI surface and stdout/stderr semantics required by recipe-driven
orchestration while delegating execution to the already-tested multitask
engine. It exists so recipe authors have a stable, intent-revealing entry
point; advanced flags remain available on `multitask run`.

## Synopsis

```
amplihack orch run <WS_FILE>
```

`<WS_FILE>` is a positional argument. If the path may begin with `-`, prefix
it with `--` (the standard end-of-options separator) when invoking — see
[Examples](#examples).

## Arguments

| Argument    | Type | Required | Description                                        |
| ----------- | ---- | -------- | -------------------------------------------------- |
| `<WS_FILE>` | path | yes      | Path to a workstreams JSON file (see schema below) |

## Behavior

- Parses the workstreams JSON at `<WS_FILE>`.
- Dispatches each workstream to the multitask engine using the same defaults
  as bare `amplihack multitask run` (see [Defaults](#defaults-equivalent-to-multitask-run)).
- Streams progress and per-workstream output to stdout; diagnostics go to
  stderr.
- Exits `0` on success — including the empty-workstreams short-circuit, which
  emits `No workstreams defined in <WS_FILE>` to stderr and returns cleanly.
- Returns a non-zero exit on parse or execution failure (see
  [Exit Codes](#exit-codes)).
- Does **not** delete `<WS_FILE>`. Cleanup is the caller's responsibility (the
  smart-orchestrator recipe uses a `trap` for that).

This is functionally equivalent to:

```
amplihack multitask run <WS_FILE>
```

with no additional flags.

### Defaults (equivalent to `multitask run`)

| Flag                | Effective default     |
| ------------------- | --------------------- |
| `--mode`            | `recipe`              |
| `--recipe`          | `default-workflow`    |
| `--max-runtime`     | unset (engine default) |
| `--timeout-policy`  | unset (engine default) |
| `--dry-run`         | `false`               |

To override any of these, use `amplihack multitask run` directly with the
corresponding flag — `orch run` intentionally exposes no flags.

## Examples

### Run a workstreams file

```bash
amplihack orch run ./workstreams.json
```

### Defensive invocation (path may start with `-`)

```bash
amplihack orch run -- "$WS_FILE"
```

### Recipe usage (smart-orchestrator.yaml)

```bash
set -o pipefail
trap 'rm -f -- "$WS_FILE"' EXIT
"${AMPLIHACK_BIN:-amplihack}" orch run -- "$WS_FILE" 2>&1 | tee /dev/stderr
```

The `tee /dev/stderr` preserves the recipe's `output: round_N_result` stdout
capture while also mirroring to stderr for live observation. Note that this
duplicates output across both streams — this matches the legacy
`python3 -u …/orchestrator.py` behavior and is intentional.

## Workstreams JSON Schema

The schema is identical to `amplihack multitask run`. The top-level value is a
**JSON array** of workstream objects (not an object with a `workstreams` key).

### Minimal (empty) example

```json
[]
```

An empty array is accepted; the engine logs `No workstreams defined in
<WS_FILE>` to stderr and exits `0`.

### Single-workstream example

```json
[
  {
    "issue": 1234,
    "branch": "feat/example",
    "task": "Implement the example feature.",
    "description": "Optional human-readable summary.",
    "recipe": "default-workflow",
    "max_runtime": 7200,
    "timeout_policy": "interrupt-preserve"
  }
]
```

### Field reference

| Field             | Type            | Required | Notes                                                                        |
| ----------------- | --------------- | -------- | ---------------------------------------------------------------------------- |
| `issue`           | number\|string  | yes      | Issue identifier; numbers and numeric strings are both accepted              |
| `branch`          | string          | yes      | Git branch name for this workstream                                          |
| `task`            | string          | yes      | Task description handed to the recipe / agent                                |
| `description`     | string          | no       | Optional human-readable summary                                              |
| `recipe`          | string          | no       | Per-workstream recipe override (otherwise uses `--recipe` default)           |
| `max_runtime`     | integer (secs)  | no       | Per-workstream runtime budget override                                       |
| `timeout_policy`  | string          | no       | `interrupt-preserve` or `continue-preserve`                                  |

See [`docs/reference/multitask-command.md`](./multitask-command.md) for the
full multitask schema and engine-level details.

## Exit Codes

| Code     | Meaning                                                                                          |
| -------- | ------------------------------------------------------------------------------------------------ |
| `0`      | All workstreams completed successfully (or the workstreams array was empty)                      |
| non-zero | Argument parsing failure, JSON parse failure, I/O error, or one or more workstreams failed       |

`amplihack orch run` does not distinguish exit codes by failure category
beyond what `clap` and `anyhow` provide for the underlying `multitask run`
implementation. If you need finer-grained categorisation, parse stderr.

## Environment

`amplihack orch run` inherits the parent process environment. No
subcommand-specific variables are read. Variables consumed by the underlying
multitask engine (e.g. `AMPLIHACK_HOME`, `AMPLIHACK_AGENT_BINARY`) apply
transparently.

`AMPLIHACK_BIN` is a recipe-level convention (not read by this binary): recipe
authors use it to override which `amplihack` executable the recipe invokes —
e.g. `"${AMPLIHACK_BIN:-amplihack}" orch run …`. It has no effect when set
inside `orch run` itself.

## Relationship to Other Subcommands

| Subcommand                | Purpose                                                                  |
| ------------------------- | ------------------------------------------------------------------------ |
| `amplihack orch run`      | Recipe-facing alias for executing a workstreams JSON file (this command) |
| `amplihack orch helper`   | Smart-orchestrator JSON helpers (port of `tools/amplihack/orch_helper.py`) |
| `amplihack multitask run` | Full multitask engine with all flags exposed                             |

`orch run` is intentionally minimal — for advanced flags (mode selection,
concurrency limits, dry-run, etc.) use `multitask run` directly.

## Migration Notes

Recipes previously invoking:

```bash
python3 -u "$ORCH_SCRIPT" "$WS_FILE"
```

should be updated to:

```bash
"${AMPLIHACK_BIN:-amplihack}" orch run -- "$WS_FILE"
```

The `ORCH_SCRIPT` candidate-search loop is no longer needed — the native
binary is resolved through `$PATH` (or the `$AMPLIHACK_BIN` recipe override).
Python is no longer a runtime dependency for this code path.

Stdout/stderr semantics are preserved: callers using `| tee /dev/stderr` and
the `output:` capture in recipe YAMLs continue to work without modification.
