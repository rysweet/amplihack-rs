# RecipeResult Reference

`RecipeResult` represents the outcome of running a workflow recipe. It captures whether the recipe succeeded, how many steps ran, and the result of each individual step.

## Contents

- [Class Overview](#class-overview)
- [Attributes](#attributes)
- [String Representation](#string-representation)
- [Step Results](#step-results)
- [Progress and Failure Context](#progress-and-failure-context)
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
| `step_results`     | `list[StepResult]` | Ordered list of results, one per step that executed.                      |
| `duration_seconds` | `float`            | Wall-clock time from recipe start to finish.                              |
| `error`            | `str \| None`      | Human-readable error message if `status` is `FAILURE`. `None` on success. |
| `progress_summary` | `dict \| None`     | Optional live-progress summary: last phase/status, heartbeat count, and log path. |
| `failure_context`  | `dict \| None`     | Optional actionable context for the first failing step.                    |

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

## Progress and Failure Context

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
- [Recipe CLI Examples](../howto/recipe-cli-examples.md) — real-world workflow scenarios
- [Recipe Resilience](../concepts/recipe-resilience.md) — how partial failures and retries are handled
