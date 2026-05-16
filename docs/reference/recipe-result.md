# RecipeResult Reference

`RecipeResult` represents the outcome of running a workflow recipe. It captures whether the recipe succeeded, how many steps ran, and the result of each individual step.

## Contents

- [Class Overview](#class-overview)
- [Attributes](#attributes)
- [String Representation](#string-representation)
- [Step Results](#step-results)
- [Usage Examples](#usage-examples)
- [Integration with Recipe Runner](#integration-with-recipe-runner)

---

## Class Overview

`RecipeResult` is the structured output of a recipe run. It is returned by the `amplihack recipe run` CLI command when `--output json` is used. The JSON schema uses the same field names documented below.

---

## Attributes

| Attribute          | Type               | Description                                                               |
| ------------------ | ------------------ | ------------------------------------------------------------------------- |
| `recipe_name`      | `str`              | The name of the recipe that was run (e.g. `"default-workflow"`).          |
| `status`           | `RecipeStatus`     | Overall outcome: `SUCCESS`, `FAILURE`, or `PARTIAL`.                      |
| `step_results`     | `list[StepResult]` | Ordered list of results, one per step that executed.                      |
| `duration_seconds` | `float`            | Wall-clock time from recipe start to finish.                              |
| `error`            | `str \| None`      | Human-readable error message if `status` is `FAILURE`. `None` on success. |

---

## String Representation

`str(result)` returns a one-line summary suitable for logging:

```
RecipeResult(<recipe-name>: <STATUS>, <N> steps)
```

**Examples:**

```python
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

```python
@dataclass
class StepResult:
    step_id: str          # e.g. "step-1", "step-2"
    step_name: str        # Human-readable step name from recipe YAML
    status: StepStatus    # SUCCESS, FAILURE, SKIPPED
    output: str | None    # Agent output or None if skipped
    duration_seconds: float
```

Accessing step results directly:

```python
result = runner.run("default-workflow", context)

for step in result.step_results:
    print(f"{step.step_id}: {step.status.value} ({step.duration_seconds:.1f}s)")

# step-1: SUCCESS (2.3s)
# step-2: SUCCESS (8.1s)
# step-3: FAILURE (1.0s)
```

---

## Usage Examples

### Check overall status

```bash
amplihack recipe run default-workflow \
  -c task_description="add input validation" \
  --verbose --output json | jq '.status'
# "SUCCESS"
```

### Count successful steps

```bash
amplihack recipe run default-workflow \
  -c task_description="add input validation" \
  --output json | jq '[.step_results[] | select(.status == "SUCCESS")] | length'
```

### Serialise to JSON

The `--output json` flag produces the full `RecipeResult` as JSON:

```bash
amplihack recipe run default-workflow \
  -c task_description="add input validation" \
  --output json | jq .
```

### Log a one-line summary

Without `--output json`, the CLI prints a one-line summary to stderr:

```
RecipeResult(default-workflow: SUCCESS, 22 steps)
```

---

## Integration with Recipe Runner

`RecipeResult` is what the CLI surfaces when `--output json` is requested:

```bash
amplihack recipe run default-workflow \
  --context '{"task_description": "add login endpoint"}' \
  --output json | jq '.status'
# "SUCCESS"
```

The JSON schema matches the fields documented in [Attributes](#attributes) with the `status` field serialised as a string value (`"SUCCESS"`, `"FAILURE"`, `"PARTIAL"`).

---

## See Also

- [Recipe CLI Reference](./recipe-cli-reference.md) — `amplihack recipe run` and `--output json`
- [Recipe CLI Examples](../howto/recipe-cli-examples.md) — real-world workflow scenarios
- [Recipe Resilience](../concepts/recipe-resilience.md) — how partial failures and retries are handled
