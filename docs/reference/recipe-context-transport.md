---
title: "Recipe Context Transport Reference"
description: "How amplihack recipe run hands recipe context to recipe-runner-rs: the context_file_v1 capability, the private context file, the --set fallback, cleanup, errors, the detached and nested paths, and the bundled nested calls that pass text by file."
last_updated: 2026-11-14
review_schedule: quarterly
owner: rysweet
doc_type: reference
related:
  - ../concepts/prompt-text-out-of-argv.md
  - ./recipe-runner-validation.md
  - ../howto/pass-large-recipe-context.md
  - ./recipe-context-environment.md
  - ../howto/stop-background-jobs-by-pid.md
---

# Recipe Context Transport Reference

`amplihack recipe run` passes the recipe context (task text, PR bodies, JSON
values and any other user or agent text) to `recipe-runner-rs` by one of two
transports. Which one it uses depends on what the runner reports in
`--capabilities`. This page lists the rules for both. For the reasons behind
them, see [Why Prompt Text Stays Out of argv](../concepts/prompt-text-out-of-argv.md#recipe-context-on-the-runners-argv).

Issue: [#2582](https://github.com/rysweet/amplihack-rs/issues/2582).

## Contents

- [Choosing the transport](#choosing-the-transport)
- [File transport](#file-transport)
- [Set transport (fallback)](#set-transport-fallback)
- [Rules common to both transports](#rules-common-to-both-transports)
- [Argument and environment budget](#argument-and-environment-budget)
- [Lifetime and cleanup](#lifetime-and-cleanup)
- [Detached and nested runs](#detached-and-nested-runs)
- [Errors](#errors)
- [What the runner still puts on argv](#what-the-runner-still-puts-on-argv)
- [Checking a live run](#checking-a-live-run)

## Choosing the transport

amplihack already probes the runner with `recipe-runner-rs --capabilities`
before every run (see [Recipe runner validation](./recipe-runner-validation.md#capability-report)).
The same report decides the context transport. No second probe is made.

The decision is made once per run, after the context is final (including the
auto-drive keys added for #2329), in this order:

1. Reserved `AMPLIHACK_*` keys are refused. Nothing is written.
2. An empty context passes neither `--context-file` nor `--set`, as before.
3. If `capabilities` contains the exact string `context_file_v1`, the run uses
   the **file transport**.
4. Otherwise the run uses the **set transport**.

The match is exact on array members. `context_file_v10` or `context_file` does
not count. A failed or invalid probe follows the existing probe failure path;
it never selects a transport.

The two transports are never mixed. With the file transport, the runner's argv
holds no `--set` pair. With the set transport, no context file is written.

The choice is logged at debug level as a boolean only:

```text
DEBUG recipe context transport file=true
```

The fallback gives no warning, because it is the expected state on the
currently pinned runner (see [What the runner still puts on argv](#what-the-runner-still-puts-on-argv)).

| Runner reports | Runner argv | Context file |
| --- | --- | --- |
| `context_file_v1`, non-empty context | `--context-file <runtime_dir>/context/context.json` | Written, 0600 |
| No `context_file_v1`, non-empty context | `--set key=value` per key, as before #2582 | None |
| Any report, empty context | Neither flag | None |

## File transport

### Location

```text
<runtime_dir>/context/            mode 0700
<runtime_dir>/context/context.json  mode 0600
```

`<runtime_dir>` is the run's private runtime folder,
`runs/<run id>/runtime` under the work root, or `r/<id prefix>` (the first
12 hex digits of the run id) when `<that>/tmp` would be longer than 80 bytes (`runtime_dir_for` in
`cleanup/run_dir.rs`). The file is never placed in:

- `<runtime_dir>/tmp/`, which is exported to every step as `TMPDIR`;
- the run's log folder;
- the work root or the repository.

### Creation

- `context/` is created with a non-recursive create at mode 0700, then set to
  0700 explicitly, so the mode is exactly 0700 whatever the umask.
- If anything named `context` already exists in the runtime folder, a folder
  or a symbolic link, the run fails. An existing entry is never reused or
  followed.
- `context.json` is written through the same atomic 0600 writer used for run
  state: create-new, no following of symbolic links, mode 0600, fsync, and no
  rename over a link.
- The file is written once per run, before the first attempt. Retries and
  recovery attempts in the same run reuse it; it is never rewritten.

### Contents

One JSON object. Every value is a JSON string, exactly as amplihack holds it:

```json
{"repo_path":"/work/clone","task_description":"Fix the vitest hang in ui/","is_critical_code":"true"}
```

amplihack does not convert `"true"` or `"42"` into a boolean or a number. The
runner applies to string values in the file the same `parse_context_value`
rules it applies to `--set`, so a recipe condition sees the same type on both
transports. The parsing rule stays in the runner and is not duplicated here.

### Runner argv

```text
recipe-runner-rs <recipe> --context-file <runtime_dir>/context/context.json [fixed flags]
```

Only the flag and the path. No key or value appears.

## Set transport (fallback)

Used when the runner does not report `context_file_v1`. The argv is identical,
byte for byte, to the one amplihack built before #2582:

```text
recipe-runner-rs <recipe> --set repo_path=/work/clone --set task_description=... [fixed flags]
```

The #1570 size checks and messages are unchanged:

- each `key=value` pair must fit in one argument (`MAX_ARG_STRLEN`, 131072
  bytes);
- the whole argv plus environment must fit in `ARG_MAX`.

On this transport, values are readable in `/proc/<pid>/cmdline` and are matched
by `pkill -f`, as before.

## Rules common to both transports

| Rule | File transport | Set transport |
| --- | --- | --- |
| Reserved `AMPLIHACK_*` keys refused before anything is written | Yes | Yes |
| Per-pair `MAX_ARG_STRLEN` check | No | Yes |
| New size cap | None; intake is already capped at 16 MiB | Unchanged |
| `RECIPE_VAR_*` and uppercase environment mirror (`context_env_pairs`) | Unchanged | Unchanged |
| Dry run | Prints neither the file contents nor the context map | Unchanged |

The environment mirror described in
[Recipe Context Environment Export](./recipe-context-environment.md) still
runs on both transports. The environment is not part of the command line, so
`pkill -f` and `pgrep -f` do not match it. Processes running as the same user
can read `/proc/<pid>/environ`; that is outside the threat model.

## Argument and environment budget

The `ARG_MAX` check counts the context's argv cost and gives the rest to the
environment mirror.

| Transport | Context argv cost (bytes) |
| --- | --- |
| None (empty context) | `0` |
| File | `len("--context-file") + 1 + len(path) + 1 + 2 * size_of::<usize>()` |
| Set | Sum over pairs, as before #2582 |

The environment budget is `budget - context_argv_cost`. On the file transport
the cost is a few hundred bytes whatever the size of the context, so more room
is left for the environment mirror.

## Lifetime and cleanup

The file is owned by a guard created when the transport is chosen.

- The guard is dropped after the last runner child has been reaped and before
  the runtime folder is removed: explicitly on a normal exit, and by
  declaration order on an early return.
- On drop it removes `context.json`, then the empty `context/` folder. It never
  removes recursively, and it ignores errors.
- The runtime folder guard (or the legacy temporary folder) also removes the
  file on drop. This covers panics and SIGTERM or SIGINT handled through the
  run's stop signal.
- After a SIGKILL of `amplihack` no cleanup can run. The file stays, 0600 in a
  0700 folder, until the existing sweep removes `runs/<run id>/runtime`. This
  is accepted. See [Clean Up After Recipe Runs](../howto/clean-up-after-recipe-runs.md).

## Detached and nested runs

`amplihack recipe run --detach` starts a tmux session that runs
`amplihack recipe detached-child`. The tmux command and the detached child's
argv hold paths only: the spec file and the log path. The context goes into
`<session>.context.json` at 0600, as it did before #2582. The detached child
then runs an ordinary `recipe run`, which applies the transport rules above.

A nested `amplihack recipe run` started from a bash step chooses its own
transport in the same way. Give it context through `--context-file` or
`--context-from-file`, not `-c key="$VALUE"`, so the value is not on the nested
`amplihack` process's own argv. See
[Give a nested recipe the context it needs](../howto/pass-large-recipe-context.md#give-a-nested-recipe-the-context-it-needs).

### Bundled nested calls

The bundled scripts and recipes that start a nested `amplihack recipe run`
pass agent output, logs and task text with `--context-from-file`. Each writes
the values to files at mode 0600 in a new folder at mode 0700, passes
`--context-from-file key=<path>`, and removes each file by its literal path and
then the folder once the nested run has exited. Fixed tokens, labels, numbers
and paths still go through `-c`. `amplifier-bundle/tools/workflow_hook_repair.sh`
already worked this way; the two callers below now do the same.

| Caller | Nested recipe | Keys passed by file | Private folder |
| --- | --- | --- | --- |
| `amplifier-bundle/tools/autodrive_loop.sh`, loop-health evaluator | the loop's health recipe | `loop_history`, `loop_last_round_output`, `loop_findings_current`, `loop_findings_previous`, `loop_test_signal`, `loop_test_signal_previous`, `loop_ci_signal`, `loop_ci_signal_previous` | `mktemp -d "${STATE_DIR}/${LOOP_NAME}-${ROUND_LABEL}-ctx.XXXXXX"`, created under the script's `umask 077` |
| `amplifier-bundle/recipes/quality-loop.yaml`, `drive-red-prs-to-fix` dispatch | `default-workflow` | `task_description` | `mktemp -d` under `$TMPDIR` (mode 0700), one per dispatched PR; the file is written in a subshell under `umask 077` |

In `autodrive_loop.sh`, the evaluator's exit status is captured before the
files are removed, and a failed `rmdir` does not fail the round. If the folder
or a file cannot be created, the loop stops with the escalation label
`EVALUATOR_CONTEXT_FAILED` and starts no evaluator; the message names the
failure, not the values, and says it is a local file write failure, not a
judgement about progress. A loop stopped while the evaluator is running can
leave its `-ctx.XXXXXX` folder behind. The folder is private, holds the same
text as the round log beside it, and its unique name never blocks a later
round.

In `quality-loop.yaml`, the `NUM` and `BRANCH` checks still run before the
dispatch, and the dispatch's exit status is kept while the file and folder are
removed, so a failed dispatch still fails the step.

The nested `amplihack` process then chooses its transport by the rules above.
On the pinned runner it still passes every value to `recipe-runner-rs` as
`--set`; see [What the runner still puts on argv](#what-the-runner-still-puts-on-argv).

## Errors

No error message or log line contains a context value. Keys are escaped before
they are shown.

| Message | Cause | What happens |
| --- | --- | --- |
| `failed to write recipe context file <path>: <io error>` | The runtime folder is not writable, the disk is full, or `context` already exists in the runtime folder | The run fails. It does not fall back to `--set`. |
| `recipe context key "<KEY>" is reserved: the AMPLIHACK_ namespace cannot be set from recipe context` (existing text) | The context holds an `AMPLIHACK_*` key | The run fails on both transports before anything is written. |
| `recipe context value for key "<KEY>" is <N> bytes; ... cannot exceed 131072 bytes` (existing text) | One pair is too large for one argument | Set transport only. On the file transport the value is passed. |

A run that should use the file transport and cannot write the file fails
rather than putting values on argv. This is deliberate: falling back would
expose the context exactly when the runner can avoid it.

## What the runner still puts on argv

The amplihack side is complete, but the protection depends on the runner:

- **Item A, `--context-file`.** The pinned runner revision (`90286bc`, see
  `claude-plugin/recipe-runner.rev`) does not report `context_file_v1`. Until
  a runner with the option is released and the pin moves, every run takes the
  set transport and context values remain on the runner's argv.
- **Item B, agent prompts.** The runner passes each agent step's prompt, which
  contains the task text, to the agent launcher as `-p <prompt>`, so the first
  8 KiB of each prompt is on the `amplihack claude` wrapper's argv. That has to
  be fixed in the runner, behind a capability such as `agent_prompt_file_v1`.
  Until then, recipe prompts avoid naming tools or processes that an agent
  might select by pattern; see
  [Stop Background Jobs by PID](../howto/stop-background-jobs-by-pid.md#why-the-recipe-copies-name-no-tools).

The bundled nested calls (see [Bundled nested calls](#bundled-nested-calls))
keep text off the nested `amplihack` process's argv on every runner version.
They do not change items A and B: the nested run's own runner and agent
wrappers behave as described here.

Item A is tracked in
[rysweet/amplihack-recipe-runner#211](https://github.com/rysweet/amplihack-recipe-runner/issues/211)
and item B in
[rysweet/amplihack-recipe-runner#205](https://github.com/rysweet/amplihack-recipe-runner/issues/205).
The interface amplihack expects for item A:

| Property | Requirement |
| --- | --- |
| Option | `--context-file <PATH>`, read once at start |
| Contents | A JSON object. String values go through `parse_context_value`; other JSON types are used as they are |
| Precedence | `--set` pairs given as well override keys from the file |
| Capability | `context_file_v1` in `--capabilities` |
| Ownership | The runner never deletes or rewrites the file |
| Opening | `O_NOFOLLOW`, regular file only |
| Errors | An unreadable file or a non-object exits with `BAD_ARGS` |
| Logging | Contents are never logged, and loaded values never go onto a child's argv |

## Checking a live run

List every process below a running `amplihack recipe run` and look for a
distinctive word from the task text. Replace `12345` with the PID of the
`amplihack` process you started:

```bash
root=12345
word=ZQXV2582TASKWORD
queue=$root
while [ -n "$queue" ]; do
  set -- $queue; pid=$1; shift; queue="$*"
  tr '\0' ' ' < /proc/$pid/cmdline 2>/dev/null | grep -q -- "$word" && echo "found in $pid"
  for c in /proc/$pid/task/*/children; do queue="$queue $(cat "$c" 2>/dev/null)"; done
done
# Output with the file transport: nothing
```

On the pinned runner the check finds the word in the `recipe-runner-rs`
process and in agent launchers; that is the fallback described above.

The behaviour test `crates/amplihack-cli/tests/issue_2582_context_off_argv.rs`
does the same walk in Rust against a runner stub, for three cases: a runner
that reports `context_file_v1`, a runner that does not (the `--set` fallback),
and a `recipe detached-child` running a nested `recipe run`.

The behaviour test
`crates/amplihack-cli/tests/issue_2582_nested_context_off_argv.rs` (Linux only)
runs two rounds of `autodrive_loop.sh` with a non-crusty loop name and a stub
`AMPLIHACK_BIN`. When the stub is started as the loop-health evaluator, it
copies `/proc/$$/cmdline` and each `--context-from-file` file into the test's
temporary folder and exits. The test plants sentinel strings in the findings,
round log, history and signals, then checks that:

- none of the sentinels, and none of the eight `key=` prefixes, appear in the
  recorded command line;
- the copied context files contain the sentinels, and were at mode 0600 in a
  folder at mode 0700;
- no `-ctx.` folder is left in the loop's state folder.

The run-level test for a failed context transport is in
`crates/amplihack-cli/src/commands/recipe/run/tests_execute.rs`. With a runner
stub that reports `context_file_v1`, a run that fails when the context
transport is chosen returns an error that does not contain the context value,
makes no runner call after `--capabilities` (so no `--set` fallback), records
a failed run in `run.json`, and finishes its admission. The test reaches this
path with a reserved key; a context file that cannot be written fails at the
same point and takes the same path.

## Related

- [Why Prompt Text Stays Out of argv](../concepts/prompt-text-out-of-argv.md) - The threat model and the #2030 and #2582 incidents
- [Recipe runner validation](./recipe-runner-validation.md) - The capability report this page reads
- [Pass Large Recipe Context and Agent Text](../howto/pass-large-recipe-context.md) - Getting context into `amplihack recipe run`
- [Stop Background Jobs by PID](../howto/stop-background-jobs-by-pid.md) - The agent guidance from #2582 part 2
