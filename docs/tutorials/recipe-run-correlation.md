# Trace a Recipe Run Across Terminal and JSON Logs

Learn how to use the recipe run ID and log pointer events to connect a terminal
session, final result file, child process, and structured runner log.

## What you will do

1. Run a recipe with stderr and stdout captured separately.
2. Extract the early and final log pointer events.
3. Confirm that the final result and progress log share one run ID.
4. Follow the pointer to the JSONL runner log.
5. Interpret success and failure statuses.

## Before you start

You need:

- `amplihack` installed
- a repository checkout
- `jq`

## Step 1: Run the recipe

From a checkout, run a recipe with a JSONL log path:

```bash
cd /home/user/src/myapp

AMPLIHACK_RECIPE_LOG_JSONL=/tmp/recipe-run.jsonl \
amplihack recipe run default-workflow \
  -c task_description="Add validation for empty display names" \
  -c repo_path=. \
  -c issue_number=753 \
  --format json > /tmp/recipe-result.json 2> /tmp/recipe-progress.log
```

Progress goes to `/tmp/recipe-progress.log`. The final result goes to
`/tmp/recipe-result.json`.

## Step 2: Extract pointer events

Filter the progress log for pointer lines:

```bash
grep '^amplihack\.recipe\.log_pointer ' /tmp/recipe-progress.log \
  | sed 's/^amplihack\.recipe\.log_pointer //' \
  | jq '{event, run_id, recipe_name, status, branch, child_pid, exit_code, log_paths}'
```

Expected output contains one early pointer and one final pointer:

```json
{
  "event": "early",
  "run_id": "5b60657b-76ef-4f49-8a22-8b89ed75f43e",
  "recipe_name": "default-workflow",
  "status": null,
  "branch": "main",
  "child_pid": null,
  "exit_code": null,
  "log_paths": null
}
{
  "event": "final",
  "run_id": "5b60657b-76ef-4f49-8a22-8b89ed75f43e",
  "recipe_name": "default-workflow",
  "status": "success",
  "branch": "main",
  "child_pid": 41822,
  "exit_code": 0,
  "log_paths": {
    "jsonl": "/tmp/recipe-run.jsonl"
  }
}
```

The early pointer exists so you can identify the run before the child process
finishes. The final pointer tells you how the wrapper terminated.

## Step 3: Confirm the final result matches

Read the result run ID:

```bash
jq -r '.run_id' /tmp/recipe-result.json
```

Compare it with the pointer run IDs:

```bash
jq -r '.run_id' /tmp/recipe-result.json

grep '^amplihack\.recipe\.log_pointer ' /tmp/recipe-progress.log \
  | sed 's/^amplihack\.recipe\.log_pointer //' \
  | jq -r '.run_id' \
  | sort -u
```

Both commands print the same UUID. That proves the stdout result and stderr
progress log came from the same `amplihack recipe run` invocation.

## Step 4: Follow the final pointer to runner logs

The final result includes a concise pointer summary:

```bash
jq '.log_pointer' /tmp/recipe-result.json
```

Example:

```json
{
  "run_id": "5b60657b-76ef-4f49-8a22-8b89ed75f43e",
  "recipe_name": "default-workflow",
  "status": "success",
  "worktree": "/home/user/src/myapp",
  "branch": "main",
  "child_pid": 41822,
  "exit_code": 0,
  "runner_path": "/home/user/.local/bin/recipe-runner-rs",
  "log_paths": {
    "jsonl": "/tmp/recipe-run.jsonl"
  }
}
```

Open the JSONL log named by `log_paths.jsonl`:

```bash
jsonl_path=$(jq -r '.log_pointer.log_paths.jsonl' /tmp/recipe-result.json)
jq 'select(.type == "heartbeat" or .type == "step_lifecycle")' "$jsonl_path"
```

`recipe-runner-rs` reads `AMPLIHACK_RECIPE_RUN_ID` and writes that value to
JSONL event `run_id` fields. Use the run ID, child PID, worktree, branch, and
timestamp window to align JSONL events with terminal logs and CI telemetry.

## Step 5: Interpret a failed run

For a failed recipe, the final pointer still exists:

```json
{
  "event": "final",
  "run_id": "5b60657b-76ef-4f49-8a22-8b89ed75f43e",
  "recipe_name": "default-workflow",
  "status": "failure",
  "child_pid": 41822,
  "exit_code": 1,
  "runner_path": "/home/user/.local/bin/recipe-runner-rs"
}
```

Then inspect the recipe failure context:

```bash
jq '.failure_context
  | {step_id, step_name, phase, elapsed_seconds, child, recent_output}' \
  /tmp/recipe-result.json
```

For wrapper failures, use the final status directly:

| Status | Meaning |
| --- | --- |
| `spawn_failure` | The wrapper did not start `recipe-runner-rs`; inspect `runner_path` and installation. |
| `parse_failure` | The child exited but the wrapper could not parse stdout; inspect stderr and child exit status. |
| `failure` | The child ran and reported a failed recipe or nonzero exit. |

`spawn_failure` and `parse_failure` may not produce a usable stdout result. In
those cases, extract the `run_id` and terminal status from the final stderr
pointer event.

## What you learned

Every recipe run has one stable UUID. The wrapper exposes it as
`AMPLIHACK_RECIPE_RUN_ID`, writes it to early and final stderr pointer lines, and
adds it to final JSON/YAML results when a result is available. The runner copies
that environment value into JSONL events. Use that UUID to connect terminal
output, final results, child process metadata, and runner log paths.

## Related

- [How to Correlate Recipe Runs with Logs](../howto/correlate-recipe-runs.md)
- [Recipe Run Correlation Reference](../reference/recipe-run-correlation.md)
- [Observe Recipe Progress and Debug a Failed Step](./recipe-progress-transparency.md)
