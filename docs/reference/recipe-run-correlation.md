# Recipe Run Correlation Reference

Reference for the stable run identity and log pointer contract used by
`amplihack recipe run`.

## Contents

- [Run identity](#run-identity)
- [stderr log pointer lines](#stderr-log-pointer-lines)
- [Pointer event schema](#pointer-event-schema)
- [Final result fields](#final-result-fields)
- [Configuration](#configuration)
- [Security and privacy](#security-and-privacy)
- [Examples](#examples)

## Run identity

Every `amplihack recipe run` invocation gets one UUID run identity. The value is
stable for the whole invocation and is exposed to the `recipe-runner-rs` child as
`AMPLIHACK_RECIPE_RUN_ID`.

```text
5b60657b-76ef-4f49-8a22-8b89ed75f43e
```

The run identity is a correlation handle only. It is not an authentication token,
authorization token, lock name, or persistence key.

The CLI generates the value before launching `recipe-runner-rs`. The wrapper
passes it to the runner as `AMPLIHACK_RECIPE_RUN_ID`; the runner side of this
contract copies that environment value into JSONL events when structured logging
is enabled. The wrapper does not rewrite JSONL files after the child exits.
Nested recipe steps, shell steps, agents, structured logs, terminal logs, and
final JSON output can all use the same value to correlate output from one recipe
run.

## stderr log pointer lines

`amplihack recipe run` emits exactly two wrapper-owned pointer lines to
`stderr`:

1. An `early` pointer before spawning `recipe-runner-rs`.
2. A `final` pointer when the wrapper reaches a terminal status.

Each line starts with the stable prefix `amplihack.recipe.log_pointer ` followed
by one JSON object:

```text
amplihack.recipe.log_pointer {"schema_version":1,"event":"early","run_id":"5b60657b-76ef-4f49-8a22-8b89ed75f43e","recipe_name":"default-workflow","cwd":"/home/user/src/myapp","worktree":"/home/user/src/myapp","branch":"main","task_description":"Fix flaky retry tests","issue_number":"753","runner_path":"/home/user/.local/bin/recipe-runner-rs","timestamp":"2026-06-12T03:55:11Z"}
```

The final pointer uses the same `run_id`:

```text
amplihack.recipe.log_pointer {"schema_version":1,"event":"final","run_id":"5b60657b-76ef-4f49-8a22-8b89ed75f43e","recipe_name":"default-workflow","status":"success","cwd":"/home/user/src/myapp","worktree":"/home/user/src/myapp","branch":"main","task_description":"Fix flaky retry tests","issue_number":"753","child_pid":41822,"exit_code":0,"runner_path":"/home/user/.local/bin/recipe-runner-rs","log_paths":{"jsonl":"/tmp/default-workflow.jsonl"},"timestamp":"2026-06-12T04:21:39Z"}
```

Use the prefix for fast log filtering and the JSON payload for structured
correlation.

## Pointer event schema

Pointer events are JSON objects serialized by the CLI wrapper. Unknown fields are
additive; consumers must ignore fields they do not understand.

| Field | Type | Allowed on | Required | Description |
| --- | --- | --- | --- | --- |
| `schema_version` | integer | early, final | yes | Pointer schema version. Current value is `1`. |
| `event` | string | early, final | yes | `early` or `final`. |
| `run_id` | string | early, final | yes | UUID for the recipe invocation. Matches `AMPLIHACK_RECIPE_RUN_ID`. |
| `recipe_name` | string | early, final | yes | Resolved recipe identity used to launch the runner: the YAML `name` when available, otherwise the CLI recipe argument. |
| `cwd` | string | early, final | yes | Effective execution directory after resolving `--working-dir`; this is not necessarily the parent process directory. |
| `worktree` | string | early, final | no | Git worktree root discovered from `cwd`; outside Git, this equals `cwd`. |
| `branch` | string | early, final | no | Current Git branch when available. Omitted outside Git or detached HEAD. |
| `task_description` | string | early, final | no | Explicit `task_description` context value, capped for log safety. |
| `issue_number` | string | early, final | no | Explicit issue number context value when supplied. |
| `issue_url` | string | early, final | no | Explicit issue URL context value when supplied. |
| `pr_number` | string | early, final | no | Explicit pull request number context value when supplied. |
| `pr_url` | string | early, final | no | Explicit pull request URL context value when supplied. |
| `work_item_id` | string | early, final | no | Explicit Azure Boards or external work item ID when supplied. |
| `work_item_url` | string | early, final | no | Explicit external work item URL when supplied. |
| `runner_path` | string | early, final | yes | `recipe-runner-rs` executable path selected by the wrapper. |
| `child_pid` | integer | final | no | Child `recipe-runner-rs` process ID when the child spawned. |
| `exit_code` | integer | final | no | Child exit code when available. |
| `status` | string | final | yes | Lowercase terminal wrapper status. See [Final statuses](#final-statuses). |
| `log_paths` | object | final | no | Known child log locations, such as `jsonl`. Omitted when no path is known. |
| `timestamp` | string | early, final | yes | RFC 3339 timestamp for the pointer event. |

### Final statuses

| Status | Meaning |
| --- | --- |
| `success` | The child exited successfully and the final result was formatted. |
| `failure` | The child spawned and exited nonzero. |
| `spawn_failure` | The wrapper could not spawn `recipe-runner-rs`. |
| `parse_failure` | The child exited but stdout could not be parsed as the expected recipe result. |

Pointer statuses are lowercase wrapper statuses. Top-level result statuses keep
the existing result-schema casing, commonly uppercase values such as `SUCCESS`,
`FAILURE`, or `PARTIAL`, or the boolean `success` field in older results.

### Metadata rules

The wrapper only logs explicit, known context:

- It copies supported issue, PR, work item, and task fields from context values.
- It does not infer issue or PR numbers from `task_description`.
- It omits unavailable fields instead of emitting empty strings.
- It caps display values at 1024 UTF-8 bytes; truncated values end with `...`
  within that byte limit.
- It never includes environment dumps, token values, stdout bodies, stderr bodies,
  or log file contents.

Supported context keys and aliases:

| Pointer field | Canonical key | Accepted aliases |
| --- | --- | --- |
| `task_description` | `task_description` | none |
| `issue_number` | `issue_number` | `issue`, `issue_id`, `github_issue_number` |
| `issue_url` | `issue_url` | `github_issue_url` |
| `pr_number` | `pr_number` | `pull_request_number`, `github_pr_number` |
| `pr_url` | `pr_url` | `pull_request_url`, `github_pr_url` |
| `work_item_id` | `work_item_id` | `work_item`, `ado_work_item_id` |
| `work_item_url` | `work_item_url` | `ado_work_item_url` |

## Final result fields

The final JSON/YAML result includes additive correlation fields when a result is
available:

```json
{
  "recipe_name": "default-workflow",
  "status": "SUCCESS",
  "run_id": "5b60657b-76ef-4f49-8a22-8b89ed75f43e",
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
  },
  "step_results": []
}
```

`run_id` is the easiest field for scripts to consume. `log_pointer` is a concise
summary of the final pointer event. It intentionally omits large or sensitive
fields from the full stderr event.

The default human output shows the run ID and a concise final pointer summary:

```text
Recipe: default-workflow
Run ID: 5b60657b-76ef-4f49-8a22-8b89ed75f43e
Status: ✓ Success
Log pointer: status=success branch=main worktree=/home/user/src/myapp child_pid=41822
```

## Configuration

No configuration is required to enable run correlation.

| Variable | Set by | Description |
| --- | --- | --- |
| `AMPLIHACK_RECIPE_RUN_ID` | `amplihack recipe run` | Stable UUID injected into the `recipe-runner-rs` child environment. Treat as read-only. |
| `AMPLIHACK_RECIPE_LOG_JSONL` | user or config | Optional path for structured runner events. The runner reads `AMPLIHACK_RECIPE_RUN_ID` and writes it to event `run_id` fields. Included in `log_paths.jsonl` when known. |
| `RECIPE_RUNNER_RS_PATH` | user or config | Optional path to the runner executable. Reflected as `runner_path`. |

Do not set `AMPLIHACK_RECIPE_RUN_ID` manually for normal use. The wrapper creates
a fresh value for each recipe invocation so concurrent runs never share an
identity by accident.

## Security and privacy

Pointer events are designed for correlation, not secrecy.

They may reveal local paths, branch names, task metadata, issue or PR IDs, the
runner executable path, child PID, and log file paths. They do not include child
output bodies, environment dumps, tokens, or log file contents.

Treat pointer logs like normal CI or terminal logs: safe for operational
debugging, but not a place for secrets.

## Examples

Extract both pointer events from a captured progress log:

```bash
grep '^amplihack\.recipe\.log_pointer ' recipe-progress.log \
  | sed 's/^amplihack\.recipe\.log_pointer //' \
  | jq '{event, run_id, recipe_name, status, child_pid, exit_code, log_paths}'
```

Compare the early and final run IDs:

```bash
jq -r '.run_id' result.json

grep '^amplihack\.recipe\.log_pointer ' recipe-progress.log \
  | sed 's/^amplihack\.recipe\.log_pointer //' \
  | jq -r '.run_id' \
  | sort -u
```

Find all CI log lines for a run:

```bash
run_id=$(jq -r '.run_id' result.json)
grep "$run_id" recipe-progress.log
```

## Related

- [Correlate Recipe Runs with Logs](../howto/correlate-recipe-runs.md)
- [Trace a Recipe Run Across Terminal and JSON Logs](../tutorials/recipe-run-correlation.md)
- [Recipe Runner Logging Reference](./recipe-runner-logging.md)
- [Recipe CLI Reference](./recipe-cli-reference.md)
- [RecipeResult Reference](./recipe-result.md)
