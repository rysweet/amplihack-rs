---
title: Dev-Orchestrator Routing Contract
description: Deterministic routing rules for smart-orchestrator task and workstream execution.
last_updated: 2026-06-11
review_schedule: quarterly
owner: amplihack
doc_type: reference
---

# Dev-Orchestrator Routing Contract

The dev-orchestrator routes every normalized `Development` workstream to `default-workflow`.

This document defines the routing contract. Rust-helper enforcement runs before workstream execution: model-produced `recipe` fields are advisory input only and cannot override the `Development` routing invariant.

## Contents

- [Routing invariant](#routing-invariant)
- [Classification authority](#classification-authority)
- [Recipe normalization](#recipe-normalization)
- [Workstream JSON API](#workstream-json-api)
- [Configuration](#configuration)
- [Examples](#examples)
- [Security contract](#security-contract)
- [Related](#related)

## Routing invariant

Any workstream whose normalized classification is `Development` must run `default-workflow`.

The override applies when the workstream recipe is:

- missing
- an empty string after trimming
- whitespace only
- any recipe other than `default-workflow`

The invariant applies per workstream. A top-level hybrid task can contain multiple workstreams with different classifications, and each workstream must be routed from its own normalized classification.

## Classification authority

The routing implementation uses the most specific classification available.

| Source | Authority | Routing effect |
| --- | --- | --- |
| Per-workstream `classification`, `task_type`, or `type` | Highest | Used to normalize that workstream's recipe |
| Top-level `task_type` | Fallback | Used only when the workstream has no per-workstream classification |
| Model-provided `recipe` | Advisory | Preserved only when it does not violate deterministic routing |

The normalized classification is authoritative for routing. The raw model text must be normalized before routing decisions are made, so common variants such as `dev`, `development`, or mixed-case `Development` resolve to the same classification.

The exact fallback rule is:

1. Use the workstream's own `classification`, `task_type`, or `type` when present.
2. Use the top-level `task_type` only when the workstream has none of those fields.
3. Route an unclassified workstream under top-level `Development` to `default-workflow`.

This keeps hybrid decomposition safe: investigation, Q&A, operations, and consensus workstreams inside a broader development request must not inherit `Development` when they carry their own non-Development classification.

## Recipe normalization

The deterministic normalization rule is:

| Normalized workstream classification | Input recipe | Normalized recipe |
| --- | --- | --- |
| `Development` | missing | `default-workflow` |
| `Development` | `""` or whitespace | `default-workflow` |
| `Development` | `investigation-workflow` | `default-workflow` |
| `Development` | any non-`default-workflow` value | `default-workflow` |
| `Development` | `default-workflow` | `default-workflow` |
| Non-Development | any value | Existing route is preserved |

Non-Development classifications must not be rewritten by the Development invariant. Their existing route selection remains in force:

| Classification | Existing routing behavior |
| --- | --- |
| `Investigation` | Uses the investigation route selected by smart-orchestrator, normally `investigation-workflow` for recipe execution |
| `Q&A` | Uses the direct analyzer-answer route for top-level Q&A; workstream recipes are preserved when a plan explicitly contains Q&A workstreams |
| `Operations` | Uses the operations route selected by smart-orchestrator; workstream recipes are preserved when a plan explicitly contains operations workstreams. The current bundle does not define an `ops-workflow` recipe |
| `Consensus` | Uses the consensus route selected by smart-orchestrator; workstream recipes are preserved when a plan explicitly contains consensus workstreams |

## Workstream JSON API

`amplihack orch helper build-workstreams-config` accepts the decomposition JSON emitted by `smart-classify-route` and writes a workstreams JSON file for `amplihack orch run` with deterministic Development routing applied.

### Input shape

```json
{
  "task_type": "Development",
  "goal": "Improve routing reliability",
  "success_criteria": [
    "Development workstreams always run default-workflow"
  ],
  "workstreams": [
    {
      "name": "routing-normalization",
      "classification": "Development",
      "description": "Fix deterministic routing normalization",
      "recipe": "investigation-workflow"
    }
  ]
}
```

Supported per-workstream classification keys:

| Field | Purpose |
| --- | --- |
| `classification` | Preferred per-workstream classification field |
| `task_type` | Backward-compatible classification field |
| `type` | Backward-compatible classification field |

The helper normalizes whichever per-workstream classification field is present. If more than one exists, the implementation treats the first supported field in its documented precedence order as the workstream classification.

### Output shape

```json
[
  {
    "issue": "TBD",
    "branch": "feat/orch-1-routing-normalization",
    "task": "Fix deterministic routing normalization",
    "description": "routing-normalization",
    "recipe": "default-workflow"
  }
]
```

The output uses the `amplihack orch run` workstreams schema. See [`orch run`](./orch-run-command.md#workstreams-json-schema) for the complete execution schema.

## Configuration

There is no configuration flag that disables the Development routing invariant.

| Setting | Effect on routing |
| --- | --- |
| `force_single_workstream=true` | Prevents parallel decomposition, but a single `Development` workstream still runs `default-workflow` |
| `AMPLIHACK_MAX_DEPTH=0` | Blocks parallel spawning and adapts to single-session execution; `Development` still runs `default-workflow` |
| `AMPLIHACK_AGENT_BINARY` | Chooses the agent backend; does not affect workflow recipe selection |
| `AMPLIHACK_HOME` | Locates recipe assets; does not affect the routing invariant |

Prompt text in `dev-orchestrator`, `smart-classify-route`, `smart-execute-routing`, and the routing hook should state the same rule for model guidance:

> Development classification always routes to `default-workflow`; model-provided recipe fields do not override that invariant.

That prompt text is supportive documentation for the model. The Rust helper is authoritative.

## Examples

### Development workstream with missing recipe

Input:

```json
{
  "task_type": "Development",
  "workstreams": [
    {
      "name": "api-fix",
      "classification": "Development",
      "description": "Fix the user API timeout"
    }
  ]
}
```

Normalized output:

```json
[
  {
    "issue": "TBD",
    "branch": "feat/orch-1-api-fix",
    "task": "Fix the user API timeout",
    "description": "api-fix",
    "recipe": "default-workflow"
  }
]
```

### Development workstream with wrong recipe

Input:

```json
{
  "task_type": "Development",
  "workstreams": [
    {
      "name": "test-regression",
      "classification": "Development",
      "description": "Add regression tests for routing",
      "recipe": "investigation-workflow"
    }
  ]
}
```

Normalized output:

```json
[
  {
    "issue": "TBD",
    "branch": "feat/orch-1-test-regression",
    "task": "Add regression tests for routing",
    "description": "test-regression",
    "recipe": "default-workflow"
  }
]
```

### Hybrid decomposition preserves non-Development routes

Input:

```json
{
  "task_type": "Development",
  "workstreams": [
    {
      "name": "trace-current-routing",
      "classification": "Investigation",
      "description": "Trace current workstream route selection",
      "recipe": "investigation-workflow"
    },
    {
      "name": "fix-routing",
      "classification": "Development",
      "description": "Enforce Development routing in Rust",
      "recipe": "investigation-workflow"
    }
  ]
}
```

Normalized output:

```json
[
  {
    "issue": "TBD",
    "branch": "feat/orch-1-trace-current-routing",
    "task": "Trace current workstream route selection",
    "description": "trace-current-routing",
    "recipe": "investigation-workflow"
  },
  {
    "issue": "TBD",
    "branch": "feat/orch-2-fix-routing",
    "task": "Enforce Development routing in Rust",
    "description": "fix-routing",
    "recipe": "default-workflow"
  }
]
```

The top-level task is `Development`, but the investigation workstream keeps its own route because its per-workstream classification is `Investigation`.

### Unclassified workstream under top-level Development

Input:

```json
{
  "task_type": "Development",
  "workstreams": [
    {
      "name": "unclassified-fix",
      "description": "Fix an unclassified development workstream",
      "recipe": "investigation-workflow"
    }
  ]
}
```

Normalized output:

```json
[
  {
    "issue": "TBD",
    "branch": "feat/orch-1-unclassified-fix",
    "task": "Fix an unclassified development workstream",
    "description": "unclassified-fix",
    "recipe": "default-workflow"
  }
]
```

The workstream has no own classification, so the helper falls back to the top-level `Development` classification.

## Security contract

Decomposition JSON is model-produced and treated as untrusted input.

The routing helper must:

- trim recipe strings before checking whether they are empty
- normalize classification before comparing it to `Development`
- prevent raw recipe strings from choosing the workflow for Development workstreams
- avoid building shell commands from model-provided recipe values
- preserve non-Development behavior without broad fallback logic that masks malformed plans

## Related

- [Getting Started with /dev](../tutorials/dev-orchestrator-tutorial.md)
- [`amplihack orch run`](./orch-run-command.md)
- [Workflow Classifier](./workflow-classifier.md)
- [Smart-Orchestrator Recovery](../concepts/smart-orchestrator-recovery.md)
