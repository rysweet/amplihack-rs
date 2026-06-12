# How to Correlate Recipe Runs with Logs

Use this guide when a recipe run produces output in multiple places and you need
to connect the terminal log, final JSON result, child process, and runner logs.

## Before you start

You need:

- `amplihack` installed
- a recipe to run
- `jq` for the JSON examples

## Capture progress and final output

Run the recipe with final JSON on stdout and progress on stderr:

```bash
cd /home/user/src/myapp

AMPLIHACK_RECIPE_LOG_JSONL=/tmp/default-workflow.jsonl \
amplihack recipe run default-workflow \
  -c task_description="Fix flaky retry tests" \
  -c repo_path=. \
  -c issue_number=753 \
  --format json > /tmp/default-workflow-result.json 2> /tmp/default-workflow-progress.log
```

The command writes:

| File | Contents |
| --- | --- |
| `/tmp/default-workflow-result.json` | Final recipe result with `run_id` and `log_pointer`. |
| `/tmp/default-workflow-progress.log` | Live progress, early pointer, final pointer, heartbeats, and diagnostics. |
| `/tmp/default-workflow.jsonl` | Structured runner events when `AMPLIHACK_RECIPE_LOG_JSONL` is set. |

## Read the run ID

The final result contains the stable run identity:

```bash
jq -r '.run_id' /tmp/default-workflow-result.json
```

Example:

```text
5b60657b-76ef-4f49-8a22-8b89ed75f43e
```

Use this value as the primary correlation key.

If the wrapper cannot produce a final JSON/YAML result, read the run ID from the
stderr pointer lines instead. This is required for `spawn_failure` and
`parse_failure` terminal paths because stdout may be empty or invalid:

```bash
grep '^amplihack\.recipe\.log_pointer ' /tmp/default-workflow-progress.log \
  | sed 's/^amplihack\.recipe\.log_pointer //' \
  | jq -r 'select(.event == "final") | .run_id'
```

## Read the early and final pointers

Pointer lines are written to stderr with the
`amplihack.recipe.log_pointer ` prefix.

```bash
grep '^amplihack\.recipe\.log_pointer ' /tmp/default-workflow-progress.log \
  | sed 's/^amplihack\.recipe\.log_pointer //' \
  | jq .
```

Expected shape:

```json
{
  "schema_version": 1,
  "event": "final",
  "run_id": "5b60657b-76ef-4f49-8a22-8b89ed75f43e",
  "recipe_name": "default-workflow",
  "status": "success",
  "worktree": "/home/user/src/myapp",
  "branch": "main",
  "issue_number": "753",
  "child_pid": 41822,
  "exit_code": 0,
  "runner_path": "/home/user/.local/bin/recipe-runner-rs",
  "log_paths": {
    "jsonl": "/tmp/default-workflow.jsonl"
  },
  "timestamp": "2026-06-12T04:21:39Z"
}
```

There are exactly two wrapper pointer events per invocation: one `early` event
and one `final` event. If a run fails before the child starts, the final event
uses `status=spawn_failure` and omits `child_pid`.

## Match terminal logs to JSON output

Compare the run ID in the final JSON with the IDs in the stderr pointer lines:

```bash
result_run_id=$(jq -r '.run_id' /tmp/default-workflow-result.json)

pointer_run_ids=$(
  grep '^amplihack\.recipe\.log_pointer ' /tmp/default-workflow-progress.log \
    | sed 's/^amplihack\.recipe\.log_pointer //' \
    | jq -r '.run_id' \
    | sort -u
)

test "$result_run_id" = "$pointer_run_ids"
```

The `test` command exits 0 when the captured stdout result and stderr progress
belong to the same recipe invocation.

## Find the child process and runner executable

Read the final pointer summary from the result:

```bash
jq '.log_pointer | {run_id, status, child_pid, exit_code, runner_path, log_paths}' \
  /tmp/default-workflow-result.json
```

Example:

```json
{
  "run_id": "5b60657b-76ef-4f49-8a22-8b89ed75f43e",
  "status": "success",
  "child_pid": 41822,
  "exit_code": 0,
  "runner_path": "/home/user/.local/bin/recipe-runner-rs",
  "log_paths": {
    "jsonl": "/tmp/default-workflow.jsonl"
  }
}
```

Use `child_pid` to correlate with process-level telemetry captured while the run
was active. Use `runner_path` to confirm which runner binary executed the recipe.

## Correlate with JSONL runner events

When `AMPLIHACK_RECIPE_LOG_JSONL` is set, the pointer final event includes the
path in `log_paths.jsonl`. The runner reads `AMPLIHACK_RECIPE_RUN_ID` from its
environment and writes the same value to JSONL event `run_id` fields; the wrapper
does not rewrite the JSONL file.

```bash
jsonl_path=$(jq -r '.log_pointer.log_paths.jsonl' /tmp/default-workflow-result.json)
run_id=$(jq -r '.run_id' /tmp/default-workflow-result.json)
jq --arg run_id "$run_id" 'select(.run_id == $run_id)' "$jsonl_path"
```

## Diagnose terminal statuses

The final pointer reports the wrapper's terminal status:

| Status | What to inspect |
| --- | --- |
| `success` | Final JSON/YAML result and any JSONL log path. |
| `failure` | `exit_code`, failing step data, bounded recent output, and JSONL events. |
| `spawn_failure` | `runner_path`, installation state, and `RECIPE_RUNNER_RS_PATH`. |
| `parse_failure` | Child `exit_code`, stderr diagnostics, and whether stdout was truncated or not JSON. |

Pointer statuses are lowercase. Top-level result statuses may be uppercase
(`SUCCESS`, `FAILURE`, `PARTIAL`) or represented by the boolean `success` field,
depending on the result producer.

For failing recipe steps, inspect the failure context:

```bash
jq '.failure_context // (.step_results[] | select(.status == "failed"))' \
  /tmp/default-workflow-result.json
```

## Related

- [Recipe Run Correlation Reference](../reference/recipe-run-correlation.md)
- [Trace a Recipe Run Across Terminal and JSON Logs](../tutorials/recipe-run-correlation.md)
- [Recipe Runner Logging Reference](../reference/recipe-runner-logging.md)
- [Troubleshoot Recipe Execution Failures](./troubleshoot-recipe-execution.md)
