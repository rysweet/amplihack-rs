# Recipe CLI Reference

Command-line reference for `amplihack recipe`.

## Contents

- [Subcommands](#subcommands)
- [amplihack recipe list](#amplihack-recipe-list)
- [amplihack recipe run](#amplihack-recipe-run)
- [Removed context keys](#removed-context-keys)
- [Nested runs and orphan stops](#nested-runs-and-orphan-stops)
- [amplihack recipe validate](#amplihack-recipe-validate)
- [amplihack recipe show](#amplihack-recipe-show)
- [Environment variables](#environment-variables)
- [Exit codes](#exit-codes)

## Subcommands

```bash
amplihack recipe <SUBCOMMAND> [OPTIONS]
```

`amplihack recipe` has no recipe-specific global flags beyond standard help.
Options such as `--verbose` and `--format` belong to individual subcommands.

## amplihack recipe list

List discovered recipes.

```bash
amplihack recipe list [RECIPE_DIR] [OPTIONS]
```

| Option | Description | Default |
| --- | --- | --- |
| `RECIPE_DIR` | Optional directory to search instead of the default recipe search path | default search path |
| `--format <format>`, `-f <format>` | Output format: `table`, `json`, or `yaml` | `table` |
| `--tags <tag>`, `-t <tag>` | Filter by tag; repeat for multiple tags | none |
| `--verbose`, `-v` | Include extra recipe metadata | `false` |

```bash
amplihack recipe list --format json
amplihack recipe list ~/.amplihack/.claude/recipes --tags dev --verbose
```

## amplihack recipe run

Execute a workflow recipe through `recipe-runner-rs`.

This section defines the stream, progress, and run-correlation contract.

```bash
amplihack recipe run <RECIPE> [--context-file PATH] [--context-from-file KEY=PATH]... [-c KEY=VALUE]... [OPTIONS]
```

| Option | Description | Default |
| --- | --- | --- |
| `<RECIPE>` | Recipe name or recipe YAML path | required |
| `--context KEY=VALUE`, `-c KEY=VALUE` | Set a context variable; repeat for multiple values | none |
| `--context-file <PATH>` | Read context from a file holding one JSON object; non-string values become text | none |
| `--context-from-file KEY=PATH` | Set `KEY` to the raw UTF-8 contents of `PATH`; repeat for multiple keys | none |
| `--dry-run` | Show the execution plan without running steps | `false` |
| `--verbose`, `-v` | Add diagnostic detail to progress output | `false` |
| `--format <format>`, `-f <format>` | Final stdout format: `table`, `json`, or `yaml` | `table` |
| `--working-dir <dir>`, `-w <dir>` | Working directory for recipe execution | current directory |
| `--step-timeout <seconds>` | Set `AMPLIHACK_STEP_TIMEOUT` for every step; `0` disables step timeouts | omitted |

Short context uses repeated `KEY=VALUE` flags:

```bash
amplihack recipe run default-workflow \
  -c task_description="Add user authentication" \
  -c repo_path=/home/user/src/myapp
```

Values that can exceed a few kilobytes, such as an issue body or a cycle
history, come from files so that they never occupy an argv element:

```bash
amplihack recipe run default-workflow \
  --context-from-file task_description=/tmp/issue-1570.md \
  -c repo_path=.
```

When a key is set by more than one source, `-c` wins over
`--context-from-file`, which wins over `--context-file`. File sources reject
keys outside `[A-Za-z0-9_]+`, `AMPLIHACK_*` keys, NUL bytes and files over
16 MiB. See [Recipe context from files](./large-payload-delivery.md#recipe-context-from-files)
for the conversion and validation rules, and
[Pass Large Recipe Context](../howto/pass-large-recipe-context.md) for worked
examples.

Capture the final JSON result while still watching live progress:

```bash
amplihack recipe run default-workflow \
  -c task_description="Add user authentication" \
  -c repo_path=. \
  --format json > result.json
```

Progress, heartbeats, correlation pointer lines, and failure diagnostics are
written to `stderr`. The final result is written to `stdout` in the selected
format. `--verbose` adds detail; basic progress does not require it.

Every run receives a UUID `run_id`. The same value appears in early and final
`amplihack.recipe.log_pointer` stderr lines, in the child environment as
`AMPLIHACK_RECIPE_RUN_ID`, and in JSON/YAML final results as `run_id` when a
result can be produced.

`--progress` is intentionally unsupported. Passing it fails with an
actionable message explaining that progress is already emitted to `stderr` by
default and that `--verbose` only increases diagnostic detail.

`amplihack recipe run` does not support `--adapter`, `--resume-from`,
`--stop-at`, `--output`, `--interactive`, or `--quiet`.

## Removed context keys

Some context keys have been removed from every bundled recipe. Passing one is
an error, not a silent no-op. `amplihack recipe run` checks the merged context
from `-c`, `--context-file` and `--context-from-file` and exits `1` before it
creates a worktree, a state directory or a tmux pane, and before any step,
`git` or `gh` command runs.

| Key | Removed by | Why |
| --- | --- | --- |
| `should_publish` | #1976 (decision 17.113) | No step ever read it. A run passed `should_publish=false` still pushed, opened a PR and posted comments. |

Every value is refused, including `true`, an empty value and `True`. The match
is on the key only: the text before the first `=`, with surrounding spaces
trimmed, compared exactly. `should_publish_extra` and `Should_Publish` are
ordinary keys and are not refused.

```console
$ amplihack recipe run default-workflow \
    -c task_description="Fix flaky retry tests" \
    -c should_publish=false
Error: context key 'should_publish' was removed by #1976 (decision 17.113). It never stopped a run from publishing; do not rely on it. Remove '-c should_publish=…' from the command.
$ echo $?
1
```

The same error appears for a `--context-file` JSON object that has a
`should_publish` key, and for `--detach` runs, which are refused before the
pane opens. The message names the key once, however many times it was passed,
and never repeats the value.

The check runs again at the start of the runner wrapper, so the pane child of
a `--detach` run and callers inside the crate are covered by the same table.
It does not inspect a `RECIPE_VAR_should_publish` that is already in the
environment; nothing reads that variable.

The removed caller key is unrelated to `terminal_state.should_publish`. That
is an internal output of `workflow-terminal-state`, still emitted and still
used to gate every publish step in `workflow-publish`. See
[Workflow Terminal-State Provider Safety](./workflow-terminal-state-provider-safety.md).

There is no caller switch that stops a recipe run from publishing. Do not rely
on task text either: a run told not to push can still push. To keep work local,
run a recipe that has no publish phase.

## Nested runs and orphan stops

A recipe step can start another `amplihack recipe run`, for example the rounds
that `autodrive_loop.sh` launches. Each `amplihack recipe run` exports two
variables to its runner child and nowhere else:

| Variable | Value |
| --- | --- |
| `AMPLIHACK_PARENT_RECIPE_PID` | Process ID of this `amplihack recipe run`. |
| `AMPLIHACK_PARENT_RECIPE_STARTTIME` | Its start time: field 22 of `/proc/<pid>/stat`, in clock ticks since boot. |

A run that inherits both values is a nested run. It watches its parent run and
stops itself when the parent exits, so a fixer whose driver was killed does not
keep pushing on its own.

- **Linux only.** The watch uses `pidfd_open` and blocks in `poll()` with no
  timeout. It wakes on the parent's exit, not on an interval. On other
  platforms, and on kernels older than 5.3, nothing is watched and the run
  behaves as it did before.
- **Parent gone at start.** If the parent PID no longer exists, or its start
  time no longer matches (the PID was reused), the run is orphaned at once
  and no runner child is spawned.
- **Parent exits mid-run.** The run prints one line to stderr, writes the
  orphan record, and tears down the runner's process group with `SIGTERM`
  then `SIGKILL`, the same teardown used for a signal. The watch wakes at
  once, but the wrapper acts on it at the next check of its existing wait
  loop, which checks every 100 ms. The stop can therefore start up to 100 ms
  after the parent exits. This is the loop's check interval, not a timeout.
- **Exit code.** An orphan stop exits `75`. A signal received before the
  orphan stop keeps its own `128+n` exit code.
- **Bad values.** If only one variable is set, or either does not parse, or
  the PID is `1` or lower, the run prints a warning and is treated as a
  top-level run. A broken watch never stops a healthy run.

```console
orphaned: parent recipe run 412337 exited; stopping
```

The final correlation record has `status: "orphaned"`; see
[Recipe Run Correlation Reference](./recipe-run-correlation.md#final-statuses).
When `AMPLIHACK_AUTODRIVE_STATE_DIR` is set, the run also writes
`orphaned.json` there; see
[Orphaned rounds](./auto-drive-to-merge.md#orphaned-rounds). `recipe run`
removes `AMPLIHACK_AUTODRIVE_STATE_DIR` from its runner's environment, so runs
nested below that level never write the file.

A run started outside the tree does not inherit the parent variables.
`--detach` removes them from the pane's environment, and
`$(amplihack agent-binary --shell)` removes them from the command it
prefixes. The prefix matters for a raw `tmux new-session` started from a
step: if that command starts the tmux server, the server copies the step's
environment, parent variables included, into its global environment and gives
them to every later session (tmux(1), "GLOBAL AND SESSION ENVIRONMENT"). A
`recipe run` in such a session, started without the prefix, would watch a
parent that has exited and stop at once with exit `75`. Start it as
`tmux new-session -d "$(amplihack agent-binary --shell) amplihack recipe run ..."`,
or remove the variables with `env -u`.

The parent is always another `amplihack recipe run`, never the human's shell
or Claude session. A top-level run, including the pane root of a `--detach`
run, watches nothing: `--detach` removes `AMPLIHACK_PARENT_RECIPE_PID`,
`AMPLIHACK_PARENT_RECIPE_STARTTIME` and `AMPLIHACK_AUTODRIVE_STATE_DIR` from
the pane environment, so a detached run outlives the session that launched it.

## amplihack recipe validate

Validate a recipe YAML file without executing it.

```bash
amplihack recipe validate <FILE> [OPTIONS]
```

| Option | Description | Default |
| --- | --- | --- |
| `<FILE>` | Recipe YAML file to validate | required |
| `--verbose`, `-v` | Include validation details | `false` |
| `--format <format>`, `-f <format>` | Output format: `table`, `json`, or `yaml` | `table` |

```bash
amplihack recipe validate ~/.amplihack/.claude/recipes/default-workflow.yaml \
  --verbose
```

## amplihack recipe show

Display recipe metadata, context defaults, and steps.

```bash
amplihack recipe show <RECIPE> [OPTIONS]
```

| Option | Description | Default |
| --- | --- | --- |
| `<RECIPE>` | Recipe name or recipe YAML path | required |
| `--format <format>`, `-f <format>` | Output format: `table`, `json`, or `yaml` | `table` |
| `--no-steps` | Omit step details | `false` |
| `--no-context` | Omit context details | `false` |

```bash
amplihack recipe show default-workflow --format yaml
```

## Environment variables

| Variable | Applies to | Description |
| --- | --- | --- |
| `RECIPE_RUNNER_RS_PATH` | `run` | Absolute path to a `recipe-runner-rs` binary. Used before `$PATH` lookup. |
| `AMPLIHACK_RECIPE_RUN_ID` | runner child process | Stable UUID generated by `amplihack recipe run` and injected into the child. Treat as read-only. |
| `AMPLIHACK_STEP_TIMEOUT` | runner child process | Set by `--step-timeout`; read by the runner as a global per-step timeout hint. |
| `AMPLIHACK_RECIPE_HEARTBEAT_INTERVAL_SECONDS` | runner | Heartbeat interval in seconds. `0` disables heartbeat lines. |
| `AMPLIHACK_RECIPE_SNIPPET_LINES` | runner | Maximum recent output lines retained per child source and stream. |
| `AMPLIHACK_RECIPE_SNIPPET_BYTES` | runner | Maximum recent output bytes retained per child source and stream. |
| `AMPLIHACK_PARENT_RECIPE_PID` | nested `recipe run` | Set by `amplihack recipe run` on its runner child. A nested run that inherits it watches that parent. See [Nested runs and orphan stops](#nested-runs-and-orphan-stops). Treat as read-only. |
| `AMPLIHACK_PARENT_RECIPE_STARTTIME` | nested `recipe run` | Start time of the parent run, from field 22 of `/proc/<pid>/stat`. Guards against a reused PID. Treat as read-only. |
| `AMPLIHACK_AUTODRIVE_STATE_DIR` | nested `recipe run` | Set by `autodrive_loop.sh` on each round. An orphaned run writes `orphaned.json` here. `recipe run` removes it from its runner's environment, so only the round's own run writes the file. |
| `AMPLIHACK_RECIPE_LOG_JSONL` | runner | Optional path for structured JSONL lifecycle, heartbeat, output snippet, and failure events. |

See [Environment Variables](./environment-variables.md) for full details.

## Exit codes

| Code | Meaning |
| --- | --- |
| `0` | Command succeeded |
| `1` | Command failed, validation failed, recipe execution failed, runner missing, arguments were malformed, or a [removed context key](#removed-context-keys) was passed |
| `75` | Nested run stopped because its parent `amplihack recipe run` exited. See [Nested runs and orphan stops](#nested-runs-and-orphan-stops). |
| `128+n` | Run stopped by signal `n`, for example `130` for `SIGINT` and `143` for `SIGTERM` |

For development workflows, exit `0` also requires terminal-state evidence. A
code-change task that stops after planning, analysis, design, or worktree setup
exits `1` with `FAILED_MISSING_TERMINAL_EVIDENCE` unless it reaches
implementation/verification, publish/PR/merge state, or an explicit no-op state.
The terminal-state shell helper has its own direct invocation exit code `2` for
invalid helper usage, but `amplihack recipe run` surfaces helper failures as
recipe failures and exits nonzero, currently `1`. See
[Workflow Terminal-State Reference](./workflow-terminal-state.md).

## See also

- [amplihack recipe Reference](./recipe-command.md) - Detailed command behavior
- [Run a Recipe End-to-End](../howto/run-a-recipe.md) - Task-oriented usage
- [Recipe Run Correlation Reference](./recipe-run-correlation.md) - Stable run IDs and log pointer events
- [Recipe Runner Logging Reference](./recipe-runner-logging.md) - stderr progress, heartbeats, bounded snippets, and JSONL events
- [Workflow Terminal-State Reference](./workflow-terminal-state.md) - Development workflow completion evidence and visible failure contract
