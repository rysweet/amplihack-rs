---
title: Native workstream orchestrator
description: Execute normalised workstream configurations with optional tracking and per-leaf routes.
last_updated: 2026-11-12
review_schedule: quarterly
owner: amplihack
doc_type: reference
---

# `amplihack orch run` — Native Workstream Orchestrator

> **[IMPLEMENTED - Validation Pending]** The issue #2553 per-leaf routing,
> optional tracking and dependency/result contracts below require implementation
> and behavioural validation. The new JSON examples describe that target.

## Overview

`amplihack orch run` executes a normalised leaf plan described by a JSON file.
Independent ready leaves can run concurrently; dependencies preserve ordering.
Operations leaves run directly, and repository-editing Development leaves use
their selected Development workflow.

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

- Parses and validates the complete workstreams JSON at `<WS_FILE>` before
  tracking creation or child execution.
- Dispatches each workstream to the multitask engine using the same defaults
  as bare `amplihack multitask run` (see [Defaults](#defaults-equivalent-to-multitask-run)).
- Streams progress and per-workstream output to stdout; diagnostics go to
  stderr.
- Exits `0` on success — including the empty-workstreams short-circuit, which
  emits `No workstreams defined in <WS_FILE>` to stderr and returns cleanly.
- Returns a non-zero exit on parse or execution failure (see
  [Exit Codes](#exit-codes)).
- Uses stable workstream UUIDs for state, progress and processes. Tracking
  issues are optional and scoped to each leaf's repository.
- Releases dependants only after prerequisite success is validated and
  persisted. Child completion events drive dispatch.
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

To override these settings, use `amplihack multitask run` directly with the
corresponding flag. Recipe defaults and overrides never change an Operations
leaf's direct route. `orch run` intentionally exposes no flags.

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
capture while also mirroring to stderr for live observation. This duplicates
output across both streams.

## Workstreams JSON Schema

The schema is identical to `amplihack multitask run`. The top-level value is a
**JSON array** of workstream objects (not an object with a `workstreams` key).

### Minimal (empty) example

```json
[]
```

An empty array is accepted; the engine logs `No workstreams defined in
<WS_FILE>` to stderr and exits `0`.

### Issue-less Operations example

```json
[
  {
    "workstream_id": "8f9f57b5-9232-49df-b985-c2c92ee997c1",
    "key": "routing-answer",
    "repository": "rysweet/amplihack-rs",
    "classification": "Operations",
    "task": "Read issue #2553 and explain its routing requirements in chat.",
    "description": "Explain Operations routing",
    "recipe": "operations-direct",
    "depends_on": [],
    "required_outputs": {"answer": "answer"},
    "tracking": {"required": false}
  }
]
```

The issue mentioned as source material in `task` is not an implicit tracking
binding. The example deliberately omits `issue`.

### Field reference

| Field | Type | Requirement and meaning |
| --- | --- | --- |
| `workstream_id` | UUID string | Required in a normalised plan; allocated once during legacy adaptation if absent |
| `key` | string | Unique plan key; supplied during compatible legacy adaptation if absent |
| `repository` | string | Authorised repository; compatible single-repository input can inherit trusted context |
| `classification` | string | Canonical per-leaf class; new mixed plans require it |
| `issue` | integer, decimal string or null | Optional; positive through `u64::MAX`; blanks and recognised placeholders mean absence |
| `branch` | string | Development branch, supplied or generated; absent for Operations |
| `task` | string | Required non-blank leaf task handed to its recipe or agent |
| `description` | string | Optional human-readable summary |
| `recipe` | string | Trusted route selected from classification; model input is advisory |
| `depends_on` | array of UUID strings | Prerequisite identities; defaults to an empty list |
| `tracking` | object | `required` defaults to false; required tracking uses an applicable issue, an explicit [producer binding](dev-orchestrator-routing.md#administrative-output-tracking-bindings), or creation `title` and `body` |
| `required_outputs` | object | Non-empty map of output names to kinds; see [completion and prerequisite JSON](workstream-state.md#completion-and-prerequisite-json) for declarations and defaults |
| `max_runtime` | integer, seconds | Existing per-workstream budget setting; does not define dependency readiness |
| `timeout_policy` | string | Existing `interrupt-preserve` or `continue-preserve` policy |
| `is_critical_code` | string | Consensus input, exactly `"true"` or `"false"`; JSON Booleans fail parsing. Copied unchanged into context and validated by the recipe |

Decomposition input uses `name` keys for dependency references; normalisation
resolves them into the UUIDs in this execution schema. Canonical serialisation
omits absent issues. See [optional issue parsing](dev-orchestrator-routing.md#optional-issue-parsing)
and the [Consensus criticality contract](consensus-criticality-contract.md#workstreams-field).

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

Variables consumed by the underlying multitask engine, such as `AMPLIHACK_HOME`
and `AMPLIHACK_AGENT_BINARY`, apply. Child environments are constructed per
leaf; inherited issue-specific values are cleared and absent tracking is
omitted. See [child context and environment](workstream-state.md#child-context-and-environment).

`AMPLIHACK_BIN` is a recipe-level convention (not read by this binary): recipe
authors use it to override which `amplihack` executable the recipe invokes —
e.g. `"${AMPLIHACK_BIN:-amplihack}" orch run …`. It has no effect when set
inside `orch run` itself.

## Relationship to Other Subcommands

| Subcommand                | Purpose                                                                  |
| ------------------------- | ------------------------------------------------------------------------ |
| `amplihack orch run`      | Recipe-facing alias for executing a workstreams JSON file (this command) |
| `amplihack orch helper`   | Smart-orchestrator JSON helpers (native Rust) |
| `amplihack multitask run` | Full multitask engine with all flags exposed                             |

`orch run` is intentionally minimal — for advanced flags (mode selection,
concurrency limits, dry-run, etc.) use `multitask run` directly.

## Migration

Existing valid issue-bearing configurations remain accepted. Legacy saved
state is adapted only after ownership validation, retaining its recorded paths.
Missing tracking never blocks execution by itself. Read
[legacy configuration and saved state](workstream-state.md#legacy-configuration-and-saved-state)
before resuming an older plan.
