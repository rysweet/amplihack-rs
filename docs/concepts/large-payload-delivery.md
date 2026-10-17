---
title: "Large Payload Delivery to Child Processes"
description: "Why amplihack never places unbounded text in a single argv element or environment value, and how it delivers large prompts, recipe context and GitHub bodies instead."
last_updated: 2026-10-16
review_schedule: quarterly
owner: rysweet
doc_type: explanation
related:
  - ../reference/large-payload-delivery.md
  - ../howto/pass-large-recipe-context.md
  - ../amplihack-rs-parity.md
  - ../reference/recipe-context-environment.md
---

# Large Payload Delivery to Child Processes

amplihack spawns many child processes whose input is text of unbounded size:
agent prompts, tmux scrollback, recipe context such as `cycle_history`, pull
request bodies and commit messages. This page explains why that text no longer
travels in argv or in an environment value, and which channel it uses instead.

For the API, constants and per-tool table, see the
[Large Payload Delivery Reference](../reference/large-payload-delivery.md).

## Contents

- [The kernel limits](#the-kernel-limits)
- [Where the limits were reached](#where-the-limits-were-reached)
- [One delivery layer instead of per-site fixes](#one-delivery-layer-instead-of-per-site-fixes)
- [Choosing a channel](#choosing-a-channel)
- [Bounding text that has no natural size](#bounding-text-that-has-no-natural-size)
- [Recipe context](#recipe-context)
- [Nested recipe runs](#nested-recipe-runs)
- [Remote and fleet hops](#remote-and-fleet-hops)
- [Security properties](#security-properties)
- [Known limits](#known-limits)

## The kernel limits

Linux enforces two limits when `execve` builds a new process image:

| Limit | Value | What it bounds |
| --- | --- | --- |
| `MAX_ARG_STRLEN` | 131,072 bytes (32 pages) | Any single argv element or any single `NAME=value` environment string |
| `ARG_MAX` | Usually 2 MiB, or a quarter of the stack limit | argv and envp together |

Exceeding either makes `execve` fail with `E2BIG`, reported as
`Argument list too long (os error 7)`. The failure happens before the child
runs, so the child cannot recover from it, and the parent sees only a spawn
error with no hint as to which string was too long.

The single-string limit is the one most often reached. A prompt of 200 KiB
fails even when the rest of the environment is empty.

## Where the limits were reached

Issue #1570 ranked the sites where agent-generated or user-supplied text
reached a single argv element or environment value. They fall into four
groups:

| Group | Examples |
| --- | --- |
| Agent prompts passed with `-p` | Auto mode turns, the fleet reasoner's `claude -p` with full scrollback, `amplihack-remote`'s base64 prompt, admiral `tmux send-keys` |
| Recipe context passed with `-c key=value` | `quality-audit-cycle` passing `cycle_history`, `multitask` launcher scripts, the autodrive loops re-passing `task_description` |
| GitHub and git text in argv | `gh issue create --body`, `gh pr create --body`, `git commit -m` with agent text, `jq --arg` with `TASK_DESCRIPTION` |
| Environment inheritance | A nested `amplihack recipe run` inheriting every parent `RECIPE_VAR_*` value and uppercase alias, then adding its own |

Each of these worked in testing with short inputs and failed in long sessions,
where a prompt or history had grown past 128 KiB.

## One delivery layer instead of per-site fixes

The sites share one problem, so they share one solution. The delivery layer in
`amplihack_utils` has two parts:

- `prompt_delivery` chooses a channel for a prompt, given what the target CLI
  supports, and prepares the command.
- `payload_delivery` writes a payload to a child's stdin on a separate thread,
  and truncates text by bytes on UTF-8 boundaries.

Recipe YAML and shell helpers cannot call Rust, so they use three
equivalents: `amplihack recipe run --context-file` and `--context-from-file`
for passing context, `amplifier-bundle/tools/recipe_context_value.sh` for
reading it inside a step, and `amplifier-bundle/tools/workflow_body_file.sh`
for GitHub bodies and commit messages.

A converted site never decides for itself how to pass a large string. It calls
the shared layer, which keeps the behaviour, the tests and the security
properties in one place.

## Choosing a channel

A child process can receive a large payload in three ways that avoid argv:

| Channel | How it works | Requirement on the child |
| --- | --- | --- |
| Tempfile flag | A 0600 temporary file, passed with the tool's own prompt-file flag | The tool documents a prompt-file flag |
| Stdin | Bytes written to the child's stdin, which is then closed | The tool reads its prompt from stdin |
| File reference | A 0600 temporary file, plus one short argument asking the agent to read it | The agent can read files at that path |

For prompts above 4096 bytes, `auto` delivery tries the channels in that order
and uses argv only when the tool supports none of them, logging a warning. The
order prefers channels where the tool reads the bytes itself over the file
reference, where the agent must choose to open the file.

The file reference exists because Copilot and Amplifier have no verified
prompt-file or stdin contract. Both can read files: Copilot auto mode runs with
`--add-dir /` and `--allow-all-paths`, so a file in the system temporary
directory is within reach. The argument is a fixed sentence of under 512 bytes
with only the file path substituted:

```text
Read and follow the complete instructions in the file /tmp/.tmpQ3x9Lk. Its contents are the task prompt.
```

The resulting choices are:

| Tool | Large prompt channel |
| --- | --- |
| Claude Code | stdin |
| Codex | stdin |
| GitHub Copilot CLI | file reference |
| Microsoft Amplifier | file reference |

Prompts of 4096 bytes or fewer still use argv for every tool, so short
interactive prompts behave as before.

### Stdin without deadlocks

Writing a large payload to a child's stdin from the thread that also reads the
child's stdout can deadlock: the child blocks writing to a full stdout pipe
while the parent blocks writing to a full stdin pipe. The shared writer runs on
its own thread, closes stdin when it finishes, and treats a broken pipe as
success, because a child that exits without reading all of its input has
already made its decision. A writer error is logged and never replaces the
child's exit status.

Async callers, which spawn with tokio, cannot hand a tokio `ChildStdin` to a
thread. They use the same module's `write_stdin_async`, which follows the same
rules: it writes the chunks in order, then flushes and closes stdin, and it
reports a reader that closed early (`BrokenPipe` or `ConnectionReset`) as
`StdinDelivery::ReaderClosed` rather than as an error. Every site that writes a
payload to a child's stdin uses one of these two writers, so the rules for an
early-closing reader are the same everywhere.

When a caller has no payload, stdin is `Stdio::null()`, not inherited, so a
child cannot block waiting on the terminal.

## Bounding text that has no natural size

Removing the argv limit does not make unbounded input acceptable. A prompt of
several megabytes is slow, expensive and mostly noise. Two sites therefore
bound their input as well as moving it:

- **Fleet reasoner.** The tmux capture is limited to the last 2,000 lines and
  then to the last 48 KiB. The transcript keeps at most 200 messages, the head
  of each message up to 4 KiB, and the last 64 KiB overall.
- **Auto mode.** The objective, plan and latest evaluation that are carried
  from turn to turn are each capped at 32 KiB, keeping the tail behind an
  `[... earlier output truncated ...]` marker.

GitHub rejects issue and pull request bodies over 65,536 characters.
`workflow_body_file.sh` keeps the first 60,000 bytes, cut on a UTF-8 boundary,
and appends a note giving the number of bytes removed. Because no character is
smaller than one byte, 60,000 bytes is at most 60,000 characters in any
language, which leaves room for the note.

## Recipe context

`amplihack recipe run -c key=value` places the value in argv, and the recipe
runner then exports it to each step's environment. Both hops hit
`MAX_ARG_STRLEN` for large values.

`--context-file <json>` and `--context-from-file key=path` read the values from
files, so only the path appears in argv. `amplihack` then hands the merged
context to the runner as `--set` arguments, the only context input the runner
has. Each pair must fit in one argument; see [Known limits](#known-limits).

Inside a run, the recipe runner pinned in `claude-plugin/recipe-runner.rev`
(`4d5187e`) gives a bash step every context value as `RECIPE_VAR_<key>` at any size,
plus an uppercase alias such as `TASK_DESCRIPTION` for top-level scalar keys. A value
whose `KEY=value` string is too large for one environment entry, or that has to
move to bring the whole environment under the runner's budget, is written to a
private 0600 file. A preamble the runner adds to the script reads it back into
a shell variable of the same name before the step's own commands run.

That variable is shell-local: it is not exported. The step's own shell sees
the value byte for byte, but a child process the step starts does not inherit
it, because exporting it would make every such child fail with `E2BIG`. Code
that needs the value must therefore run in the step's shell, which is why the
recipes `source` their helpers instead of running them as separate commands.

`recipe_context_value.sh` gives steps one call that works at any size:
`write_context_value KEY OUT` copies the value into a file, and fails if the
key is not set. The re-invocation, GitHub and `jq` sites all read their text
this way, so a large value goes to `gh` and `jq` through a file or stdin and
never through argv.

Runners older than this pin behaved differently. When the whole context did
not fit the runner's budget, they wrote it to a 0600 JSON file named by
`AMPLIHACK_CONTEXT_FILE` and left most `RECIPE_VAR_*` variables unset.
`write_context_value` still reads that file when `RECIPE_VAR_<key>` is unset,
so the recipes keep working when an older runner is selected with
`RECIPE_RUNNER_RS_PATH`.

`-c key=@path` was rejected as a design. Agent-generated text can legitimately
start with `@`, and a value such as `@octocat please review` would then be
read as a path. A separate flag keeps literal values literal.

## Nested recipe runs

A bash step that runs `amplihack recipe run` starts a nested run that inherits
the parent's environment. That environment already holds every parent context
value twice, as `RECIPE_VAR_<key>` and as its uppercase alias. The nested run
then adds its own context on top, so a large parent value could push the
nested spawn past `ARG_MAX` even when the nested recipe never used it.

The nested run now removes inherited `RECIPE_VAR_*` variables, their
uppercase aliases and `AMPLIHACK_CONTEXT_FILE` before adding its own context.
The pinned runner never sets the context file, but an older runner does, and a
nested step would then read the parent's values when its own `RECIPE_VAR_*`
variables were absent. A nested recipe receives only
the context its step passes explicitly. Names on the reserved denylist, such as
`PATH` and `HOME`, and all other `AMPLIHACK_*` names are never removed.

See [Nested and sub-recipe propagation](../reference/recipe-context-environment.md#nested-and-sub-recipe-propagation)
for the exact rule.

## Remote and fleet hops

Moving a prompt off argv on the local machine does not help if the remote side
rebuilds the same `-p "$PROMPT"` command. `amplihack-remote` and the fleet
admiral therefore change both ends:

1. The prompt travels on `azlin` stdin, byte for byte, after the API key line.
2. The remote shell writes it to a 0600 file created with `mktemp`.
3. The remote command keeps its existing shape and only swaps `-p` for the
   file: `amplihack claude --auto --max-turns <n> --prompt-file <path>`, with
   `ANTHROPIC_API_KEY` still exported first.
4. The file is removed when the run ends. In the detached tmux variants the
   removal is part of the tmux command, because the launching script returns
   before the agent starts.

`--prompt-file` is a launch-subcommand option that requires `--auto`. Auto
mode is the only mode it can serve: `amplihack claude` has no `--ultrathink`,
`--analyze` or `--fix` option, so remote and admiral runs in those modes already
failed on the VM. They are now refused locally, before anything is sent.

Because `--prompt-file` is parsed by the subcommand rather than passed through
after `--`, a VM running an older amplihack stops with
`error: unexpected argument '--prompt-file' found` instead of starting a run
with the file path as its prompt. There is no fallback to argv.

## Security properties

| Property | How it holds |
| --- | --- |
| Temporary files are private | `NamedTempFile` or `mktemp` with random names, created after `umask 077`, mode 0600 |
| Files do not outlive their use | `DeliveryHandle` deletes its file on drop; shell sites use `trap ... EXIT`; detached tmux runs remove the prompt file from inside the tmux command, and the launcher removes it if the session cannot be started |
| Payloads stay out of logs | Errors and warnings report sizes and modes, never payload bytes or file contents |
| Paths cannot inject commands | The admiral validates the remote path against `^/[A-Za-z0-9._/-]+$` and rejects `..`; the agent, mode, turn count and session name are allowlisted or validated, so no value placed in the stage 2 command can contain a quote, `$`, a backquote or `\` |
| Context keys cannot set control variables | File-sourced keys must match `[A-Za-z0-9_]+`, and `AMPLIHACK_*` keys are rejected |
| Large prompts no longer appear in `/proc/<pid>/cmdline` | Prompts above 4096 bytes travel on stdin or in 0600 files, which other users cannot read. Prompts of 4096 bytes or fewer, and prompts at the sites still on argv, remain visible in the child's command line |

## Known limits

- The recipe runner still renders `{{placeholder}}` values into the text of
  bash steps. A large value used as a placeholder produces a large script. The
  runner writes scripts over 64 KiB to a temporary file, so size alone does not
  fail the step, but the value is still pasted in as shell text.
- A value spilled by the runner is visible to the step's shell only. A step
  that runs `bash some-helper.sh`, or any other child process, and expects the
  child to read `$RECIPE_VAR_<key>` from its environment gets an empty value
  once the value is large. The recipes in this repository source their helpers,
  and a test fails if one runs `recipe_context_value.sh`,
  `workflow_body_file.sh`, `workflow_context.sh` or
  `workflow_doc_review_checkpoint.sh` as a separate command.
- The runner removes a spilled value whose name is not a shell identifier or
  is one bash treats specially (such as `BASH_ENV`, `ENV` or `CDPATH`), with a
  `Dropping env var` warning. `RECIPE_VAR_<key>` is never affected. The
  uppercase alias is: a large value under a key such as `bash_env` loses its
  `BASH_ENV` alias but still arrives as `$RECIPE_VAR_bash_env`.
- A `{{key}}` placeholder inside a quoted heredoc is pasted into the script
  text, so a value containing the delimiter line ends the heredoc and runs as
  shell. Recipes converted by this change read such values with
  `write_context_value` instead.
- `recipe-runner-rs` takes context only as `--set` arguments, so
  `amplihack recipe run` refuses, before spawning the runner, a context value
  longer than one argument (128 KiB), naming the key and size. Many values may
  together exceed 128 KiB. Values that grow past that size inside a run, such
  as step outputs, reach bash steps intact.
- Remote and admiral runs support auto mode only.
- File-reference delivery depends on the agent reading the file. It is used
  only above 4096 bytes and only for tools with no stronger channel.
- A temporary file is not removed if amplihack is killed with `SIGKILL`. It
  remains 0600.
- `platform_cli`, `copilot_strategy` and `turn.rs` build argv from prompts but
  have no caller that passes large text. They still use argv.

## Related

- [Large Payload Delivery Reference](../reference/large-payload-delivery.md) - API, constants and the per-tool table
- [Pass Large Recipe Context](../howto/pass-large-recipe-context.md) - Convert a recipe step that passes large text
- [amplihack-rs Parity Reference](../amplihack-rs-parity.md) - Prompt delivery modes and `AMPLIHACK_PROMPT_DELIVERY`
- [Recipe Context Environment Export](../reference/recipe-context-environment.md) - The aggregate environment budget
