# Recipe Step Records and Lifetime

Reference for how a headless recipe step is tied to the process that started
it, the record each step writes on disk, and `amplihack recipe steps`, which
lists those records and reports steps that have lost their parent.

## Contents

- [Purpose](#purpose)
- [Headless steps](#headless-steps)
- [Lifetime guarantee](#lifetime-guarantee)
  - [Binding order](#binding-order)
  - [What each kill ends](#what-each-kill-ends)
  - [Exit status of a headless launch](#exit-status-of-a-headless-launch)
  - [Parent already gone](#parent-already-gone)
  - [Binding refused by the kernel](#binding-refused-by-the-kernel)
  - [Non-Linux targets](#non-linux-targets)
- [Step records](#step-records)
  - [Location and permissions](#location-and-permissions)
  - [Record schema (format 1)](#record-schema-format-1)
    - [The parent field](#the-parent-field)
  - [JSON Schema](#json-schema)
  - [When records are written and removed](#when-records-are-written-and-removed)
  - [Who removes stale records](#who-removes-stale-records)
  - [Diagnostics](#diagnostics)
  - [Rules for readers](#rules-for-readers)
- [Classification](#classification)
- [recipe steps](#recipe-steps)
  - [Which entries are listed](#which-entries-are-listed)
  - [Table output](#table-output)
  - [JSON output](#json-output)
  - [Exit codes](#exit-codes)
- [Handling orphans and stale records](#handling-orphans-and-stale-records)
- [Limitations](#limitations)
- [Related](#related)

---

## Purpose

A recipe runner starts each agent step through an `amplihack` launch path:

- `amplihack claude`, `amplihack copilot`, `amplihack codex`,
  `amplihack amplifier` and `amplihack rustyclawd`;
- `amplihack profile run <name> -- ...`, which launches Claude under a
  [profile](./profile-command.md);
- `amplihack claude --supervise NAME`, which records the session under a
  name and then launches it (see [agent command](./agent-command.md#amplihack-claude-supervise)).

All of these are launchers in the sense used on this page, and each binds and
records a headless step in the same way. A headless step has no pane from
which a person can steer it. Two guarantees apply to every headless step:

1. **Lifetime.** On Linux, when the process that started the step exits, the
   step stops too.
2. **Record.** Each step writes a small JSON file naming its launcher, its
   agent and its parent, so a step that has lost its parent can be found
   without guessing from process lists.

Interactive launches are unaffected by both.

## Headless steps

A launch is a headless step when both of these hold:

- `AMPLIHACK_RECIPE_RUN_ID` is set and not empty. `amplihack recipe run` sets
  it for the runner and everything the runner starts (see
  [Recipe Run Correlation](./recipe-run-correlation.md#run-identity)).
- stdin and stdout are **not both** terminals.

A launch with a terminal on both stdin and stdout keeps normal job control,
because a person can see and steer it. A launch with a terminal on only one of
the two counts as headless. This is the usual case when `recipe run` is
started from a terminal: the runner passes its stdin through and sends the
step's stdout to a file.

`AMPLIHACK_NONINTERACTIVE` plays no part in this decision.

| `AMPLIHACK_RECIPE_RUN_ID` | stdin | stdout | Headless step |
|---|---|---|---|
| unset or empty | any | any | no |
| set | terminal | terminal | no |
| set | terminal | file or pipe | yes |
| set | `/dev/null`, file or pipe | file or pipe | yes |
| set | `/dev/null`, file or pipe | terminal | yes |

The decision is made once per process, when the launcher starts. The
binding, the agent spawn and the record all use that one answer.

## Lifetime guarantee

### Binding order

On Linux, a headless step is bound in two places, both with SIGTERM as the
parent-death signal (`PR_SET_PDEATHSIG`):

1. **The launcher is bound to its parent.** The launcher reads its parent
   pid as the first thing `amplihack` does, before the update notice,
   self-heal or asset repair, any of which can take seconds. The binding
   itself happens later, at the start of the launch subcommand and before the
   [usage-limit supervisor](./profile-command.md#usage-limit-wait) runs: the
   launcher sets the parent-death signal, then reads its parent pid again and
   compares it with the first reading (see
   [Parent already gone](#parent-already-gone)). The
   parent is the recipe runner or, when the supervisor is active, the
   supervising `amplihack` process. The supervisor's attempt child is itself
   a launch and binds to the supervisor in the same way.
2. **The agent is bound to the launcher.** The agent sets its own
   parent-death signal between `fork` and `exec`, then checks that the
   launcher is still its parent. It is started in the launcher's process
   group and session. It does not get a new session or process group.

The binding is made on the launcher's main thread, which also forks the
agent. The parent-death signal follows the thread that forked the process,
not the whole parent process, so this matters (see [Limitations](#limitations)).

Interactive launches are not bound and start the agent exactly as before.

### What each kill ends

| Event | Result |
|---|---|
| The runner exits for any reason, including SIGKILL | The kernel sends the launcher SIGTERM. The launcher sends the agent SIGTERM, waits up to 3 s, sends SIGKILL if the agent is still running, removes its step record and exits with 143. |
| The launcher receives SIGTERM, SIGINT or SIGHUP | The launcher ends the agent in the same way, removes its record and exits with `128 + N` (see [Exit status](#exit-status-of-a-headless-launch)). |
| The launcher is killed with SIGKILL | The kernel sends the agent SIGTERM. The record stays on disk and `recipe steps` reports the step as `orphan` until the agent has exited, then `stale`. The current recipe runner does this to an agent step whenever the step times out. `recipe run` removes the stale record once its runner has exited (see [Who removes stale records](#who-removes-stale-records)). |
| The agent exits on its own | The launcher removes its record and exits with the agent's status. |

The table shows a launcher with no usage-limit supervisor. When a supervisor
is active, the kernel signals the supervisor, which passes SIGTERM on to its
attempt child, and the attempt child ends the agent as shown above.

The time from the runner's exit to the agent's exit is usually under a
second, and at most about 3 s after the launcher receives SIGTERM.

The signal reaches the agent process only. Processes that the agent started
itself, such as a test run started from a tool call, are not signalled. The
agent stays in the launcher's process group, which is also the runner's, so
the launcher has no group it could signal without also signalling the
runner. Whether such a process stops depends on whether the agent ends its
own children when it receives SIGTERM. Any that survive still carry
`AMPLIHACK_RECIPE_RUN_ID` and are ended by
[`amplihack recipe stop <run_id>`](./recipe-command.md#recipe-stop). Ending
the agent's process tree automatically is tracked in
[#2074](https://github.com/rysweet/amplihack-rs/issues/2074).

`recipe run` is covered separately: if it is killed with SIGKILL, the kernel
sends the runner SIGTERM (see [recipe run](./recipe-command.md#recipe-run)).
Once the runner exits, the chain above applies.

### Exit status of a headless launch

A headless launcher that stops because of a signal exits with `128 + N`,
where `N` is the first of SIGTERM, SIGINT or SIGHUP that it received. A
runner that is still alive, for example after `recipe stop` or a SIGTERM sent
to the launcher alone, therefore sees the step fail, not succeed.

| First signal received | Exit status |
|---|---|
| SIGTERM | 143 |
| SIGINT | 130 |
| SIGHUP | 129 |
| Not identified | 143 |

An interactive launch that stops on a signal still exits with 0, as before.

### Parent already gone

The first read of the parent pid is taken when `amplihack` starts, before
any startup work. If the parent exits at any point between then and the
binding, the launcher has already been reparented, to init or to a
subreaper such as `systemd --user`. The second read then returns that new
parent and no longer matches the first. The launcher starts nothing, writes
no record, prints one line on stderr and exits with status 1:

```text
amplihack: headless step parent exited before launch; not starting
```

This line is fixed text.

Reading the parent only at the binding would miss this case. The new parent
would pass the check, the agent would run bound to a process that never
exits, and the record would name that process as `parent`, so the step
would be reported as `live` indefinitely. An integration test holds a
launcher between self-heal and dispatch, kills its runner there, and checks
that no agent starts and no record is written.

One window remains: if the parent exits after it forks the launcher but
before `amplihack` reaches its first instruction, the first read already
returns the new parent. That window covers only `exec` and the process
start-up, and nothing in it waits on a lock or the network.

The agent makes the same check against the launcher. If the launcher has
gone by the time the agent's binding is set, the spawn fails with `ESRCH`
and the agent never runs.

### Binding refused by the kernel

The two bindings handle a refusal differently:

- **Launcher.** If the kernel refuses `PR_SET_PDEATHSIG` for the launcher, it
  prints one warning on stderr and continues without the binding. The agent
  is still bound to the launcher. The record still names the parent, so if
  the parent then exits, `recipe steps` reports the step as `orphan`.
- **Agent.** If the kernel refuses it for the agent, the spawn fails and the
  step does not run. An agent is never started unbound in a headless step.

### Non-Linux targets

On macOS and other non-Linux Unix targets there is no parent-death signal, so
neither binding is applied and a headless step can outlive its runner. The
agent is spawned exactly as for an interactive launch. The step still writes
its record, with every `start_ticks` set to `null`, and `recipe steps` still
reports it once its parent has gone.

## Step records

### Location and permissions

```text
<HOME>/.amplihack/runtime/recipe-steps/<recipe_run_id>/<launcher_pid>.json
```

- **`<HOME>`** is the value of `HOME`, which must be an absolute path.
  `AMPLIHACK_HOME`, `AMPLIHACK_RUNTIME_ROOT` and
  `AMPLIHACK_WORKFLOW_RUNTIME_DIR` do not move this location. The last two
  point at per-run directories that are removed when the run ends, and a
  record must outlive its run to be useful.
- **`<recipe_run_id>`** is the value of `AMPLIHACK_RECIPE_RUN_ID`. It must be
  1 to 128 ASCII characters from `A-Z`, `a-z`, `0-9`, `_` and `-`. The UUIDs
  that `recipe run` generates always qualify. With any other value no record
  is written and one [diagnostic](#diagnostics) is printed, which does not
  include the value. The step is still headless and still bound.
- **`<launcher_pid>`** is the pid of the launcher that started the agent.

Directories and files are protected as follows:

- `recipe-steps` and each run-id directory are created with mode `0700`, and
  records with mode `0600`. The modes are set at creation. An existing
  directory is never `chmod`ed.
- Before writing, the launcher checks `recipe-steps` and the run-id directory
  without following symlinks. It refuses to write when either is a symlink,
  is not a directory, is not owned by the current user, or is writable by
  group or others. This protects runs under `sudo` that keep the caller's
  `HOME`. `HOME` itself and the directories between `HOME` and
  `recipe-steps` are trusted, so a symlinked `~/.amplihack` still works.
- A record is written to `<launcher_pid>.json.<uuid>.tmp`, opened with
  `O_CREAT | O_EXCL | O_NOFOLLOW` and mode `0600`, flushed to disk, then
  renamed to `<launcher_pid>.json`. If any of these steps fails, the
  temporary file is removed. Readers ignore `.tmp` files.

A run with three agent steps that have started looks like this:

```console
$ ls -l ~/.amplihack/runtime/recipe-steps/5b60657b-76ef-4f49-8a22-8b89ed75f43e/
-rw------- 1 alice alice 612 Nov  7 10:15 41907.json
-rw------- 1 alice alice 612 Nov  7 10:15 41911.json
-rw------- 1 alice alice 611 Nov  7 10:16 41958.json
```

### Record schema (format 1)

```json
{
  "format": 1,
  "run_id": "5b60657b-76ef-4f49-8a22-8b89ed75f43e",
  "workspace_run_id": "0d4e2c1a-7f33-4b5c-9e8d-1a2b3c4d5e6f",
  "run_dir": "/home/alice/.amplihack/work/runs/0d4e2c1a-7f33-4b5c-9e8d-1a2b3c4d5e6f",
  "tool": "claude",
  "cwd": "/home/alice/src/myapp",
  "created_at": "2026-11-07T10:15:03Z",
  "launcher": { "pid": 41907, "start_ticks": 9184420 },
  "agent":    { "pid": 41912, "start_ticks": 9184431 },
  "parent":   { "pid": 41822, "start_ticks": 9183977 }
}
```

| Field | Type | Meaning |
|---|---|---|
| `format` | integer | Record format. Always `1` for this layout. |
| `run_id` | string | `AMPLIHACK_RECIPE_RUN_ID`, the recipe run this step belongs to. Always equal to the run-id directory name. |
| `workspace_run_id` | string or `null` | `AMPLIHACK_RUN_ID`, or `null` when it is unset or empty. |
| `run_dir` | string or `null` | `AMPLIHACK_RUN_DIR`, the run record directory, or `null` when it is unset or empty. |
| `tool` | string | The agent tool the launcher started: `claude`, `copilot`, `codex` or `amplifier`. `amplihack rustyclawd` records `claude`. |
| `cwd` | string | Working directory of the launcher when the record was written. Bytes that are not UTF-8 are replaced with U+FFFD. |
| `created_at` | string | RFC 3339 UTC time, to the second with a `Z` suffix, at which the record was written. |
| `launcher` | object | The `amplihack` process that started the agent. `launcher.pid` always equals the file name's pid. |
| `agent` | object | The agent process. |
| `parent` | object or `null` | The launcher's parent: the recipe runner, the supervising `amplihack` process, or a shell that started the launcher (see [Limitations](#limitations)). `null` only in the case given under [The parent field](#the-parent-field). |
| `*.pid` | integer | Process id, from 1 to 2147483647 (a signed 32-bit value, as `pid_t` is on Linux). |
| `*.start_ticks` | integer or `null` | Field 22 of `/proc/<pid>/stat`, the process start time in clock ticks since boot. `null` on non-Linux targets or when it could not be read. |

Every field is always present. Optional values are written as `null` and are
never left out.

A record never contains the agent's arguments, environment variables other
than the three named above, prompts or tokens. Record contents are never
written to logs. A serialised record larger than 64 KiB is not written.

#### The parent field

`parent.pid` is the value `getppid()` returned on the launcher's first read,
when `amplihack` started and before any binding (see
[Binding order](#binding-order)). The same read is used on every target and
whether or not the kernel accepted the binding.

`parent` is `null` when, and only when, that read returned 0. Linux returns 0
when the parent is outside the launcher's PID namespace, so it has no pid the
launcher can name. Any value of 1 or more is recorded as it is, including 1.
`parent.start_ticks` is read for that pid in the same way as for the launcher
and agent, and is `null` when it cannot be read.

### JSON Schema

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "amplihack recipe step record, format 1",
  "type": "object",
  "required": ["format", "run_id", "workspace_run_id", "run_dir", "tool",
               "cwd", "created_at", "launcher", "agent", "parent"],
  "properties": {
    "format": { "const": 1 },
    "run_id": { "type": "string", "pattern": "^[A-Za-z0-9_-]{1,128}$" },
    "workspace_run_id": { "type": ["string", "null"] },
    "run_dir": { "type": ["string", "null"] },
    "tool": { "type": "string" },
    "cwd": { "type": "string" },
    "created_at": { "type": "string", "format": "date-time" },
    "launcher": { "$ref": "#/$defs/proc" },
    "agent": { "$ref": "#/$defs/proc" },
    "parent": { "oneOf": [ { "$ref": "#/$defs/proc" }, { "type": "null" } ] }
  },
  "$defs": {
    "proc": {
      "type": "object",
      "required": ["pid", "start_ticks"],
      "properties": {
        "pid": { "type": "integer", "minimum": 1, "maximum": 2147483647 },
        "start_ticks": { "type": ["integer", "null"], "minimum": 0 }
      }
    }
  }
}
```

The schema does not set `additionalProperties`. Later revisions of format 1
may add fields, and readers ignore fields they do not recognise.

### When records are written and removed

- **One record per agent.** The launcher that spawns the agent writes the
  record immediately after the spawn succeeds, before it starts waiting for
  the agent. If the spawn fails, nothing is written. A supervising
  `amplihack` process spawns no agent and writes no record. Its attempt child
  writes the record and names the supervisor as `parent`. A running step
  therefore has one record, with or without supervision.
- **Window before the write.** Between the spawn and the write there is a
  short window in which the agent is running and has no record yet.
- **Removal.** When the agent has exited, when the launcher has ended it after
  SIGTERM, SIGINT or SIGHUP, or when the launch returns with an error, the
  launcher removes its own record. It first checks again that the run-id
  directory is a real directory owned by the current user. It then removes
  the run-id directory only if it is empty. It never removes `recipe-steps`
  itself, never removes anything recursively, and never reports a removal
  failure.
- **SIGKILL.** A launcher killed with SIGKILL cannot remove its record. This
  is routine, not rare: the current recipe runner kills an agent step's
  launcher with SIGKILL whenever the step times out. `recipe run` removes
  such records (see [Who removes stale records](#who-removes-stale-records)).
- **Launches with no record.** Interactive launches write no record. Neither
  do processes started outside the launcher's own agent spawn, such as
  `--auto` launches and blarify helpers (see [Limitations](#limitations)).

### Who removes stale records

`amplihack recipe run` owns the records its runner's steps leave behind. It
removes them in two passes:

1. **After each runner attempt.** Once the runner process has exited, every
   `stale` record under that attempt's run id is removed, and then the
   run-id directory if it is empty. `live` and `orphan` records are kept, so
   an orphan stays visible to `recipe steps`. A launcher that is still
   exiting after its parent-death signal removes its own record.
2. **At the end of the run.** A run-id directory from any earlier run is
   removed when every entry in it is a `stale` record and each record file
   was last modified longer ago than the cleanup keep window
   (`AMPLIHACK_CLEANUP_KEEP_HOURS`, 24 hours by default). This pass
   collects the records of a `recipe run` that was itself killed with
   SIGKILL. It runs under the same conditions as the
   [end-of-run sweep](../howto/clean-up-after-recipe-runs.md): not for
   `--dry-run`, and not when `AMPLIHACK_CLEANUP_ON_EXIT=0`.

Neither pass sends a signal. Each record is classified with the same
start-time check `recipe steps` and `recipe stop` use (see
[Classification](#classification)). It is removed only when all of these
hold:

- it is `stale`: no process it names is alive with its recorded start time;
- read a second time just before the removal, it is unchanged and still
  `stale`;
- `recipe-steps` and its run-id directory are real directories, owned by
  the current user and not writable by group or others.

A run-id directory that holds an `unreadable` entry is left whole, and so
is any file whose name is not a record name. Nothing is removed
recursively, and a failed removal is not reported.

### Diagnostics

A record failure never stops the step. The launcher prints one line on
stderr and carries on:

```text
amplihack: not writing a step record: <reason>; continuing
```

For example, when `~/.amplihack/runtime/recipe-steps` is a symlink:

```text
amplihack: not writing a step record: unsafe directory /home/alice/.amplihack/runtime/recipe-steps; continuing
```

| `<reason>` begins with | Cause |
|---|---|
| `invalid recipe run id` | `AMPLIHACK_RECIPE_RUN_ID` fails the run-id rule. The value itself is not printed. |
| `HOME is not set` | `HOME` is unset, empty or not an absolute path. |
| `unsafe directory <path>` | `recipe-steps` or the run-id directory is a symlink, is not a directory, is not owned by the current user, or is writable by group or others. |
| `record too large` | The serialised record exceeds 64 KiB. |
| `cannot create <path>` | A directory could not be created. The operating-system error follows. |
| `cannot write <path>` | The temporary file could not be created, written, flushed or renamed. The operating-system error follows. |

The whole line passes through the same sanitiser as `recipe steps` table
output, so control characters and bidirectional formatting characters in a
path are printed as `?`.

The launcher binding prints its own lines, described in
[Parent already gone](#parent-already-gone) and
[Binding refused by the kernel](#binding-refused-by-the-kernel).

### Rules for readers

Tools other than `recipe steps` that read records should follow the same
rules:

- Check `format` before any other field. A record whose `format` is missing
  or is not `1` is unreadable.
- Ignore fields you do not recognise.
- Treat a record with any `pid` below 1 or above 2147483647 as unreadable.
- Treat a record whose `run_id` or `launcher.pid` does not match its path as
  unreadable.
- Open regular files only, without following symlinks and without blocking
  (`O_NOFOLLOW | O_NONBLOCK`), and read at most 64 KiB.
- Treat a process as the recorded one only while its start time still
  matches `start_ticks`. Pids are reused.
- Treat a `state` you do not recognise in `recipe steps` JSON output as
  `unreadable`.
- Never signal a process or delete a file on the strength of a record alone.
  Any tool that acts on records needs its own fresh check of the process's
  owner, start time and ancestry, and its own review. amplihack's own
  removal of stale records is described under
  [Who removes stale records](#who-removes-stale-records).

## Classification

`recipe steps` reports each record in one of four states. A process counts as
**alive** only when all of these hold:

- its pid exists;
- it is not a zombie (state `Z` or `X` in `/proc/<pid>/stat`);
- its start time equals `start_ticks`, when `start_ticks` is known.

When `start_ticks` is `null`, existence alone is checked. On non-Linux
targets existence is tested with `kill(pid, 0)`: `ESRCH` means gone, and
`EPERM` means alive. A `parent` of `null` counts as alive.

| Launcher | Parent | Agent | State |
|---|---|---|---|
| alive | alive | any | `live` |
| alive | gone | any | `orphan` |
| gone | any | alive | `orphan` |
| gone | any | gone | `stale` |

| State | Meaning |
|---|---|
| `live` | The step is running under the process that started it. |
| `orphan` | The step, or its agent, is still running but the process that should end it has gone. |
| `stale` | Nothing recorded is running. The record was left by a launcher killed with SIGKILL, most often by the recipe runner when the step timed out. `recipe run` removes it once its runner has exited (see [Who removes stale records](#who-removes-stale-records)). |
| `unreadable` | The entry could not be read or parsed. The reason is given (see [Which entries are listed](#which-entries-are-listed)). It is never treated as a process. |

A pid that has been reused by an unrelated process has a different start
time, so it counts as gone and is never reported as the recorded step.

On Linux, with the [lifetime guarantee](#lifetime-guarantee) in force, an
`orphan` normally lasts no more than a few seconds: the time the launcher or
agent takes to exit after the parent-death signal. A step that stays
`orphan`, or one that stays `live` after its runner has gone, points at one
of the [limitations](#limitations).

## recipe steps

```
amplihack recipe steps [--orphans] [-f, --format <table|json>]
```

Lists every step record under `~/.amplihack/runtime/recipe-steps`, across all
run ids, with its [state](#classification). It only reads: it never sends a
signal and never removes or changes a file. It takes no positional
arguments.

| Option | Default | Effect |
|---|---|---|
| `--orphans` | off | Show only `orphan` and `unreadable` entries. `live` and `stale` entries are left out. |
| `-f`, `--format` | `table` | `table` or `json`. Any other value, including `yaml`, is a usage error. |

Entries are sorted by run id, then by launcher pid as a number.

```bash
# Every step record, all runs
amplihack recipe steps

# Steps that have lost their parent, and entries that could not be read
amplihack recipe steps --orphans

# Agent pids of orphaned steps, for a script
amplihack recipe steps --orphans --format json \
  | jq '.[] | select(.state == "orphan") | .record.agent.pid'
```

When `recipe-steps` does not exist, or holds no records, table output is
empty and JSON output is `[]`. Both exit 0.

### Which entries are listed

`recipe steps` looks only at `<root>/<run id>/<pid>.json`, where `<run id>`
follows the run-id rule and `<pid>` matches `^[1-9][0-9]{0,9}$` and fits in
a signed 32-bit integer. Each record file is opened with `O_NOFOLLOW` and
`O_NONBLOCK` and checked with `fstat`, so a symlink is never followed and a
FIFO never blocks the listing.

| Entry | How it is listed |
|---|---|
| `<pid>.json` that is a symlink, FIFO or other non-regular file | `unreadable`, reason `not a regular file` |
| `<pid>.json` larger than 64 KiB | `unreadable`, reason `record too large` |
| `<pid>.json` that is not valid JSON, or has a field of the wrong type or a pid above 2147483647 | `unreadable`, reason `invalid JSON` |
| Record with no `format` field | `unreadable`, reason `missing format` |
| Record whose `format` is not `1` | `unreadable`, reason `unsupported format <n>`, where `<n>` is the integer read |
| Record with a pid below 1 | `unreadable`, reason `invalid pid` |
| Record whose `run_id` or `launcher.pid` does not match its path | `unreadable`, reason `record does not match its path` |
| `<pid>.json` that could not be opened or read | `unreadable`, reason `cannot read: <error>` |
| Run-id directory that is a symlink | `unreadable`, reason `run directory is a symlink`. It is not descended. |
| Run-id directory owned by another user | `unreadable`, reason `run directory is not owned by this user`. It is not descended. |
| Run-id directory that cannot be listed, for example because its mode denies read or search to the user | `unreadable`, reason `cannot read: <error>`, with the run-id directory as its path. None of its records are listed. |
| `recipe-steps` itself, when it cannot be examined or listed for any reason other than not existing, and it is not one of the exit-1 cases under [Exit codes](#exit-codes) | One `unreadable` entry, reason `cannot read: <error>`, with `recipe-steps` as its path. Nothing else is listed and the exit code is 0. |
| Directory named like a record | Ignored |
| `*.tmp`, and any other name that is not `<pid>.json` (for example `07.json` or `99999999999.json`) | Ignored |
| Entry in the root whose name is not a valid run id, or a regular file named like one | Ignored |

`recipe-steps` and run-id directories that are writable by group or others
are listed normally. A group- or world-writable `recipe-steps` is not a cause
of exit 1. Only the launcher refuses to write into such a directory.

An entry for a run-id directory sorts before the records of that run id. An
entry for `recipe-steps` itself is the only entry.

The reason phrases in this table and in the
[Diagnostics](#diagnostics) table are fixed text, and scripts may match on
them. `<error>` is the operating system's error message and is not fixed. An
error message never includes the contents of a file.

### Table output

```console
$ amplihack recipe steps
STATE       RUN ID                                TOOL     LAUNCHER  AGENT  PARENT  CREATED               CWD                    RECORD                                                                                       REASON
live        5b60657b-76ef-4f49-8a22-8b89ed75f43e  claude   41907     41912  41822   2026-11-07T10:15:03Z  /home/alice/src/myapp  /home/alice/.amplihack/runtime/recipe-steps/5b60657b-76ef-4f49-8a22-8b89ed75f43e/41907.json  -
orphan      9c1d2e44-0a5b-4c3e-9f61-2d7a8b6c0e13  copilot  38211     38215  38190   2026-11-07T08:02:41Z  /home/alice/src/myapp  /home/alice/.amplihack/runtime/recipe-steps/9c1d2e44-0a5b-4c3e-9f61-2d7a8b6c0e13/38211.json  -
stale       9c1d2e44-0a5b-4c3e-9f61-2d7a8b6c0e13  claude   38240     38244  38190   2026-11-07T08:03:10Z  /home/alice/src/myapp  /home/alice/.amplihack/runtime/recipe-steps/9c1d2e44-0a5b-4c3e-9f61-2d7a8b6c0e13/38240.json  -
unreadable  9c1d2e44-0a5b-4c3e-9f61-2d7a8b6c0e13  -        -         -      -       -                     -                      /home/alice/.amplihack/runtime/recipe-steps/9c1d2e44-0a5b-4c3e-9f61-2d7a8b6c0e13/38302.json  unsupported format 2
```

- A missing value, such as every record field of an `unreadable` entry or a
  `parent` of `null`, is shown as `-`.
- The columns appear in the order shown, and the order is fixed.
- The `REASON` column is filled for `unreadable` entries only.
- `RUN ID` is taken from the path. It is `-` for an entry for
  `recipe-steps` itself.
- Every cell passes through a sanitiser that prints control characters
  (C0, DEL and C1) and the bidirectional formatting characters U+202A to
  U+202E and U+2066 to U+2069 as `?`. A planted `cwd` or path therefore
  cannot send terminal escape sequences or reorder the line.
- With no entries, nothing is printed, not even the header.

### JSON output

The output is one array. Every element has the same four keys:

```console
$ amplihack recipe steps --format json
[
  {
    "state": "orphan",
    "path": "/home/alice/.amplihack/runtime/recipe-steps/9c1d2e44-0a5b-4c3e-9f61-2d7a8b6c0e13/38211.json",
    "record": {
      "format": 1,
      "run_id": "9c1d2e44-0a5b-4c3e-9f61-2d7a8b6c0e13",
      "workspace_run_id": null,
      "run_dir": null,
      "tool": "copilot",
      "cwd": "/home/alice/src/myapp",
      "created_at": "2026-11-07T08:02:41Z",
      "launcher": { "pid": 38211, "start_ticks": 8722105 },
      "agent": { "pid": 38215, "start_ticks": 8722119 },
      "parent": { "pid": 38190, "start_ticks": 8721980 }
    },
    "reason": null
  },
  {
    "state": "unreadable",
    "path": "/home/alice/.amplihack/runtime/recipe-steps/9c1d2e44-0a5b-4c3e-9f61-2d7a8b6c0e13/38302.json",
    "record": null,
    "reason": "unsupported format 2"
  }
]
```

| Key | Type | Meaning |
|---|---|---|
| `state` | string | `live`, `orphan`, `stale` or `unreadable`. |
| `path` | string | Absolute path of the record file, of the run-id directory, or of `recipe-steps` itself. |
| `record` | object or `null` | The record as read, in the [format 1 schema](#record-schema-format-1). `null` for `unreadable`. |
| `reason` | string or `null` | Why the entry is unreadable. `null` for every other state. |

JSON output is plain JSON. Characters below U+0020 are escaped as JSON
requires, but DEL, C1 and bidirectional formatting characters are passed
through unchanged. Use table output to read records in a terminal.

### Exit codes

| Exit code | Meaning |
|---|---|
| 0 | Listing completed, including when `recipe-steps` is missing or empty. |
| 1 | `HOME` is unset, empty or not absolute, or `recipe-steps` exists but is a symlink, is not a directory, or is not owned by the current user. One line is printed on stderr. These are the only causes of exit 1. A group- or world-writable `recipe-steps`, and any directory or record that cannot be read, are listed as described under [Which entries are listed](#which-entries-are-listed). |
| 2 | Usage error, for example `--format yaml` or a positional argument. |

The two exit-1 messages are:

```text
amplihack: cannot list step records: HOME is not set
amplihack: cannot list step records: unsafe directory /home/alice/.amplihack/runtime/recipe-steps
```

The exit code does not depend on whether orphans were found. To check for
orphans in a script, filter the JSON output on `state`:

```bash
amplihack recipe steps --orphans --format json \
  | jq -e 'any(.[]; .state == "orphan")' >/dev/null && echo "orphaned steps found"
```

## Handling orphans and stale records

`recipe steps` reports and does nothing else. To act on what it reports:

- **An `orphan` that does not clear within a few seconds:** end the whole
  run with `amplihack recipe stop <run_id>`, which checks start times before
  it signals anything (see [recipe stop](./recipe-command.md#recipe-stop)).
  Then run `amplihack recipe steps` again: the entry is gone, or is now
  `stale`.
- **A `stale` record:** nothing to do in most cases. `recipe run` removes
  the stale records of its own run when its runner exits, and those of
  earlier runs after the cleanup keep window (see
  [Who removes stale records](#who-removes-stale-records)). A stale record
  can stay for longer when its run was started with
  `AMPLIHACK_CLEANUP_ON_EXIT=0`, or when its run-id directory also holds an
  `unreadable` entry. Such a record can be deleted by hand: delete the file
  shown in the `RECORD` column, then the run-id directory if it is empty.

```bash
rm ~/.amplihack/runtime/recipe-steps/9c1d2e44-0a5b-4c3e-9f61-2d7a8b6c0e13/38240.json
rmdir ~/.amplihack/runtime/recipe-steps/9c1d2e44-0a5b-4c3e-9f61-2d7a8b6c0e13
```

Do not signal a pid taken from a record without first checking that its
start time still matches `start_ticks`, and never do so when `start_ticks`
is `null`.

## Limitations

- **Forking thread.** The parent-death signal fires when the thread that
  forked the process exits, not only when the whole parent process exits. A
  runner that started steps from a short-lived thread would end them early.
  The launcher binds itself and forks the agent from its main thread, so the
  agent's binding is sound. The launcher's binding rests on an assumption
  about the recipe runner, which is a separate binary that amplihack does not
  control: that it forks each agent step from a thread that lives at least as
  long as the step. The current runner forks agent steps from its main
  execution thread, one after another, so the assumption holds today. The
  integration tests check the binding with a stand-in runner that starts the
  launcher as a direct child in the same way and is then killed: the step
  must still be running just before the kill and must end after it. Those
  tests do not run the real runner, so a runner release that moved agent
  steps onto short-lived threads would not be caught by them.
- **setuid and file-capability agents.** The kernel clears the parent-death
  signal when a process executes a setuid or file-capability binary. Such an
  agent is not bound to its launcher.
- **Launchers started by a bash step.** A launcher run from inside a bash
  step (for example `amplihack claude -p ...` in a script) binds to that bash
  process, and the record names it as `parent`. Bash steps themselves are not
  bound to the runner. The shell normally waits for the launcher, so it stays
  alive after the runner dies, and `recipe steps` goes on reporting the step
  as `live`. End such a run with `amplihack recipe stop <run_id>`.
- **`--auto` launches and blarify helpers.** Only the agent spawned by the
  launcher's own launch path is bound and recorded. `amplihack claude --auto`
  and the other auto-mode launches, and the blarify indexing helpers, start
  their children elsewhere. In a headless step the auto-mode launcher itself
  is still bound to its parent, but the processes it starts are not bound and
  have no record. Recipe agent steps do not use auto mode.
  `amplihack recipe stop <run_id>` ends them.
- **Grandchildren.** Processes started by the agent are not bound and are
  not signalled by the launcher (see [What each kill ends](#what-each-kill-ends)).
  In the incident that led to #2018, these were the processes doing the
  damage. This page therefore does not claim to close #2018 in full. Ending
  the agent's process tree is tracked in
  [#2074](https://github.com/rysweet/amplihack-rs/issues/2074).
- **Non-Linux targets.** There is no lifetime guarantee, and with
  `start_ticks` of `null` a reused pid can make a dead process look alive.
  See [Non-Linux targets](#non-linux-targets).
- **Window before the record.** A running agent has no record between its
  spawn and the write.
- **Killed launcher.** A launcher killed with SIGKILL leaves its record
  behind, reported as `orphan` and then `stale`. The recipe runner does this
  on every agent-step timeout. `recipe run` removes the record (see
  [Who removes stale records](#who-removes-stale-records)). A record that is
  still `orphan` when the runner exits is kept until a later end-of-run pass
  finds it `stale` and older than the keep window.
- **32-bit pids.** Pids are stored and read as signed 32-bit integers. A
  record with a larger pid is listed as `unreadable` with reason
  `invalid JSON`, not counted as gone.
- **Same-user processes.** Any process running as the same user can write,
  change or remove records, and can race the directory checks. The checks
  protect against other users and a `sudo` run with a shared `HOME`, not
  against the user's own processes. `recipe steps` reports what it finds and
  acts on none of it.
- **JSON output and terminals.** JSON output passes DEL, C1 and
  bidirectional formatting characters through. Only table output is
  sanitised.
- **Unknown states.** A consumer of the JSON output should treat a `state` it
  does not recognise as `unreadable`.
- **No signal is sent on the basis of a record.** No part of amplihack
  signals a process because a record names it. The launcher removes only its
  own record, `recipe steps` only reads, and `recipe run` removes only
  records that it has checked twice to be `stale`.

## Related

- [amplihack recipe](./recipe-command.md) - `recipe run`, signal handling, `recipe stop` and `recipe steps`
- [Recipe Run Correlation](./recipe-run-correlation.md) - Where `AMPLIHACK_RECIPE_RUN_ID` comes from
- [Recipe Executor Environment](./recipe-executor-environment.md) - The environment recipe steps run in
- [Usage-limit wait](./profile-command.md#usage-limit-wait) - The supervising process that becomes a step's parent
- [Clean Up After Recipe Runs](../howto/clean-up-after-recipe-runs.md) - `AMPLIHACK_RUN_ID` and `AMPLIHACK_RUN_DIR`
