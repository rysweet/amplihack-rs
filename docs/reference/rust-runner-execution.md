# Rust Runner Execution Reference

`recipe-runner-rs` executes recipes. `amplihack recipe run` is the supported
entry point that resolves the runner binary, passes context, and formats the
final result.

## Contents

- [Execution contract](#execution-contract)
- [Stream contract](#stream-contract)
- [Run identity and pointers](#run-identity-and-pointers)
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
4. Generates a stable recipe run UUID and injects it as
   `AMPLIHACK_RECIPE_RUN_ID`.
5. Emits early and final `amplihack.recipe.log_pointer` lines to stderr.
6. Preserves the runner's stderr progress stream.
7. Writes the final result to stdout in the requested `table`, `json`, or `yaml`
   format.

## Stream contract

| Stream | Contents | Consumer |
| --- | --- | --- |
| `stderr` | Human-readable lifecycle progress, heartbeats, and failure diagnostics | Humans and CI logs |
| `stdout` | Final result in the selected `--format` | Scripts and pipelines |

Progress is emitted by default. `--verbose` may add child launch details and
expanded diagnostics, but basic progress does not require it.

`amplihack recipe run` does not support a `--progress` flag. Passing it fails
with an actionable message explaining that progress is already on stderr by
default.

## Run identity and pointers

The wrapper owns the top-level run identity. It generates one UUID per
`amplihack recipe run` invocation and passes that UUID to the child process:

```text
AMPLIHACK_RECIPE_RUN_ID=5b60657b-76ef-4f49-8a22-8b89ed75f43e
```

The wrapper also writes two pointer events to stderr:

| Event | Timing | Purpose |
| --- | --- | --- |
| `early` | Before child spawn | Gives users and CI a run ID before the recipe finishes or fails. |
| `final` | On success, nonzero exit, spawn failure, or parse failure | Records terminal status plus child PID, exit code, runner path, and known log paths. |

Pointer lines use this prefix:

```text
amplihack.recipe.log_pointer {"schema_version":1,"event":"final","run_id":"5b60657b-76ef-4f49-8a22-8b89ed75f43e","status":"success"}
```

See [Recipe Run Correlation Reference](./recipe-run-correlation.md) for the full
schema and metadata rules.

## JSONL events

Set `AMPLIHACK_RECIPE_LOG_JSONL` to write structured events to a file:

```bash
AMPLIHACK_RECIPE_LOG_JSONL=/tmp/default-workflow.jsonl \
amplihack recipe run default-workflow \
  -c task_description="Add retry metrics"
```

The runner reads `AMPLIHACK_RECIPE_RUN_ID` from its environment and copies the
value to JSONL event `run_id` fields. The wrapper supplies the environment value
but does not post-process the JSONL file.

Lifecycle event:

```json
{
  "type": "step_lifecycle",
  "run_id": "5b60657b-76ef-4f49-8a22-8b89ed75f43e",
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
  "run_id": "5b60657b-76ef-4f49-8a22-8b89ed75f43e",
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
  "run_id": "5b60657b-76ef-4f49-8a22-8b89ed75f43e",
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
| `AMPLIHACK_RECIPE_RUN_ID` | Stable UUID injected by the wrapper into the child process and copied by the runner into JSONL event `run_id` fields. |
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
| Pointer metadata | Pointer events include known context and local paths for correlation, but never env dumps, token values, stdout bodies, stderr bodies, or log contents. |

## Related

- [RecipeResult Reference](./recipe-result.md)
- [Recipe Run Correlation Reference](./recipe-run-correlation.md)
- [Recipe CLI Reference](./recipe-cli-reference.md)
- [Recipe Runner Logging Reference](./recipe-runner-logging.md)
- [Rust Runner Execution Architecture](../concepts/rust-runner-execution-architecture.md)
