# Recipe Runner Logging Reference

Reference for live recipe progress, heartbeat events, bounded subprocess output
snippets, run correlation pointers, and additive JSON fields emitted by
`amplihack recipe run`.

## Contents

- [Output contract](#output-contract)
- [Run identity and log pointers](#run-identity-and-log-pointers)
- [Live progress lines](#live-progress-lines)
- [Heartbeat events](#heartbeat-events)
- [Recent output snippets](#recent-output-snippets)
- [Failure context](#failure-context)
- [JSON result fields](#json-result-fields)
- [JSONL log events](#jsonl-log-events)
- [Configuration](#configuration)
- [Examples](#examples)

## Output contract

`amplihack recipe run` keeps human-readable progress and machine-readable final
results on separate streams.

| Stream | Contents | Intended consumer |
| --- | --- | --- |
| `stderr` | Live recipe progress, step lifecycle lines, heartbeats, failure diagnostics, bounded snippets | Humans, CI logs, terminal UIs |
| `stdout` | Final command output in the requested `--format` (`table`, `json`, or `yaml`) | Shell pipelines, scripts, `jq` |

Default runs emit progress to `stderr` without requiring `--verbose`.
`--verbose` increases detail; it is not required for basic progress.

```sh
amplihack recipe run default-workflow \
  -c task_description="Add cache headers" \
  -c repo_path=. \
  --format json > result.json
```

The terminal still shows live progress because progress is written to `stderr`,
while `result.json` contains only the final JSON result from `stdout`.

## Run identity and log pointers

Every run gets a stable UUID before the runner child starts. The wrapper exposes
that UUID as `AMPLIHACK_RECIPE_RUN_ID` in the child environment and emits two
correlation pointer lines to `stderr`.

Early pointer:

```text
amplihack.recipe.log_pointer {"schema_version":1,"event":"early","run_id":"5b60657b-76ef-4f49-8a22-8b89ed75f43e","recipe_name":"default-workflow","cwd":"/home/user/src/myapp","worktree":"/home/user/src/myapp","branch":"main","task_description":"Add cache headers","runner_path":"/home/user/.local/bin/recipe-runner-rs","timestamp":"2026-06-12T03:55:11Z"}
```

Final pointer:

```text
amplihack.recipe.log_pointer {"schema_version":1,"event":"final","run_id":"5b60657b-76ef-4f49-8a22-8b89ed75f43e","recipe_name":"default-workflow","status":"success","worktree":"/home/user/src/myapp","branch":"main","child_pid":41822,"exit_code":0,"runner_path":"/home/user/.local/bin/recipe-runner-rs","log_paths":{"jsonl":"/tmp/default-workflow.jsonl"},"timestamp":"2026-06-12T04:21:39Z"}
```

Pointer events include only explicit known metadata. Missing branch, issue, PR,
child PID, and log path fields are omitted. See
[Recipe Run Correlation Reference](./recipe-run-correlation.md) for the complete
schema.

## Live progress lines

Every step emits lifecycle progress in real time.

```text
[recipe default-workflow] started (23 steps)
[step 04/23 requirements-clarification] started agent=prompt-writer
[step 04/23 requirements-clarification] heartbeat elapsed=60s status=running phase=agent
[step 04/23 requirements-clarification] completed elapsed=91s
[step 05/23 implementation-planning] started agent=architect
[step 05/23 implementation-planning] failed elapsed=34s error="agent exited with code 1"
```

Progress lines include:

| Field | Description |
| --- | --- |
| Recipe name | The recipe being executed, such as `default-workflow` |
| Step index | Current 1-based step number and total step count when known |
| Step id/name | Stable step identifier from the recipe YAML |
| Phase | `recipe`, `agent`, `subprocess`, `bash`, or `finalize` |
| Status | `started`, `running`, `completed`, `failed`, or `skipped` |
| Elapsed time | Wall-clock time since the step or recipe started |
| Child identity | Agent name, subprocess command label, PID, or nested recipe name when available |

## Heartbeat events

Long-running recipe, agent, subprocess, and nested-recipe steps emit periodic
heartbeats so callers can distinguish active work from a hung process.

Default heartbeat behavior:

| Setting | Default | Description |
| --- | --- | --- |
| First heartbeat | After one interval | Avoids noise for short steps |
| Interval | 60 seconds | Rate-limited per active step |
| Stream | `stderr` | Human-readable line plus structured log event |
| Contents | Step id/name, phase, status, elapsed time, child identity when known | Enough context to know what is still running |

Heartbeat lines never include full child output. They may include a short
progress hint such as the active agent name or subprocess label.

## Recent output snippets

The runner keeps bounded rolling buffers for each active subprocess, agent, and
nested recipe. Snippets are attributed by source and stream.

| Attribute | Behavior |
| --- | --- |
| Source | `agent:<name>`, `subprocess:<pid>`, `recipe:<name>`, or `step:<id>` |
| Stream | `stdout`, `stderr`, or `combined` |
| Bound | Limited by line count and byte count |
| Retention | Recent output only; older lines are dropped |
| Redaction | No additional redaction guarantee; snippets are bounded but may contain whatever the child printed |
| Live output | Snippets are summarized in progress logs; noisy child output is not streamed unbounded |

Do not print secrets from recipe steps. The transparency feature limits snippet
size and attribution, but it is not a substitute for secret-safe child tools.

Snippets appear in:

- step failure diagnostics printed to `stderr`
- JSON result fields when `--format json` is used
- JSONL recipe log events when recipe logging is enabled

## Failure context

Failures include actionable context instead of only a generic step failure.

```text
[step 08/23 code-implementation] failed elapsed=12m14s agent=builder
error: agent exited with code 1

recent stderr from agent:builder (last 20 lines, 8192 bytes max):
  error[E0425]: cannot find value `cache_policy` in this scope
  --> src/http/cache.rs:42:18
   |
42 |     apply_policy(cache_policy);
   |                  ^^^^^^^^^^^^ not found in this scope

recent stdout from agent:builder:
  Running cargo test --quiet
```

Failure diagnostics include these fields when known:

| Field | Description |
| --- | --- |
| `step_id` | Stable recipe step id |
| `step_name` | Human-readable step name or label |
| `phase` | Current phase at failure time |
| `status` | `failed` |
| `elapsed_seconds` | Step duration |
| `child` | Agent, subprocess, or nested recipe identity |
| `exit_code` | Child process exit code when applicable |
| `recent_output` | Bounded attributed stdout/stderr snippets |

## JSON result fields

The final JSON result remains backward-compatible. Existing fields are preserved;
new fields are optional and additive.

Top-level result:

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `recipe_name` | string | yes | Recipe name |
| `success` | boolean | yes, when used by the existing result schema | Boolean overall result |
| `status` | string | yes, when used by the existing result schema | Overall result such as `SUCCESS`, `FAILURE`, or `PARTIAL` |
| `step_results` | array | yes | Ordered step results |
| `context` | object | no | Final context values |
| `duration_seconds` | number | no | Total recipe duration |
| `progress_summary` | object | no | Last known phase/status, heartbeat count, and log path |
| `failure_context` | object | no | Failure context for the first failing step |
| `run_id` | string | no | Stable UUID for this recipe invocation |
| `log_pointer` | object | no | Concise final pointer summary with status, child PID, runner path, worktree, branch, and log paths |

Step result additive fields:

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `step_id` | string | yes | Stable step id |
| `status` | string | yes | `completed`, `failed`, or `skipped` |
| `output` | string | no | Step output or summary |
| `error` | string | no | Error message |
| `step_name` | string | no | Human-readable name |
| `phase` | string | no | Last phase for the step |
| `elapsed_seconds` | number | no | Step duration |
| `started_at` | string | no | RFC 3339 timestamp |
| `completed_at` | string | no | RFC 3339 timestamp |
| `child` | object | no | Agent, subprocess, or nested recipe identity |
| `last_heartbeat_at` | string | no | RFC 3339 timestamp of last heartbeat |
| `recent_output` | array | no | Bounded attributed snippets |

`child` object:

```json
{
  "kind": "agent",
  "name": "builder",
  "pid": 12345,
  "command": "copilot --agent builder"
}
```

`recent_output` entry:

```json
{
  "source": "agent:builder",
  "stream": "stderr",
  "line_count": 20,
  "byte_count": 4096,
  "truncated": true,
  "text": "error[E0425]: cannot find value `cache_policy` in this scope\n..."
}
```

Consumers should ignore unknown fields and treat every field in this section as
optional unless listed as required in the existing schema. The `amplihack` CLI
must preserve additive fields emitted by `recipe-runner-rs` when it formats
`--format json` or `--format yaml`; parsing the runner JSON and then dropping
unknown diagnostic fields is a bug.

## JSONL log events

When recipe logging is enabled, the log file contains JSONL events with enough
context to reconstruct progress.

The runner reads `AMPLIHACK_RECIPE_RUN_ID` from its environment and writes that
same value to event `run_id` fields. The `amplihack recipe run` wrapper supplies
the environment value but does not rewrite JSONL files after the child exits.

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
  "step_name": "Code implementation",
  "phase": "agent",
  "status": "running",
  "child": { "kind": "agent", "name": "builder" },
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
    "line_count": 12,
    "byte_count": 2048,
    "truncated": false,
    "text": "Running cargo test --quiet\n..."
  },
  "timestamp": "2026-06-03T18:15:30Z"
}
```

## Configuration

Defaults are conservative and bounded. Configure them through environment
variables or the matching keys in `~/.amplihack/config`.

| Environment variable | Config key | Default | Description |
| --- | --- | --- | --- |
| `AMPLIHACK_RECIPE_HEARTBEAT_INTERVAL_SECONDS` | `recipe.heartbeat_interval_seconds` | `60` | Seconds between heartbeat lines per active step. Set `0` to disable heartbeats. |
| `AMPLIHACK_RECIPE_SNIPPET_LINES` | `recipe.snippet_lines` | `20` | Maximum recent lines retained per source/stream. |
| `AMPLIHACK_RECIPE_SNIPPET_BYTES` | `recipe.snippet_bytes` | `8192` | Maximum bytes retained per source/stream. |
| `AMPLIHACK_RECIPE_LOG_JSONL` | `recipe.log_jsonl` | unset | Optional path for structured JSONL recipe events. |
| `AMPLIHACK_RECIPE_RUN_ID` | none | generated | Stable run UUID injected by the wrapper into the child environment and copied by the runner into JSONL event `run_id` fields. Treat as read-only. |

Environment variables take precedence over config-file keys.

Use `--verbose` to include child launch details and additional snippet context:

```sh
amplihack recipe run default-workflow \
  -c task_description="Update dependencies" \
  --verbose
```

`--progress` is not a supported `amplihack recipe run` flag. Passing it fails
with an actionable message explaining that progress is already emitted to
`stderr` by default and that `--verbose` only increases diagnostic detail.

## Examples

Capture final JSON while watching progress:

```sh
amplihack recipe run default-workflow \
  -c task_description="Add input validation" \
  -c repo_path=. \
  --format json > /tmp/recipe-result.json
```

Extract pointer events from a captured progress log:

```sh
grep '^amplihack\.recipe\.log_pointer ' /tmp/progress.log \
  | sed 's/^amplihack\.recipe\.log_pointer //' \
  | jq '{event, run_id, status, child_pid, exit_code, log_paths}'
```

Write a JSONL event log:

```sh
AMPLIHACK_RECIPE_LOG_JSONL=/tmp/default-workflow.jsonl \
amplihack recipe run default-workflow \
  -c task_description="Fix flaky retry tests" \
  -c repo_path=.
```

Show the failing step and recent stderr from the final JSON:

```sh
jq '.step_results[]
  | select(.status == "failed")
  | {step_id, elapsed_seconds, child, recent_output}' /tmp/recipe-result.json
```

## Related

- [Run a Recipe End-to-End](../howto/run-a-recipe.md) - Usage guide
- [Correlate Recipe Runs with Logs](../howto/correlate-recipe-runs.md) - Run ID workflow
- [Recipe Run Correlation Reference](./recipe-run-correlation.md) - Pointer schema and final result fields
- [RecipeResult Reference](./recipe-result.md) - Final result schema
- [amplihack recipe Reference](./recipe-command.md) - Command-line flags
- [Recipe Runner Architecture](../concepts/recipe-runner-architecture.md) - CLI and runner responsibility split
