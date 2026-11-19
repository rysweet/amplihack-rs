---
title: Recipe CLI Reference
description: Recipe commands, operator guidance, text encoding and validation rules.
type: reference
updated: 2026-11-12
---

# Recipe CLI Reference

Command-line reference for `amplihack recipe`.

## Contents

- [Subcommands](#subcommands)
- [amplihack recipe list](#amplihack-recipe-list)
- [amplihack recipe run](#amplihack-recipe-run)
- [Removed context keys](#removed-context-keys)
- [Nested runs and orphan stops](#nested-runs-and-orphan-stops)
- [amplihack recipe guide](#amplihack-recipe-guide)
- [amplihack recipe validate](#amplihack-recipe-validate)
- [Recipe text encoding](#recipe-text-encoding)
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
`AMPLIHACK_PARENT_RECIPE_STARTTIME`, `AMPLIHACK_AUTODRIVE_STATE_DIR` and
`AMPLIHACK_RUN_GUIDANCE_DIRS` from the pane environment, so a detached run outlives the session that launched it.

## amplihack recipe guide

Add an instruction to a recipe run that is already going. Every agent step
that starts after the instruction is added receives it at the end of its
prompt.

```bash
amplihack recipe guide <TARGET> [-] [--file <PATH>] [--from-step]
```

| Argument | Description |
| --- | --- |
| `<TARGET>` | The run to guide: a detached session name, a run ID, a GitHub issue, or a GitHub pull request URL. See [Targets](#targets). |
| `-` | Read the instruction from stdin. This is also what happens when neither `-` nor `--file` is given. |
| `--file <PATH>` | Read the instruction from this file instead of stdin. Cannot be combined with `-`. |
| `--from-step` | Allow the command inside a recipe run. Without it, the command exits 2 when `AMPLIHACK_RECIPE_RUN_ID` is set. See [Guidance from an agent step](#guidance-from-an-agent-step). |

Builds (`smart-orchestrator`, `default-workflow` and similar) and drives
(`auto-drive`) are both recipe runs, and both take guidance in the same way.

Use this when you see a run going the wrong way and want to correct it
without stopping it. The instruction is stored on disk. Nothing is sent to a
running agent; the next agent step reads it when it starts.

### Targets

The run is found from what it recorded when it started, so the command works
from any shell, with `AMPLIHACK_WORK_ROOT` and `AMPLIHACK_WORK_ROOT_EXPLICIT`
set to anything or to nothing. A run started with a non-default work root is
found as easily as one started with the default.

At start, `recipe run` prints an `amplihack.recipe.log_pointer` line that
includes `run_dir`, the absolute path of the run's folder (see
[Recipe Run Correlation Reference](./recipe-run-correlation.md)). A detached
run's pointer line is kept in `<session>.log` in
[`recipe.log_dir`](./user-config.md#recipelog_dir) (default
`~/.amplihack/recipe-logs`), which does not depend on the work root.

A run that retries writes one pointer line per attempt, each carrying that
attempt's own correlation ID and all naming the same `run_dir`. The folder is
what identifies the run across its attempts, and the `attempts[]` of the
`run.json` in it lists every correlation ID the run has used.

| Form | Example | Where the instruction is stored |
| --- | --- | --- |
| Detached session name (up to 85 characters) | `amp-smart-orchestrator-myapp-20261112-091200` | The folder of the run started in that session |
| Run ID: folder name (`AMPLIHACK_RUN_ID`) or correlation ID (`run_id`, `AMPLIHACK_RECIPE_RUN_ID`, the ID `recipe stop` takes), including the ID of a later attempt of a run that retried | `5b9d0f6e-77a2-4f0b-b1c4-0e3a9d2c6f81` | That run's folder |
| Issue: `<number>`, `#<number>` or an issue URL | `2783`, `'#2783'`, `https://github.com/rysweet/amplihack-rs/issues/2783` | The folder of the run that recorded that issue |
| Pull request URL | `https://github.com/rysweet/amplihack-rs/pull/2535` | The auto-drive state folder for that pull request |

`#<number>` starts a comment in most shells, so quote it: `'#2783'`.

The first match is used, in this order:

1. **Issue URL.** A target of the form `https://github.com/<owner>/<repo>/issues/<number>`
   is looked up as an issue: the pointer line whose `issue_url` names the same
   owner, repository and number, comparing owner and repository whole and
   without case. A run that recorded only an `issue_number`, and so names no
   repository, is not matched here; rule 5 reaches it. This rule never creates
   the state folder. If no recorded run names the issue, the command exits 1
   and nothing is tried further.
2. **Pull request URL.** A `/pull/<number>` URL goes to the pull request's
   state folder. Any other target that contains `://` (a URL that is neither
   an issue URL nor a pull request URL) exits 2 and nothing else is tried.
3. **Session name.** The pointer line in `<session>.log`. If it records
   `run_dir`, that folder is used. The name may be up to 85 characters --
   the 80 `recipe run --detach` truncates a pattern to, plus the longest
   `-<n>` suffix it appends when that name is already taken -- and may not
   contain `/`, `:`, `.`, `#` or a control character. A target that fails
   this is not looked up as a session, and the not-found line names the
   session log among the places it did not reach:
   `no session log (the name is 94 characters; a detached session name is at
   most 85)`.
4. **Run ID.** The `run_dir` recorded by the pointer line that carries that
   `run_id`, searched across the log directory. Only the first pointer line
   of each log is read, so a run that retried is recorded under its first
   attempt's correlation ID alone; the ID of a later attempt, which is the
   one `recipe stop` takes and the one that attempt's agent steps carry, is
   found next in the `attempts[]` of the `run.json` of each recorded folder
   (every attempt of a run shares the folder the first line recorded). If
   nothing recorded names the ID, it is looked up under `<work root>/runs/`,
   first as a folder name and then as the correlation ID in each run's
   `run.json`.
5. **Bare issue number.** `<number>` or `#<number>`, tried only after rules 3
   and 4 find nothing. It matches the pointer line's `issue_number` alone,
   without checking the repository. When more than one repository or run
   matches, the newest run is used and a warning on stderr names the run ID
   that was chosen. The warning appears only when the command succeeds and
   only when there was more than one match; stdout is the same either way.

Where both the recorded folder and the work root in the environment resolve,
the recorded folder is used: it is the folder the run actually uses.

A run with no recorded `run_dir` (one started before the field existed, or a
run whose pointer line was not kept in a log) is still looked up by run ID or
session name through the work root in the environment, as it was before. It
is not found by issue, because the issue lookup depends on the recorded
pointer line.

#### Recorded claims and named inputs

The command treats two kinds of input differently.

- **A recorded claim** is a `run_dir`, session or issue taken from a pointer
  line or a `run.json`. Unsafe paths and ownership failures become notes
  in the not-found message, and the next rule is still tried. Naming, ID
  and mode disagreements are retained as warnings when a candidate is used.
- **An operator-named input** is something you typed: the target and the
  `--file` path. These are refused with an error when they fail: a malformed
  pull request URL, a session log you named that is a symbolic link or not
  trusted, a destination folder that fails a check, or a `--file` path that
  cannot be read.

Checks applied to a recorded folder before it is written to:

- The raw path string is absolute and has no empty, `.` or `..` segment. The
  segments are checked on the string as recorded, before any normalisation
  can remove them.
- The recorded path does not contain U+FFFD (the replacement character that
  stands for bytes that are not valid UTF-8).
- No part of the recorded path, the ancestors included, holds a control
  character, a bidirectional or zero-width mark, or a line or paragraph
  separator. The checks below read only the last two components and the work
  root, so without this one a folder such as
  `/x/<ESC>]0;title<BEL>/runs/<run id>` would pass them all and then be
  printed back on the success path.
- Its parent is named `runs`. A name outside the run-ID grammar is reported
  as evidence without discarding the folder.
- Neither the folder nor `run.json` is a symbolic link, and neither `runs`
  nor the work root is a symbolic link (checked with `lstat`, so a link is
  seen rather than followed).
- The work root, `runs`, run folder and `run.json` are owned by the current
  user (uid equals euid). Group or other write permissions are reported as
  discovery evidence. Before appending, the run folder and guidance files
  still pass the existing destination checks, including the restriction on
  group or other write permissions. No permissions are changed.
- Log files are owned by the current user and are not writable by group or
  others.
- The log directory passes the same trust check. A failure is reported as a
  note.
- `run.json` exists and parses. A report ID that differs from the folder
  name, or a pointer ID absent from the report, is printed in the success
  warning with the path and IDs. These disagreements do not discard the
  candidate. A later attempt can still be found through the report's IDs.
- The work root implied by the folder (the parent of `runs`) passes the
  location checks used for `AMPLIHACK_WORK_ROOT`. Discovery reports mode
  disagreements; creation and cleanup retain their stricter mode checks.

Logs are searched newest first. A run ID or a session name stops at the first
match; an issue keeps looking, so a run that is named more than once can be
reported. All regular `*.log` files are searched, with no log-count cap.
The not-found message counts skipped logs and explains read, trust and JSON
parse failures, including serde_json's recursion limit. Symlinks and special
files are never opened. A log that cannot be read or is not trusted is
skipped; a log you name as a session is refused with an error.

**Session names.** Nothing is refused here. A target that breaks the rule in
step 3 above -- too long, or holding `/`, `:`, `.`, `#` or a control
character -- is simply not looked up as a session: the not-found line names
the session log among the places it did not reach, giving the reason in
brackets, and the later rules still get their turn. The command then exits 1
only if none of them matches either. If the name passes the rule and the log
is missing or has no pointer line yet, the command exits 1 and names the log
path it tried. Logs are read one line at a time until the first pointer
line or EOF, with no byte-offset, pointer-length or field-length cutoff.
Reading stops at that line, so the size of a log beyond it does not affect
the cost of a scan. Long prefixes or lines therefore cost more to read, but
do not hide a run. Invalid JSON is reported as an unusable pointer with its
parse error, rather than as an absent pointer.

**Pull request URLs.** The URL must have the form
`https://github.com/<owner>/<repo>/pull/<number>`. A trailing `/`, a query
string or a fragment is allowed and ignored. These are refused with exit 2:

- a scheme other than `https`, or a host other than lower-case `github.com`;
- a user name, password or port in the URL;
- `%` escapes;
- an owner longer than 39 bytes or a repository name longer than 100 bytes;
- `.` or `..` as the owner or repository;
- a pull request number of `0`, or one that does not fit in 64 bits;
- any other path, such as `/pull/7/files`.

The pull request is not looked up on GitHub. A well-formed URL is accepted
and its folder is created if it does not exist, so you can add guidance
before you relaunch a drive. A pull request target writes only to that state
folder; a run started with the pull request already reads it.

**Finding a run without the environment:**

```bash
env -u AMPLIHACK_WORK_ROOT -u AMPLIHACK_WORK_ROOT_EXPLICIT \
  amplihack recipe guide amp-smart-orchestrator-myapp-20261112-091200 - <<'EOF'
Keep the existing public API; add the cache headers in the middleware only.
EOF
```

```text
guidance appended: /srv/builds/work/runs/8c0e2a54-1f7d-4c56-9a39-2d1b6f0c7e15/guidance/20261112T100241.007311204Z-49102-0.md (75 bytes, run)
```

### Giving the text

The instruction is never given on the command line, where it would appear in
`ps` output and shell history. Give it on stdin or in a file.

On stdin, with a quoted heredoc so that the shell expands nothing in it:

```bash
amplihack recipe guide amp-smart-orchestrator-myapp-20261112-091200 - <<'EOF'
Keep the public API of the retry module unchanged.
Add the new backoff setting as an optional field whose default is the
current value.
EOF
```

From a file:

```bash
amplihack recipe guide 8c0e2a54-1f7d-4c56-9a39-2d1b6f0c7e15 --file ~/notes/tests-only.md
```

From a script, with the text in a variable:

```bash
printf '%s' "$GUIDE_TEXT" | amplihack recipe guide "$target" -
```

Any other argument after `<TARGET>` is refused with exit 2 before anything
else is checked, and nothing is written, not even the pull request folder for
a URL target:

```text
recipe guide: instruction text is not accepted on the command line; pipe it on stdin (or pass -) or use --file <path>
```

The message never repeats the refused argument. This includes an argument
that starts with `-`, such as `--force`: anything after `<TARGET>` other than
`-`, `--file <PATH>` and `--from-step` gets the same refusal.

The rules for the two sources:

- Give one source only. `-` together with `--file` exits 2 with
  `recipe guide: give the text on stdin (-) or with --file <path>, not both`.
- When `--file` is given, stdin is not read. `--file -` names a file called
  `-`; it does not mean stdin.
- The file is opened once and read with your own permissions. A symbolic link
  is followed. A file that is missing or cannot be read exits 1 with a message
  that names the path and the system error, never the contents.
- Both sources have the same size limit and the same checks.
- Oversize text on stdin is read to the end so that the error can give its
  exact size; nothing past the limit is kept. Do not pipe an endless source
  such as `/dev/zero` into the command.
- A `--file` read stops one byte past the limit. For a regular file the error
  gives the file's exact size. For a path that is not a regular file, such as
  a named pipe or `/dev/zero`, the error says only that the text is more than
  the limit, so the command never reads without end. The same wording is used
  for a regular file whose recorded size is at or below the limit although
  more was read, such as a file under `/proc` or one that grew during the
  read.
- If `--file` is not given and stdin is a terminal, the command exits 2 with a
  usage message instead of waiting for input. This also applies when `-` is
  given.

Leading and trailing white space is removed before the text is checked and
stored. The text is refused, with exit 2 and nothing written, when it:

- is empty or only white space;
- is longer than 51,200 bytes (50 KiB) after trimming;
- is not valid UTF-8;
- contains NUL or another control character other than tab, line feed and
  a carriage return directly followed by a line feed (CRLF line ends are
  accepted; a carriage return on its own is refused);
- contains a bidirectional-override or zero-width character;
- has a line equal to one of the block delimiters (see
  [What the agent receives](#what-the-agent-receives)).

The wording of the text is not checked. An instruction may name any command
or recipe, for example `recipe run smart-orchestrator`.

Error messages name the reason, a path, a size or a character. They never
repeat the text.

### Where guidance is stored

| Target | Folder |
| --- | --- |
| Run ID or session name | `<work root>/runs/<run folder name>/guidance/` |
| Pull request URL | `${AMPLIHACK_STATE_DIR:-$HOME/.amplihack/state}/auto-drive/<key>/guidance/` |

`<work root>` is `AMPLIHACK_WORK_ROOT`, default `$HOME/.amplihack/work`; see
[Settings](../howto/clean-up-after-recipe-runs.md#settings).

`<key>` is the text `https://github.com/<owner>/<repo>/<number>` with every
byte outside `A-Z`, `a-z`, `0-9`, `.`, `_` and `-` replaced by `_`. The key
is always built from this `https` form, whatever form the repository's
`origin` remote has. For
`https://github.com/rysweet/amplihack-rs/pull/2535` the folder is:

```text
~/.amplihack/state/auto-drive/https___github.com_rysweet_amplihack-rs_2535/guidance/
```

The key depends only on the pull request. A drive relaunched for the same
pull request reads the same folder, whatever state folder its build step
chose.

When `origin` is an `https` remote, this is also the drive's own state
folder, because `autodrive_state.sh` builds its key from the same text. When
`origin` is an `ssh` or `scp`-style remote, such as
`git@github.com:rysweet/amplihack-rs.git`, `autodrive_state.sh` builds its
key from the raw remote URL and gets a different folder, such as one
starting `git_github.com_`. The guidance folder is then separate from the
drive's state folder, but the drive still reads it; see
[What the agent receives](#what-the-agent-receives).

Each instruction is one file, created once and never changed:

```text
guidance/20261112T091405.120458911Z-48211-0.md
```

The name is the UTC time the entry was written, to the nanosecond, then the
writing process's ID and a sequence number. Sorting the names gives the order
in which entries were added. The file holds a short header, a blank line and
the text:

```text
timestamp: 2026-11-12T09:14:05.120458911Z
caller: alice

Keep the public API of the retry module unchanged.
Add the new backoff setting as an optional field whose default is the
current value.
```

`caller` is the user name of the account that ran the command, taken from
the system's user database, then `$USER`, then `unknown`. ` run=<id>` is
added when the command runs inside a recipe run (with `--from-step`); an ID
that fails the run ID check is written as ` run=invalid`. Line breaks and unsafe characters are removed
from this field, and it is cut to 256 bytes. No credential or token file is
read.

**Limits.** One entry holds at most 51,200 bytes of text. One `guidance/`
folder holds at most 256 KiB of entries; an append that would pass that is
refused with exit 1 and nothing is written. Entries cannot be edited or
removed through this command.

**Permissions.** `guidance/` and any folder created above it are created one
at a time at mode 0700. Entries are written to a temporary file at 0600 and
then given their final name, so a reader never sees a partly written entry.
Before writing, the command refuses (exit 1) a `guidance/` folder, or any
folder on the path to it that it checks, that:

- is a symbolic link;
- is not a directory;
- is owned by another user, including `root` when you run the command with
  `sudo`; the message names the owning user ID;
- can be written by its group or by other users. On a host whose umask is
  `002`, a state folder created by hand can fail this check; remove the group
  write bit with `chmod g-w <folder>`.

The command never changes the permissions of an existing folder. The checks
start at the state folder (or the work root's `runs/` folder) and do not
cover the folders above it, so do not set `AMPLIHACK_STATE_DIR` to a path
under a folder that other users can write to, such as `/tmp`.

### Guidance from an agent step

Inside a recipe run, where `AMPLIHACK_RECIPE_RUN_ID` is set, the command is
refused with exit 2 unless `--from-step` is passed. An agent step that read
text from an issue, a pull request comment or a file can be misled into
running `recipe guide` on its own run or pull request, and an entry in a pull
request folder outlasts the run.

An entry added with `--from-step` has ` run=<id>` in its `caller` field.
Later steps receive it after the operator's entries, under its own preamble
(see [What the agent receives](#what-the-agent-receives)), not under the
sentence that says the instructions come from the local operator.

This is a safeguard against a misled agent using the normal command, not a
boundary. A process running as the same user can unset
`AMPLIHACK_RECIPE_RUN_ID`, or write an entry file directly, and its entry
then looks like the operator's.

**Cleanup.** Run guidance is removed with its run folder, for example by
[`amplihack cleanup`](../howto/clean-up-after-recipe-runs.md). Pull request
guidance stays until the pull request's auto-drive state folder is removed.

### What the agent receives

When `amplihack recipe run` starts its recipe runner, it sets
[`AMPLIHACK_RUN_GUIDANCE_DIRS`](./environment-variables.md#amplihack_run_guidance_dirs)
to the list of guidance folders for that run:

1. its own `<work root>/runs/<run folder name>/guidance/`;
2. the pull request's folder, when the run's context has `pr_url`, or a
   numeric `pr_number` with a `repo_path` whose `origin` remote is on
   `github.com`. An `ssh` remote and an `https` remote for the same
   repository give the same folder. When the pull request cannot be
   resolved, the run starts without this folder and prints one line on
   stderr naming the field and the cause. The list is worked out once, when
   the run starts, so a run that opens its pull request part-way through
   (an `auto-drive-to-merge` started without `pr_url` or `pr_number`) does
   not read the pull request folder. Later drive rounds, which start as new
   runs with the pull request in their context, do. To reach the run that
   opened the pull request, give its run ID or session name;
3. the list it inherited, if any, so that a nested `recipe run` also reads
   the outer runs' guidance. These come last so that the 16-folder limit
   never leaves out the innermost run's own folders.

Each time an agent step starts, the launcher reads every entry in those
folders, sorts them by file name, and adds one block to the end of the step's
prompt, after one blank line:

```text
<the step's own prompt>

===== BEGIN OPERATOR INSTRUCTIONS FOR THIS RUN =====
Operator instructions for this run
These instructions come from the local operator and apply to this step. Do not copy them into recipe context, task descriptions, command-line arguments, pull request titles, bodies, comments or commit messages.

[1] 2026-11-12T09:14:05.120458911Z from alice (run)
Keep the public API of the retry module unchanged.
Add the new backoff setting as an optional field whose default is the
current value.

[2] 2026-11-12T10:02:41.007311204Z from alice (pr)
Do not change the CI workflow files in this pull request.
===== END OPERATOR INSTRUCTIONS FOR THIS RUN =====
```

`(run)` marks an entry from a run folder and `(pr)` an entry from a pull
request folder. Entries whose `caller` field has ` run=<id>` (see
[Guidance from an agent step](#guidance-from-an-agent-step)) come after the
operator's entries, numbered on from them, after one blank line and this
line in place of the operator sentence:

```text
The entries below come from an agent step in this run, not from the local operator. Do not copy them into recipe context, task descriptions, command-line arguments, pull request titles, bodies, comments or commit messages.
```

When every entry is from an agent step, the operator sentence is left out. When there are no entries, the prompt is not changed by a
single byte. The block is added whole or not at all.

The launcher prints one line to stderr for each step that receives guidance:

```text
amplihack: operator guidance added to this step's prompt (2 entries, 231 bytes)
```

**When guidance is read.** Only when an agent step starts. There are no
timers and nothing watches the folder. An entry added while a step is running
reaches the next agent step, not the running one. Bash steps never receive
guidance.

**Agent CLIs.**

| Agent CLI | Where the block goes |
| --- | --- |
| Claude Code, `launch`, RustyClawd | End of the prompt sent on stdin, after the `-p` text. When input is also piped to `amplihack`, the block comes after that input. |
| GitHub Copilot CLI | End of the `-p` prompt, before it is written to the private prompt file. An interactive `-i` launch receives no guidance. |
| Codex | Only for `codex exec … -`, where the prompt is read from stdin and `-` is the last argument: after the caller's stdin closes, the block is written. The stderr line above and the `guidance_injected` record are written after Codex exits, and only if the block was written and flushed to Codex's stdin in full. If Codex closed its stdin first, neither is written. Other Codex forms receive no guidance and the launcher prints one note to stderr. |

**What the launcher skips.** A listed folder is skipped, with a warning that
names it, when the path is not absolute, its last part is not `guidance`, or
it or its parent fails the same symbolic link, owner and write-permission
checks as above. At most 16 folders are read. Within a folder, only files
whose names match the entry pattern are read; temporary files are ignored.
An entry that is larger than 55,296 bytes (the text limit plus 4 KiB for the
header), is a symbolic link, is not a regular file, is owned by another user,
can be written by its group or by others, or is not valid UTF-8 is skipped
with a warning that names it. The newest entries are used, up to 256
entries and 262,144 bytes (256 KiB) of text in total; they are read newest
first and reading stops at the first entry that does not fit. If entries are
left out, a warning names the limit and how many older entries were left
out. Entries skipped by a check do not count against either limit.

**Never published.** Guidance is added only to the prompt text given to the
agent CLI. It is never written to the recipe context, step outputs,
`run.json`, `guidance.log`, the runner's JSONL log, or any message from
`amplihack`. Steps that build pull request titles, bodies, comments and
commit messages work from step outputs, so the text does not reach them
through the workflow. The block also tells the agent not to copy it. A model
that ignores that instruction can still copy the text into its own output;
treat guidance as text that may become public, and put no secrets in it.

### The guidance log

Each guidance folder has a log beside it, `guidance.log` (0600, one JSON
object per line, only ever appended to):

```text
<work root>/runs/<run folder name>/guidance.log
~/.amplihack/state/auto-drive/<key>/guidance.log
```

An append writes:

```json
{"event":"guidance_appended","ts":"2026-11-12T09:14:05.120458911Z","caller":"alice","bytes":141,"entry":"20261112T091405.120458911Z-48211-0.md"}
```

An agent step that received guidance writes to the log of each folder it
read. For most agent CLIs this happens once the agent process has started.
For Codex it happens after Codex exits, and only when the block was written
to Codex's stdin in full; see [Agent CLIs](#what-the-agent-receives). A
successful write means the pipe accepted the bytes, not that Codex read them.

```json
{"event":"guidance_injected","ts":"2026-11-12T09:20:11.554013873Z","step_run_id":"5b9d0f6e-77a2-4f0b-b1c4-0e3a9d2c6f81","entries":2,"bytes":231}
```

Neither record holds the guidance text. A refused append is reported on
stderr only and is not logged. If the entry is written but the log line
cannot be, the command prints a warning and still exits 0; the entry is not
removed.

The sealed `.evidence` snapshots that the auto-drive merge gate checks are
not changed. The gate's private-state check accepts a pull request state
folder that holds `guidance/` and `guidance.log`. This matters only when
`origin` is an `https` remote, because only then is the guidance folder the
drive's own state folder. With an `ssh` remote the guidance folder is a
separate folder that the gate does not check.

### Output and exit codes

On success for a run ID or session name, the command prints one line to
stdout:

```text
guidance appended: /home/alice/.amplihack/work/runs/8c0e2a54-1f7d-4c56-9a39-2d1b6f0c7e15/guidance/20261112T091405.120458911Z-48211-0.md (141 bytes, run)
```

For a pull request URL the first line says `guidance saved for pull
request` rather than `guidance appended`, and a second line names the pull
request's guidance folder and says which runs read it:

```text
guidance saved for pull request: /home/alice/.amplihack/state/auto-drive/https___github.com_rysweet_amplihack-rs_2535/guidance/20261112T100241.007311204Z-49102-0.md (58 bytes, pr)
written to /home/alice/.amplihack/state/auto-drive/https___github.com_rysweet_amplihack-rs_2535/guidance; it is read at the next agent step of each run started with this pull request in its context (pr_url or pr_number), such as a later auto-drive round. A run that opened this pull request itself does not read it; address that run by its run ID or session name.
```

The command does not check whether any run reads the folder. A drive that
opened the pull request during its build step was started without it and
does not read this folder; see [Known limits](#known-limits).

| Code | Meaning | Example message on stderr |
| --- | --- | --- |
| `0` | The entry was written | |
| `1` | No run matches the target. One line names the target and each place looked at: the session log path, the log directory, the `run.json` of each recorded run folder, the recorded run folder if one was named but is absent, and `<work root>/runs/` | `recipe guide: no recipe run or detached session named "nightly-fix"; looked at session log /home/alice/.amplihack/recipe-logs/nightly-fix.log (not found), pointer lines in /home/alice/.amplihack/recipe-logs (3 pointer record(s)), the attempts in run.json of 3 recorded run folder(s), /home/alice/.amplihack/work/runs (no run folder or run.json with that ID)` |
| `1` | An issue URL that no recorded run names. Nothing is created under the state folder | `recipe guide: no recipe run or detached session named "https://github.com/rysweet/amplihack-rs/issues/2783"; looked at pointer lines in /home/alice/.amplihack/recipe-logs (3 pointer record(s))` |
| `1` | The target could be a run ID, but the work root cannot be resolved | `recipe guide: cannot look for run "nightly-fix": HOME is not set, so the default work root cannot be resolved` |
| `1` | `AMPLIHACK_STATE_DIR` (or `HOME`) is set to a relative path, for a pull request URL target | `recipe guide: cannot find the state folder: AMPLIHACK_STATE_DIR is not an absolute path` |
| `1` | The session's log has no pointer line | `recipe guide: /home/alice/.amplihack/recipe-logs/nightly-fix.log has no amplihack.recipe.log_pointer line` |
| `1` | The chosen destination folder failed a safety check. A recorded folder that fails becomes a note and the next rule is tried; it does not cause this exit | `recipe guide: refusing /home/alice/.amplihack/work/runs/8c0e…/guidance: it is owned by uid 0, not uid 1000` |
| `1` | The folder already holds 256 KiB of guidance | `recipe guide: guidance folder /home/alice/.amplihack/work/runs/8c0e…/guidance would hold 307285 bytes of entries; the limit is 262144` |
| `1` | The `--file` path cannot be opened or read | `recipe guide: cannot read /home/alice/notes/tests-only.md: No such file or directory (os error 2)` |
| `2` | Text given as an argument after `<TARGET>` | `recipe guide: instruction text is not accepted on the command line; pipe it on stdin (or pass -) or use --file <path>` |
| `2` | Both `-` and `--file` given | `recipe guide: give the text on stdin (-) or with --file <path>, not both` |
| `2` | No `--file` and stdin is a terminal | `recipe guide: no text given and stdin is a terminal; pipe the text on stdin (or pass -) or use --file <path>` |
| `2` | Empty text | `recipe guide: guidance text is empty` |
| `2` | Oversize text on stdin, or in a regular `--file` file | `guidance entry is 61440 bytes; the limit is 51200 bytes` |
| `2` | Oversize text from a `--file` path that is not a regular file, or a regular file whose recorded size is wrong | `guidance entry is more than 51200 bytes; the limit is 51200 bytes` |
| `2` | Invalid UTF-8, a refused character or a delimiter line | `recipe guide: guidance text contains a control character (U+001B)` |
| `2` | Malformed pull request URL, or any URL that is neither an issue URL nor a pull request URL | `recipe guide: not a GitHub pull request URL: port is not allowed` |
| `2` | Run inside a recipe run without `--from-step` | `recipe guide: refusing to add guidance from inside a recipe run (AMPLIHACK_RECIPE_RUN_ID is set); an agent step that means to add an entry must pass --from-step, and the entry is then shown as coming from an agent step` |

Every failure prints one line and writes nothing.

The not-found line reports the number of pointer records read and how many
logs were skipped. When `HOME` is unset the comparison with the home folder is
skipped, and the line says that the work root could not be resolved.

For a target that could be a run ID, the line also says how many recorded run
folders had their `run.json` read for it: `the attempts in run.json of 3
recorded run folder(s)`. When a folder could not be used at all -- it is gone,
its work root is unusable, it has no readable `run.json` -- it is not counted
there, the note gives both numbers (`the attempts in run.json of 1 of 3
recorded run folder(s)`), and that folder is named separately with its reason,
as below. The count is of folders, not of records: a folder named by several
records is read once.

A folder a record named that was not used is listed after `not used:`, at most
three of them, followed by `, and <n> more` counting the rest, whose reasons
are left out so the line stays bounded. A reason is given once however many
rules reached the folder. Control characters in text taken from a log are
escaped as `\u{..}`, as are the bidirectional marks and the line and paragraph
separators, so a crafted log cannot add lines or terminal sequences to the
message.

The same escaping applies to every line the command prints, on stdout as well
as stderr and on success as well as failure. The success line names the folder
the entry was written to, and that folder is no longer taken only from the
caller's own environment, so it is escaped in the same way: a path printed back
is never a way for a terminal sequence to reach the terminal.

### Known limits

- **Foreground runs leave no log.** A run started without `--detach` writes
  no session log, so it cannot be found by session name or from the log
  directory. It is addressable only through the environment (the work root
  in `AMPLIHACK_WORK_ROOT`) or by a pull request URL.
- **A drive that has not opened its pull request yet.** Guidance given by
  pull request URL reaches every drive or round started with that pull
  request in its context, including relaunches. A drive still in its first
  build step was started without a pull request, so it does not read that
  folder, even after it opens the pull request. The command prints a second
  line saying so (see [Output and exit codes](#output-and-exit-codes)). Use
  the drive's run ID or session name instead.
- **Large Claude prompts.** When a prompt is too large for one argument, the
  launcher passes a short preview with `-p` and the rest in a system prompt
  file. The block follows the preview on stdin; the system prompt file is not
  changed.
- **No removal.** Entries cannot be edited or deleted through the command.
- **Who can add guidance.** Any process running as your user can add
  guidance to your runs. There is no other check.
- **Where the text goes.** The command takes the text only from stdin or a
  file, so it does not appear in `ps` output or shell history. Guidance is
  sent to the model provider and is kept in the agent CLI's transcripts. Do
  not put secrets in it.

### Worked example: correcting a detached run

Start a detached run:

```console
$ amplihack recipe run smart-orchestrator --detach -w ~/src/myapp \
    --context-from-file task_description=task.md -c repo_path=.
session: amp-smart-orchestrator-myapp-20261112-091200
log: /home/alice/.amplihack/recipe-logs/amp-smart-orchestrator-myapp-20261112-091200.log
```

The log shows the design step planning a change to a public function. Tell
the run to keep it:

```console
$ amplihack recipe guide amp-smart-orchestrator-myapp-20261112-091200 - <<'EOF'
Keep the public API of the retry module unchanged.
Add the new backoff setting as an optional field whose default is the
current value.
EOF
guidance appended: /home/alice/.amplihack/work/runs/8c0e2a54-1f7d-4c56-9a39-2d1b6f0c7e15/guidance/20261112T091405.120458911Z-48211-0.md (141 bytes, run)
```

When the next agent step starts, the run's log shows:

```text
amplihack: operator guidance added to this step's prompt (1 entry, 141 bytes)
```

For an auto-drive, give the pull request URL so that a relaunch of the drive
keeps the instruction:

```console
$ amplihack recipe guide https://github.com/rysweet/amplihack-rs/pull/2535 - <<'EOF'
Do not change the CI workflow files in this pull request.
EOF
guidance saved for pull request: /home/alice/.amplihack/state/auto-drive/https___github.com_rysweet_amplihack-rs_2535/guidance/20261112T100241.007311204Z-49102-0.md (58 bytes, pr)
written to /home/alice/.amplihack/state/auto-drive/https___github.com_rysweet_amplihack-rs_2535/guidance; it is read at the next agent step of each run started with this pull request in its context (pr_url or pr_number), such as a later auto-drive round. A run that opened this pull request itself does not read it; address that run by its run ID or session name.
```

If the drive opened this pull request itself and is still running, also give
the instruction to the drive's run ID or session name, as in the first
example; the drive's current run does not read the pull request folder.

## amplihack recipe validate

Validate a recipe file containing YAML or JSON without executing it.

```bash
amplihack recipe validate <FILE> [OPTIONS]
```

| Option | Description | Default |
| --- | --- | --- |
| `<FILE>` | Recipe file containing YAML or JSON to validate | required |
| `--verbose`, `-v` | Include validation details | `false` |
| `--format <format>`, `-f <format>` | Output format: `table`, `json`, or `yaml` | `table` |

```bash
amplihack recipe validate ~/.amplihack/.claude/recipes/default-workflow.yaml \
  --verbose
```

## Recipe text encoding

The CLI recipe parser accepts UTF-8 YAML and JSON. JSON strings may contain
supplementary Unicode characters directly or encode them as valid UTF-16
surrogate pairs. Both forms produce identical decoded recipe text. No flag,
environment variable or configuration change is required.

These complete recipes decode to the same prompt:

```json
{"name":"unicode-review","steps":[{"id":"review","type":"agent","prompt":"Review: \uD83D\uDE80, \uD834\uDD1E.\nKeep punctuation!"}]}
```

```json
{"name":"unicode-review","steps":[{"id":"review","type":"agent","prompt":"Review: 🚀, 𝄞.\nKeep punctuation!"}]}
```

The decoded prompt contains exactly these two lines:

```text
Review: 🚀, 𝄞.
Keep punctuation!
```

JSON escaping still applies. The JSON string `"\\uD83D\\uDE80"` decodes to
the literal text `\uD83D\uDE80`. The parser preserves those backslashes and
does not interpret the decoded text again. Actual lone, reversed, interrupted
or malformed surrogate escapes are rejected, including `"\uD83D"`,
`"\uDE80"`, `"\uDE80\uD83D"`, `"\uD83Dx\uDE80"` and `"\uD83G"`.

The parser tries YAML decoding first. If decoding fails, it tries JSON decoding
on the original input, requiring a complete JSON document without trailing
content. If both decoders fail, it returns the original YAML error. A schema
error after successful YAML decoding does not trigger a JSON retry. Existing
YAML syntax and scalar behaviour remain unchanged.

Both decoding paths apply the same recipe validation. The original file and
input limits remain 1,000,000 bytes, measured before escape decoding. Recipes
require a top-level mapping, a string `name`, non-empty `steps` and non-blank,
unique step identifiers. Field types and the supported explicit step types
(`bash`, `agent` and `recipe`) remain unchanged, as does step-type inference.
Duplicate mapping keys are rejected at every depth, including context and
additional fields. Keys that become equal after escape decoding also count
as duplicates. Additional fields remain available in the parsed recipe;
unknown-field and retired-field warning behaviour remains unchanged.

This encoding contract applies to CLI recipe parsing and pre-validation.
Recipe execution passes the original file to `recipe-runner-rs`, whose parsing
support is independent. Successful CLI validation does not establish runner
compatibility or change formatter behaviour.

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
| `AMPLIHACK_RUN_GUIDANCE_DIRS` | agent-step launcher | Set by `amplihack recipe run` on its runner child: the guidance folders whose entries are added to each agent step's prompt. See [amplihack recipe guide](#what-the-agent-receives). Do not set it by hand. |
| `AMPLIHACK_RECIPE_LOG_JSONL` | runner | Optional path for structured JSONL lifecycle, heartbeat, output snippet, and failure events. |

See [Environment Variables](./environment-variables.md) for full details.

## Exit codes

| Code | Meaning |
| --- | --- |
| `0` | Command succeeded |
| `2` | `recipe guide` only: usage error, such as text given on the command line, both `-` and `--file`, empty, oversize or refused text, a malformed pull request URL, or a call inside a recipe run without `--from-step`. See [Output and exit codes](#output-and-exit-codes). |
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
- [Auto-Drive to Merge](./auto-drive-to-merge.md) - The drives that pull request guidance reaches
- [Run a Recipe End-to-End](../howto/run-a-recipe.md) - Task-oriented usage
- [Recipe Run Correlation Reference](./recipe-run-correlation.md) - Stable run IDs and log pointer events
- [Recipe Runner Logging Reference](./recipe-runner-logging.md) - stderr progress, heartbeats, bounded snippets, and JSONL events
- [Workflow Terminal-State Reference](./workflow-terminal-state.md) - Development workflow completion evidence and visible failure contract
