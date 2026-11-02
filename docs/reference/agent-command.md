# Reference: `amplihack agent`

Register long-lived Claude Code sessions and resume them from their saved
conversation after their tmux session is lost. See the
[how-to](../howto/supervise-agent-sessions.md) for a walkthrough.

## Contents

- [Synopsis](#synopsis)
- [Common rules](#common-rules)
- [Value rules](#value-rules)
- [Register](#register)
- [Unregister](#unregister)
- [Status](#status)
- [Supervise](#supervise)
- [`amplihack claude --supervise`](#amplihack-claude-supervise)
- [SessionEnd markers](#sessionend-markers)
- [Files](#files)
- [Registry](#registry)
- [Supervisor log](#supervisor-log)
- [Errors](#errors)
- [Warnings](#warnings)
- [Exit codes](#exit-codes)
- [Service unit](#service-unit)
- [Platform support](#platform-support)

## Synopsis

```text
amplihack agent register NAME [--pid PID | --pane TARGET] [--tmux-socket PATH]
                              [--session SESSION] [--window WINDOW] [--dir PATH]
                              [--conversation ID] [--model MODEL] [--effort EFFORT]
                              [--add-dir DIR]...
                              [--permission-mode MODE | --dangerously-skip-permissions]
                              [--profile NAME] [--subprocess-safe] [--no-reflection]
                              [--env KEY=VALUE]...
amplihack agent unregister NAME
amplihack agent supervise [--once] [--interval SECS] [--batch N] [--gap SECS] [--max-restarts N]
amplihack agent status [--json]
amplihack claude [--profile NAME] [--subprocess-safe] [--no-reflection] --supervise NAME [-- CLAUDE_ARGS...]
```

| Command | Behaviour |
|---------|-----------|
| `agent register NAME` | Record a session under NAME, read from a running `claude` process or given by flags. Replaces any record with the same NAME. |
| `agent unregister NAME` | Remove a record. Never touches tmux or any process. |
| `agent supervise` | Check every active agent and resume the lost ones, every `--interval` seconds or once. |
| `agent status` | Print every record and its latest recovery result. Read-only. |
| `claude --supervise NAME` | Record the session being launched under NAME, launch it as usual, and record a clean exit. |

There are no other `agent` subcommands.

## Common rules

- Results go to stdout. Warnings go to stderr as `warning: ...` lines, and a
  failure ends with one `error: ...` line on stderr.
- Usage errors from the argument parser exit 2 before any file is read or
  written. Every other failure exits 1.
- A rejected value is never repeated in a message. A message names the flag,
  field or path and states the rule; for a value read from a process it gives
  the value's length in bytes instead. File contents are never quoted.
- Text that comes from outside amplihack, such as paths and tmux error lines,
  is printed and logged with control characters and the
  [format characters](#value-rules) replaced by `?`.
- No `agent` command writes under `~/.claude` or a profile directory. They
  only read `sessions/` and `projects/` there.
- `agent` commands do not restage amplihack's framework files before they
  run, so `status --json` output never contains an install banner.

## Value rules

Each value is checked where it enters (a flag, a running process, tmux
output, a hook), again when the registry is loaded, and again before every
registry write.

In this table, *control characters* also covers the Unicode format
characters that can make shown text differ from what it holds: the
direction embeddings, overrides and isolates (U+202A to U+202E and U+2066 to
U+2069), the zero-width characters and direction marks (U+200B to U+200F)
and the byte-order mark (U+FEFF).

| Value | Rule |
|-------|------|
| NAME | `^[A-Za-z0-9][A-Za-z0-9_.-]{0,63}$` |
| Conversation id | `^[A-Za-z0-9][A-Za-z0-9-]{0,63}$`. Starting with a letter or digit means an id can never be read as a Claude Code flag. |
| tmux session | 1 to 64 characters; no `:`, `.`, `#` or control characters. tmux rewrites `:` and `.` in session names and expands `#` formats. |
| tmux window | 1 to 64 characters; no `#` or control characters. |
| tmux socket | Absolute UTF-8 path, at most 107 bytes, no control characters. |
| Launch directory | Absolute UTF-8 path to an existing directory, with no `#` or control characters. Existence is checked by `register` and `claude --supervise`, not on load. |
| `--add-dir` | Made absolute against the launch directory; no `#` or control characters. |
| `--model`, `--effort`, `--permission-mode` | 1 to 256 bytes; no control characters; must not start with `-`. |
| `--profile` | The [profile name rules](profile-command.md#name-rules): the pattern `^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$`, no `..`, no trailing `.`, and no reserved name such as `CON` or `LPT1`. `register` and `claude --supervise` also require the profile's directory under the profile root to exist and not be a symlink. |
| `--env` | See [Recorded environment](#recorded-environment). |

## Register

```text
amplihack agent register NAME [flags]
```

### Register flags

| Flag | Field | Notes |
|------|-------|-------|
| `--pid PID` | | Read this `claude` process. Cannot be combined with `--pane`. |
| `--pane TARGET` | | Read the `claude` process running in this tmux pane. TARGET is any tmux target, such as `area-search:main` or `%12`. |
| `--tmux-socket PATH` | `tmux_socket` | Socket of the tmux server that hosts the agent. |
| `--session SESSION` | `tmux_session` | |
| `--window WINDOW` | `tmux_window` | |
| `--dir PATH` | `launch_dir` | The directory the session runs in. Claude Code finds a conversation by this directory. |
| `--conversation ID` | `conversation_id` | |
| `--model MODEL` | `model` | |
| `--effort EFFORT` | `effort` | |
| `--add-dir DIR` | `add_dirs` | Repeat for each directory. Given at all, the list replaces the one found. |
| `--permission-mode MODE` | `permission_mode` | Cannot be combined with `--dangerously-skip-permissions`. |
| `--dangerously-skip-permissions` | `skip_permissions` | |
| `--profile NAME` | `amplihack_flags.profile` | |
| `--subprocess-safe` | `amplihack_flags.subprocess_safe` | |
| `--no-reflection` | `amplihack_flags.no_reflection` | |
| `--env KEY=VALUE` | `env` | Repeatable. See [Recorded environment](#recorded-environment). |

A flag replaces the value `register` found for its field. A field without a
flag keeps the value found.

### Which process is read

| Given | Process |
|-------|---------|
| `--pid PID` | PID. It must be a `claude` process of yours that is not a zombie. Otherwise `register` fails with `process PID is not a claude process of this user`, whether the pid does not exist, belongs to another user or runs another program. |
| `--pane TARGET` | `register` asks tmux for the pane's process with `display-message -p -t TARGET '#{pane_pid}'`, on the socket from `--tmux-socket`, else on the first field of its own `$TMUX`, else on tmux's default server for `register`'s environment. It then uses the nearest `claude` process below that one. Finding none is an error. |
| Neither, without `--conversation` | The nearest `claude` ancestor of `register` itself. |
| Neither, with `--conversation ID` | The nearest `claude` ancestor, only when its conversation is known and equals ID. Its conversation is found as in [Fields read from the process](#fields-read-from-the-process). When it differs or cannot be found, no process is used: the ancestor is read only for that comparison, and nothing from it is recorded. This lets one session register another session that is already gone. |
| Neither, and no `claude` ancestor | No process. |

With `--pane`, and when it looks for an ancestor, `register` fails with
`cannot read the process table` when `/proc` cannot be read. With `--pid` that
is the `--pid` message above.

With no process, `--session`, `--window`, `--dir` and `--conversation` are
required; otherwise `register` fails with
`no claude process found; pass --session, --window, --dir and --conversation`.
With no process, `claude_pid` and `proc_start` are not recorded, and nothing
is read from any process: no options, no amplihack flags and no environment.

A process is a `claude` process when its `comm` is `claude`, when the base
name of its `argv[0]` is `claude`, or when it is a `node` process whose script
argument ends in `@anthropic-ai/claude-code/cli.js`.

Only processes whose real and effective uid are both yours are read.
`register` refuses a process whose command line contains `-p` or `--print`
(`process PID is a claude -p run, not an interactive session`). If a
process's start time changes while it is being read, its pid was reused, and
the process is treated as not found.

### Fields read from the process

| Field | Source |
|-------|--------|
| `conversation_id` | `sessionId` from `sessions/<pid>.json` in `~/.claude` or in any profile directory, used only when the file is yours, its `pid` is the process and its `procStart` equals the process's start time (field 22 of `/proc/<pid>/stat`). A `sessions/` directory that fails the [privacy check](#liveness) is an error that names it. Without such a file, the `--resume` value on the `claude` command line, unless `--fork-session` is also there: a forked session writes a new conversation. |
| `launch_dir` | The session file's `cwd`, else the process's working directory. |
| `model`, `effort`, `add_dirs`, `permission_mode`, `skip_permissions` | The `claude` command line; see [Command-line parsing](#command-line-parsing). |
| `amplihack_flags` | `--profile`, `--subprocess-safe` and `--no-reflection` on the command line of the nearest ancestor that is `amplihack claude` or `amplihack launch`. Every other option of that subcommand is skipped with its value, and reading stops at `--` or at the first argument that is not one of its options, as clap does. A profile whose directory no longer exists is not recorded, with a [warning](#warnings). With no `--profile` there, the profile is the one whose directory holds the process's session file; see [Which profile is recorded](#which-profile-is-recorded). |
| `env` | Allowlisted variables from that ancestor's environment, or from the `claude` process's when there is no such ancestor. |
| `tmux_socket` | The first comma-separated field of `TMUX` in the `claude` process's environment. With `--pane`, the socket tmux was asked on; when that was tmux's default server, its path `TMUX_TMPDIR/tmux-UID/default`, with `TMUX_TMPDIR` taken from `register`'s environment and `/tmp` when it is unset or empty. |
| `tmux_session`, `tmux_window` | `tmux display-message -p -t PANE '#{session_name}<TAB>#{window_name}'`. With `--pane`, PANE is TARGET, asked on the same server. Otherwise PANE is `TMUX_PANE` from the process's environment, asked on the server named by the first field of its `TMUX`, whatever `--tmux-socket` says; without both variables tmux is not asked. tmux is not asked when both `--session` and `--window` are given. The answer must be exactly the two fields and a newline; anything else is `tmux display-message returned an unexpected answer`. |
| `claude_pid`, `proc_start` | The process's pid and start time. |

When a process is found but a required field cannot be read, `register`
fails with `claude process PID found, but ITEMS could not be discovered; pass FLAGS`.

`amplihack claude` passes `--dangerously-skip-permissions` to every Claude Code
it starts. When a command line has both a permission mode and that flag, the
permission mode is recorded and `skip_permissions` is `false`.

### Command-line parsing

`register` and `claude --supervise` read only these options from a Claude
Code command line, and stop at a bare `--`:

- `--model V`, `--model=V`
- `--effort V`, `--effort=V`
- `--permission-mode V`, `--permission-mode=V`
- `--dangerously-skip-permissions`
- `--add-dir V...`, which takes the following arguments up to the next one
  that starts with `-`, and `--add-dir=V`; both repeatable
- `--resume ID`, `--resume=ID`, `-r ID`; the value is taken only when the
  next argument does not start with `-`
- `--fork-session`
- `-p`, `--print`

For single-value options the last one wins. No other option's value is
interpreted. A value that fails the [value rules](#value-rules) is not
recorded, and a warning gives the flag and the value's length in bytes:
`warning: ignored --resume from the claude command line (7 bytes): RULE`.

### Recorded environment

Only these variables are ever recorded, matched by exact name:

| Variable |
|----------|
| `ANTHROPIC_MODEL` |
| `ANTHROPIC_SMALL_FAST_MODEL` |
| `ANTHROPIC_DEFAULT_OPUS_MODEL` |
| `ANTHROPIC_DEFAULT_SONNET_MODEL` |
| `ANTHROPIC_DEFAULT_HAIKU_MODEL` |
| `CLAUDE_CODE_SUBAGENT_MODEL` |
| `CLAUDE_CODE_EFFORT_LEVEL` |
| `CLAUDE_CODE_MAX_OUTPUT_TOKENS` |
| `MAX_THINKING_TOKENS` |
| `AMPLIHACK_DEFAULT_MODEL` |
| `AMPLIHACK_AGENT_BINARY` |

- A value must be at most 4096 bytes, with no control characters.
  `AMPLIHACK_AGENT_BINARY` must also be `amplifier`, `claude`, `codex` or
  `copilot`.
- Every other variable is left out. That includes credentials and variables
  that redirect API traffic (`ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`,
  `ANTHROPIC_BASE_URL`, `ANTHROPIC_CUSTOM_HEADERS`, `GH_TOKEN`,
  `GITHUB_TOKEN`), the config-directory selectors (`CLAUDE_CONFIG_DIR`,
  `AMPLIHACK_CLAUDE_PROFILE`, `AMPLIHACK_CLAUDE_PROFILE_ROOT`), and the
  variables `amplihack claude` sets on the process it starts.
- `--env KEY=VALUE` records VALUE under KEY and replaces a value found for
  the same KEY. KEY must be on the list; otherwise `register` fails with
  `--env: only these variables can be recorded: ` followed by the list. The
  rejected argument is not repeated, because an argument typed in the wrong
  place can be a credential.
- With no process, only `--env` entries are recorded.
- An environment buffer read from `/proc` is used only to copy the listed
  names, and is never printed, logged or put in a message.

### Register output

One line on stdout:

```text
registered coordinator: session coordinator, window main, conversation 0f8e5c1a-3b2d-4c6e-9a7f-1d2e3f4a5b6c, directory /home/alex/src/mistt
```

Warnings go to stderr; see [Warnings](#warnings).

### Register effects

In one locked registry write, `register`:

1. refuses the registration when another `active` agent has the same
   conversation id (`this conversation is already used by registered agent OTHER`),
   or the same tmux socket and session
   (`tmux session S is already used by registered agent OTHER`). A record with
   no socket is compared as the default socket. Re-registering a NAME is not a
   conflict with itself;
2. replaces any record with the same NAME by a new one: state `active`;
   `restarts`, `last_recovery`, `alert`, `lost_at`, `ended_at` and `ended_by`
   cleared; a new `launch_id`; a new `registered_at`, and `launched_at`
   equal to it; `claude_pid` and `proc_start` from the process, or none. A
   supervisor pass that was working on the old record leaves the new one
   alone.

Before the write, `register` checks that the Claude config directories are
private (see [Liveness](#liveness)) and, when a socket is recorded, that its
directory is private (see [tmux calls](#tmux-calls)).

## Unregister

```text
amplihack agent unregister NAME
```

Removes NAME's record in one locked write and prints `unregistered NAME`. An
unknown NAME fails with `no registered agent named NAME` (exit 1). The
session, its tmux session and its processes are left alone. Its lines in the
log are kept.

## Status

```text
amplihack agent status [--json]
```

- Reads the registry without the lock. It checks `~/.amplihack` and
  `~/.amplihack/agents/` as the other commands do, but never creates them.
- With no registry, or no agents, it prints `no agents registered`, or
  `{"agents":[]}` with `--json`, and exits 0.
- A registry that cannot be loaded fails with the [error](#errors) for its
  cause (exit 1). Nothing is printed on stdout, so `--json` output is either
  complete or empty.
- No process or tmux checks are made. `restarts_last_hour` is computed when
  the command runs.

### Text form

One row per agent, sorted by name. Columns are padded to their widest cell,
with two spaces between them.

| Column | Content |
|--------|---------|
| `NAME` | Agent name. |
| `STATE` | `active`, `stopped`, or `ended ENDED_AT (ENDED_BY)`. |
| `CONVERSATION` | Conversation id, or `-`. |
| `DIRECTORY` | Launch directory. |
| `LAST RECOVERY` | `LOGGED_AT RESULT` from the agent's latest recovery line, or `-`. |

After the rows comes one `ALERT NAME: ALERT` line for each agent with an
alert.

```text
NAME         STATE                                 CONVERSATION                          DIRECTORY              LAST RECOVERY
area-search  active                                e7b1d4c9-2a6f-4e3b-8d5c-9f1a7e2b6d48  /home/alex/src/search  2026-10-29T23:27:43Z resumed
coordinator  active                                0f8e5c1a-3b2d-4c6e-9a7f-1d2e3f4a5b6c  /home/alex/src/mistt   2026-10-29T23:27:41Z resumed
custodian    stopped                               3d8a6f2b-1e5c-4b7a-9d2e-6c4f8a1b3e57  /home/alex/src/mistt   2026-10-30T00:41:40Z stopped: restart limit
writer       ended 2026-10-30T08:02:15Z (wrapper)  b4e9a2d7-6c1f-4d8b-a3e5-7f2c9d6b1a84  /home/alex/src/mistt   2026-10-29T23:28:44Z resumed
ALERT custodian: stopped: restart limit (3 restarts in the last hour)
```

### JSON form

One JSON document and a newline: `{"agents":[...],"version":1}`, with
`agents` sorted by name. Every key is always present; an absent value is
`null`.

| Top-level key | Type |
|---------------|------|
| `agents` | array of agent objects, described below |
| `version` | integer, `1`. It changes only when a key is removed or renamed, a key's type or meaning changes, or a new value is added to a key that holds one of a fixed set of values, such as `state` or `ended_by`. |

Each agent object:

| Key | Type |
|-----|------|
| `name` | string |
| `state` | `"active"`, `"ended"` or `"stopped"` |
| `conversation_id` | string or null |
| `launch_dir` | string |
| `tmux_session` | string |
| `tmux_window` | string |
| `restarts_last_hour` | integer: resumes in the last 3600 seconds |
| `last_recovery` | null, or an object with `logged_at` and `result` (strings) and `conversation_id`, `lost` and `resumed` (string or null) |
| `alert` | string or null |
| `ended_at` | string or null |
| `ended_by` | `"wrapper"`, `"session-end"` or null |

For each agent, `last_recovery` holds the fields of the agent's latest
recovery line in the [supervisor log](#supervisor-log), with `-` read as
`null`.

The `custodian` object from the example above, as
`amplihack agent status --json | jq '.agents[] | select(.name == "custodian")'`
prints it:

```json
{
  "alert": "stopped: restart limit (3 restarts in the last hour)",
  "conversation_id": "3d8a6f2b-1e5c-4b7a-9d2e-6c4f8a1b3e57",
  "ended_at": null,
  "ended_by": null,
  "last_recovery": {
    "conversation_id": "3d8a6f2b-1e5c-4b7a-9d2e-6c4f8a1b3e57",
    "logged_at": "2026-10-30T00:41:40Z",
    "lost": "2026-10-30T00:41:40Z",
    "result": "stopped: restart limit",
    "resumed": null
  },
  "launch_dir": "/home/alex/src/mistt",
  "name": "custodian",
  "restarts_last_hour": 3,
  "state": "stopped",
  "tmux_session": "custodian",
  "tmux_window": "main"
}
```

Keys are printed in alphabetical order, at the top level and in every
object.

Stability rule for `version` 1:

- A key is never removed or renamed, and its type and meaning never change.
- New keys may be added to any object. They take their alphabetical place,
  so read keys by name, never by position, and ignore keys you do not know.
- New values may be added to a string that is documented as one of a fixed
  set, such as `state` or `ended_by`, only with a new `version`.
- Recorded environment values never appear in the output, in either form.
  `status` prints no environment variables at all.

## Supervise

```text
amplihack agent supervise [--once] [--interval SECS] [--batch N] [--gap SECS] [--max-restarts N]
```

### Supervise flags

| Flag | Default | Range | Effect |
|------|---------|-------|--------|
| `--once` | off | | Make one pass, then exit. |
| `--interval SECS` | 60 | 1 or more | Time from the end of one pass to the start of the next. |
| `--batch N` | 6 | 1 or more | Agents resumed per batch. |
| `--gap SECS` | 60 | 0 or more | Wait between batches. There is no wait after the last batch. |
| `--max-restarts N` | 3 | 1 or more | Launch attempts allowed per agent in any hour; see [Restart limit](#restart-limit). |

A value out of range is a usage error (exit 2).

### Start-up and the loop

1. Off Linux, `supervise` prints
   `amplihack agent supervise needs Linux /proc; nothing was started` and
   exits 1.
2. It works out the path of the running `amplihack` executable once. A path
   that ends in ` (deleted)`, because `amplihack update` replaced the binary,
   loses that suffix when the plain path exists. A path that is not
   absolute, not UTF-8, or contains `=` or a control character is refused
   before any lock is taken:
   `the amplihack binary path must be absolute and UTF-8, without '=' or control characters, for env to run it; nothing was started`
   (exit 1).
3. It takes `~/.amplihack/agents/supervisor.lock` without waiting and writes
   its pid into it. If another supervisor holds the lock, it prints
   `another amplihack agent supervisor is running (pid N)`, or `(pid unknown)`
   when the file does not hold a pid, and exits 1. The lock is held until the
   process exits, with `--once` too, and is never inherited by tmux or any
   other program the supervisor starts.
4. It makes a pass. Without `--once`, it waits `--interval` seconds after the
   pass ends, then makes the next one.

SIGTERM and SIGINT stop the loop between agents or during a wait, and
`supervise` exits 0. A pass that fails (an unreadable registry, for example)
makes `--once` exit 1. Without `--once` the error is printed as an `error:`
line on stderr and the next pass runs after the interval, so a passing
problem does not put the service into a restart loop.

### A pass

1. Load the registry without the lock. A load failure ends the pass with an
   error.
2. Read `/proc` once, keeping only your processes that are not zombies. List
   the Claude config directories and read the session files in each. A
   failure here does not end the pass; it makes liveness unknown.
3. Refresh conversation ids: for each active agent whose recorded process is
   still alive (same pid and start time), take `sessionId` from that
   process's session file. When it differs, for example after `/clear`, it
   replaces the recorded id, unless another active agent has it. A
   `sessions/` directory that fails the privacy check changes nothing.
4. Apply [SessionEnd markers](#sessionend-markers).
5. [Classify](#classification) each `active` agent. `ended` and `stopped`
   agents are not looked at.
6. Run the [pre-checks](#pre-checks) on lost agents, including the
   [restart limit](#restart-limit).
7. [Order the rest and resume them in batches](#order-and-batches).

A pass never changes an agent that was registered again, or that stopped
being `active`, while the pass was working on it. A line is written to the
log after the registry write it belongs to.

### Liveness

An agent's conversation is running when any of these holds:

| Signal | Condition |
|--------|-----------|
| (a) Session file | A `sessions/*.json` file in `~/.claude` or in any profile directory has `sessionId` equal to the conversation id, and its `pid` is a live process of yours whose start time equals the file's `procStart`. |
| (b) Command line | A `claude` process of yours that is not a zombie has the conversation id in its command line. |
| (c) Recorded process | The recorded `claude_pid` and `proc_start` belong to a live `claude` process. |

With no conversation id, only (c) applies.

Liveness is *unknown*, and counts as running, when:

- `/proc` cannot be read;
- the location of the profile root cannot be worked out:
  `AMPLIHACK_CLAUDE_PROFILE_ROOT` is not a valid absolute path, `HOME` is not
  set, or `~/.amplihack/profile-root` cannot be read or is not valid. The
  reason is the [profile root error](#errors) text;
- the profile root, a config directory or a `sessions/` directory exists but
  cannot be listed;
- a config directory fails the privacy check below;
- a process named `claude` has a command line that cannot be read.

Any of the three signals wins over unknown.

These are not failures: a profile root that does not exist means there are
no profiles, a config directory without `sessions/` has no session files, and
a session file that cannot be parsed is skipped.

The config directories are `~/.claude` and every directory directly under
the profile root, found as `amplihack profile` finds it. Profile directories
that are symlinks are skipped. `~/.claude`, the profile root, each profile
directory and each `sessions/` directory below them must be owned by you and
not writable by other users (group write is accepted for your own private
group); `~/.claude` and the root may be symlinks to directories that pass. If
any fails, liveness is unknown with the reason
`Claude config directory is not private`, and nothing is resumed. Session
files are read without following symlinks, at most 64 KiB each; files owned
by another user are skipped, and only `pid`, `sessionId`, `procStart` and
`cwd` are used.

### tmux session state

The supervisor asks `tmux [-S SOCKET] has-session -t =SESSION`, once for each
distinct socket and session in a pass. `-S` is passed when a socket is
recorded. The rows below are tried in order, and the first that applies
decides. Without a recorded socket, rows 1 to 3 look at tmux's default
socket, `TMUX_TMPDIR/tmux-UID/default` for the supervisor's environment
(`/tmp` when `TMUX_TMPDIR` is unset or empty), which is the one tmux uses
without `-S`. So after a reboot, with no tmux server started yet, the
session is absent rather than unknown.

| Order | Outcome | State |
|-------|---------|-------|
| 1 | The socket's directory exists, and it, a directory above it or an existing socket file fails the [socket checks](#tmux-calls) | unknown, reason `socket directory is not private`; tmux is not run |
| 2 | The socket's directory does not exist | absent; tmux is not run |
| 3 | The socket file does not exist | absent; tmux is not run |
| 4 | tmux exits 0 | present |
| 5 | tmux exits 1 with `can't find session`, `no server running` or `session not found` on stderr | absent |
| 6 | Anything else: tmux missing, a timeout, a signal, other text | unknown, with the reason |

An unknown state counts as present. Because the socket checks come first, a
socket directory that another user created after a reboot leaves a lost
agent skipped with `skipped: tmux session exists (tmux state unknown: socket directory is not private)`,
even when no socket file is in it.

### Classification

| Conversation | tmux session | Outcome | Line written |
|--------------|--------------|---------|--------------|
| running or unknown | present or unknown | healthy; `lost_at` cleared, and a `skipped:` or `failed:` result in `last_recovery` cleared | none |
| running | absent | skipped | `skipped: already running` |
| unknown | absent | skipped | `skipped: already running (liveness unknown: REASON)` |
| not running | present | skipped | `skipped: tmux session exists` |
| not running | unknown | skipped | `skipped: tmux session exists (tmux state unknown: REASON)` |
| not running | absent | lost: `lost_at` set if empty | see [Pre-checks](#pre-checks) |

### Which profile is recorded

`amplihack claude` picks its config directory from `--profile`, else an
inherited `AMPLIHACK_CLAUDE_PROFILE` (which auto-switch may redirect to
another profile), else the default profile pointer, else `~/.claude`. A
profile that is the login rotator's live login at launch is not used as a
directory: its login is in `~/.claude`, so the launch runs against
`~/.claude`. A `--profile` on the command line therefore does not say where
the conversation is written. The profile recorded is taken from the
directory that holds the process's sessions file, whichever way the launch
chose it:

| How the launch chose its directory | `amplihack_flags.profile` |
|------------------------------------|---------------------------|
| Session file in a profile directory (`--profile NAME`, an inherited `AMPLIHACK_CLAUDE_PROFILE` after any auto-switch, or the default pointer) | that directory's profile name |
| Session file in `~/.claude` (no profile, or a profile that was the rotator's live login at launch, including one named by `--profile`) | not recorded |
| No session file found, `--profile NAME` given to `register` or `claude --supervise` | NAME |
| No session file found, `--profile NAME` on the `amplihack claude` ancestor (`register`) | NAME |
| No session file found and no `--profile` | not recorded |

`agent register --profile NAME` whose process's session file is in another
directory records that directory's profile, or none for `~/.claude`, and
prints `note: --profile NAME not recorded; the conversation is in ...`.
`claude --supervise` records the profile when its id thread finds the
session file, replacing the `--profile` recorded at launch.

Because a profile found this way is recorded, the
[resume command](#resume) passes `--profile NAME` for it, and a later
change of the default pointer, of the inherited variable or of the
rotator's live login does not move the resume to another directory. With
no profile recorded, the resume and the [pre-check](#pre-checks) both
follow the default pointer as it is at the time of the resume; a
conversation in `~/.claude` is then not resumed while the pointer names a
profile other than the rotator's live one.

### Pre-checks

Each lost agent is checked in this order; the first failure decides.

| Check | Result |
|-------|--------|
| The agent has a conversation id | `failed: no conversation id` |
| The launch directory exists | `failed: launch directory missing` |
| The transcript exists | `failed: no transcript`; also when the recorded profile's directory no longer exists, or the default profile pointer cannot be read. If the transcript was archived, `amplihack sessions restore ID` brings it back |
| The [restart limit](#restart-limit) is not reached | `stopped: restart limit`, followed by the ALERT line |

The transcript is `CONFIG/projects/ENCODED/ID.jsonl`, where CONFIG is the
directory the resume will use. That is the directory `amplihack claude`
resolves for the [resume command](#resume), which removes
`AMPLIHACK_CLAUDE_PROFILE` and passes `--profile` only when one is recorded:
the recorded profile's directory, else the default profile pointer's, else
`~/.claude`. A profile that is the login rotator's live login resolves to
`~/.claude`, as it does for the launch. ENCODED is the launch directory with every `/` and `.`
replaced by `-`; the name with every character other than an ASCII letter or
digit replaced by `-` is accepted as well.

The daily [session archive](./sessions-command.md#live-sessions) keeps the
transcript of every agent whose state is `active` or `stopped`, however long
it waits to be resumed, and archives an `ended` agent's transcript like any
other old session. A transcript can still be missing because it was
archived before the agent was registered, or by hand. In that case
`amplihack sessions restore ID` restores it, and a later pass resumes the
agent if it is `active`.

### Restart limit

Every launch attempt counts: a `resumed` result and a `failed: tmux: LINE`
result each append the attempt's time to `restarts`, so an agent whose launch
keeps failing is stopped by the limit instead of being retried every pass.
Skips and the other failures start nothing and do not count. Before an agent
is resumed, its `restarts`
entries from the last 3600 seconds are counted (entries up to 300 seconds in
the future count too, and later ones are dropped, so a clock change cannot
block an agent for long). When the count is `--max-restarts` or more, the
agent is not resumed. Instead:

- its state becomes `stopped`, and `alert` is set to
  `stopped: restart limit (N restarts in the last hour)`;
- two lines are written:

  ```text
  2026-10-30T00:41:40Z agent=custodian conversation=3d8a6f2b-1e5c-4b7a-9d2e-6c4f8a1b3e57 lost=2026-10-30T00:41:40Z resumed=- result=stopped: restart limit
  2026-10-30T00:41:40Z ALERT: agent custodian stopped: restart limit (3 restarts in the last hour)
  ```

With the default of 3, an agent can be resumed three times in an hour, and a
fourth loss in that hour stops it. A stopped agent becomes `active` again only
through `agent register`, which also clears its restart history, or through
`amplihack claude --supervise NAME`, which keeps the history.

### Order and batches

The remaining lost agents are ordered by the number of lines from

```text
git -c core.fsmonitor=false -C LAUNCH_DIR status --porcelain --untracked-files=no
```

highest first, then by name. git runs with `GIT_OPTIONAL_LOCKS=0` and
`GIT_TERMINAL_PROMPT=0`, without `GIT_DIR`, `GIT_WORK_TREE`,
`GIT_INDEX_FILE` or `GIT_COMMON_DIR`, with stdin closed, a 10-second timeout
and output capped at 1 MiB. A directory that is not a repository, or any
error, counts as 0. `git status` can still run a clean filter
(`filter.NAME.clean`) set in the repository's own configuration. That
configuration belongs to you, and git's `safe.directory` check refuses a
repository that another user owns, so this runs nothing you did not set up.

The agents are resumed `--batch` at a time, with `--gap` seconds between
batches. Immediately before each resume, the supervisor:

1. reads the agent's record again, and drops the agent without a line if it
   was removed, registered again, or is no longer `active`;
2. reads `/proc` and the session files again, and repeats the liveness, tmux
   and transcript checks. A changed outcome writes its own line instead of a
   resume;
3. skips a conversation already resumed in this pass, with
   `skipped: already running`.

### Resume

The supervisor creates the agent's tmux session with one argv and no shell:

```text
tmux [-S SOCKET] new-session -d -s SESSION -n WINDOW -c LAUNCH_DIR -- PANE_ARGV
```

PANE_ARGV is, in order:

1. `env`
2. `-u NAME` for each of these names, in this order and without repeats: the
   names in `SESSION_MARKERS` (`amplihack-utils`), then
   `CLAUDE_CODE_SESSION_ID`, `CLAUDE_CODE_ENTRYPOINT`, `CLAUDE_PROJECT_DIR`,
   `AMPLIHACK_SESSION_ID`, `AMPLIHACK_DEPTH`, `AMPLIHACK_SESSION_DEPTH`,
   `AMPLIHACK_TREE_ID`, `CLAUDE_CONFIG_DIR` and `AMPLIHACK_CLAUDE_PROFILE`.
   `TMUX` and `TMUX_PANE` are kept, because the pane needs its own.
3. `KEY=VALUE` for each recorded variable, sorted by name.
4. The `amplihack` executable, then `claude`.
5. `--subprocess-safe` and `--no-reflection` when recorded, `--profile P`
   when recorded, then `--supervise NAME`.
6. `--`
7. `--resume ID`, then the [resume message](#resume-message) as one argument.
8. `--model M` and `--effort E` when recorded, then `--add-dir D` for each
   directory. They follow the message because `--add-dir` takes every
   following argument up to the next option.
9. `--permission-mode X` when recorded, otherwise
   `--dangerously-skip-permissions` when recorded.

Example, wrapped for reading, with MESSAGE standing for the message:

```text
tmux -S /tmp/tmux-1000/default new-session -d -s coordinator -n main -c /home/alex/src/mistt -- \
  env -u CLAUDECODE -u CLAUDE_CODE -u CLAUDE_CODE_SESSION_ID -u CLAUDE_PROJECT_DIR \
  -u CLAUDE_CODE_ENTRYPOINT -u COPILOT_CLI -u GITHUB_COPILOT -u GITHUB_COPILOT_AGENT \
  -u COPILOT_AGENT -u AMPLIHACK_SESSION_ID -u AMPLIHACK_DEPTH -u AMPLIHACK_SESSION_DEPTH \
  -u AMPLIHACK_TREE_ID -u CLAUDE_CONFIG_DIR -u AMPLIHACK_CLAUDE_PROFILE \
  CLAUDE_CODE_EFFORT_LEVEL=high \
  /home/alex/.local/bin/amplihack claude --no-reflection --profile work --supervise coordinator \
  -- --resume 0f8e5c1a-3b2d-4c6e-9a7f-1d2e3f4a5b6c MESSAGE \
  --model opus --effort high --add-dir /home/alex/src/mistt-docs --dangerously-skip-permissions
```

Every element is a constant, a validated registry field, the `amplihack`
path or the message. Because PANE_ARGV always has more than one element, tmux
runs it directly and never through `sh -c`.

When the recorded socket's directory is missing, for example after a reboot
cleared `/tmp`, the supervisor checks the deepest existing directory above it
and creates each missing directory with mode `0700`, one level at a time,
before it runs tmux. `new-session` starts a tmux server on the socket if none
is running.

| tmux outcome | Effect |
|--------------|--------|
| Exit 0 | The resume time is appended to `restarts`, `last_recovery` is set, `lost_at` is cleared, and a `resumed` line is written with the old `lost_at`. The resumed `amplihack claude --supervise` writes its own `launch_id` and `launched_at`. |
| Anything else | `failed: tmux: LINE`, where LINE is tmux's first stderr line, at most 200 characters. The attempt's time is appended to `restarts`, so it counts towards the [restart limit](#restart-limit), even when the repeated line is not written again. |

#### Server placement

The supervisor checks whether a tmux server is running before the first
resume on each tmux socket in a pass, with the same `-S SOCKET` that the
resume uses:

```text
tmux [-S SOCKET] list-sessions
```

The rows are tried in order, as for the [session state](#tmux-session-state)
check, and the first that applies decides. Rows 1 to 3 look at the socket
before tmux is run, so after a reboot, when the socket's directory or file
does not exist yet, the server counts as absent.

| Order | Outcome | Server state | Launch |
|-------|---------|--------------|--------|
| 1 | The socket's directory exists, and it, a directory above it or an existing socket file fails the [socket checks](#tmux-calls) | unknown, reason `socket directory is not private`; tmux is not run | None. The result is `skipped: tmux session exists (tmux state unknown: REASON)`, and no restart is counted. |
| 2 | The socket's directory does not exist | absent; tmux is not run | Through `systemd-run`, below. |
| 3 | The socket file does not exist | absent; tmux is not run | Through `systemd-run`, below. |
| 4 | `list-sessions` exits 0 | present | Plain `tmux new-session`, as above. |
| 5 | `list-sessions` exits 1 with `no server running` on stderr | absent | Through `systemd-run`, below. |
| 6 | Anything else: tmux missing, a timeout, a signal, other text | unknown | None, as in row 1. |

When the server is absent and `systemd-run` is on `PATH`, the
`new-session` argv is run as:

```text
systemd-run --user --scope --collect --quiet --unit=amplihack-tmux-server-TS-N -- tmux [-S SOCKET] new-session ...
```

TS is the Unix time of the launch in whole seconds. N is a counter that
starts at 1 in each pass and goes up by one for each socket on which the
pass finds no server, so scopes started on two sockets in the same second do not share
a name. Both are decimal integers, and nothing else is placed in the unit
name. `-S SOCKET`, when recorded, comes before `new-session`. tmux then
starts its server inside the transient scope
`amplihack-tmux-server-TS-N.scope`, outside the service's control group.
`--collect` removes the scope once its last process exits. `systemd-run`
follows the [tmux call rules](#tmux-calls): one argv, no shell, stdin
closed, a 10-second timeout and output capped at 64 KiB per stream. It is
always run with `--user`; amplihack never runs it, or `systemctl`, for the
system manager.

| `systemd-run` outcome | Effect |
|-----------------------|--------|
| Exit 0 | The resume succeeded, as for tmux exit 0. |
| Not on `PATH` | Plain `tmux new-session`, then a WARNING line with REASON `systemd-run is not on PATH`. |
| Non-zero exit, timeout or signal | `tmux [-S SOCKET] has-session -t =SESSION` runs first, read as in the [session state](#tmux-session-state) check. If the session exists, the resume succeeded and nothing more is started. If its state is unknown, nothing more is started, the result is `skipped: tmux session exists (tmux state unknown: REASON)`, and a restart is counted, because `systemd-run` may have started the session. Otherwise plain `tmux new-session` runs once, then a WARNING line with REASON `systemd-run: LINE` (its first stderr line, or why it could not be run), `systemd-run exited N` (no stderr), `systemd-run was ended by a signal`, or `systemd-run did not finish within 10 s`. |

The WARNING line, written to the [supervisor log](#supervisor-log) only when
the plain `tmux new-session` exits 0:

```text
TIME WARNING: agent NAME tmux server started without its own scope: REASON
```

The outcome of a plain `tmux new-session` decides the resume as in the
table above. When it fails, the result is `failed: tmux: LINE` and no
WARNING line is written: the failure line already records the attempt.
Later resumes on the same socket in the pass use plain tmux without a new
check, or, when the server state was unknown, are skipped with the same
result. A server that something else ends between two of them
is started again by the later resume without a scope and without a WARNING
line; the next pass checks again.

#### Resume message

The message is a constant with two times:

> amplihack agent supervisor: the terminal session for this agent was lost at
> LOST_AT. This conversation was resumed at NOW. Before continuing, check what
> your interrupted step had already done (git status, open pull requests,
> running processes) and do not repeat finished steps.

LOST_AT is the agent's `lost_at`: the time the supervisor first found the
agent lost. NOW is the time of the resume. Both are UTC times such as
`2026-10-29T23:27:41Z`. Nothing else, from a file, a transcript or tmux, is
ever put into a conversation.

#### Environment of a resumed session

A new tmux window gets the tmux server's global environment, not the
supervisor's. When the supervisor starts the server, that is the
supervisor's own environment without the [client removals](#tmux-calls);
when a server is already running on the socket, it is that server's. In the
window, `env` then removes the names in step 2 and sets the recorded
variables, and `amplihack claude` sets its own launch variables as for any
launch.

### tmux calls

Every tmux command that `register`, `claude --supervise` and the supervisor
run:

- is found on `PATH` and run with one argv, stdin closed, a 10-second timeout
  and output capped at 64 KiB per stream;
- runs without `TMUX`, `TMUX_PANE`, `CLAUDECODE`, `CLAUDE_CODE_SESSION_ID`,
  `CLAUDE_CODE_ENTRYPOINT`, `CLAUDE_PROJECT_DIR`, `CODEX_THREAD_ID`,
  `CODEX_SESSION_ID`, `CLAUDE_CONFIG_DIR`, `AMPLIHACK_CLAUDE_PROFILE`, and
  every variable whose name starts with `COPILOT_` or `GITHUB_COPILOT`;
- when it passes `-S SOCKET`, runs only after these checks:
  1. the socket's directory is a directory, not a symlink, owned by you, with
     no permission bits for other users and not writable by other users;
  2. every directory above it, up to `/`, is owned by you or by root, and has
     the sticky bit if other users can write to it, as `/tmp` has;
  3. an existing socket file is a socket owned by you, not a symlink.

  A tmux client sends its whole environment to whatever listens on the
  socket, so these checks keep the supervisor's environment from reaching
  another user's listener. A failure makes `has-session` unknown, makes a
  resume `failed: tmux: socket directory is not private`, and makes
  `register` and `claude --supervise` exit 1 with
  `tmux socket: its directory must be owned by you and closed to other users`.
  Without `-S`, `has-session` first makes these checks on tmux's default
  socket, as in [tmux session state](#tmux-session-state); other commands
  leave it to tmux's own check of its default directory.

No command kills a tmux server, session, window or pane, sends keys, or uses
`remain-on-exit`.

`systemd-run`, used only for [server placement](#server-placement), is found
on `PATH` and run under the same rules: one argv, stdin closed, a 10-second
timeout, output capped at 64 KiB per stream, and the same variables removed.

## `amplihack claude --supervise`

```text
amplihack claude [--profile NAME] [--subprocess-safe] [--no-reflection] --supervise NAME [-- CLAUDE_ARGS...]
```

`--supervise NAME` records the session being launched under NAME, then runs
the usual `amplihack claude` launch unchanged. `--supervise` and NAME are not
passed to Claude Code; CLAUDE_ARGS are. Help text:
`Record this session under NAME so 'amplihack agent supervise' can resume it if its tmux session is lost.`

`--supervise` cannot be combined with `--auto`, `--append`, `--docker` or
`--checkout-repo` (usage error, exit 2).

### Preconditions

Each of these exits 1 before Claude Code starts:

| Condition | Message |
|-----------|---------|
| Not Linux | `amplihack claude --supervise needs Linux; nothing was started` |
| `$TMUX` or `$TMUX_PANE` empty or unset | `amplihack claude --supervise needs tmux: $TMUX and $TMUX_PANE are not set` |
| NAME fails the [value rules](#value-rules) | `NAME: RULE` |
| `-p` or `--print` in CLAUDE_ARGS | `amplihack claude --supervise needs an interactive session; -p and --print are not supported` |
| The socket from `$TMUX` fails the [checks](#tmux-calls) | `tmux socket: its directory must be owned by you and closed to other users` |
| tmux cannot name the pane's session and window | `tmux display-message failed: LINE`, or `tmux display-message returned an unexpected answer` |
| A tmux name fails the value rules | `tmux session name RULE` or `tmux window name RULE` |
| The current directory cannot be read | `cannot read the current directory: KIND` |
| The current directory fails the value rules | `current directory: RULE` |
| `--profile` fails the name rules, its directory is missing, or the profile root cannot be worked out | the [`--profile` and profile root errors](#errors) |
| Another active agent has the same socket and tmux session, or the conversation id from `--resume` | the [register conflict messages](#register-effects) |
| The registry cannot be loaded or locked | the [error](#errors) for its cause |

### What it records

In one locked write, before the launch, NAME's record is created or updated
in place:

| Field | Value |
|-------|-------|
| `tmux_socket` | The first comma-separated field of `$TMUX`. |
| `tmux_session`, `tmux_window` | `tmux display-message -p -t $TMUX_PANE '#{session_name}<TAB>#{window_name}'`. |
| `launch_dir` | The current directory. |
| `conversation_id` | `--resume ID` in CLAUDE_ARGS, unless `--fork-session` is also there. Otherwise cleared, and found later; see below. |
| `model`, `effort`, `add_dirs`, `permission_mode`, `skip_permissions` | CLAUDE_ARGS, [parsed](#command-line-parsing) as for `register`. |
| `amplihack_flags` | This command's `--profile`, `--subprocess-safe` and `--no-reflection`. Without `--profile`, the profile is recorded later, with the conversation id; see below and [Which profile is recorded](#which-profile-is-recorded). |
| `env` | The [allowlisted variables](#recorded-environment) in the current environment. |
| `state` | `active`. |
| `launch_id`, `launched_at` | A new id, and now. |
| `lost_at`, `ended_at`, `ended_by`, `claude_pid`, `proc_start` | Cleared. |
| `alert` | Cleared when the record was `stopped`. |
| `restarts`, `registered_at`, `last_recovery` | Kept. A new record gets `registered_at` now. |

Keeping `restarts` means a crash loop is still counted when each resume
starts a new launch.

While Claude Code runs, `--supervise` writes nothing to the terminal. In the
background it looks, every second for up to 120 seconds, for the `claude`
process it started and that process's session file. When it finds them it
records `claude_pid`, `proc_start` and the conversation id, unless the id
belongs to another active agent or the record no longer carries this
launch's `launch_id`. When no `--profile` was given, it also records the
profile whose directory held the session file, as `amplihack_flags.profile`;
a session file in `~/.claude` records no profile. If it finds nothing in that time, or cannot record what
it found, it appends one `WARNING:` line to the
[supervisor log](#supervisor-log) with the last reason, such as a config or
`sessions/` directory that fails the privacy check, or
`no claude process with a session file found within 120 seconds`.

### Clean exit

Before the launch, `--supervise` starts watching for SIGHUP and SIGTERM. When
the launch returns successfully and neither signal arrived, it marks the
record `ended`, with `ended_by: wrapper` and `ended_at` now, if the record is
still `active` and still carries this launch's `launch_id`. A failure to
write prints
`warning: could not mark agent NAME as ended: REASON; the supervisor may resume it`
and does not change the exit status.

| How the session ends | Record |
|----------------------|--------|
| Claude Code exits with status 0, for example after `/exit` | `ended` (`wrapper`) |
| SIGINT reaches `amplihack claude` | `ended` (`wrapper`) |
| Claude Code exits with another status | stays `active`; `amplihack claude` exits with Claude Code's status before `--supervise` can record anything |
| SIGHUP: the terminal or tmux server was lost | stays `active` |
| SIGTERM: the session was told to stop | stays `active` |

The exit status is that of the usual launch.

## SessionEnd markers

Sessions registered with `agent register` do not run under `--supervise`.
Their clean exits are reported by amplihack's `SessionEnd` hook, the
`session-stop-event` subcommand of `amplihack-hooks`, which `amplihack install`
and the amplihack Claude Code plugin both install.

### Writing a marker

The hook writes `~/.amplihack/agents/ended/SESSION_ID` only when all of these
hold:

- the hook input's `reason` is `prompt_input_exit` or `logout`;
- the input carries its own `session_id`, which passes the conversation id
  rule;
- `~/.amplihack` is a directory owned by you that other users cannot write
  to, not a symlink; `~/.amplihack/agents/` is private (mode `0700`, owned by
  you); and `registry.json` in it is a regular file owned by you.

The directory `ended/` is created with mode `0700`, and the file is written
atomically with mode `0600`:

```json
{"at":1793322100,"reason":"prompt_input_exit","session_id":"3d8a6f2b-1e5c-4b7a-9d2e-6c4f8a1b3e57"}
```

`at` is the time in Unix seconds. Any failure is logged at debug level and
ignored; the hook's output and exit status do not change.

These are the `reason` values Claude Code sends with `SessionEnd`:

| Reason | When Claude Code sends it | Marker |
|--------|---------------------------|--------|
| `prompt_input_exit` | The session is left from its prompt, for example with `/exit`. | written |
| `logout` | `/logout`. | written |
| `clear` | `/clear` starts a new conversation in the same session. | none |
| `resume` | `/resume` switches the session to another conversation. Like `clear`, the session goes on; the supervisor follows the new conversation id at its next pass. | none |
| `other` | Any other ending, including a lost terminal or tmux server (SIGHUP) and SIGTERM. | none |

A missing reason, or any value not in this table, writes no marker. A
session lost with its terminal therefore never ends its agent through a
marker.

### Applying a marker

At the start of each pass the supervisor reads `ended/`. When `ended/` is a
symlink, or not a directory owned by you with no group or other permission
bits, nothing in it is read or deleted, and the pass prints
`warning: ~/.amplihack/agents/ended: REASON; no markers were read`. In a
private `ended/`, it skips names that start with `.`, and accepts a file only
when its name passes the conversation id rule, it is a regular file owned by
you with no group or other permission bits, it is at most 4 KiB, its
`session_id` equals the file name, and its `reason` is `prompt_input_exit` or
`logout`. Symlinks are never followed.

| Marker | Effect |
|--------|--------|
| An `active` agent has this conversation id, the marker's `at` is later than the agent's `launched_at`, and the recorded process is not alive | Agent `ended`, `ended_by: session-end`, `ended_at` now. Marker deleted after the write. |
| As above, but the recorded process is still alive | Marker kept. It came from another `claude --resume` of the same conversation. |
| A matching agent, but the marker is not later than `launched_at`, or the agent is not `active` | Marker deleted. |
| No matching agent | Kept for 7 days, then deleted. |
| Invalid file | Deleted once it is more than 7 days old. |

An `active` agent is matched before any other agent with the same id.
Deletion removes only entries directly inside `ended/`. Ending an agent,
through a marker or through `claude --supervise`, writes no line in the
[supervisor log](#supervisor-log).

## Files

All files are under `~/.amplihack/agents/`.

| Path | Mode | Content and rules |
|------|------|-------------------|
| `~/.amplihack/` | `0700` when created | If it exists, it must be a directory owned by you, not a symlink, that other users cannot write to. Group write is accepted only when the group is your own private group. Its mode is never changed. |
| `agents/` | `0700` | Created if missing, except by `status`; if present, it must be a directory owned by you with no group or other permission bits, not a symlink. |
| `registry.json` | `0600` | The [registry](#registry). Read without following symlinks; must be a regular file owned by you with no group or other bits; at most 8 MiB. Written only by atomic replace. |
| `registry.lock` | `0600` | Held around each registry read-modify-write, waiting at most 10 seconds. |
| `supervisor.lock` | `0600` | Held by the running supervisor; holds its pid. |
| `supervisor.log` | `0600` | The [supervisor log](#supervisor-log). |
| `ended/` | `0700` | [SessionEnd markers](#sessionend-markers). |

Lock files and the log are created with mode `0600` when missing. They are
opened without following symlinks, and must be regular files owned by you,
with one hard link and no group or other permission bits. Nothing is
repaired for you. Lock files are never deleted, and `status` creates no
files.

## Registry

### Registry format

```json
{
  "version": 1,
  "agents": {
    "coordinator": {
      "state": "active",
      "tmux_session": "coordinator",
      "tmux_window": "main",
      "tmux_socket": "/tmp/tmux-1000/default",
      "launch_dir": "/home/alex/src/mistt",
      "conversation_id": "0f8e5c1a-3b2d-4c6e-9a7f-1d2e3f4a5b6c",
      "claude_pid": 41237,
      "proc_start": 88123456,
      "model": "opus",
      "effort": "high",
      "add_dirs": [
        "/home/alex/src/mistt-docs"
      ],
      "skip_permissions": true,
      "amplihack_flags": {
        "subprocess_safe": false,
        "no_reflection": true,
        "profile": "work"
      },
      "env": {
        "CLAUDE_CODE_EFFORT_LEVEL": "high"
      },
      "registered_at": "2026-10-29T21:12:04Z",
      "launched_at": "2026-10-29T23:27:44Z",
      "launch_id": "6f1c0d8e2a4b4f0e9c7d3b5a1e2f4c6d",
      "restarts": [
        "2026-10-29T23:27:41Z"
      ],
      "last_recovery": {
        "logged_at": "2026-10-29T23:27:41Z",
        "conversation_id": "0f8e5c1a-3b2d-4c6e-9a7f-1d2e3f4a5b6c",
        "lost": "2026-10-29T23:27:40Z",
        "resumed": "2026-10-29T23:27:41Z",
        "result": "resumed"
      }
    }
  }
}
```

Times are UTC strings of the form `2026-10-29T23:27:41Z`. Agents are written
in name order.

| Field | Type | Meaning |
|-------|------|---------|
| `version` | integer | Always `1`. |
| `agents` | object | Records keyed by agent name. |
| `state` | string | `active`, `ended` or `stopped`. |
| `tmux_session`, `tmux_window` | string | The agent's tmux session and window. |
| `tmux_socket` | string, optional | The tmux server's socket. Without it, tmux's default socket for the process that reads the record. |
| `launch_dir` | string | The directory the session runs in. |
| `conversation_id` | string, optional | The conversation to resume. |
| `claude_pid`, `proc_start` | integer, optional | The `claude` process last recorded, and its start time in clock ticks after boot. Written and cleared together; one without the other is ignored. |
| `model`, `effort` | string, optional | Recorded Claude Code options. |
| `add_dirs` | array of strings | Absolute `--add-dir` directories. |
| `permission_mode` | string, optional | Never present when `skip_permissions` is `true`. |
| `skip_permissions` | boolean | `--dangerously-skip-permissions` recorded. |
| `amplihack_flags` | object | `subprocess_safe` and `no_reflection` (booleans) and `profile` (string, optional). |
| `env` | object | [Recorded variables](#recorded-environment), name to value. |
| `registered_at` | time | Set by `agent register`, or when `claude --supervise` creates the record. |
| `launched_at` | time | The latest launch. SessionEnd markers older than this are ignored. |
| `launch_id` | string | 32 lowercase hexadecimal characters, new for each launch. |
| `lost_at` | time, optional | When the supervisor first found the agent lost. |
| `ended_at`, `ended_by` | time and `wrapper` or `session-end`, optional | Present only when `state` is `ended`. |
| `restarts` | array of times | Resumes in the last 24 hours, oldest first. |
| `last_recovery` | object, optional | The fields of the agent's latest recovery line: `logged_at`, `conversation_id`, `lost`, `resumed` and `result`. |
| `alert` | string, optional | Present only when `state` is `stopped`. |

Optional fields are left out when unset, and `null` reads the same as
absent. Fields this version does not know are kept through every write, at
the top level and in each record. `amplihack_flags`, `env` and
`last_recovery` are replaced whole, so unknown keys inside them are not kept.

### States

| State | Meaning | Looked at by a pass |
|-------|---------|---------------------|
| `active` | Supervised. An `active` agent with `lost_at` set has been found lost and not yet resumed. | yes |
| `ended` | Finished cleanly. | no |
| `stopped` | Stopped by the restart limit. | no |

| From | Event | To | Written by |
|------|-------|----|------------|
| none or any | `agent register` | `active`, history cleared | `register` |
| none or any | `claude --supervise` starts | `active`, history kept | the wrapper |
| `active` | clean exit under `--supervise` | `ended` (`wrapper`) | the wrapper |
| `active` | a SessionEnd marker applies | `ended` (`session-end`) | the supervisor |
| `active` | restart limit reached | `stopped` | the supervisor |
| `active` | resumed | `active`, one more restart | the supervisor |
| any | `agent unregister` | removed | `unregister` |

The supervisor never makes an agent `active`.

### Loading and writing

Every load checks, in order:

1. the file's type, owner, mode and size, as in [Files](#files);
2. `version`: above 1 fails as written by a newer amplihack; missing or 0
   fails as not valid;
3. the registry format, including the `state` and `ended_by` values;
4. every field against the [value rules](#value-rules) (without the
   existence checks), every agent name, every time, and every `env` name
   against the allowlist.

A registry that fails any check is never written. Every command that needs
it fails with the [error](#errors) for the cause until you fix or remove the
file.

Every write loads the file again, applies the change, drops `restarts`
entries older than 24 hours or more than 300 seconds in the future, checks
every record again, and replaces the file atomically. A write that would
leave an invalid record, or a file larger than 8 MiB, is refused and the file
is left as it was.

## Supervisor log

`~/.amplihack/agents/supervisor.log`, also printed on the supervisor's
stdout, so the journal of the [service](#service-unit) holds the same lines.

```text
TIME agent=NAME conversation=ID|- lost=TIME|- resumed=TIME|- result=RESULT
TIME ALERT: agent NAME stopped: restart limit (N restarts in the last hour)
TIME WARNING: agent NAME conversation not recorded: REASON
TIME WARNING: agent NAME tmux server started without its own scope: REASON
```

A line is a recovery line when its second field starts with `agent=`, an
ALERT line when it is `ALERT:`, and a WARNING line when it is `WARNING:`.
Each form keeps its fields in the order shown; new forms may be added with a
new second field.

- TIME is UTC, such as `2026-10-29T23:27:41Z`.
- `lost` is the agent's `lost_at` when set. `resumed` is set only when RESULT
  is `resumed`.
- RESULT is always last and runs to the end of the line. Every other field
  contains no spaces. It is `resumed`, a skip from
  [Classification](#classification) (or `skipped: already running` for a
  conversation already resumed in this pass), a failed
  [pre-check](#pre-checks), `failed: tmux: LINE` when tmux refused to
  [create the session](#resume), or `stopped: restart limit`, which the ALERT
  line follows.
- The `conversation not recorded` WARNING line is written only to the log,
  by [`claude --supervise`](#what-it-records) when it cannot record the
  conversation of the session it started.
- The `tmux server started without its own scope` WARNING line is written by
  the supervisor when it had to start a tmux server without `systemd-run`;
  see [Server placement](#server-placement). It is also printed on stdout.
- REASON in a WARNING line is cut to 200 characters, and its control and
  format characters, including the escape that starts an ANSI sequence, are
  replaced by `?`, so it cannot end the line or forge another one. Neither WARNING line is stored in `last_recovery`.

A healthy agent writes no line. A line with the same agent, conversation and
RESULT as the agent's previous line is not written again, except `resumed`,
which is always written. A healthy agent's stored `skipped:` or `failed:`
result is cleared, so the same result is written again if it comes back.
Each written recovery line is also stored in the agent's `last_recovery`.

The log is opened without following symlinks and appended to. If the append
fails, the supervisor prints
`warning: PATH: REASON; the line was printed but not logged`, and the line
has still gone to stdout. The log is never rotated or truncated by amplihack.

## Errors

Each error is one line, printed as `error: MESSAGE`, with exit 1.

| Situation | Commands | Message |
|-----------|----------|---------|
| `HOME` unset | all | `cannot find the home directory: HOME is not set` |
| `~/.amplihack` unsafe | all, including `status` | `PATH: is a symlink`, `PATH: is not a directory`, `PATH: owned by another user` or `PATH: writable by other users` |
| `agents/` unsafe | all, including `status` | `PATH: is not a real directory` (a symlink or not a directory), or `PATH: is not private (owner-only mode 0700)` (another owner, or any group or other permission bit) |
| Lock file unsafe | `registry.lock`: `register`, `unregister`, `supervise`, `claude --supervise`; `supervisor.lock`: `supervise` | `PATH: is a symlink`, `PATH: is not a regular file`, `PATH: is owned by another user`, `PATH: has more than one hard link` or `PATH: is not private (owner-only mode 0600)` |
| Registry file unsafe | all | `PATH: must be a regular file owned by you with mode 0600; it was not changed` |
| Registry larger than 8 MiB | all | `PATH: larger than 8 MiB; it was not changed` |
| Registry not valid JSON | all | `PATH: not valid JSON (line L, column C); it was not changed` |
| Registry `version` missing or not a known number | all | `PATH: field version is not valid; it was not changed` |
| Registry JSON not in the registry format | all | `PATH: does not match the registry format (line L, column C); it was not changed` |
| Registry agent name invalid on load | all | `PATH: an agent name is not valid; it was not changed` |
| Registry field invalid on load | all | `PATH: agent NAME: field FIELD is not valid; it was not changed` |
| Registry from a newer amplihack | all | `PATH: written by a newer amplihack (registry version N); upgrade amplihack. It was not changed` |
| Registry lock busy | `register`, `unregister`, `supervise`, `claude --supervise` | `PATH: the registry lock was held for 10 s by another amplihack process; try again` |
| Write would leave an invalid field | `register`, `unregister`, `supervise`, `claude --supervise` | `PATH: refusing to write agent NAME: field FIELD is not valid; it was not changed`, or `PATH: refusing to write an agent with an invalid name; it was not changed` |
| Write would exceed 8 MiB | as above | `PATH: the registry would exceed 8 MiB; it was not changed` |
| Invalid NAME, flag or directory | `register`, `unregister`, `claude --supervise` | `SUBJECT: RULE`, for example `--conversation: must be 1 to 64 letters, digits or '-', starting with a letter or digit` |
| Invalid tmux name from tmux | `register`, `claude --supervise` | `tmux session name RULE` or `tmux window name RULE` |
| Unexpected answer from tmux | `register`, `claude --supervise` | `tmux display-message returned an unexpected answer` |
| Invalid `--profile` | `register`, `claude --supervise` | `--profile: names match [A-Za-z0-9][A-Za-z0-9._-]{0,63}, without '..' or a trailing '.'`. This is the text `amplihack profile` gives; a reserved name such as `CON` gets the same text. |
| Profile directory missing | `register`, `claude --supervise` | `--profile: the profile's directory must exist under the profile root and must not be a symlink` |
| `--env` name not allowed | `register` | `--env: only these variables can be recorded: LIST` |
| `--env` malformed | `register` | `--env: expected KEY=VALUE`, `--env: value must be at most 4096 bytes, with no control characters`, or for `AMPLIHACK_AGENT_BINARY` `--env: value must be one of claude, copilot, codex or amplifier` |
| `--pid` not usable | `register` | `process PID is not a claude process of this user` |
| Target in print mode | `register` | `process PID is a claude -p run, not an interactive session` |
| No process and missing flags | `register` | `no claude process found; pass --session, --window, --dir and --conversation` |
| Fields not found | `register` | `claude process PID found, but ITEMS could not be discovered; pass FLAGS` |
| No `claude` process in the pane | `register --pane` | `no claude process runs in that tmux pane` |
| `/proc` cannot be read | `register` without `--pid` | `cannot read the process table`, or `cannot list the process table: REASON` |
| `--pid` or `--pane` off Linux | `register` | `--pid and --pane need Linux /proc; pass --session, --window, --dir and --conversation` |
| tmux could not name the pane | `register`, `claude --supervise` | `tmux display-message failed: LINE` |
| Socket directory unsafe | `register`, `claude --supervise` | `tmux socket: its directory must be owned by you and closed to other users` |
| Config directory unsafe | `register` | `DIRECTORY is not private: it must be owned by you and not writable by other users`, where DIRECTORY is `~/.claude`, `the profile root`, `the directory of profile NAME`, `~/.claude/sessions` or `the sessions directory of profile NAME`. A `sessions/` directory is checked only when a running process is read. |
| Config directory unreadable | `register` | `~/.claude is not a directory`, `~/.claude cannot be read`, or `a Claude sessions directory cannot be read` |
| Profile root unusable | `register`; `claude --supervise` with `--profile` | `cannot list profile directories: ` followed by `AMPLIHACK_CLAUDE_PROFILE_ROOT is not a valid absolute path`, `HOME is not set`, `FILE: REASON`, `the profile root cannot be read` or `the profile root is not a directory`. A profile root that does not exist is not an error. |
| tmux session in use | `register`, `claude --supervise` | `tmux session S is already used by registered agent OTHER` |
| Conversation in use | `register`, `claude --supervise` | `this conversation is already used by registered agent OTHER` |
| Unknown NAME | `unregister` | `no registered agent named NAME` |
| Supervisor already running | `supervise` | `another amplihack agent supervisor is running (pid N)`, or `(pid unknown)` |
| Not Linux | `supervise` | `amplihack agent supervise needs Linux /proc; nothing was started` |
| Executable path unknown | `supervise` | `cannot find the amplihack binary: REASON; nothing was started` |
| Unusable executable path: not absolute, not UTF-8, or containing `=` or a control character | `supervise` | `the amplihack binary path must be absolute and UTF-8, without '=' or control characters, for env to run it; nothing was started` |
| Not Linux, no tmux, print mode | `claude --supervise` | see [Preconditions](#preconditions) |

For `supervise`, a problem with the config directories or the profile root is
not an error: it makes liveness unknown, with the same text as the reason.

## Warnings

Warnings go to stderr and do not change the exit status.

| Situation | Text |
|-----------|------|
| A value on the `claude` command line fails the value rules | `warning: ignored FLAG from the claude command line (N bytes): RULE` |
| An allowlisted variable fails the value rules | `warning: ignored NAME from the environment of process PID: RULE` |
| The process's environment cannot be read | `warning: could not read the environment of process PID; no variables were recorded` |
| `register`: the process's directory fails the value rules | `warning: ignored the directory of process PID (N bytes): RULE` |
| `register`: the `amplihack claude` ancestor's profile has no directory, or the profile root cannot be worked out | `warning: ignored the profile of process PID (N bytes): REASON` |
| `supervise`: `ended/` is not a private directory | `warning: ~/.amplihack/agents/ended: REASON; no markers were read` |
| `supervise`: an applied or stale marker cannot be deleted | `warning: PATH: REASON; the marker was not removed` |
| A log line cannot be appended | `warning: PATH: REASON; the line was printed but not logged`. REASON is the I/O error, or one of the lock file texts in [Errors](#errors) when the log is a symlink, not a regular file, owned by another user, hard-linked or not private. |
| `claude --supervise` cannot record a clean exit | `warning: could not mark agent NAME as ended: REASON; the supervisor may resume it` |

## Exit codes

| Code | Meaning |
|------|---------|
| 0 | Success. Also a completed `supervise --once` pass, even with `failed` results; a supervise loop ended by SIGTERM or SIGINT; and `status` with no agents. |
| 1 | Any [error](#errors): a refusal, a validation failure, a held lock, a registry that cannot be loaded, or an unsupported platform. |
| 2 | Usage error from the argument parser, including a flag out of range or a flag combination that is not allowed. |
| other | `amplihack claude --supervise` only: Claude Code's own exit status, passed through by the launch. |

## Service unit

amplihack does not install a unit. This one runs the supervisor as a systemd
user service:

```ini
[Unit]
Description=amplihack agent supervisor

[Service]
ExecStart=%h/.local/bin/amplihack agent supervise
Environment=PATH=%h/.local/bin:/usr/local/bin:/usr/bin:/bin
Restart=always
RestartSec=10
KillMode=process

[Install]
WantedBy=default.target
```

Save it as `~/.config/systemd/user/amplihack-agent-supervisor.service`, then:

```bash
systemctl --user daemon-reload
systemctl --user enable --now amplihack-agent-supervisor.service
loginctl enable-linger "$USER"
```

| Setting | Why |
|---------|-----|
| `ExecStart` | The absolute path of `amplihack`. Adjust it to the install path, and add [supervise flags](#supervise-flags) here. |
| `Environment=PATH=` | Absolute directories only, with no empty entries, covering `tmux`, `git`, `systemd-run`, `claude` and `amplihack`. Every directory must be writable only by you or by root, because these programs are found through it. A tmux server the supervisor starts takes this environment as its global environment, so resumed sessions see the same `PATH`. |
| `Restart=always`, `RestartSec=10` | systemd starts the supervisor again if it exits for any reason. |
| `KillMode=process` | The supervisor starts tmux servers in scopes of their own (see [Server placement](#server-placement)), so this matters only for a server started without one. Such a server, and every resumed agent in it, lives in the service's control group. With systemd's default, `control-group`, stopping or restarting the service would end all of them. With `process`, only the supervisor stops, and each later start logs `Found left-over process ... in control group while starting unit` for every process still running there. That is expected. |
| `WantedBy=default.target` | `enable` starts the service with your user manager. |

Leave out:

- **Sandboxing** (`PrivateTmp=`, `ProtectHome=`, `ProtectSystem=`,
  `NoNewPrivileges=`, `PrivateDevices=`, `ReadOnlyPaths=` and similar) and
  **resource limits** (`MemoryMax=`, `TasksMax=`, `CPUQuota=`). Resumed tmux
  servers and agents are created inside the service's control group and
  namespaces and inherit both. With `PrivateTmp=yes`, for example, a resumed
  agent's tmux server sits in a private `/tmp` that your own `tmux attach`
  cannot reach.
- **Credentials.** Claude Code reads its login from `~/.claude`, so the unit
  needs no tokens. Do not add secrets with `Environment=`: the unit file and
  `systemctl --user show` expose them, and anything in the unit's
  environment reaches every resumed session.

`systemd-run --user` needs `XDG_RUNTIME_DIR` and `DBUS_SESSION_BUS_ADDRESS`,
which the user manager sets for its services on most distributions. Without
them, servers are started without a scope and WARNING lines are written.

To see the scopes and the agents in them:

```bash
systemctl --user status 'amplihack-tmux-server-*'
```

An alternative to scopes is a tmux server run as its own user service, such
as a `tmux-server.service` that runs `tmux new-session -d`. While it runs,
the supervisor finds a server present on the default socket and does not
call `systemd-run` for agents on that socket. Two limits apply:

- Resumed agents take the tmux server's environment, not this unit's
  `PATH`. Set the same `Environment=PATH=` in `tmux-server.service`, with
  the same writable-directory rule.
- It covers only the default socket. Agents recorded with `-S SOCKET` on
  another socket are still placed in scopes.

With `loginctl enable-linger`, your user manager starts at boot, so registered
agents are resumed after a reboot without anyone logging in. Without it, the
supervisor runs only while you are logged in, and when systemd stops your
user manager at logout it ends the resumed agents as well.

The supervisor's lines go to your user journal
(`journalctl --user -u amplihack-agent-supervisor.service`). On most
distributions, members of the `adm` or `systemd-journal` group can read it.

The login rotator needs the same treatment: issue #1759 covers running it as
`amplihack-rotator.service`, and an installer shared by both units belongs
with that work. Such an installer would write this unit unchanged. The two
units are independent: either runs without the other.

## Platform support

| Platform | Behaviour |
|----------|-----------|
| Linux | Everything on this page. |
| Other Unix, such as macOS | `register` works only with `--session`, `--window`, `--dir` and `--conversation`; `--pid` and `--pane` are refused. `unregister` and `status` work. `supervise` and `claude --supervise` exit 1 without starting anything. |
| Other systems | Every `agent` command and `claude --supervise` exit 1. |

## Related

- [Bring Agent Sessions Back After a Terminal Loss](../howto/supervise-agent-sessions.md) -
  Register sessions, run the service, and handle clean exits and crash loops
- [`amplihack profile` reference](profile-command.md) - Profiles used by
  `--profile`, their name rules and the profile root
- [`amplihack sessions` reference](sessions-command.md#live-sessions) - The
  daily archive, which keeps registered agents' transcripts, and `restore`
- [CLI reference](cli.md) - All `amplihack` subcommands
