---
title: amplihack recipe reference
description: Commands and options for inspecting and executing recipes.
doc_type: reference
last_updated: 2026-11-11
---

# amplihack recipe — Reference

Full CLI reference for the `amplihack recipe` subcommand, which lists,
inspects, validates, and runs YAML recipe files.

## Contents

- [Synopsis](#synopsis)
- [Subcommands](#subcommands)
  - [recipe list](#recipe-list)
  - [recipe show](#recipe-show)
  - [recipe validate](#recipe-validate)
  - [recipe run](#recipe-run)
  - [Usage-limit relaunch recovery](#usage-limit-relaunch-recovery)
  - [Detached runs](#detached-runs)
  - [recipe stop](#recipe-stop)
  - [Run records](#run-records)
- [Output formats](#output-formats)
- [Progress and stream contract](#progress-and-stream-contract)
- [Recipe file format](#recipe-file-format)
  - [Required fields](#required-fields)
  - [Optional fields](#optional-fields)
  - [Step fields](#step-fields)
  - [Step types](#step-types)
- [Context variables](#context-variables)
  - [Supplying context](#supplying-context)
  - [Context inference](#context-inference)
  - [Context environment variables](#context-environment-variables)
- [Git context behavior](#git-context-behavior)
- [Recipe search path](#recipe-search-path)
- [Recipe runner binary](#recipe-runner-binary)
- [Exit codes](#exit-codes)
- [Related](#related)

---

## Synopsis

```
amplihack recipe <SUBCOMMAND> [OPTIONS]
```

---

## Subcommands

### recipe list

List all recipes discovered in the [recipe search path](#recipe-search-path).

```
amplihack recipe list [--dir <DIR>] [--format <FORMAT>] [--tag <TAG>]... [--verbose]
```

| Flag | Default | Description |
|------|---------|-------------|
| `--dir <DIR>` | (search path) | Search only this directory instead of the default path |
| `--format <FORMAT>` | `table` | Output format: `table`, `json`, or `yaml` |
| `--tag <TAG>` | (none) | Filter by tag; repeat for AND logic (e.g. `--tag ci --tag lint`) |
| `--verbose` | false | Include version, author, and step count in table output |

```sh
# List all recipes (table)
amplihack recipe list

# List with full metadata
amplihack recipe list --verbose

# Filter to recipes tagged both "ci" and "lint"
amplihack recipe list --tag ci --tag lint

# Output as JSON for scripting
amplihack recipe list --format json
```

**Example output (table):**

```
Available Recipes (3):

• default-workflow
  Run the full 23-step development workflow
  Tags: dev, workflow

• smart-orchestrator
  Intelligent task routing and delegation
  Tags: dev, orchestration

• verification
  5-step verification workflow for trivial changes
  Tags: dev, verification
```

**Example output (JSON, --verbose):**

```json
[
  {
    "name": "default-workflow",
    "description": "Run the full 23-step development workflow",
    "version": "1.0.0",
    "author": "amplihack",
    "tags": ["dev", "workflow"],
    "step_count": 23
  }
]
```

---

### recipe show

Display the full contents of a recipe file: metadata, context variables, and
step list.

```
amplihack recipe show <FILE> [--format <FORMAT>] [--no-steps] [--no-context]
```

| Flag | Default | Description |
|------|---------|-------------|
| `<FILE>` | (required) | Path to the recipe YAML file |
| `--format <FORMAT>` | `table` | Output format: `table`, `json`, or `yaml` |
| `--no-steps` | false | Omit the step list from output |
| `--no-context` | false | Omit context variable defaults from output |

```sh
amplihack recipe show ~/.amplihack/.claude/recipes/default-workflow.yaml

amplihack recipe show ~/.amplihack/.claude/recipes/default-workflow.yaml \
  --format json
```

---

### recipe validate

Validate a recipe YAML file and report any structural errors.

```
amplihack recipe validate <FILE> [--format <FORMAT>] [--verbose]
```

| Flag | Default | Description |
|------|---------|-------------|
| `<FILE>` | (required) | Path to the recipe YAML file |
| `--format <FORMAT>` | `table` | Output format: `table`, `json`, or `yaml` |
| `--verbose` | false | Include description and step count in success output |

```sh
# Validate a recipe
amplihack recipe validate ~/.amplihack/.claude/recipes/default-workflow.yaml

# Verbose: show description and step count on success
amplihack recipe validate ~/.amplihack/.claude/recipes/default-workflow.yaml \
  --verbose
```

**Success output (table):**

```
✓ Recipe is valid
  Name: default-workflow
  Description: Run the full 23-step development workflow
  Steps: 23
```

**Failure output (table):**

```
✗ Recipe is invalid
  Error: Every step must have a non-empty 'id' field
```

**Validation rules:**

- `name` field must be present and non-null
- `steps` field must be present with at least one step
- Every step must have a non-empty `id`
- Step `id` values must be unique within the recipe
- `type` field (if present) must be `bash`, `agent`, or `recipe`
- File size must be under 1 MB

**JSON output on failure:**

```json
{
  "valid": false,
  "errors": ["Every step must have a non-empty 'id' field"]
}
```

Exit code is 1 on validation failure; 0 on success.

---

### recipe run

Execute a recipe by delegating to the `recipe-runner-rs` binary.

```
amplihack recipe run <FILE> [-c KEY=VALUE]... [--dry-run] [--fresh] [--verbose]
  [--bundle <installed|checkout>]
  [--format <FORMAT>] [--working-dir <DIR>] [--step-timeout <SECONDS>]
  [--detach | --no-detach] [--governor-wait <DURATION>]
  [--governor-class <CLASS>] [--workstream <ID>]
```

| Flag | Default | Description |
|------|---------|-------------|
| `<FILE>` | (required) | Recipe name, looked up only in the selected bundle's `recipes/`, or a path to a recipe YAML file. See [Resolving the recipe argument](./recipe-run-bundle.md#resolving-the-recipe-argument) |
| `-c KEY=VALUE` | (none) | Set a context variable; repeat for multiple values |
| `--bundle <CHOICE>` | `installed` | Bundle that supplies recipes, sub-recipes, agents and tools for the whole run: `installed` (`AMPLIHACK_HOME`, then `paths.home`, then `$HOME/.amplihack`) or `checkout` (nearest ancestor of the working directory with `.git` and `amplifier-bundle/`). Same as `-c bundle=<CHOICE>`; the two must agree. See [Recipe run bundle selection and pinning](./recipe-run-bundle.md) |
| `--dry-run` | false | Print the execution plan without running any steps |
| `--fresh` | false | Bypass historical recovery for this launch and its delegated execution; preserve existing history |
| `--verbose` | false, or [`recipe.verbose`](./user-config.md#recipeverbose) | Increase progress detail on stderr; basic step progress is emitted by default. On when either the flag or `recipe.verbose = true` is set |
| `--format <FORMAT>` | `table` | Output format for results: `table`, `json`, or `yaml` |
| `--working-dir <DIR>` | `.` | Working directory for step execution |
| `--detach` | false, or [`recipe.detach`](./user-config.md#recipedetach) `= "tmux"` | Run in a new tmux session with a private log, print the session and log path, and return. See [Detached runs](#detached-runs) |
| `--no-detach` | | Run in the foreground even when `recipe.detach = "tmux"` |
| `--governor-wait <DURATION>` | `AMPLIHACK_GOVERNOR_WAIT`, then `governor.wait_secs`, else `10m` | Longest time to wait for the [capacity governor](./capacity-governor.md) to admit the run. `0` means do not wait |
| `--governor-class <CLASS>` | inferred, else `ordinary` | Priority class for admission: `coordination`, `recovery`, `review` or `ordinary`. See [Reserves and classes](./capacity-governor.md#reserves-and-classes) |
| `--workstream <ID>` | `AMPLIHACK_WORKSTREAM`, then the issue or run id | Workstream the run is counted against for [fair queueing](./capacity-governor.md#fair-queue) |
| `--step-timeout <SECONDS>` | `AMPLIHACK_STEP_TIMEOUT`, then [`recipe.step_timeout`](./user-config.md#recipestep_timeout), else none | Apply a global per-step timeout ceiling to **every** step in the recipe. Useful for CI guard rails. `0` disables all step timeouts. When omitted, agent steps run without a per-step timeout and bash steps use the YAML-defined `timeout_seconds` only when present |

When the [capacity governor](./capacity-governor.md) is turned on (it ships off; see [Kill switch and governor keys](./capacity-governor.md#kill-switch-and-governor-keys)), then after the pre-flight checks and before `recipe-runner-rs` starts, `recipe run` (but not `--dry-run`) asks it for a slot and holds it until the recipe returns. Before the slot is recorded, the run's `AMPLIHACK_TREE_ID` is settled (inherited, or generated once) and the environment built for agent steps reuses it, so agent steps run under that slot. If the governor makes the run wait, a reason line goes to stderr; if the wait runs out or the run is refused, the runner is not started and the command exits with 80. With `--detach`, the governor flags are passed on to the run in the tmux session, and that run asks the governor; the detaching command itself holds no slot.

Before spawning `recipe-runner-rs`, `recipe run` checks the selected bundle and copies it into `<run dir>/runtime/bundle/amplifier-bundle`. It sets `AMPLIHACK_HOME` and `AMPLIHACK_RUN_BUNDLE` to `<run dir>/runtime/bundle`, sets `AMPLIHACK_RUN_BUNDLE_CHOICE` and `AMPLIHACK_STATE_HOME`, and passes the runner `-R <copy>/recipes`, `--agent-dir <copy>/agents` and `--no-default-search`, so every step uses the same bundle until the run ends, across an `amplihack update`. Nested runs reuse the outer copy. A dry run makes no copy: it passes the source bundle's `recipes/` and `agents/` and does not set these variables. See [Recipe run bundle selection and pinning](./recipe-run-bundle.md).

The Rust CLI also injects, when available, `AMPLIHACK_ASSET_RESOLVER` into the child environment. That gives recipes a stable native way to resolve `amplifier-bundle/...` assets without assuming the Python package layout. Additionally, when `--step-timeout` is provided, the CLI sets [`AMPLIHACK_STEP_TIMEOUT`](./environment-variables.md#amplihack_step_timeout) in the child environment so `recipe-runner-rs` can read and apply the override. Without the flag, an `AMPLIHACK_STEP_TIMEOUT` already in the environment is passed on unchanged, and otherwise a configured `recipe.step_timeout` is exported.

> **No per-step timeouts on agent steps by default.** As of the resolution
> of [issue #439](https://github.com/rysweet/amplihack-rs/issues/439), the
> bundled recipes under `amplifier-bundle/recipes/*.yaml` no longer set
> `timeout:` / `timeout_seconds:` on agent steps. Models routinely think
> longer than any reasonable per-step timeout, and aborting them
> mid-thought corrupts orchestrator state. Agent steps now run to
> completion. To impose a hard ceiling (e.g., for CI), pass
> `--step-timeout <SECONDS>`. Bash steps may still carry a
> `timeout_seconds` value, but only when they call external network
> services (`gh api`, `curl`, `git fetch`, etc.) where a stuck socket
> could hang indefinitely; in that case the floor is **1800s**.

```sh
# Dry run — inspect the plan before executing
amplihack recipe run ~/.amplihack/.claude/recipes/default-workflow.yaml \
  -c task_description="Add rate limiting to the API" \
  --dry-run

# Execute with context variables
amplihack recipe run ~/.amplihack/.claude/recipes/default-workflow.yaml \
  -c task_description="Fix the failing pagination tests" \
  -c repo_path=/home/user/src/myproject

# Override every step with a 30-minute ceiling (CI guard rail)
amplihack recipe run ~/.amplihack/.claude/recipes/default-workflow.yaml \
  -c task_description="Large refactoring task" \
  --step-timeout 1800

# Disable all step timeouts (default behavior for agent steps; explicit here)
amplihack recipe run ~/.amplihack/.claude/recipes/default-workflow.yaml \
  -c task_description="Complex migration requiring extended agent time" \
  --step-timeout 0

# Output results as JSON
amplihack recipe run ~/.amplihack/.claude/recipes/verification.yaml \
  -c task_description="Bump the version number" \
  --format json

# Run an investigation from any directory, including a non-git temp directory
tmpdir=$(mktemp -d)
cd "$tmpdir"
amplihack recipe run ~/.amplihack/.claude/recipes/investigation-workflow.yaml \
  -c task_description="Explain the deployment options" \
  -c repo_path=.
```

**Example output (table, dry run):**

```
Recipe: default-workflow
Status: ✓ Success

Steps:
  ✓ requirements-clarification: completed
    Output: [DRY RUN] Would execute agent: requirements-clarifier
  ✓ implementation-planning: completed
    Output: [DRY RUN] Would execute agent: architect
  ...
```

**Example output (JSON):**

```json
{
  "recipe_name": "default-workflow",
  "success": true,
  "step_results": [
    {
      "step_id": "requirements-clarification",
      "status": "completed",
      "output": "[DRY RUN] Would execute agent: requirements-clarifier",
      "error": ""
    }
  ]
}
```

Step status values: `completed`, `failed`, `skipped`.

A SIGHUP, SIGINT or SIGTERM to `recipe run` (closing the terminal, Ctrl-C,
`tmux kill-session`, `kill`) stops the whole run, whenever it arrives: during
setup, while a step runs, or during retry backoff, which it cuts short. The
runner's process group gets SIGTERM, then SIGKILL after the teardown grace
(`AMPLIHACK_TEARDOWN_GRACE_SECS`, default 5 s), as on a timeout. On Linux every
descendant found under the runner beforehand (by parent and process group, so
one that started its own session too) is then terminated the same way.
`recipe run` exits with `128 + signal` (129 for SIGHUP, 130 for Ctrl-C). The
runner is started in its own session, so before this the signal ended only
`recipe run` and left the runner and its agents running.

SIGKILL cannot be caught. If `recipe run` is killed that way, on Linux the
kernel sends the runner SIGTERM (`PR_SET_PDEATHSIG`); agents the runner started
can outlive it. `amplihack recipe stop <run_id>` ends the runner's process
group only while the runner is still alive, because it finds the run through
the [run record](#run-records), and a record whose processes have exited
matches nothing. Agents that outlived the runner are then left running. Stop
them through their tmux session, or by pid.

### Usage-limit relaunch recovery

Durable relaunch recovery is supported by the pinned Rust runner. Use
`--fresh` to bypass historical replay for one launch. See the
[recovery reference](recipe-recovery.md) for persistence policies and resources.

An identified launch selects the latest matching recipe/work attempt.
For `smart-orchestrator`, supply the same repository and a positive
`issue_number` or validated matching `issue_url`. Canonical Git worktrees
share a repository identity; independent clones remain separate. An
unidentified launch runs fresh outside shared recovery namespaces.

Recovery requires a durable usage-limit stop in that latest attempt and
an owner that is no longer active. A newer success, non-limit failure, or
unknown termination prevents recovery from older history. Current live
session ownership, authorization, governor admission, and recursion
accounting are established before dependent replay.

The runner restores compatible completed results and typed context changes
in order, including children beneath incomplete parent steps. It executes
the first incomplete or invalidated step. Compatibility covers effective
inputs, execution definitions and settings, cumulative repository state,
and required files/artifacts. Workflow-produced branch changes are checked
against recorded progression. Missing or unverifiable history causes a
logged fresh fallback; unsafe current storage or failed durable publication
stops execution.

`--fresh` bypasses this historical replay throughout the tree, including
detached and delegated execution. It preserves earlier records and records
the current attempt, which can support a later recovery. It does not reset
Git or clear an explicit `resume_checkpoint`.

`--dry-run` admits no recovery attempt, writes no recovery records, and
does not advance the latest marker, including when combined with `--fresh`.
The existing same-process usage-limit wait/retry behavior remains in place.

Stderr identifies the source run and actual restart step with its nested
invocation path, or explains the fresh fallback. JSON/YAML preserve the
additive [recovery metadata](recipe-result.md#recovery-metadata).
See [relaunch commands](../howto/resume-recipe-after-usage-limit.md) and
[the recovery contract](recipe-recovery.md) for policies and storage rules.

### Detached runs

`recipe run --detach` (or every `recipe run` once
`amplihack config set recipe.detach tmux` is set) starts the run in a new tmux
session and returns:

```console
$ amplihack recipe run smart-orchestrator --detach -w ~/src/myapp \
    --context-from-file task_description=task.md -c repo_path=.
session: amp-smart-orchestrator-myapp-20261020-101500
log: /home/alice/.amplihack/recipe-logs/amp-smart-orchestrator-myapp-20261020-101500.log
```

- **Session name** — [`recipe.tmux_session_pattern`](./user-config.md#recipetmux_session_pattern),
  default `amp-{recipe}-{repo}-{ts}`: the recipe's file stem or name, the git
  root's directory name, and a UTC timestamp. Any character tmux does not
  accept becomes `-`. When a live session or an earlier run's log already has
  the name, `-2`, `-3`, ... is appended, so a pattern without `{ts}`, or two
  launches in the same second, still works.
- **Log** — `<session>.log` in [`recipe.log_dir`](./user-config.md#recipelog_dir),
  default `~/.amplihack/recipe-logs`. The directory is created 0700 and the log
  0600. It receives everything the run prints, which the tmux pane shows too.
- **Environment** — the run gets your shell's environment, not the tmux
  server's. The agent CLI is resolved in your shell, as a foreground run would
  resolve it, and handed over tagged; every agent session marker the tmux
  server holds is removed. The environment travels in a 0600 file in the log
  directory that the pane reads and deletes at once, never on a command line.
  If the pane has not read it within 15 s, or the launch is interrupted, the
  file is deleted and the launch fails.
- **Context** — `-c`, `--context-file` and `--context-from-file` are read
  when you launch, not later, and reach the run in a 0600
  `<session>.context.json` in the log directory, removed when the run ends.
  No context value is on the pane's command line.
- **Start-up check** — the launch returns only after the pane has written
  `amplihack: detached run started (pid N)` to the log. If the pane cannot
  start the run, it writes the reason there instead, and the launch prints it
  and fails.
- **Stopping** — `amplihack recipe stop <session>`, `amplihack recipe stop <run_id>`
  with the run id from the `early` line in the log, or `tmux kill-session`.

Needs `tmux` on `PATH`.

Recovery settings, including `--fresh`, travel in the
private launch payload. The detaching process does not register a recovery
attempt or advance its latest marker; admission happens once in the executing pane.

### recipe stop

```
amplihack recipe stop <SESSION | RUN_ID>
```

Stops a recipe run and its whole process tree: `recipe run`, the recipe runner
and every agent and command it started.

- **A tmux session** started by `recipe run --detach` (a session with no
  pane running `amplihack recipe detached-child` is refused): only that
  pane's process group gets SIGTERM, and `recipe run` there tears its runner
  down. Anything of its tree still running after 15 s gets SIGKILL. Then the
  session is killed, which closes any other pane in it (a `tail -f` you split
  off) without a signal from `recipe stop`.
- **A run id**, the `run_id` of a run's `amplihack.recipe.log_pointer` line
  (the attempt id, `AMPLIHACK_RECIPE_RUN_ID`): the processes are taken from
  the attempt's [run record](#run-records). `recipe run` and the runner's
  process group get SIGTERM, then SIGKILL after 15 s. `recipe run` tears its
  runner down, prints its `final` line and exits non-zero. Linux only.

An empty target, or one that starts with `-`, is refused. Otherwise tmux is
asked first: if it has a session with that name, the session is stopped and
the run-id path is not used.

Each process is signalled only while its start time still matches the one
recorded, so a reused pid is never hit. `recipe stop` never signals itself or
its parent, and by run id refuses a run it descends from. It prints `stopped <what> (<n> processes)` and exits 0, or exits 1
when nothing matches.

### Run records

On Linux, every `recipe run` attempt that starts a runner writes a record of
its own processes into its run directory (on other platforms none is written):

```text
<work root>/runs/<run dir id>/procs/<run_id>.json    procs/ 0700, file 0600
```

`<work root>` is resolved as in
[Settings](../howto/clean-up-after-recipe-runs.md#settings). `<run dir id>` is
the run directory's name (`AMPLIHACK_RUN_ID`), not the `run_id` that
`recipe stop` takes. `<run_id>` is the attempt id from the log pointer lines;
a run that retries writes one record per attempt. Each record is removed when
its attempt ends, and `procs/` is removed with the last one, so a finished run
leaves no `procs/` behind. A record left by a killed run is removed with its
run directory by [`amplihack cleanup`](../howto/clean-up-after-recipe-runs.md).

| Field | Meaning |
|-------|---------|
| `version` | Always `1` |
| `run_id` | The attempt's run id, the same as in its log pointer lines |
| `recipe_run.pid` | The pid of the `recipe run` process that owns the attempt |
| `recipe_run.start_ticks` | Its start time in clock ticks, field 22 of `/proc/<pid>/stat` |
| `runner.pid` | The runner's pid, the `child_pid` of the `final` line |
| `runner.pgid` | The runner's process group. The runner starts its own session, so this equals `runner.pid` |
| `runner.start_ticks` | The runner's start time in clock ticks |

The record holds nothing else: no environment, arguments, paths or context.

#### When the record exists

`recipe run` writes the record straight after the runner starts and before it
prints the `early` log pointer line, so a script that has read that line can
run `recipe stop <run_id>` at once. This includes a detached run's pane. The
record is removed when the attempt ends, however it ends, and before any retry
backoff; during backoff, stop the run through its tmux session.

No record is written for a `--dry-run`, when the runner cannot be started or
its probe fails, or when an orphan stop arrives before the runner starts. If
the record cannot be written, `recipe run` prints a warning and continues. A
record left by a SIGKILLed `recipe run` stays until cleanup; its start times no
longer match, so it never causes a signal.

#### How `recipe stop <run_id>` uses the record

- **The id is checked first:** 1 to 128 bytes of ASCII letters, digits, `.`,
  `_` and `-`, not starting with `.`. No file is read for a refused id.
- **The record is found** by listing `<work root>/runs/` and opening
  `<entry>/procs/<run_id>.json` without following symlinks (entries and
  `procs/` directories that are symlinks are skipped). It is used only if it
  is a regular file of at most 4 KiB, belongs to you, is not writable by group
  or others, has exactly the fields above with `version` `1` and the requested
  `run_id`, and has both pids above 1 with `runner.pgid` equal to `runner.pid`.
- **A pid is kept** only while the start time in `/proc/<pid>/stat` matches
  the record, and never if it is `recipe stop`'s own pid or its parent's.
  `recipe stop` refuses when its own `AMPLIHACK_RECIPE_RUN_ID`, session or
  process group is the run's, and when it descends from the run: it walks its
  own parent chain (`/proc/self/stat`, then each parent's `stat`) and refuses
  if any ancestor is the recorded `recipe run` or runner with the recorded
  start time, or is in the runner's process group. This covers a step of a
  nested `recipe run` asked to stop an outer run: the nested run has its own
  `AMPLIHACK_RECIPE_RUN_ID` and its runner its own session, but the outer
  runner is still its ancestor.
- **Signals:** SIGTERM to the kept pids and to the runner's group; after up to
  15 s, SIGKILL to the group and then to any kept pid still running. The start
  time is checked again before every signal. `recipe stop` waits only on the
  two recorded pids, and signals the group only while its leader, the runner,
  still matches its start time. If the runner exits within the 15 s, the group
  gets no SIGKILL from `recipe stop`: members that ignored SIGTERM are left to
  `recipe run`'s own teardown, which got SIGTERM in the same pass and SIGKILLs
  the runner's remaining group members (`snapshot_group` in `run/execute.rs`).
- **No other `/proc` access:** besides the two recorded pids' `stat` it reads
  only the `stat` of itself and its ancestors; it does not list `/proc`, reads
  no `environ`, and never writes or removes a record. The strace check in
  [Keep Tests off the Host Process Table](../howto/keep-tests-off-the-host-process-table.md#recipe-stop-run_id-reads-only-its-own-runs-pids)
  enforces this.

#### Errors

Each error exits 1 and signals nothing. The Linux messages quote the id as a
string literal, so control characters in it are printed escaped.

| Case | Message |
|------|---------|
| The id is invalid | `recipe stop: invalid run id "<run_id>"` |
| The id is `recipe stop`'s own run | `recipe stop: refusing to stop run "<run_id>" from inside it` |
| No usable record, or a stale one | `recipe stop: no tmux session or running recipe run named "<run_id>"` |
| The work root, `runs/`, a run folder or a record cannot be read (for example, permission denied), and no usable record was found | The same "not found" message, followed by the reason, such as `cannot list <work root>/runs: Permission denied` |
| Any run id on a platform other than Linux | `recipe stop: no tmux session named <run_id>; stopping by run id needs /proc and is supported on Linux only` |

#### Runs that `recipe stop <run_id>` does not find

Besides runs that have no record (see
[When the record exists](#when-the-record-exists)) and runs on platforms
other than Linux, `recipe stop <run_id>` does not find runs started by an
amplihack built before run records existed, or runs whose `recipe stop` sees a
different work root (through `AMPLIHACK_WORK_ROOT` or `HOME`). Stop these
through their tmux session, or by signalling `recipe run`.

Accepted risks: a pid reused in the microseconds between the start-time check
and `kill(2)`, which only pidfds would close; a descendant of the run whose
link to it was broken (an intermediate process exited, so it was reparented to
init or a subreaper) and that cleared its environment and started a new
session, which the "from inside it" check does not detect; and agents that started their own session and outlived
their runner, which are not in the runner's group. Stop those through their
tmux session or by pid.

---

## Output formats

All `recipe` subcommands support `--format table` (default), `--format json`,
and `--format yaml`.

| Format | Use case |
|--------|----------|
| `table` | Human-readable terminal output |
| `json` | Scripting, CI pipelines, `jq` processing |
| `yaml` | Config file integration |

`recipe run` writes the final result in the selected format to stdout. Live
progress, heartbeats, and failure diagnostics are written to stderr.

---

## Progress and stream contract

`amplihack recipe run` forwards `recipe-runner-rs` progress to stderr by
default. This makes normal runs observable while keeping stdout safe for
structured output.

```sh
amplihack recipe run default-workflow \
  -c task_description="Add validation" \
  -c repo_path=. \
  --format json > result.json
```

In this example, the terminal shows step progress and heartbeats from stderr.
`result.json` contains only the final JSON result.

Progress includes:

- recipe started/completed/failed
- step started/completed/failed/skipped
- rate-limited heartbeat lines for long-running recipe, agent, subprocess, and
  nested-recipe steps
- bounded recent stdout/stderr snippets in failure diagnostics
- early and final `amplihack.recipe.log_pointer` correlation events

`--verbose` adds detail such as child launch metadata and extra snippet context.
`--progress` is not a supported `amplihack recipe run` flag. Passing it fails
with an actionable message explaining that progress is already emitted to
`stderr` by default and `--verbose` only increases diagnostic detail.

Every run receives one UUID `run_id` before the runner child starts. The wrapper
injects the same value into the child environment as
`AMPLIHACK_RECIPE_RUN_ID`, writes it to early and final stderr pointer events,
and includes it in final JSON/YAML results when a result can be produced.

Pointer lines use the stable prefix `amplihack.recipe.log_pointer ` followed by
one JSON object:

```text
amplihack.recipe.log_pointer {"schema_version":1,"event":"early","run_id":"5b60657b-76ef-4f49-8a22-8b89ed75f43e","recipe_name":"default-workflow","cwd":"/home/user/src/myapp","worktree":"/home/user/src/myapp","branch":"main","runner_path":"/home/user/.local/bin/recipe-runner-rs","agent_binary":"claude","agent_binary_source":"user_config","amplihack_home":"/home/user/.amplihack","timestamp":"2026-06-12T03:55:11Z"}
amplihack.recipe.log_pointer {"schema_version":1,"event":"final","run_id":"5b60657b-76ef-4f49-8a22-8b89ed75f43e","recipe_name":"default-workflow","status":"success","cwd":"/home/user/src/myapp","worktree":"/home/user/src/myapp","branch":"main","child_pid":41822,"exit_code":0,"runner_path":"/home/user/.local/bin/recipe-runner-rs","agent_binary":"claude","agent_binary_source":"user_config","amplihack_home":"/home/user/.amplihack","log_paths":{"jsonl":"/tmp/default-workflow.jsonl"},"timestamp":"2026-06-12T04:21:39Z"}
```

`agent_binary` and `agent_binary_source` record the agent CLI the steps run
under and the resolver layer that chose it (`env`, `user_config`,
`session_marker`, `launcher_context` or `default`). `amplihack_home` is the
`AMPLIHACK_HOME` handed to the runner.

Final pointer `status` values are lowercase wrapper statuses: `success`,
`failure`, `spawn_failure`, or `parse_failure`. Top-level recipe result statuses
remain whatever the result schema emits, commonly uppercase values such as
`SUCCESS`, `FAILURE`, or `PARTIAL`, or the boolean `success` field in older
results. Do not compare pointer status values with top-level result status
values without normalizing them.

The final JSON result may include optional additive fields such as
`duration_seconds`, `progress_summary`, `failure_context`, `run_id`,
`log_pointer`, `step_name`, `phase`, `child`, `last_heartbeat_at`, and
`recent_output`. Consumers must ignore unknown fields. The CLI must preserve
these fields when formatting JSON or YAML instead of parsing and dropping
unknown runner fields. See [RecipeResult Reference](./recipe-result.md) and
[Recipe Run Correlation Reference](./recipe-run-correlation.md) for schema
details.

---

## Recipe file format

A recipe is a YAML file. The top-level document must be a mapping.

### Required fields

| Field   | Type   | Description |
|---------|--------|-------------|
| `name`  | string | Unique identifier for the recipe |
| `steps` | list   | Ordered list of step objects (at least one) |

### Optional fields

| Field         | Type   | Default   | Description |
|---------------|--------|-----------|-------------|
| `description` | string | `""`      | Human-readable description shown in `list` output |
| `version`     | string | `"1.0.0"` | Semantic version |
| `author`      | string | `""`      | Author or team name |
| `tags`        | list   | `[]`      | Labels used to filter with `--tag` |
| `context`     | map    | `{}`      | Default values for context variables |

### Step fields

| Field         | Type    | Required | Description |
|---------------|---------|----------|-------------|
| `id`          | string  | yes      | Unique step identifier within the recipe |
| `type`        | string  | no       | `bash`, `agent`, or `recipe` (inferred if omitted) |
| `command`     | string  | no       | Shell command to run (for `bash` steps) |
| `agent`       | string  | no       | Agent name to invoke (for `agent` steps) |
| `prompt`      | string  | no       | Prompt to send to the agent |
| `condition`   | string  | no       | Expression; step is skipped when false |
| `output`      | string  | no       | Variable name to store step output |
| `parse_json`  | bool    | no       | Parse step output as JSON |
| `timeout`     | integer | no       | Step timeout in seconds. **Discouraged on `agent` steps** — agent reasoning is variable and per-step timeouts cause spurious mid-thought aborts (see [issue #439](https://github.com/rysweet/amplihack-rs/issues/439)). Permitted on `bash` steps that perform external network I/O (`gh`, `curl`, `git fetch`); use a generous floor of **1800s** to guard against stuck sockets, not to bound work. Omit otherwise. |
| `working_dir` | string  | no       | Working directory override for this step |
| `recipe`      | string  | no       | Sub-recipe path (for `recipe` steps) |
| `context`     | map     | no       | Context overrides for a sub-recipe step |
| `mode`        | string  | no       | Step execution mode |
| `auto_stage`  | bool    | no       | Auto-stage git changes after step completes |

Recipe-level and step-level `recovery` mappings opt into durable recording.
See [persistence policies](recipe-recovery.md#recipe-persistence-policy)
and [step resources](recipe-recovery.md#step-replay-policy-and-resources).

### Step types

| Type     | Description |
|----------|-------------|
| `bash`   | Run a shell command. Uses the `command` field. |
| `agent`  | Invoke an amplihack agent. Uses `agent` and `prompt` fields. |
| `recipe` | Execute a sub-recipe. Uses the `recipe` field. |

---

## Context variables

Context variables are key-value string pairs that fill template slots in a
recipe's step commands and prompts.

### Supplying context

Pass context on the command line with `-c`:

```sh
amplihack recipe run recipe.yaml \
  -c task_description="Refactor the auth module" \
  -c repo_path=/home/user/src/myapp
```

Multiple `-c` flags are supported. Format is `key=value`.

### Context inference

If a required context variable is missing from `-c`, `amplihack recipe run`
checks environment variables in this order:

1. `AMPLIHACK_CONTEXT_<KEY>` (uppercased key) for any variable
2. `AMPLIHACK_TASK_DESCRIPTION` for the `task_description` key
3. `AMPLIHACK_REPO_PATH` for the `repo_path` key (defaults to `.`)

```sh
export AMPLIHACK_TASK_DESCRIPTION="Add OpenAPI docs to all endpoints"
amplihack recipe run ~/.amplihack/.claude/recipes/default-workflow.yaml
# task_description is inferred from $AMPLIHACK_TASK_DESCRIPTION
```

Recipe defaults in the `context:` block are the lowest-priority source.

### Context environment variables

| Variable | Applies to |
|----------|-----------|
| `AMPLIHACK_CONTEXT_<KEY>` | Any context variable named `<key>` |
| `AMPLIHACK_TASK_DESCRIPTION` | `task_description` |
| `AMPLIHACK_REPO_PATH` | `repo_path` |

---

## Git context behavior

`amplihack recipe run` does not require the current working directory to be a
Git repository unless the selected recipe or step needs Git semantics. Recipes
validate universal inputs first, such as `task_description` and whether
`repo_path` exists, then each host-tool step checks its own prerequisites.
This keeps routing, Q&A, and investigation recipes usable from existing scratch
directories, containers, and temporary folders.

The bundled recipes follow these Git-context rules:

| Recipe or step type | Git requirement | Behavior outside a Git repository |
|---------------------|-----------------|-----------------------------------|
| `smart-orchestrator` routing and dry-runs | Optional | Runs classification and routing. Dry-runs stop there and do not execute downstream Git-required steps; normal runs let selected Git-only steps report their own precondition. |
| `investigation-workflow` and Q&A workflows | Not required | Do not require Git. History-specific analysis is skipped or marked unavailable when no repository is present. |
| Development, publish, PR, worktree, and TDD workflow steps | Required when they create branches, commits, worktrees, PRs, or inspect tracked changes | Fail before the Git operation with a structured, actionable precondition error. |
| Telemetry and status-only Git checks | Optional | Print a visible `[skip] not a git repo ...` note and continue. |

Strict Git-required steps fail with an error shaped like:

```text
ERROR: step <workflow>/<step> requires a git repo at /work/app; either `git init` or rerun from a checkout
```

That error means the recipe is valid, but the selected workflow needs repository
state. To resolve it, either initialize the directory:

```sh
git init
amplihack recipe run ~/.amplihack/.claude/recipes/default-workflow.yaml \
  -c task_description="Implement the feature" \
  -c repo_path=.
```

or rerun from an existing checkout:

```sh
cd /home/user/src/myproject
amplihack recipe run ~/.amplihack/.claude/recipes/default-workflow.yaml \
  -c task_description="Implement the feature" \
  -c repo_path=.
```

Optional Git telemetry never hides failures with `|| true`. When no repository
is present, it emits a visible skip note instead:

```text
[skip] not a git repo at /tmp/amplihack-demo; skipping operations file-change git telemetry
```

This distinction is intentional: optional observations may be skipped, but
required host-tool work must fail before doing partial or misleading work.

---

## Recipe search path

This search path applies to `recipe list`, `recipe show` and `recipe validate`.
`recipe run` does not use it: a bare recipe name is looked up only in the
selected bundle's `recipes/` directory. See
[Resolving the recipe argument](./recipe-run-bundle.md#resolving-the-recipe-argument).

`amplihack recipe list` (without `--dir`) searches these directories in order,
later entries overriding earlier ones with the same recipe name:

1. `<git-repo-root>/amplifier-bundle/recipes/` (detected by walking up from cwd)
2. `~/.amplihack/.claude/recipes/`
3. `amplifier-bundle/recipes/` (relative to cwd)
4. `amplifier-bundle/recipes/` (relative to cwd)
5. `.claude/recipes/` (relative to cwd)

When a recipe name appears in multiple directories, the last matching file wins.
This allows local overrides of bundled recipes.

---

## Recipe runner binary

`amplihack recipe run` delegates actual execution to a separate
`recipe-runner-rs` binary. The binary is located using these sources in order:

1. `RECIPE_RUNNER_RS_PATH` environment variable (a `~/` prefix expands to
   `$HOME`; a bare name is looked up on `$PATH`)
2. `recipe-runner-rs` on `$PATH`
3. `$CARGO_HOME/bin/recipe-runner-rs` (where `cargo install` puts it)
4. `~/.cargo/bin/recipe-runner-rs`
5. `~/.local/bin/recipe-runner-rs`

The same lookup is used by `amplihack install` and the launch-time freshness
refresh, so all three agree on which binary is in use.

The recovery integration requires the selected binary to
advertise `durable_recovery_v1`, including when `--fresh` is requested.
Validation enforces that capability. Supported runner
delivery retains the pinned upstream revision and ancestry validation.
A binary with partial recovery support does not satisfy this requirement; see
[the capability contract](recipe-recovery.md#capability-and-dependency-contract).

If no binary is found, `recipe run` fails with:

```
recipe-runner-rs binary not found. Install it:
  cargo install --git https://github.com/rysweet/amplihack-recipe-runner
or set RECIPE_RUNNER_RS_PATH.
```

Set `RECIPE_RUNNER_RS_PATH` to point at a custom build or wrapper:

```sh
export RECIPE_RUNNER_RS_PATH=/opt/amplihack/bin/recipe-runner-rs
amplihack recipe run recipe.yaml -c task_description="..."
```

---

## Exit codes

| Exit code | Meaning |
|-----------|---------|
| 0 | Success (list, show, validate passed; run completed with all steps succeeded) |
| 1 | Error (validate failed; run failed; recipe-runner-rs not found; malformed context args) |
| 2 | Usage error (for example an invalid `--workstream` or `--governor-wait`) |
| 80 | Not started: the [capacity governor](./capacity-governor.md#waiting-denial-and-exit-code-80) refused the run or its wait ran out |

---

## Failure modes

Recipe execution can fail at multiple levels. For diagnosis:

| Failure | Symptom | See |
|---|---|---|
| Runner binary missing | `recipe-runner-rs: command not found` | [Recipe search path](#recipe-search-path) and [Binary Resolution](./binary-resolution.md) |
| Step timeout | Bash step killed after `timeout_seconds` (network-I/O steps only; agent steps no longer carry per-step timeouts) | [Recipe Executor Environment](./recipe-executor-environment.md) |
| Routing gap (no step runs) | Empty result after execution phase | [Smart-Orchestrator Recovery](../concepts/smart-orchestrator-recovery.md) |
| Duplicate issues filed | Repeated `gh issue create` on retries | [Issue Deduplication](./issue-dedup.md) |
| Condition parse error | `condition` field uses unsupported expression | [Agentic Step Patterns](../concepts/agentic-step-patterns.md) |
| Context variable missing | `{{var}}` renders as empty string | [Context variables](#context-variables) |
| Run not started, exit 80 | `amplihack: governor: not started (<code>): ...` on stderr | [Capacity Governor](./capacity-governor.md#reason-codes) |
| Opaque long-running step | No visible child output for a while | [Recipe Runner Logging](./recipe-runner-logging.md) |

For general troubleshooting, see [Troubleshoot Recipe Execution Failures](../howto/troubleshoot-recipe-execution.md).

---

## Related

- [Run a Recipe End-to-End](../howto/run-a-recipe.md) — Step-by-step guide to executing recipes
- [Recipe Run Bundle Selection and Pinning](./recipe-run-bundle.md): `--bundle`, the per-run bundle copy, nested runs and bundle errors
- [Capacity Governor](./capacity-governor.md): When `recipe run` may start, waits or exits with 80
- [Environment Variables](./environment-variables.md) — `AMPLIHACK_CONTEXT_*`, `AMPLIHACK_TASK_DESCRIPTION`, `RECIPE_RUNNER_RS_PATH`
- [Recipe Runner Logging](./recipe-runner-logging.md) — stderr progress, heartbeats, snippets, and JSONL logs
- [Parity Test Scenarios](./parity-test-scenarios.md) — `tier4-recipe-run.yaml` test cases
- [Agent Binary Routing](../concepts/agent-binary-routing.md) — How `AMPLIHACK_AGENT_BINARY` affects recipe step execution
- [Recipe Runner Architecture](../concepts/recipe-runner-architecture.md) — Why the runner is an external binary
- [Agentic Step Patterns](../concepts/agentic-step-patterns.md) — When to use bash vs agent vs recipe steps
