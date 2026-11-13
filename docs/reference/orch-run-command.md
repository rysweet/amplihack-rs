---
title: Native workstream orchestrator reference
description: Workstream plan schema, per-leaf execution and the orch run command.
last_updated: 2026-11-13
review_schedule: quarterly
owner: amplihack
doc_type: reference
---

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

## Behaviour

- Normalises and validates every task, key, classification, issue and dependency
  before external actions. Empty or malformed plans are errors.
- Resolves Operations to `operations-direct` before development setup; mixed
  plans retain every leaf and dependency.
- Saves the canonical plan, tracking intent and results. Completion and
  cancellation events drive scheduling without timed probes.
- Uses the same native engine as `amplihack multitask run`. Non-zero agent or
  runner exits remain failures even when their prose claims success.

## Defaults (equivalent to `multitask run`)

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
`python3 -u …/orchestrator.py` behaviour and is intentional.

## Workstreams JSON Schema

The schema is identical to `amplihack multitask run`. The top-level value is a
**JSON array** of workstream objects (not an object with a `workstreams` key).

### Minimal (empty) example

```json
[]
```

An empty array is accepted; the engine logs `No workstreams defined in
<WS_FILE>` to stderr and exits `0`.

### Issue-less example

```json
[
  {
    "key": "triage",
    "classification": "Operations",
    "task": "Triage issue #2553 in rysweet/amplihack-rs and report in the result.",
    "description": "triage",
    "recipe": "operations-direct"
  }
]
```

`issue` is absent, which is valid. The key `triage` identifies execution and
is saved before launch. Several issue-less leaves retain separate identities
and runtime artefacts. Operations needs no Development branch or workspace.
When a key is not supplied, normalisation allocates a safe unique key and
persists it; attaching an issue later does not change it.

### Single-workstream example

```json
[
  {
    "issue": 2553,
    "classification": "Development",
    "branch": "feat/issue-2553-operations-routing",
    "task": "Implement Operations routing and optional tracking for issue #2553.",
    "description": "Optional human-readable summary.",
    "recipe": "default-workflow",
    "max_runtime": 7200,
    "timeout_policy": "interrupt-preserve"
  }
]
```

### Field reference

This table describes the execution
schema. The current `WorkstreamConfig` requires `branch` for every entry and
does not define `key`, `classification`, `depends_on` or `create_issue`.
Those fields do not enable their documented semantics in the current binary;
branchless examples fail deserialisation. `recipe` is currently honoured
without classification enforcement. The helper also drops the four new
fields and replaces input `task` with `description`. See the
[implementation boundary](dev-orchestrator-routing.md#implementation-boundary).

The engine accepts the following fields. `dependencies` is the canonical
spelling; `depends_on` remains an alias. Runtime budgets and timeout policies
are retained as metadata and do not install a timer.

| Field             | Type            | Required | Notes                                                                        |
| ----------------- | --------------- | -------- | ---------------------------------------------------------------------------- |
| `issue`           | number\|string\|null | no  | Tracking issue. A positive integer, a decimal string, `#N`, or a `github.com` issue URL. Omitted, `null`, blank and recognised placeholders such as `TBD` mean the workstream has no tracking issue and still runs. See [Optional tracking issues](./dev-orchestrator-routing.md#optional-tracking-issues) |
| `key` | string | no on initial input; always saved | Immutable workstream identity. Safe unique keys are allocated when omitted; initially tracked work preserves numeric identities. Reused on resume. |
| `classification` | string | yes in mixed plans | Leaf classification; Operations always selects `operations-direct`. Decomposition accepts `task_type` and `type` as compatibility fields. |
| `branch` | string | Development preparation | Git branch for Development; not required or looked up for pure Operations. |
| `depends_on` | array of strings | no | Prerequisite workstream keys; references must exist and be acyclic. Successful results are passed by key. |
| `create_issue` | Boolean | no; defaults to false | If true and tracking is absent, acquire and persist an issue before executing this leaf. Existing issues are reused. |
| `task`            | string          | yes      | Task description handed to the recipe / agent                                |
| `description`     | string          | no       | Optional human-readable summary                                              |
| `recipe`          | string          | no       | Advisory recipe selection. Operations always selects `operations-direct`; Development selects `default-workflow` or eligible `quick-change`. Other routes retain their established rules; see [Dev-Orchestrator Routing Contract](./dev-orchestrator-routing.md) |
| `max_runtime`     | integer (secs)  | no       | Per-workstream runtime budget override                                       |
| `timeout_policy`  | string          | no       | `interrupt-preserve` or `continue-preserve`                                  |
| `is_critical_code` | string        | no       | Required input for a `consensus-workflow` workstream: exactly `true` or `false`. Must be a JSON string (`"true"`, not `true`); a Boolean or any other type makes the file fail to parse. Copied unchanged into `context.json`; validated by the recipe. See [Consensus Criticality Contract](./consensus-criticality-contract.md#workstreams-field) |

See [`docs/reference/multitask-command.md`](./multitask-command.md) for the
engine-level details. [Workstream state and recovery](workstream-state.md)
defines immutable identity, tracking acquisition and ownership requirements.

## Exit Codes

| Code     | Meaning                                                                                          |
| -------- | ------------------------------------------------------------------------------------------------ |
| `0`      | All workstreams completed successfully                      |
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
| `amplihack orch helper`   | Smart-orchestrator JSON helpers (native Rust) |
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
