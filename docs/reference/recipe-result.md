---
title: RecipeResult reference
description: Structured recipe results, progress, failure context, and recovery metadata.
doc_type: reference
last_updated: 2026-11-11
---

# RecipeResult Reference

`RecipeResult` represents the outcome of running a workflow recipe. It captures whether the recipe succeeded, how many steps ran, and the result of each individual step.

## Contents

- [Class Overview](#class-overview)
- [Attributes](#attributes)
- [String Representation](#string-representation)
- [Step Results](#step-results)
- [Progress and Failure Context](#progress-and-failure-context)
- [Terminal-state failures](#terminal-state-failures)
- [Recovery metadata](#recovery-metadata)
- [Usage Examples](#usage-examples)
- [Integration with Recipe Runner](#integration-with-recipe-runner)

---

## Class Overview

`RecipeResult` is the structured output of a recipe run. It is returned by the `amplihack recipe run` CLI command when `--format json` is used. The JSON schema uses the same field names documented below.

---

## Attributes

| Attribute          | Type               | Description                                                               |
| ------------------ | ------------------ | ------------------------------------------------------------------------- |
| `recipe_name`      | `str`              | The name of the recipe that was run (e.g. `"default-workflow"`).          |
| `status`           | `RecipeStatus`     | Overall outcome: `SUCCESS`, `FAILURE`, or `PARTIAL`.                      |
| `success`          | `bool \| None`     | Optional boolean outcome field used by CLI JSON output when present.      |
| `step_results`     | `list[StepResult]` | Ordered step results. |
| `duration_seconds` | `float`            | Wall-clock time from recipe start to finish.                              |
| `error`            | `str \| None`      | Human-readable error message if `status` is `FAILURE`. `None` on success. |
| `progress_summary` | `dict \| None`     | Optional live-progress summary: last phase/status, heartbeat count, and log path. |
| `failure_context`  | `dict \| None`     | Optional actionable context for the first failing step.                    |
| `run_id`           | `str \| None`      | Stable UUID generated for this `amplihack recipe run` invocation.          |
| `log_pointer`      | `dict \| None`     | Concise final pointer summary with status, worktree, child PID, runner path, and log paths. |
| `recovery` | `dict \| None` | Recovery decision, source run, restored steps, and restart location. |

---

## String Representation

`str(result)` returns a one-line summary suitable for logging:

```
RecipeResult(<recipe-name>: <STATUS>, <N> steps)
```

**Examples:**

```rust
result = runner.run("default-workflow", context)

print(str(result))
# RecipeResult(default-workflow: SUCCESS, 22 steps)

failed_result = runner.run("quick-fix", context)
print(str(failed_result))
# RecipeResult(quick-fix: FAILURE, 3 steps)
```

The summary includes:

- The recipe name as written in the recipe YAML file
- The overall `STATUS` in uppercase
- The count of steps that **completed** (not the total steps defined in the recipe)

> **Note:** Prior to v0.9.2, `str(result)` returned a verbose multi-line representation that included individual step IDs (e.g. `step-1`). Code that parsed `str(result)` should be updated to use `result.step_results` directly for structured access.

---

## Step Results

Each entry in `step_results` is a `StepResult`:

```rust
@dataclass
class StepResult:
    step_id: str          # e.g. "step-1", "step-2"
    step_name: str        # Human-readable step name from recipe YAML
    status: StepStatus    # SUCCESS, FAILURE, SKIPPED
    output: str | None    # Agent output or None if skipped
    duration_seconds: float
    phase: str | None     # agent, bash, subprocess, recipe, finalize
    child: dict | None    # Agent, subprocess, or nested recipe identity
    last_heartbeat_at: str | None
    recent_output: list[dict]
```

Accessing step results directly:

```rust
result = runner.run("default-workflow", context)

for step in result.step_results:
    print(f"{step.step_id}: {step.status.value} ({step.duration_seconds:.1f}s)")

# step-1: SUCCESS (2.3s)
# step-2: SUCCESS (8.1s)
# step-3: FAILURE (1.0s)
```

---

## Progress, Failure, and Correlation Context

The JSON schema is backward-compatible. Existing fields remain unchanged; newer
runner versions add optional progress and diagnostic fields.

### `progress_summary`

`progress_summary` describes the last known live-progress state for the recipe.

```json
{
  "last_step_id": "code-implementation",
  "last_step_name": "Code implementation",
  "phase": "agent",
  "status": "completed",
  "heartbeat_count": 7,
  "log_path": "/tmp/default-workflow.jsonl"
}
```

### `failure_context`

When a recipe fails, `failure_context` repeats the most actionable step-level
details at the top level.

```json
{
  "step_id": "code-implementation",
  "step_name": "Code implementation",
  "phase": "agent",
  "status": "failed",
  "elapsed_seconds": 734.2,
  "child": {
    "kind": "agent",
    "name": "builder"
  },
  "recent_output": [
    {
      "source": "agent:builder",
      "stream": "stderr",
      "line_count": 20,
      "byte_count": 4096,
      "truncated": true,
      "text": "error[E0425]: cannot find value `cache_policy` in this scope\n..."
    }
  ]
}
```

### `run_id`

`run_id` is the stable UUID for the whole recipe invocation. It matches:

- `AMPLIHACK_RECIPE_RUN_ID` in the runner child environment
- the `run_id` in early and final stderr pointer events
- the `run_id` in JSONL events when recipe logging is enabled and the runner
  copies `AMPLIHACK_RECIPE_RUN_ID` into those events

```json
{
  "run_id": "5b60657b-76ef-4f49-8a22-8b89ed75f43e"
}
```

### `log_pointer`

`log_pointer` is the final pointer summary included in JSON/YAML results when a
result can be produced.

```json
{
  "log_pointer": {
    "run_id": "5b60657b-76ef-4f49-8a22-8b89ed75f43e",
    "recipe_name": "default-workflow",
    "status": "success",
    "worktree": "/home/user/src/myapp",
    "branch": "main",
    "child_pid": 41822,
    "exit_code": 0,
    "runner_path": "/home/user/.local/bin/recipe-runner-rs",
    "log_paths": {
      "jsonl": "/tmp/default-workflow.jsonl"
    }
  }
}
```

The full stderr pointer event can include additional display-only context such as
`task_description`, issue fields, and PR fields. The result summary stays
concise so scripts can consume it safely.

Pointer `status` values are lowercase wrapper statuses such as `success`,
`failure`, `spawn_failure`, and `parse_failure`. Top-level result statuses keep
their existing schema casing, commonly `SUCCESS`, `FAILURE`, or `PARTIAL`, or
the boolean `success` field.

### `recent_output`

`recent_output` contains bounded rolling snippets from child stdout/stderr.

| Field | Type | Description |
| --- | --- | --- |
| `source` | `str` | `agent:<name>`, `subprocess:<pid>`, `recipe:<name>`, or `step:<id>` |
| `stream` | `str` | `stdout`, `stderr`, or `combined` |
| `line_count` | `int` | Number of retained lines |
| `byte_count` | `int` | Number of retained bytes |
| `truncated` | `bool` | `true` when older output was dropped |
| `text` | `str` | Recent output text; bounded but not additionally redacted by this feature |

Consumers should ignore unknown fields and handle all progress fields as
optional. The `amplihack` CLI must preserve additive fields emitted by
`recipe-runner-rs` when formatting JSON or YAML; parsing the runner output and
then dropping unknown diagnostic fields is a bug. See
[Recipe Runner Logging Reference](./recipe-runner-logging.md) for the full
logging contract.

---

## Terminal-state failures

Development workflows include terminal-state evidence in the failing step output
when completion cannot be proven. These fields are additive and must be
preserved by JSON/YAML consumers. The target terminal-state feature emits
boolean markers as canonical lowercase strings (`"true"` or `"false"`) because
the shell helper output is key/value text; consumers should accept either those
strings or native booleans:

```json
{
  "terminal_success": "false",
  "terminal_state": "FAILED_MISSING_TERMINAL_EVIDENCE",
  "terminal_reason": "development workflow stopped after workflow-worktree; implementation, verification, publish, or explicit no-op evidence is required",
  "observed_phases": "workflow-prep,workflow-worktree",
  "missing_evidence": "implementation_completed,verification_completed,publish_state_reached,terminal_no_op"
}
```

When `workflow-terminal-state` fails, the overall `RecipeResult.status` is
`FAILURE` and the process exits nonzero. Callers must not reinterpret missing
terminal evidence as a successful planning run. See
[Workflow Terminal-State Reference](./workflow-terminal-state.md) for the full
field contract.

---

## Recovery metadata

JSON and YAML results preserve the additive `recovery` object emitted by the
runner. Its absence means recovery information is unavailable. The object is
separate from saved step results and never contains semantic context or IPC
credentials.

| Field | Type | Meaning |
| --- | --- | --- |
| `mode` | string | `resumed` if an eligible source was imported; otherwise `fresh` |
| `reason` | string | Bounded decision code from the table below |
| `source_run_id` | string or null | Source launch's `AMPLIHACK_RUN_ID`; does not replace this result's current `run_id` |
| `source_sequence` | integer or null | Source launch's monotonic recovery admission sequence |
| `restored_steps` | array | Reused completion locations, each with `invocation_path` and `step_id` |
| `restart` | object or null | First semantic step executed after replay, with `invocation_path` and `step_id`; null if none executed |
| `invalidated_steps` | array | Rejected completion locations, each with `invocation_path`, `step_id`, and bounded `reason` |

Invocation paths distinguish call sites, nested recipes, parallel branches,
and iterations. Treat them as opaque identifiers rather than splitting
them to infer execution order. Current live admission and authorization
checks do not count as a semantic restart step. A completed parent can
cover restored child work; consumers must not sum parent and child
durations as time spent in the new launch.

Illustrative recovery metadata after a design interruption:

```json
{
  "mode": "resumed",
  "reason": "usage_limit",
  "source_run_id": "3f0c9a52-6d1e-4b8a-9f57-2c41d0e7a6b3",
  "source_sequence": 3,
  "restored_steps": [
    {
      "invocation_path": "smart-orchestrator/smart-classify-route",
      "step_id": "classify-and-decompose"
    }
  ],
  "restart": {
    "invocation_path": "smart-orchestrator/smart-execute-routing/execute-single-round-1-development/workflow-design",
    "step_id": "step-05b-api-design"
  },
  "invalidated_steps": []
}
```

The example shows a subset of restored locations; a result reports all
restored completions. Existing `step_results` retain original structured
outputs and result semantics. Recorded durations belong to the original
execution; the top-level `duration_seconds` measures the current launch.
Recovery metadata, rather than a fabricated zero-duration success,
distinguishes restoration from execution.

| Decision or invalidation code | Meaning |
| --- | --- |
| `usage_limit` | Compatible progress restored from the latest eligible predecessor |
| `fresh_requested` | Historical replay bypassed by `--fresh` |
| `identity_unavailable` | No verifiable work identity; fresh outside shared namespaces |
| `no_history` | No matching predecessor |
| `latest_not_usage_limit` | Latest outcome is success, non-limit failure, or unknown |
| `owner_active` | Previous owner is still active and cannot be recovered |
| `state_unavailable` | Historical detail is missing, incomplete, corrupt, unsupported, or otherwise unverifiable |
| `definition_changed` | Execution-relevant recipe, step, child, or hook definition changed |
| `input_changed` | Effective inputs or relevant execution settings changed |
| `repository_changed` | Current revision or worktree differs from verifiable cumulative state |
| `resource_unavailable` | Required file, artifact, or live resource cannot be verified or rebound |
| `persistence_disallowed` | Required semantic values cannot be saved under the recipe policy |
| `replay_disallowed` | Step policy requires execution rather than historical reuse |

An invalidated suffix can coexist with `mode: resumed` when an earlier
prefix was restored. If no completions are reusable, `mode` is `fresh`
and `reason` explains why. Consumers should tolerate additional reason
codes and fields. Metadata contains no saved context, prompt bodies,
artifact contents, or attachment credentials. Unsafe current storage and
failed durable writes are execution errors, not successful fresh fallbacks.

See [recovery contracts](recipe-recovery.md) for completion and status
semantics and [the recovery guide](../howto/resume-recipe-after-usage-limit.md)
for interpreting the launch decision.

## Usage Examples

### Check overall status

```bash
amplihack recipe run default-workflow \
  -c task_description="add input validation" \
  --format json | jq '.status // .success'
# true
```

### Count successful steps

```bash
amplihack recipe run default-workflow \
  -c task_description="add input validation" \
  --format json | jq '[.step_results[] | select(.status == "completed" or .status == "SUCCESS")] | length'
```

### Serialise to JSON

The `--format json` flag produces the full `RecipeResult` as JSON:

```bash
amplihack recipe run default-workflow \
  -c task_description="add input validation" \
  --format json | jq .
```

### Inspect failure context

```bash
amplihack recipe run default-workflow \
  -c task_description="fix cache policy failure" \
  --format json > result.json

jq '.failure_context
  | {step_id, step_name, elapsed_seconds, child, recent_output}' result.json
```

### Log a one-line summary

Without `--format json`, the CLI prints the final result in the default
human-readable table format on stdout:

```
Recipe: default-workflow
Status: ✓ Success

Steps:
  ✓ requirements-clarification: completed
```

---

## Integration with Recipe Runner

`RecipeResult` is what the CLI surfaces when `--format json` is requested:

```bash
amplihack recipe run default-workflow \
  -c task_description="add login endpoint" \
  --format json | jq '.status // .success'
# true
```

The JSON schema matches the fields documented in [Attributes](#attributes).
Existing result producers may expose either `success` (boolean) or `status`
(`"SUCCESS"`, `"FAILURE"`, `"PARTIAL"`); consumers should accept both.

---

## See Also

- [Recipe CLI Reference](./recipe-cli-reference.md) — `amplihack recipe run` and `--format json`
- [Recipe Run Correlation Reference](./recipe-run-correlation.md) — `run_id` and `log_pointer` fields
- [Recipe CLI Examples](../howto/recipe-cli-examples.md) — real-world workflow scenarios
- [Recipe Resilience](../concepts/recipe-resilience.md) — how partial failures and retries are handled
- [Workflow Terminal-State Reference](./workflow-terminal-state.md) — development completion evidence and failure semantics
