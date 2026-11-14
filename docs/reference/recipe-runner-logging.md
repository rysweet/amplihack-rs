---
title: Recipe Runner Logging Reference
description: Progress output, correlation, and the required Git command record.
doc_type: reference
last_updated: 2026-11-13
---

# Recipe Runner Logging Reference

On Linux, every real run records the Git commands its runner, steps, agents
and hooks launch, using an observer. A run whose recording fails does not
succeed. On other platforms the run proceeds without recording and says so
(see [When recording is unavailable](#when-recording-is-unavailable)).
Trace2 output is kept for diagnostics only. See
[required recording](recipe-checkout-isolation.md#required-recording).

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
- [Git command record](#git-command-record)
  - [When recording is unavailable](#when-recording-is-unavailable)
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

## Git command record

On Linux, every recipe run that is not a dry run requires the observer. Each attempt
writes a private journal under `git-trace2/` (the directory retains its legacy
name). Successful executable stops provide executable identity, cwd, launch
routing, run and attempt identity, and UTC time. Repository/index attribution
uses the observed Git options and routing. Cleared-environment absolute Git
and Rust hook descendants are covered. Independent
nested runs receive separate journals and an acknowledgement after their
observed scope has ended and evidence is synchronized.

After successful collection, the run publishes `git-commands.jsonl`,
`diagnostics/git/commands.jsonl`, and `git-recording.json`; `run.json` includes
the recording receipt. Command records have schema version `1`, `run_id`,
`attempt_id`, `executable`, `argv`, `cwd`, `git_dir`, `index_file`,
`git_environment`, `time`, `sid` and `pid`. Compatibility aliases are
`command` for `argv`, `index_path` for `index_file`, and `timestamp` for `time`.
The receipt's scope is explicitly `runner-descendants`.

Required storage, collector loss, incomplete receipts, bounds, and publication
failures prevent success. Raw evidence is retained when final publication
fails. A successful empty journal requires a completed observer receipt.
The mechanism does not establish observation of preparation/release processes,
arbitrary non-Git index writes, renamed Git executables, unrelated processes,
or tamper resistance against the same user. See the
[isolation limits](recipe-checkout-isolation.md#limits).

### When recording is unavailable

Off Linux (macOS and every other non-Linux target) there is no observer. A
real run still executes in its private checkout, but no Git command is
recorded. The run prints one notice to stderr and continues:

```text
amplihack: Git command recording is unavailable on this platform; the run continues in its private checkout without recording
```

At the end of the run, `<run dir>/git-recording.json` (mode `0600`) holds:

```json
{
  "schema_version": 1,
  "coverage": "unavailable",
  "complete": false,
  "reason": "Git command recording needs the Linux ptrace observer; this platform has none",
  "backend": null
}
```

| Field | Value |
| --- | --- |
| `schema_version` | `1` |
| `coverage` | `"unavailable"`. A Linux run has `"established"`. |
| `complete` | Always `false`. |
| `reason` | A fixed sentence naming why. It contains no path, environment value or command line. |
| `backend` | `null`. A Linux run has `"linux-ptrace-exec"`. |

`run.json` copies this object into its existing `git_recording` field. No
schema field is added; only the new `coverage` value. There is no
`git-commands.jsonl` and no `diagnostics/git/commands.jsonl` for such a run.

`coverage: "unavailable"` means no Git audit evidence was produced. It is not
an empty record, and nothing treats it as one: the receipt check that a
completed Linux record must pass (`coverage` `established`, `complete` `true`,
`backend` `linux-ptrace-exec`) rejects it, and no merge or readiness gate
accepts it as evidence.

The runner's environment is cleaned as on Linux: `AMPLIHACK_OBSERVER_PID`,
`GIT_TRACE2_EVENT`, `GIT_TRACE2_ENV_VARS` and the caller's `GIT_TRACE2`,
`GIT_TRACE2_PERF`, `GIT_TRACE2_CONFIG_PARAMS` and `GIT_TRACE2_REDACT` are
removed. A run still needs a usable work root; without one it stops with
`cannot initialize required Git recording; provide a writable absolute
AMPLIHACK_WORK_ROOT and retry`.

Check which kind of record a run has:

```sh
jq -r '.git_recording.coverage' \
  ~/.amplihack/work/runs/3f0c9a52-6d1e-4b8a-9f57-2c41d0e7a6b3/run.json
# established   (Linux)
# unavailable   (macOS)
```

The platform is chosen from the build target. No flag or environment variable
selects the unobserved path on Linux.

### Diagnostic Trace2 compatibility

The following retained description concerns the cooperative diagnostic API and
older installs. It does **not** describe the production coverage or
failure policy. The historical probe remains useful evidence of why Trace2
cannot implement required recording.

When recording is available, a run writes a cooperative summary of reported
Git commands capable of changing an index:
`<run dir>/git-commands.jsonl`, next to `run.json`. The run dir is
`AMPLIHACK_RUN_DIR`, normally `<work root>/runs/<run id>`. Use the file to
investigate reported activity after a run (issue #2096). Command presence
does not prove an index write; absence does not prove that no command ran.
Missing routing fields do not establish the effective index.

### How it is recorded

Before the runner starts, amplihack creates `<run dir>/git-trace2/` (mode
`0700`) and adds two variables to the runner's environment. Children can
inherit them, but reduced environments can drop them:

| Variable | Value |
| --- | --- |
| `GIT_TRACE2_EVENT` | `<run dir>/git-trace2/`. A directory, so each git process writes its own event file. |
| `GIT_TRACE2_ENV_VARS` | `GIT_DIR,GIT_INDEX_FILE,GIT_WORK_TREE,PWD` |

These values replace any `GIT_TRACE2_EVENT` or `GIT_TRACE2_ENV_VARS` set by
the caller. The caller's `GIT_TRACE2`, `GIT_TRACE2_PERF`,
`GIT_TRACE2_CONFIG_PARAMS` and `GIT_TRACE2_REDACT` are removed, so a run never
writes trace output to a place the caller chose.

When the runner exits, on success or failure, amplihack reads the event files,
writes `git-commands.jsonl`, and deletes `git-trace2/`. A run in which no step
ran a matching command still gets `git-commands.jsonl`, with no lines. The
record never changes the run's exit code.

### Fields

One JSON object per line, sorted by `time` and then `sid`. A line with no
`time` sorts first.

| Field | Type | Value |
| --- | --- | --- |
| `time` | string or `null` | UTC start time of the git process, from its Trace2 `start` event. |
| `argv` | array of strings | The command line, with secrets removed (see [Redaction](#redaction)). |
| `cwd` | string or `null` | `PWD` of the git process; when that is missing, the worktree git reported for the repository. |
| `git_dir` | string or `null` | `GIT_DIR` of the git process, or `null` when it was not set. |
| `index_file` | string or `null` | `GIT_INDEX_FILE` of the git process, or `null` when it was not set. |
| `sid` | string | The Trace2 session id. A child git process's `sid` starts with its parent's. |

Example line, from a step that ran `git add notes.md` in the run's checkout:

```json
{"time":"2026-11-10T09:14:03.512877Z","argv":["git","add","notes.md"],"cwd":"/home/user/.amplihack/work/runs/3f0c9a52-6d1e-4b8a-9f57-2c41d0e7a6b3/runtime/checkout","git_dir":null,"index_file":null,"sid":"20261110T091403.512877Z-H1c2d3e4f-P0000a1b2"}
```

This `cwd` assumes the run's checkout is at `<run dir>/runtime/checkout`, the
usual place. When that path is too long, the checkout is at
`<work root>/r/<short id>/checkout` instead, as described in
[Recipe run and auto-drive checkout isolation](./recipe-checkout-isolation.md).

A non-null `index_file` naming another repository's index is a diagnostic
lead. It does not by itself prove a write, resolve relative routing, or
establish ownership.

### Commands recorded

A git process is recorded when its subcommand is one of: `add`, `am`,
`apply`, `checkout`, `cherry-pick`, `commit`, `merge`, `mv`, `pull`,
`read-tree`, `rebase`, `reset`, `restore`, `revert`, `rm`, `stash`,
`switch`, `update-index`, `worktree`. The subcommand is found after git's
global options, such as `-C <dir>` and `-c <key>=<value>`. Read-only commands such as
`status`, `diff` and `log` are not recorded.

### Redaction

`argv` is written with:

- each `-c <key>=<value>` value replaced: `-c http.extraHeader=<redacted>`;
- the user part of a URL replaced: `https://<redacted>@git.example.com/team/app.git`.

Matching uses the command line as git saw it; only the written copy is
redacted. Warnings printed while recording name a path and an error kind,
never a command line or an environment value.

Redaction is not complete. It does not cover `--config-env`, a `-c` given
after the subcommand, or a secret passed in any other argument. The record
can therefore contain command lines with secrets in them, so
`git-commands.jsonl` is written with mode `0600` and `git-trace2/` is made
with mode `0700`. Quote digests or commit ids from it, not raw lines.

### When there is no record

| Case | What happens |
| --- | --- |
| The work root cannot be used, so the run has no run dir and uses a private temporary folder | No record. The runner gets no `GIT_TRACE2_EVENT` or `GIT_TRACE2_ENV_VARS`; any the caller set are removed, along with the other caller trace variables listed above. |
| `git-trace2/` cannot be created, or something already exists at that path | A warning on stderr. The run continues without a record and the variables are removed. |
| `git-commands.jsonl` cannot be written | A warning on stderr. `git-trace2/` is kept for inspection; it may contain full command lines, so treat it as private. |

The summariser treats the event files as untrusted: it reads only regular
files at the top of `git-trace2/`, does not follow symbolic links, skips lines
longer than 1 MiB and files larger than 64 MiB, and ignores lines it cannot
parse. Symbolic links, folders and other files that are not regular are
skipped silently. Each file skipped for its size or a read error prints one
warning on stderr that names the file's path and the error kind, never its
contents.

### Limits

The record is cooperative. It shows what git processes reported, not
everything that happened:

- A step can unset `GIT_TRACE2_EVENT` or set it elsewhere, and its git
  commands then go unrecorded.
- A step that starts its children with a reduced environment drops the
  variables for those children. The runner's own `bounded_env`, used for
  steps that run with a bounded environment, is one such case.
- A step can write files into `git-trace2/` that look like git events.
- A nested `amplihack recipe run` records into its own run dir, not its
  parent's.

Use the record to trace an incident afterwards. Do not use it as a merge
gate or as proof that a command did not run. What protects the caller's
checkout is the run's own checkout, described in
[Recipe run and auto-drive checkout isolation](./recipe-checkout-isolation.md).

### Read the record

List every command that set its own index:

```sh
jq -c 'select(.index_file != null) | {time, argv, cwd, index_file}' \
  ~/.amplihack/work/runs/3f0c9a52-6d1e-4b8a-9f57-2c41d0e7a6b3/git-commands.jsonl
```

List the commands run outside the run's own checkout. Set `co` to the
checkout the run used: `$run/runtime/checkout`, or
`<work root>/r/<short id>/checkout` for a run whose path was too long.
With the wrong value, every command is listed as outside:

```sh
run=~/.amplihack/work/runs/3f0c9a52-6d1e-4b8a-9f57-2c41d0e7a6b3
co="$run/runtime/checkout"
jq -c --arg co "$co" \
  'select(.cwd != null and (.cwd | startswith($co) | not)) | {argv, cwd}' \
  "$run/git-commands.jsonl"
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
- [Recipe Run and Auto-Drive Checkout Isolation](./recipe-checkout-isolation.md) - The run's own checkout and the auto-drive review directory
