# Rust Runner Execution Reference

`recipe-runner-rs` executes recipes. `amplihack recipe run` is the supported
entry point that resolves the runner binary, passes context, and formats the
final result.

**Status:** Planned finished-state contract for recipe-runner transparency.

## Contents

- [Execution contract](#execution-contract)
- [Stream contract](#stream-contract)
- [JSONL events](#jsonl-events)
- [Environment variables](#environment-variables)
- [Security model](#security-model)
- [Related](#related)

## Execution contract

Use the wrapper CLI:

```bash
amplihack recipe run default-workflow \
  -c task_description="Add cache headers" \
  -c repo_path=. \
  --format json
```

The wrapper:

1. Resolves `recipe-runner-rs`.
2. Resolves recipe names and recipe YAML paths.
3. Passes context as runner `--set key=value` values or a context file when
   arguments would be too large.
4. Preserves the runner's stderr progress stream.
5. Writes the final result to stdout in the requested `table`, `json`, or `yaml`
   format.

## Stream contract

| Stream | Contents | Consumer |
| --- | --- | --- |
| `stderr` | Human-readable lifecycle progress, heartbeats, and failure diagnostics | Humans and CI logs |
| `stdout` | Final result in the selected `--format` | Scripts and pipelines |

Progress is emitted by default. `--verbose` may add child launch details and
expanded diagnostics, but basic progress does not require it.

`amplihack recipe run` does not support a `--progress` flag. Passing it should
fail with an actionable message explaining that progress is already on stderr by
default.

## JSONL events

Set `AMPLIHACK_RECIPE_LOG_JSONL` to write structured events to a file:

```bash
AMPLIHACK_RECIPE_LOG_JSONL=/tmp/default-workflow.jsonl \
amplihack recipe run default-workflow \
  -c task_description="Add retry metrics"
```

Lifecycle event:

```json
{
  "type": "step_lifecycle",
  "recipe_name": "default-workflow",
  "step_index": 8,
  "total_steps": 23,
  "step_id": "code-implementation",
  "step_name": "Code implementation",
  "phase": "agent",
  "status": "started",
  "child": { "kind": "agent", "name": "builder" },
  "elapsed_seconds": 0.0,
  "timestamp": "2026-06-03T18:12:00Z"
}
```

Heartbeat event:

```json
{
  "type": "heartbeat",
  "recipe_name": "default-workflow",
  "step_id": "code-implementation",
  "phase": "agent",
  "status": "running",
  "elapsed_seconds": 180.0,
  "timestamp": "2026-06-03T18:15:00Z"
}
```

Output snippet event:

```json
{
  "type": "output_snippet",
  "recipe_name": "default-workflow",
  "step_id": "code-implementation",
  "source": "agent:builder",
  "stream": "stderr",
  "recent_output": {
    "line_count": 20,
    "byte_count": 4096,
    "truncated": true,
    "text": "error[E0425]: cannot find value `cache_policy` in this scope\n..."
  },
  "timestamp": "2026-06-03T18:15:30Z"
}
```

Machine-readable events belong in the JSONL file. The user-visible stderr stream
uses concise text lines for lifecycle progress and diagnostics.

## Environment variables

| Variable | Description |
| --- | --- |
| `RECIPE_RUNNER_RS_PATH` | Optional path to the runner binary. |
| `AMPLIHACK_STEP_TIMEOUT` | Global per-step timeout hint set by `--step-timeout`. |
| `AMPLIHACK_RECIPE_HEARTBEAT_INTERVAL_SECONDS` | Heartbeat interval in seconds; `0` disables heartbeat lines. |
| `AMPLIHACK_RECIPE_SNIPPET_LINES` | Maximum recent output lines retained per child source/stream. |
| `AMPLIHACK_RECIPE_SNIPPET_BYTES` | Maximum recent output bytes retained per child source/stream. |
| `AMPLIHACK_RECIPE_LOG_JSONL` | Optional path for structured JSONL events. |

## Security model

| Property | Requirement |
| --- | --- |
| No wrapper shell interpolation | The wrapper launches `recipe-runner-rs` with argv values, not a shell string. Recipe `bash` steps may still execute shell code because that is their explicit step type. |
| Bounded output capture | Recent output snippets are capped by line and byte limits before reaching stderr, JSON results, or JSONL logs. |
| Secret handling | This feature does not add a new redaction guarantee. Child tools and recipes must avoid printing secrets. |
| Additive JSON compatibility | The wrapper must preserve optional diagnostic fields emitted by the runner when formatting JSON or YAML. |
| Visible failure | Runner launch, parse, and step failures must surface actionable stderr context instead of silent success. |

## Related

- [RecipeResult Reference](./recipe-result.md)
- [Recipe CLI Reference](./recipe-cli-reference.md)
- [Recipe Runner Logging Reference](./recipe-runner-logging.md)
- [Rust Runner Execution Architecture](../concepts/rust-runner-execution-architecture.md)
