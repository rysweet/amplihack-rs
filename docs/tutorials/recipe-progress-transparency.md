# Observe Recipe Progress and Debug a Failed Step

Learn how to run a recipe, watch live progress, capture the structured result,
and use bounded output snippets to diagnose a failed step.

**Status:** Planned tutorial for the finished recipe-runner transparency
feature.

## What you will do

In this tutorial you will:

1. Run `default-workflow` with live progress visible in the terminal.
2. Save the final JSON result without mixing it with progress output.
3. Read heartbeat lines during a long-running agent step.
4. Inspect recent stdout/stderr snippets when a step fails.
5. Tune heartbeat and snippet limits for a single run.

## Before you start

You need:

- `amplihack` installed
- a repository checkout
- `jq` for the JSON examples

## Run a recipe and watch progress

From a repository checkout:

```sh
cd /home/user/src/myapp

amplihack recipe run default-workflow \
  -c task_description="Add validation for empty display names" \
  -c repo_path=.
```

The terminal shows lifecycle progress on `stderr` as each step starts, runs, and
finishes:

```text
[recipe default-workflow] started (23 steps)
[step 01/23 requirements-clarification] started agent=prompt-writer
[step 01/23 requirements-clarification] completed elapsed=24s
[step 02/23 implementation-planning] started agent=architect
```

Short steps may only show `started` and `completed`. Long-running steps also
emit heartbeat lines.

## Capture JSON without losing progress

Progress is written to `stderr`; final structured output is written to `stdout`.
That means you can redirect the final result while still watching progress:

```sh
amplihack recipe run default-workflow \
  -c task_description="Add validation for empty display names" \
  -c repo_path=. \
  --format json > /tmp/default-workflow-result.json
```

While the recipe runs, the terminal still shows progress. After it finishes,
inspect the final result:

```sh
jq '{recipe_name, success, duration_seconds}' /tmp/default-workflow-result.json
```

Example output:

```json
{
  "recipe_name": "default-workflow",
  "success": true,
  "duration_seconds": 842.3
}
```

## Recognize a healthy long-running step

When an agent step takes longer than the heartbeat interval, the runner prints a
bounded status line:

```text
[step 08/23 code-implementation] heartbeat elapsed=180s status=running phase=agent agent=builder
```

This means the runner is still alive, the active step is known, and the child
agent has not exited. Heartbeats are rate-limited per active step, so the log
does not flood during long workflows.

## Inspect a failed step

If a step fails, the terminal shows the step identity, elapsed time, child
identity, and recent output snippets:

```text
[step 08/23 code-implementation] failed elapsed=12m14s agent=builder
error: agent exited with code 1

recent stderr from agent:builder (last 20 lines, 8192 bytes max):
  error[E0425]: cannot find value `cache_policy` in this scope
  --> src/http/cache.rs:42:18
```

The same context is available in the JSON result:

```sh
jq '.step_results[]
  | select(.status == "failed")
  | {step_id, step_name, elapsed_seconds, child, recent_output}' \
  /tmp/default-workflow-result.json
```

Use the `source` and `stream` fields to tell whether the snippet came from an
agent, a shell subprocess, or a nested recipe:

```json
{
  "step_id": "code-implementation",
  "step_name": "Code implementation",
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

## Tune diagnostics for one run

Use environment variables when you need different heartbeat or snippet limits
for one command.

Increase heartbeat frequency while debugging:

```sh
AMPLIHACK_RECIPE_HEARTBEAT_INTERVAL_SECONDS=15 \
amplihack recipe run default-workflow \
  -c task_description="Debug hanging test generation" \
  -c repo_path=.
```

Keep larger snippets for a noisy compiler failure:

```sh
AMPLIHACK_RECIPE_SNIPPET_LINES=60 \
AMPLIHACK_RECIPE_SNIPPET_BYTES=32768 \
amplihack recipe run default-workflow \
  -c task_description="Fix build failures after dependency update" \
  -c repo_path=. \
  --format json > /tmp/failure.json
```

## Write a JSONL event log

Set `AMPLIHACK_RECIPE_LOG_JSONL` when a CI job or monitoring tool needs a
stream of structured events:

```sh
AMPLIHACK_RECIPE_LOG_JSONL=/tmp/default-workflow.jsonl \
amplihack recipe run default-workflow \
  -c task_description="Add retry budget metrics" \
  -c repo_path=.
```

View heartbeat events:

```sh
jq 'select(.type == "heartbeat")' /tmp/default-workflow.jsonl
```

View output snippets:

```sh
jq 'select(.type == "output_snippet") | {step_id, source, stream, recent_output}' \
  /tmp/default-workflow.jsonl
```

## What you learned

Recipe runs are observable by default. Live progress stays on `stderr`, final
structured output stays on `stdout`, long-running steps emit rate-limited
heartbeats, and failures include bounded recent output from the active child
process.

## Related

- [Recipe Runner Logging Reference](../reference/recipe-runner-logging.md)
- [Run a Recipe End-to-End](../howto/run-a-recipe.md)
- [Troubleshoot Recipe Execution Failures](../howto/troubleshoot-recipe-execution.md)
