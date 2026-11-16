---
title: "Recipe Context Transport Reference"
description: "How amplihack recipe run hands recipe context to recipe-runner-rs: the context_file_v1 capability, the private 0600 context file, input validation, the --set compatibility path, cleanup, errors, the detached and nested paths, and the bundled nested calls that pass text by file."
last_updated: 2026-11-16
review_schedule: quarterly
owner: rysweet
doc_type: reference
related:
  - ../concepts/prompt-text-out-of-argv.md
  - ./recipe-runner-validation.md
  - ./agent-prompt-channels.md
  - ../howto/pass-large-recipe-context.md
  - ./recipe-context-environment.md
  - ../howto/stop-background-jobs-by-pid.md
---

# Recipe Context Transport Reference

`amplihack recipe run` passes the recipe context (task text, PR bodies, JSON
values and any other user or agent text) to `recipe-runner-rs` in a private
file. The runner's command line carries the flag and the path, and no key and
no value. This page lists the rules, the file's location and mode, the input
validation, the compatibility path for an older runner, cleanup, and the
errors a user can see. For the reasons behind them, see
[Why Prompt Text Stays Out of argv](../concepts/prompt-text-out-of-argv.md#recipe-context-on-the-runners-argv).

Issue: [#2582](https://github.com/rysweet/amplihack-rs/issues/2582).

Two options share the name `--context-file`, on either side of amplihack. The
one a user passes to `amplihack recipe run` names a JSON file of context to
read (see
[Pass a whole context as JSON](../howto/pass-large-recipe-context.md#pass-a-whole-context-as-json));
the one on this page is the argument amplihack passes to the runner. A run can
use either, both or neither: whatever context amplihack ends up holding goes
to the runner by the transport chosen below.

## Contents

- [Choosing the transport](#choosing-the-transport)
- [Seeing which transport a run used](#seeing-which-transport-a-run-used)
- [File transport](#file-transport)
- [Input validation](#input-validation)
- [Payload parity](#payload-parity)
- [Set transport (older runners)](#set-transport-older-runners)
- [Rules common to both transports](#rules-common-to-both-transports)
- [Argument and environment budget](#argument-and-environment-budget)
- [Lifetime and cleanup](#lifetime-and-cleanup)
- [Detached and nested runs](#detached-and-nested-runs)
- [Bundled nested calls](#bundled-nested-calls)
- [Errors](#errors)
- [What the runner puts on argv](#what-the-runner-puts-on-argv)
- [What this does and does not protect](#what-this-does-and-does-not-protect)
- [Checking a live run](#checking-a-live-run)
- [Tests](#tests)

## Choosing the transport

amplihack already probes the runner with `recipe-runner-rs --capabilities`
before every run (see [Recipe runner validation](./recipe-runner-validation.md#capability-report)).
The same report decides the context transport. No second probe is made.

The pinned runner, `2833a0b8` in `claude-plugin/recipe-runner.rev`
(recipe-runner-rs v0.7.0), reports `context_file_v1`, so every run on the
pinned runner takes the file transport. A runner selected through
`RECIPE_RUNNER_RS_PATH`, or an older installed runner the freshness check
kept rather than replaced, may not report it; the `--set` path exists for
those cases.

A report that fails validation ends the run before any transport is chosen,
so a degraded runner does not fall back to `--set`; the choice below is made
only among reports that already validated.

The decision is made once per run, after the context is final and after the
workspace is prepared, in this order:

1. The auto-drive authorization keys go into the context last: `no_merge`,
   `autodrive_dry_run`, `autodrive_authorization`,
   `autodrive_invocation_file` and `autodrive_state_dir`. They are written
   after the user's context and after admission, so no recipe context value
   can replace one (#2329), and whichever transport is chosen carries what
   this step produced.
2. A reserved key ends the run with an error. Nothing is written. See
   [Input validation](#input-validation).
3. An empty context passes neither `--context-file` nor `--set`.
4. If `capabilities` contains the exact string `context_file_v1`, the run uses
   the **file transport**.
5. Otherwise the run uses the **set transport**.

The match is exact on array members. `context_file_v10` and `context_file` do
not count. A failed or invalid probe follows the existing probe failure path;
it never selects a transport.

The transport is chosen once and applied on every spawn attempt of that run,
including recovery attempts. The two are never mixed: with the file transport
the runner's argv holds no `--set` pair, and with the set transport no context
file is written.

| Runner reports | Runner argv | Context file |
| --- | --- | --- |
| `context_file_v1`, non-empty context | `--context-file <runtime_dir>/context/context.json` | Written, 0600 |
| No `context_file_v1`, non-empty context | `--set key=value` per key, as before #2582 | None |
| Any report, empty context | Neither flag | None |

## Seeing which transport a run used

The transport is recorded once in the run's log, at info level, by name:

```text
INFO recipe context transport=file
```

The record holds the transport name and nothing else: no key, no value and no
count derived from one. It is there so that a run which took the set transport
is visible to whoever reads the log. The pinned runner reports the capability
on every host amplihack runs on, so in practice `transport=set` means a runner
older than v0.7.0 was in use: usually one supplied through
`RECIPE_RUNNER_RS_PATH`, more rarely an older installed runner the freshness
check kept, since a failed check is a warning rather than an error and a
recorded skip holds across launches within its cooldown. Either way it is a
local condition that would otherwise show up only in `/proc`. See
[Set transport](#set-transport-older-runners).

## File transport

### Location

```text
<runtime_dir>/context/              mode 0700
<runtime_dir>/context/context.json  mode 0600
```

`<runtime_dir>` is the run's private runtime folder, `runs/<run id>/runtime`
under the work root, or `r/<id prefix>` (the first 12 hex digits of the run
id) when `<that>/tmp` would be longer than 80 bytes (`runtime_dir_for` in
`cleanup/run_dir.rs`). The file is never placed in:

- `<runtime_dir>/tmp/`, which is exported to every step as `TMPDIR`;
- `artifacts/`, which is archived, bundled and attached to run reports;
- the run's log folder;
- the work root or the repository.

The path is built from the runtime folder and the two fixed names above. No
part of it comes from a context value, an environment variable or a flag,
because the path itself lands on the runner's argv. The file name encodes no
key name and no digest of a value.

### Creation

- `context/` is created with a non-recursive create at mode 0700, then set to
  0700 explicitly, so the mode is exactly 0700 whatever the umask.
- If anything named `context` already exists in the runtime folder, a folder,
  a file or a symbolic link, the run ends with an error. An existing entry is
  never reused and never followed.
- `context.json` is written through the same atomic 0600 writer used for run
  state: `create_new`, `O_NOFOLLOW`, mode 0600, fsync, and no rename over a
  link.
- The file is written once per run, before the first spawn attempt. Retries
  and recovery attempts in the same run reuse it; it is never rewritten.

There is no window at a wider mode. The create requests 0700, so the mode the
folder lands at is `0700 & ~umask` — never more than 0700, whatever the umask
is. The explicit chmod is there for the other direction: a restrictive umask
such as 0177 would leave the folder at 0600, which the run itself could not
traverse. The chmod raises a too-narrow mode to exactly 0700; it never has to
narrow a permissive one. The parent runtime folder is itself 0700 and owned by
the run's user, which `runtime_dir_for` guarantees.

### Contents

One JSON object. Every value is a JSON string, exactly as amplihack holds it:

```json
{"repo_path":"/work/clone","task_description":"Fix the vitest hang in ui/","is_critical_code":"true"}
```

amplihack does not convert `"true"` or `"42"` into a boolean or a number, and
does not uppercase or trim keys. See [Payload parity](#payload-parity).

### Runner argv

```text
recipe-runner-rs <recipe> --context-file <runtime_dir>/context/context.json [fixed flags]
```

Only the flag and the path. No key and no value appears.

## Input validation

The file transport adds no validation of its own. It reuses the checks that
were already in place for `--set`, and runs the one of them that can end a run
before any file exists.

### Reserved keys

`refuse_reserved_keys` runs inside `ContextTransport::choose`, before the
folder or the file is created, so a context holding a reserved key leaves
nothing on disk. One predicate, `is_reserved_context_key`, serves both
transports, and both call it: `choose` runs it before a transport is picked,
and `pass_context` runs it again on the `--set` arm, so either path alone
refuses the key. The message names the key, escaped, and no value.

A key is reserved when its uppercase form is any of:

| Reserved form | Why | Issue |
| --- | --- | --- |
| Anything beginning with `AMPLIHACK_` | The namespace belongs to the subprocess environment builder; a context key here could repoint a launch, including the Claude profile it selects | #1567 |
| A name in `amplihack_git::REPOSITORY_ENV_VARS` (`GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE`, `GIT_OBJECT_DIRECTORY` and the rest) | Git repository routing. A context value here would point a step's `git` at another repository | #2096 |
| Anything beginning with `GIT_CONFIG` | `GIT_CONFIG_GLOBAL`, `GIT_CONFIG_COUNT` and the `GIT_CONFIG_KEY_<n>`/`GIT_CONFIG_VALUE_<n>` pairs carry configuration, and git configuration includes execution-bearing settings such as `core.pager` and the `*.sshCommand` family | #2096 |
| `GIT_EXEC_PATH` or `GIT_TEMPLATE_DIR` | Both name a folder git executes from or copies hooks out of | #2096 |

The command policy already removes these names from a runner child's
environment before launch. Refusing them as context keys closes the other
door, through which a recipe could have put them back.

The longer denylist of `PATH`, `HOME`, `LD_PRELOAD`, `BASH_ENV` and the rest
belongs to the environment mirror alone, where those names would change how a
step's own shell and tools behave. It is not a transport check: those keys
reach the recipe as ordinary context on either transport and are skipped when
the mirror is built. See
[Reserved-name denylist](./recipe-context-environment.md#reserved-name-denylist).

### NUL bytes

A value holding a NUL is rejected where it enters, in `context_sources.rs`,
for `--context-file` and `--context-from-file` sources:

```text
--context-from-file /tmp/task.md: value for key 'task_description' contains a NUL byte
```

A NUL cannot arrive through `-c key=value` at all, because `execve` would have
rejected amplihack's own argument. By the time a transport is chosen, no value
can hold one, so the transport does not check again.

### Size

| Transport | Bound | Where it is applied |
| --- | --- | --- |
| Both | 16 MiB per file-sourced value (`CONTEXT_SOURCE_MAX_BYTES`) | Intake |
| Set | 131072 bytes per `key=value` pair (`MAX_ARG_STRLEN`) | When the arguments are built |
| Set | `ARG_MAX` for argv plus environment | When the arguments are built |
| File | None beyond intake | The flag and the path cost a few hundred bytes whatever the context holds |

The file transport deliberately gains no payload cap of its own. The intake
bound already covers every value large enough to matter, and a second limit
here would be a check with no case behind it.

## Payload parity

A recipe condition sees the same value on either transport. Every value is
written as a JSON string, and the runner applies `parse_context_value` to a
string value from the file exactly as it applies it to a `--set` value. Native
JSON types would diverge: `{"n": 3}` would reach the recipe as the number 3
whichever text produced it, while `--set n=03` yields the string `03`.

This property is the reason the payload is strings and not native JSON. The
parsing rule stays in the runner and is not duplicated in amplihack.

The runner's own precedence is unchanged: `--set` pairs given alongside
`--context-file` override keys from the file. amplihack never passes both.

## Set transport (older runners)

Used when the runner's report does not list `context_file_v1`. With the
pinned revision that leaves exactly one case: a runner supplied through
`RECIPE_RUNNER_RS_PATH` that is older than v0.7.0. The pinned runner reports
the capability on every host amplihack runs on.

There is no second case, because amplihack itself is Unix-only. `amplihack-cli`
has no non-Unix build: `mod context_transport` is declared unconditionally and
the module imports `std::os::unix::fs`, as hundreds of other files in the crate
do, and CI compiles Linux and macOS targets only. So `transport=set` in a run
log has exactly one explanation, and it is the one above.

This is a compatibility path for that one case, not a transport to rely on.
The argv is identical, byte for byte, to the one amplihack built before
#2582:

```text
recipe-runner-rs <recipe> --set repo_path=/work/clone --set task_description=... [fixed flags]
```

The #1570 size checks and messages are unchanged:

- each `key=value` pair must fit in one argument (`MAX_ARG_STRLEN`, 131072
  bytes);
- the whole argv plus environment must fit in `ARG_MAX`.

On this transport, values are readable in `/proc/<pid>/cmdline` and are matched
by `pkill -f`, as they were before #2582.

## Rules common to both transports

| Rule | File transport | Set transport |
| --- | --- | --- |
| Reserved keys end the run before anything is written | Yes | Yes |
| NUL byte in a value ends the run at intake | Yes | Yes |
| Per-pair `MAX_ARG_STRLEN` check | No | Yes |
| `RECIPE_VAR_*` and uppercase environment mirror (`context_env_pairs`) | Unchanged | Unchanged |

The environment mirror described in
[Recipe Context Environment Export](./recipe-context-environment.md) still
runs on both transports. The environment is not part of the command line, so
`pkill -f` and `pgrep -f` do not match it. Its values are readable by a
process running as the same user through `/proc/<pid>/environ`, as they were
before #2582; see
[What this does and does not protect](#what-this-does-and-does-not-protect).

### Dry runs

`--dry-run` writes the context file as a real run does and passes
`--context-file` next to `--dry-run`, so the arguments the runner is given are
the ones a real run would give it. A dry run's runtime folder is a private
`amplihack-dry-*` folder under `TMPDIR` rather than `runs/<run id>/runtime`;
inside it `context/` is 0700 and `context.json` is 0600, and both are gone
when the command returns. Neither the file's contents nor the context map is
printed.

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

When the cost leaves too little room, the run ends before any spawn. The
message names neither a transport nor a value:

```text
recipe context totals 139264 bytes of runner arguments, more than the 131072 bytes
of ARG_MAX left after the inherited environment. Pass large values as file paths
in the context instead
```

On the file transport this message is unreachable in practice, which is the
point of the cost table above: the same context that could not be spawned as
`--set` arguments fits in a path.

## Lifetime and cleanup

The file is owned by a guard created when the transport is chosen.

- The guard is released by the run's ordered teardown as the step named
  `recipe context file`, immediately before the `run report` step: after the
  last runner child has been reaped, and before the workspace is finished. On
  an early return the declaration order gives the same result, because the
  transport is declared after the workspace. Both the named step and the
  declaration order carry a comment saying why, because no merge tool or
  compiler can check this ordering.
- On drop it removes `context.json` by its literal name, then the now-empty
  `context/` folder with `rmdir`. It never removes recursively, and it ignores
  errors, so an entry it did not create is left where it is rather than
  deleted.
- A `context.json.tmp` sibling exists only if the atomic writer failed between
  creating it and renaming it, which is also the only case in which the run
  ends rather than spawning. The guard's `rmdir` then fails and `context/`
  stays until the runtime folder guard below removes the runtime folder and
  its contents. The writer removes a stale `.tmp` by name before it creates
  one, inside a folder this run has just created, so a leftover never becomes
  the file a later run reads.
- The runtime folder guard (or the legacy temporary folder) also removes the
  file on drop. This covers panics and SIGTERM or SIGINT handled through the
  run's stop signal.
- After a SIGKILL of `amplihack` no cleanup can run. The file stays, 0600 in a
  0700 folder, until the existing sweep removes `runs/<run id>/runtime`. The
  sweep removes the folder and its contents whatever they are named, which is
  why the context file belongs under the runtime folder and nowhere else. See
  [Clean Up After Recipe Runs](../howto/clean-up-after-recipe-runs.md).

## Detached and nested runs

`amplihack recipe run --detach` starts a tmux session that runs
`amplihack recipe detached-child`. The tmux command and the detached child's
argv hold paths only: the spec file and the log path. The context goes into
`<session>.context.json` at 0600, as it did before #2582. The detached child
then runs an ordinary `recipe run`, which applies the transport rules above.

A nested `amplihack recipe run` started from a bash step chooses its own
transport in the same way. Give it context through `--context-file` or
`--context-from-file`, not `-c key="$VALUE"`, so the value is not on the
nested `amplihack` process's own argv. See
[Give a nested recipe the context it needs](../howto/pass-large-recipe-context.md#give-a-nested-recipe-the-context-it-needs).

## Bundled nested calls

The bundled scripts and recipes that start a nested `amplihack recipe run`
pass every free-text value with `--context-from-file`. Fixed tokens, labels,
exit codes, refs and paths still go through `-c`: they are short, they are not
user or agent text, and a pattern taken from one would match nothing useful.

The rule is per value, not per key. A key whose value is a label today and a
paragraph of agent output tomorrow goes by file.

### How each caller writes its files

There is no shared helper. Each caller writes its own files. Six of the seven
share the same four properties, and the properties are what the tests check:

- the folder comes from `mktemp -d`, so its name is never taken from a context
  value;
- the folder is 0700 and each file 0600, either from a `umask 077` subshell or
  from the loop's own `umask 077`;
- every value reaches its file through `printf` or a redirect — a shell builtin
  or the shell itself — so it is never an argument to another command and never
  appears in any argv;
- the nested run's exit status is captured before the files are removed, and a
  failed removal never changes the round's outcome.

The seventh, `merge_ready_prior`, differs on the first and the fourth; both
differences are deliberate and are described under the table.

| Caller | Function | Keys passed by file |
| --- | --- | --- |
| `amplifier-bundle/tools/autodrive_escalate.sh`, called from `autodrive_loop.sh` | `eval_ctx_write` / `eval_ctx_remove` | The eight loop-health keys: `loop_history`, `loop_last_round_output`, `loop_findings_current`, `loop_findings_previous`, `loop_test_signal`, `loop_test_signal_previous`, `loop_ci_signal`, `loop_ci_signal_previous` |
| `amplifier-bundle/tools/autodrive_state.sh`, called from `autodrive_loop.sh` | `autodrive_use_task_file` | `task_description`, copied once to a 0600 file named by every round |
| `amplifier-bundle/tools/autodrive_completion_context.sh` | `completion_context` | `loop_history`, `loop_baseline_ref`, `loop_findings_previous`, `loop_test_signal_previous`, `loop_ci_signal_previous`, `loop_clean_token` |
| `amplifier-bundle/tools/workflow_hook_repair.sh` | `hook_repair_evaluate` | `loop_last_round_output` (the tail of the hook output), `loop_history` |
| `amplifier-bundle/recipes/quality-loop.yaml`, `drive-red-prs-to-fix` | the step itself | `task_description`, which holds the PR title and the failing check names |
| `amplifier-bundle/recipes/quality-audit-cycle.yaml`, `run-recursive-cycle` | the step itself | `task_description`, `cycle_history` |
| `amplifier-bundle/tools/autodrive_escalate.sh`, called from `autodrive_loop.sh` in the merge-ready loop only | `merge_ready_prior` | `prior_writeup`, the previous round's write-up ([#2575](https://github.com/rysweet/amplihack-rs/issues/2575)) |

Short scalars stay on argv deliberately: `loop_child_exit_code`,
`loop_round_verdict`, `loop_round_label`, `loop_name`, `loop_repo_path`,
`loop_agent_working_dir`, `loop_health_enforce` and `pr_number`. The same key
can differ between callers, because the rule is per value: the auto-drive loop
sends `loop_findings_current` by file, since its value is a paragraph of
findings text, while `workflow_hook_repair.sh` sends the same key with `-c`,
since its value is a list of hook identifiers.

`merge_ready_prior` is the exception to the first and fourth properties. It
writes no `mktemp -d` folder: the file lives in the loop's own `--state-dir`,
which `autodrive_loop.sh` creates under its global `umask 077`. It creates the
file as `mktemp "${STATE_DIR}/.prior_writeup.XXXXXX"` in a `umask 077`
subshell, fills it with `cat -- "$1" > "$t"` — a redirect, so the write-up text
is never an argument — and `mv -f` it into place, so no round ever reads a
half-written file. It copies the source only when that source is a regular
file and not a symbolic link; anything else leaves the file empty rather than
following the link. And it is not removed when the round ends: the next round replaces it in
place, because this file is the loop's record of the previous round rather than
a per-round scratch copy. In round one there is no prior write-up, the file is
created empty, and the key is passed anyway, so the step never sees a missing
key.

`eval_ctx_write` writes its eight files in the loop's own `umask 077` and
relies on it; `completion_context` and the `quality-loop.yaml` step each set
`umask 077` in a subshell, and `completion_context` adds `set -C` so a planted
name is not written through. `eval_ctx_remove` and the `quality-loop.yaml`
step remove each file by its literal name and then `rmdir` the folder;
`autodrive_completion_context.sh` removes its `mktemp -d` folder with
`rm -rf --` on the path it created itself.

### Failure handling

In `autodrive_loop.sh`, a folder or file that cannot be written stops the loop
with the escalation label `EVALUATOR_CONTEXT_FAILED` and starts no evaluator.
The message names the failure, not the values, and says it is a local file
write failure rather than a judgement about progress. The evaluator's exit
status is captured before `eval_ctx_remove` runs, and a failed `rmdir` does not
fail the round. A loop stopped while the evaluator is running can leave its
folder behind; the folder is private, holds the same text as the round log
beside it, and its unique name never blocks a later round.

In `quality-loop.yaml` the `NUM` and `BRANCH` checks still run before the
dispatch, and the dispatch's exit status is kept while the file and folder are
removed, so a failed dispatch still fails the step.

## Errors

No error message and no log line contains a context value. Keys are escaped
before they are shown. The same disclosure rule applies to tracing fields,
correlation records and `run.json`: a key, a path, a byte count and an I/O
error may appear; a context value, in whole or in part, may not.

| Message | Cause | What happens |
| --- | --- | --- |
| `failed to write recipe context file <path>: <io error>` | The runtime folder is not writable, the disk is full, or `context` already exists in the runtime folder | The run ends with an error. It does not fall back to `--set`. |
| `recipe context key "<KEY>" is reserved: the AMPLIHACK_ namespace and Git execution configuration cannot be set from recipe context` | The context holds a key reserved by `is_reserved_context_key`: the `AMPLIHACK_` namespace, a Git repository selector, `GIT_CONFIG*`, `GIT_EXEC_PATH` or `GIT_TEMPLATE_DIR` | The run ends with an error on both transports, before anything is written. |
| `--context-from-file <path>: value for key '<KEY>' contains a NUL byte` | A file-sourced value holds `\0` | The run ends at intake, before the capability probe. |
| `recipe context value for key "<KEY>" is <N> bytes; ... cannot exceed 131072 bytes` (existing text) | One pair is too large for one argument | Set transport only. On the file transport the value is passed. |
| `recipe context totals <N> bytes of runner arguments, ...` | The context's argv cost leaves too little of `ARG_MAX` for the environment | Both transports, though unreachable in practice on the file transport. |

A run that should use the file transport and cannot write the file ends with
an error rather than putting values on argv. This is deliberate: falling back
would expose the context exactly when the runner can avoid it.

## What the runner puts on argv

On the pinned runner, nothing amplihack hands the runner reaches any process's
command line as text.

| Child | What its argv holds |
| --- | --- |
| `recipe-runner-rs` | `--context-file <path>` and fixed flags |
| `amplihack claude` for an agent step | `--system-prompt-file <path>`, a bare `-p`, `--add-dir .`, and a model name. The task goes to the agent on stdin |
| `amplihack codex` for an agent step | `exec --output-last-message <path>`, a model name and `-`. The task goes to the agent on stdin |
| `amplihack copilot` for an agent step | `--allow-all-tools`, `--add-dir <dir>`, and `-p` with a fixed pointer sentence naming the prompt file. The sentence is a constant plus a path and carries no recipe content |
| A nested `amplihack recipe run` from a bundled caller | `--context-from-file key=<path>`, labels, exit codes and paths |

This is the `agent_prompt_file_v1` capability, which the pinned runner reports
alongside `context_file_v1`. Both came from runner changes
[#211](https://github.com/rysweet/amplihack-recipe-runner/issues/211) and
[#205](https://github.com/rysweet/amplihack-recipe-runner/issues/205), and both
took effect in amplihack when the pin moved to v0.7.0.

The interface amplihack expects for `--context-file`:

| Property | Requirement |
| --- | --- |
| Option | `--context-file <PATH>`, read once at start |
| Contents | A JSON object. String values go through `parse_context_value`; other JSON types are used as they are |
| Precedence | `--set` pairs given as well override keys from the file |
| Capability | `context_file_v1` in `--capabilities` |
| Ownership | The runner never deletes or rewrites the file |
| Opening | `O_NOFOLLOW`, regular file only |
| Errors | An unreadable file or a non-object exits with `BAD_ARGS`, with no excerpt of the file in the message |
| Logging | Contents are never logged, and loaded values never go onto a child's argv |

Text a caller puts on their own command line is outside this. See
[What argv still contains](./agent-prompt-channels.md#what-argv-still-contains)
for the five remaining cases, all of them a prompt typed or scripted by the
caller into an interactive wrapper invocation.

## What this does and does not protect

On Linux `/proc/<pid>/cmdline` is mode 0444 and readable by every local
account, while `/proc/<pid>/environ` is 0400 and readable only by the process's
own user. Moving context into a 0600 file
closes the wider of the two disclosure channels and removes the handle a
pattern kill had on context text. It leaves exposure no broader than the
environment mirror the same values already travel in.

Confidentiality against a process running as the run's own user but outside
the run is not achieved and is not claimed. Three copies of the same values
remain readable by that user, and each of them was there before #2582:

| Copy | Where | Lifetime | Matched by `pkill -f` |
| --- | --- | --- | --- |
| The environment mirror | `RECIPE_VAR_*` and the uppercase names in `/proc/<pid>/environ`, mode 0400 | The runner child's life | No |
| The context file | `<runtime_dir>/context/context.json`, mode 0600 in a 0700 folder | Until the guard is released, or until the sweep after a SIGKILL | No |
| The correlation record | The `log_pointer` events, which name `task_description` on the `amplihack` process's own stderr and in its 0600 run log (#1606) | The log's retention | No |

None of the three is on a command line, so none can be matched by a pattern
kill; that is the property #2582 restores, and it is narrower than
confidentiality. Recipe context is not a place for secrets; let a step read
those from its own environment or a credential helper.

The context file is the one of the three that holds authorization material on
an auto-drive run, because the transport is chosen after the auto-drive
authorization keys are written into the context. Its mode and its folder's
mode are therefore load-bearing, not hygiene.

The capability report is compatibility negotiation, not a privilege boundary.
A runner that reports the capability falsely would receive a file it does not
read, and the run would proceed without its context. The report is not signed,
not verified and must not later be hardened as though it were.

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
# Output on the pinned runner: nothing
```

The walk needs `CONFIG_PROC_CHILDREN`. On a kernel without it,
`/proc/<pid>/task/*/children` is absent, the queue empties after the first
process and the check passes without having looked at anything. Count the
processes you visited and treat a count of one as a failure of the check, not
a pass.

## Tests

| Test | What it proves |
| --- | --- |
| `crates/amplihack-cli/tests/issue_2582_real_runner_argv.rs` (Linux only) | The acceptance condition. A live `amplihack recipe run`, through the pinned runner, over a tree with a nested sub-recipe, a nested `amplihack recipe run` and an agent step: the sentinel reaches the recipe's context, and `/proc/<pid>/cmdline` holds it in no process of the tree, nor does any `run.json`. Asserts a minimum tree size, that `--context-file` is on both runners, and that the agent step received the task, so an empty walk fails rather than passing. `task_description` is excluded from the stdout and stderr check alone, because both log-pointer events name it (#1606): that is the correlation record working, on this process's own stderr and its 0600 log, which no other account can read and `pkill -f` cannot match |
| `tests/support/pinned_runner.rs` | Shared with `main` (#2658): resolves a runner built at the revision in `claude-plugin/recipe-runner.rev`, caching the build. It does not read capabilities; the acceptance test above runs `--capabilities` on the runner it resolves and fails, naming what the runner reported, if `context_file_v1` is absent |
| `crates/amplihack-cli/tests/issue_2582_context_off_argv.rs` | The same `/proc` walk against a runner stub, for a runner that reports the capability, one that does not (the `--set` path, the walker's positive control), and a `recipe detached-child` running a nested `recipe run` |
| `crates/amplihack-cli/tests/issue_2582_nested_context_off_argv.rs` (Linux only) | The bundled nested calls: no sentinel and no `key=` prefix in the nested child's command line, the context files present at 0600 in a 0700 folder, and no folder left behind |
| `crates/amplihack-cli/src/commands/recipe/run/tests_context.rs` | Payload parity across the two transports, reserved-key parity, the argv byte count per variant, an empty context emitting neither flag, the folder and file modes, a symlink planted at `context` ending the run with nothing written, a failed write leaving nothing on disk, and the file and folder gone after the guard is released |
| `crates/amplihack-cli/src/commands/recipe/run/tests_execute.rs` | A run whose context transport fails returns an error holding no context value, makes no runner call after `--capabilities`, records a failed run in `run.json`, and finishes its admission. A dry run writes the file at 0600 in a 0700 folder and leaves neither behind. A recipe context that sets `no_merge` or `autodrive_authorization` does not win: the authorization keys are written after it, so the transport carries the authorized values |

The agent step of the acceptance test runs a small binary of the test's own,
injected through the launcher-binary mechanism, which blocks on a FIFO. That
holds the process tree live while the walk runs, and the test releases it by
writing to the FIFO. There is no sleep, no timer, no `timeout` wrapper and no
retry anywhere in it: a hang in that test is a defect in the test, and wrapping
it in a `timeout` is how a shared process group comes to be signalled
([#2674](https://github.com/rysweet/amplihack-rs/issues/2674)).

The resolver has no skip path. A test that skipped when the capability was
absent would pass against the v0.6.0 runner on `PATH` for the wrong reason,
which is the main way a live test here could be vacuously green. It resolves,
in order: `RECIPE_RUNNER_QA_BINARY`, `RECIPE_RUNNER_RS_PATH`, then a
`cargo install --locked --git` build of the pinned revision, cached under
`$CARGO_TARGET_TMPDIR/pinned-recipe-runner/<pin>/` behind a blocking lock
shared by every test binary. Both overrides are honoured, so a runner-backed
test can be pointed at another binary, but every source must pass the same
receipt check against the pinned revision: an override whose receipt names a
different revision panics rather than falling back to the cache or to the
host, so an older runner cannot quietly stand in for the pin. The `--set`
path is covered instead by a runner stub that reports no `context_file_v1`.
If an environment cannot reach the checkout, give it access rather than
adding a skip. The order and the receipt rules are set out in
[Run tests against the pinned recipe runner](../howto/run-tests-against-the-pinned-recipe-runner.md).

Every test in this area uses tempfile `HOME`, `AMPLIHACK_HOME` and profile
roots, and reads and writes neither the real `~/.claude` nor the real
`~/.amplihack`. Any process a test starts is stopped by its recorded PID and
waited for; no test uses `pkill`, `pkill -f`, `killall` or `kill $(pgrep ...)`,
in teardown or anywhere else, since the pattern kill is one of the two
capabilities this change removes.

### Comments the pin move invalidated

Three source comments stated that the pinned runner does not report
`context_file_v1`. That was true of `90286bc` and is false of `2833a0b8`, so
all three were corrected when the pin moved:

| File | What it says now |
| --- | --- |
| `crates/amplihack-cli/src/commands/recipe/run/execute.rs` | `pass_context`'s doc comment says the `--set` path runs only for a runner that does not report the capability, that the pinned runner does report it, and that this path is reached only by a runner older than the pin supplied through `RECIPE_RUNNER_RS_PATH` |
| `crates/amplihack-cli/tests/issue_2582_context_off_argv.rs` | The module doc says the pinned runner reports the capability, so the file transport is the live path, and names the `--set` case as both the older-runner path and the walker's positive control |
| `crates/amplihack-cli/tests/issue_2582_nested_context_off_argv.rs` | The module doc names the hop this test covers and points at the two tests that cover the next one |

The module doc of `context_transport.rs` was not in that list, because it
described the capability check without claiming anything about the pinned
revision. It now says which path a normal run takes, and what the file does
not do: 0600 in a 0700 folder keeps other accounts out, but a process running
as this user can still read it, as it always could read the environment
mirror. What the change removes is the world-readable copy on argv.

## Related

- [Why Prompt Text Stays Out of argv](../concepts/prompt-text-out-of-argv.md) - The threat model and the #2030 and #2582 incidents
- [Agent Prompt Channels Reference](./agent-prompt-channels.md) - The channel each agent launch path uses
- [Recipe runner validation](./recipe-runner-validation.md) - The capability report this page reads
- [Pass Large Recipe Context and Agent Text](../howto/pass-large-recipe-context.md) - Getting context into `amplihack recipe run`
- [Keep Shell Test Stubs from Blocking on stdin](../howto/keep-test-stubs-from-blocking-on-stdin.md) - The stdin rule for the shell stubs the #2582 rounds touched
- [Stop Background Jobs by PID](../howto/stop-background-jobs-by-pid.md) - The agent guidance from #2582 part 2
