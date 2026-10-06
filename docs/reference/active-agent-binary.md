# Active Agent Binary — Resolver Reference

## Overview

The **active agent binary** is the AI tool (`claude`, `copilot`, `codex`, or `amplifier`) that the current process should treat as its runtime. It is resolved by a single shared function, used by every read site across `amplihack-cli`, `amplihack-utils`, `amplihack-workflows`, and `amplihack-hooks`. One shell helper approximates it (see below).

**Canonical entry point (Rust):**

```rust
use amplihack_utils::agent_binary;

let binary: String = agent_binary::resolve(&cwd);
```

**Canonical entry point (CLI wrapper):**

```rust
use amplihack_cli::env_builder::agent_binary_resolver;

let binary: String = agent_binary_resolver::resolve(&cwd);
```

**Shell:** `amplifier-bundle/skills/migrate/scripts/migrate.sh` (`detect_cli`)
keeps the layers that are cheap to state in shell: `AMPLIHACK_AGENT_BINARY`
(and its default-guess tag), then the session markers (the same list, in the
same order, as `agent_binary::SESSION_MARKERS`). Its one extra layer, the
parent process chain, comes next, because it is evidence of the running
session too. Everything below that, the launcher context and the default, it
asks `amplihack agent-binary`, the Rust resolver itself, so there is one copy
of the walk-up, the 24-hour bound, the RFC 3339 check and the trust checks. The
resolver's notice, which names every unusable context file, reaches the
user's terminal unchanged. Without `amplihack` on `PATH`, or with a build too
old to have the subcommand, `detect_cli` warns that no launcher context was
read and answers `copilot`. `tests/issue_1525_migrate_detect_cli_parity.sh`
checks the shell side, including that the two marker lists match, and
`bins/amplihack/tests/issue_1525_migrate_detect_cli_uses_the_resolver.rs` runs
`detect_cli` against the real binary. There is no Python implementation in
this repository. The Rust resolver is authoritative wherever another
implementation differs.

## Resolution Precedence

The resolver evaluates sources in order and returns the first valid value. A value is "valid" only if it survives normalization (trim, lowercase) and matches the allowlist.

| # | Source | Notes |
| - | --- | --- |
| 1 | `AMPLIHACK_AGENT_BINARY` env var | Explicit override. Used by CI, tests, and external consumers that have not migrated yet. Ignored while tagged `AMPLIHACK_AGENT_BINARY_SOURCE=default:<same binary>` (see below). It outranks a session marker naming a different CLI; `recipe run` and `agent-binary` then say so on stderr (see below). |
| 2 | Live session marker | An environment variable the hosting CLI exports, such as `CLAUDECODE`, `CLAUDE_CODE_SESSION_ID`, `CLAUDE_CODE_ENTRYPOINT` or `COPILOT_CLI`. The full list is `agent_binary::SESSION_MARKERS`. Inside tmux it may come from the server's copy of whatever started the server; see [Handing the binary to a detached launch](#handing-the-binary-to-a-detached-launch). |
| 3 | `<repo>/.claude/runtime/launcher_context.json` `launcher` field | Persisted state, possibly written by a different session. Consulted only while fresh, and never in or above a world-writable or foreign-owned directory. A file in such a directory is not read, but it is named when the answer was inferred (see below). |
| 4 | Built-in default | `"copilot"` |

If a source produces a value that fails validation (allowlist, length, character class), the resolver ignores it and falls through to the next source. **No source ever coerces an invalid value into a different name.** What is said about it depends on the source:

- A `launcher_context.json` whose `launcher` field fails is logged at WARN (`ignoring an unusable launcher_context.json`, with its path and the reason).
- An `AMPLIHACK_AGENT_BINARY` that fails gets no log line of its own. If the launcher context or the default then answers, the resolver's generic fallback WARN (`no usable AMPLIHACK_AGENT_BINARY (unset, rejected, or a parent's default guess) …`) covers it.

Both WARNs are hidden at the default tracing filter, so `amplihack agent-binary` and `amplihack recipe run` say it on stderr:

- A rejected `AMPLIHACK_AGENT_BINARY` is named whichever layer answered in its place, a session marker included: `AMPLIHACK_AGENT_BINARY is set but is not one of amplifier, claude, codex or copilot`, then what answered, e.g. `; COPILOT_CLI, the copilot session marker in this environment, answered`. The user named a CLI and got a different one, so the line is needed even when a marker is the answer.
- A launcher context that failed is named when the answer was inferred: `ignored <file>: it does not name amplifier, claude, codex or copilot as its launcher. Fix or delete it.` When a session marker or a valid `AMPLIHACK_AGENT_BINARY` answers, the file decided nothing and no notice names it; only its WARN is still logged.

### Resolving once for a whole recipe run

`amplihack recipe run` resolves the binary once, at entry, and exports it to
`recipe-runner-rs` as `AMPLIHACK_AGENT_BINARY`. It has to: every step runs
under the runner's curated environment, where the session markers of the CLI
that started the run may be gone, and a nested `amplihack` resolving on its own
there would fall through to the default (issue #1481).

The launcher-context walk-up for that decision starts at the run's working
directory (`--working-dir`, default `.`), where the steps run. The top level
then reads the same context file a nested `amplihack` in a step would.

That one answer is also what the root-sandbox pre-flight checks (issue
#1482): it refuses an agent recipe as root outside a sandbox only when the
steps would actually run `claude`, because it looks at the same binary the
runner is handed rather than resolving again from the caller's directory.

When the answer was inferred rather than observed (layer 3 or 4), recipe run
prints a notice on stderr saying why. From layer 3 it names the file it read,
by its full path. The walk-up visits ancestors, so that file need not be in
the working directory. The notice adds a line for every
`launcher_context.json` the walk-up passed over because it could not be used
(issue #1525):

```text
amplihack: agent steps will run under 'copilot' (no AMPLIHACK_AGENT_BINARY or agent session marker was found). Set AMPLIHACK_AGENT_BINARY to choose a different agent CLI.
amplihack: ignored /home/u/repo/.claude/runtime/launcher_context.json: it is empty. Fix or delete it.
```

The reasons are: empty, not valid JSON (with line and column), JSON without a
string `launcher` field, no `timestamp`, a `timestamp` that is not RFC 3339, a
launcher outside the allowlist, larger than 64 KiB, unreadable, a symlink out
of its directory, or a file in the world-writable or foreign-owned directory
where the walk-up stopped. That last one is not read at all, so even a fresh,
valid file there is listed, with the directory and why it is not trusted:

```text
amplihack: ignored /home/u/repo/.claude/runtime/launcher_context.json: it is under /home/u/repo, which any user can write to, so it was not read. Fix or delete it.
```

The usual causes are a checkout made world-writable (`chmod o-w` the directory
to trust it again) or a container running as root over a checkout the host
user owns (`which another user owns`). A reason never quotes the file. A stale file (older than 24h)
is not listed, because sessions end and an old file is expected. A file whose
age cannot be known is different: however recent it is, it will never be used,
so it is listed. The walk-up still continues past an unusable file, so a
parent directory's file can still answer, but the notice now says so. Rust
callers get the same evidence from `agent_binary::resolve_detailed`
(`Resolution::context_file` and `Resolution::unusable_contexts`).

A rejected `AMPLIHACK_AGENT_BINARY` gets a notice whatever answered in its
place. When a session marker (layer 2) answers, that answer counts as
observed, but the user still asked for a CLI and got another one, so the line
names the marker:

```text
amplihack: agent steps will run under 'copilot' (AMPLIHACK_AGENT_BINARY is set but is not one of amplifier, claude, codex or copilot; COPILOT_CLI, the copilot session marker in this environment, answered). Set AMPLIHACK_AGENT_BINARY to one of amplifier, claude, codex or copilot to choose an agent CLI.
```

The steps are handed the marker's answer as a valid value, so a nested run
does not repeat the line.

An answer from layer 1 is a choice, not an inference, and it still wins over a
session marker. But when the marker names a different CLI, recipe run names
the marker it overruled. These docs tell you to export the variable to choose a
CLI, so a profile line written for one CLI and still exported inside another's
session looks exactly like this. Without the line, every step would run under
the wrong CLI with nothing in the output saying why (issue #1335):

```text
amplihack: agent steps will run under 'copilot' (AMPLIHACK_AGENT_BINARY is set and overrides CLAUDECODE, the claude session marker in this environment). If this is a claude session, unset AMPLIHACK_AGENT_BINARY to run under claude.
```

The line names a variable, not a session, because a variable is all the
process can see. Inside tmux it may not even be the caller's: tmux copies the
environment of whatever process started its server into the server's global
environment, and every later `new-session` starts from that copy (tmux(1),
GLOBAL AND SESSION ENVIRONMENT). On a host where agents of both CLIs start
tmux sessions, a server started from a Copilot session holds `COPILOT_CLI=1`
for every run launched into it, from Claude Code included. So when `TMUX` is
set, the notice asks the server (`tmux show-environment -g <variable>`, with
a two-second limit) about the marker. If the server's global environment holds
the same value, the marker may be the server's starter's.

Claude Code sets `CLAUDECODE=1` in every session, so for its markers that
comparison cannot tell the server's starter from any other Claude Code
session. When the process holds `CLAUDE_CODE_SESSION_ID`, the notice asks
about that instead. If the server holds the same ID, the environment is the
server's copy. If it holds a different ID, or none, the marker belongs to a
Claude Code session running in a pane, and it counts as an observation.
Without a session ID, and for Copilot's markers, which have no such
companion, the marker's own value is compared. An explicit value over a
marker the server holds is not told to step aside for it:

```text
amplihack: agent steps will run under 'claude' (AMPLIHACK_AGENT_BINARY is set). COPILOT_CLI, a copilot session marker, is set too, but this tmux server's global environment holds the same value, so it may come from whatever started the server rather than from a copilot session.
```

A marker that *answered* (layer 2) is normally an observation and announced
by nothing. One the tmux server holds is announced, because without the
hand-off below it is how a run launched from Claude Code runs every step under
copilot:

```text
amplihack: agent steps will run under 'copilot' (COPILOT_CLI is set, but this tmux server's global environment holds the same value, so it may come from whatever started the server rather than from a copilot session). To hand a detached run the CLI you launch it from, prefix its command with $(amplihack agent-binary --shell -w <dir>), inside double quotes; to choose one, set AMPLIHACK_AGENT_BINARY.
```

`amplihack agent-binary` prints the same line without the hand-off advice. It
*is* the hand-off, and in the same environment it would carry this same answer
across:

```text
amplihack: resolved the agent binary to 'copilot' (COPILOT_CLI is set, but this tmux server's global environment holds the same value, so it may come from whatever started the server rather than from a copilot session). Set AMPLIHACK_AGENT_BINARY to choose an agent CLI.
```

A marker set by a CLI running inside the pane stays an observation and prints
nothing. The server does not hold that marker with that value, or, for Claude
Code, does not hold that session ID. This covers a Claude Code session in a
pane of a server that another Claude Code session started.

An explicit value that matches the marker, or that is set where no marker is
visible, prints nothing. The notice does not quote the raw value; it shows the
normalized name. On the way down, recipe run and recipe-runner-rs remove only
`CLAUDECODE` (the runner drops other unprotected variables only under
environment-size pressure), so a nested `recipe run` under a deliberate
override usually still sees another Claude marker and repeats the line.
Agent-step stderr is shown only when a step fails. Rust callers get the
marker, variable and binary, from `Resolution::session_marker`.

When the answer came from layer 4, recipe run also exports
`AMPLIHACK_AGENT_BINARY_SOURCE=default:<binary>`. The tag keeps a guess a guess
on the way down:

- the resolver ignores `AMPLIHACK_AGENT_BINARY` while the tag still names its
  value, so a session marker visible at a lower level still wins. A step that
  sets a *different* binary has made a choice, and the stale tag does not veto
  it;
- a launcher (`amplihack copilot`, ...) started with a tagged value naming
  itself does not write `launcher_context.json`, and hands the value on to its
  own children still tagged (`EnvBuilder::with_launched_agent_binary`, used by
  both the interactive launcher and `--auto`). The guess itself therefore never
  becomes persisted state that pins later runs in the checkout, however deep
  the nesting. What can be persisted further down is an *observation*: if the
  CLI that the guess launched exports its own session marker (Copilot CLI sets
  `COPILOT_CLI`), a nested `recipe run` inside it resolves from that marker,
  exports the value untagged, and a launcher below that records it, because
  that session really did run.

Setting the *same* value again does not lift the tag, because the two cannot
be told apart. To make that value an instruction, unset
`AMPLIHACK_AGENT_BINARY_SOURCE` as well. The stderr notice says so when it
sees an inherited guess.

Any code that sets `AMPLIHACK_AGENT_BINARY` explicitly through
`EnvBuilder::with_agent_binary` clears the tag.

### Handing the binary to a detached launch

Resolving once at the top only helps if the top can see the session. A
detached launch cannot. `tmux new-session` gives the new command the tmux
server's global environment, not the caller's, and tmux copied that from
whatever process started the server (tmux(1), GLOBAL AND SESSION ENVIRONMENT).
On the far side the caller's markers are missing, and the starter's are
present:

- A server started from a plain shell has no marker, so the run takes the
  `copilot` default, announced as a guess (#1335).
- A server started from a Copilot session holds `COPILOT_CLI=1`. A run
  launched into it from Claude Code resolves to copilot from that marker. To
  the far side that is an observation, not a guess; only the tmux check above
  now announces it.
- A server started from the caller's own CLI happens to give the right
  answer. That is why this can work for weeks and then stop when another
  agent restarts the server.

`amplihack agent-binary` resolves in the caller's shell and prints the answer
(issue #1525):

```console
$ amplihack agent-binary
claude (session_marker)
$ amplihack agent-binary --shell
env -u CLAUDECODE -u CLAUDE_CODE -u CLAUDE_CODE_SESSION_ID -u CLAUDE_PROJECT_DIR -u CLAUDE_CODE_ENTRYPOINT -u COPILOT_CLI -u GITHUB_COPILOT -u GITHUB_COPILOT_AGENT -u COPILOT_AGENT AMPLIHACK_AGENT_BINARY=claude AMPLIHACK_AGENT_BINARY_SOURCE=
```

Use the `--shell` form inline, inside the double-quoted command, so that it
expands before tmux runs anything:

```bash
tmux new-session -d -s recipe-runner \
  "cd /path/to/repo && $(amplihack agent-binary --shell -w /path/to/repo) amplihack recipe run ..."
```

The double quotes are what make it work (POSIX shell, §2.2.2–2.2.3). In
single quotes nothing expands in your shell: tmux runs the text as given, and
`$(amplihack agent-binary --shell)` runs in the new session. Its markers there
are the server's, so it resolves the server starter's CLI and hands that on,
with every marker removed. Its own stderr notice lands in the tmux pane.

That case is not silent in the run's log. When the answer `--shell` resolves
rests on a marker the tmux server holds, it hands it on tagged
`AMPLIHACK_AGENT_BINARY_SOURCE=tmux_server:<marker variable>`. The run that
receives it still runs under that CLI, which is what it would resolve with no
hand-off at all, and prints:

```text
amplihack: agent steps will run under 'copilot' (AMPLIHACK_AGENT_BINARY was handed on by amplihack agent-binary --shell, which read it from COPILOT_CLI while the tmux server's global environment held the same value, so it may come from whatever started that server rather than from a copilot session). If that hand-off was in a single-quoted tmux command, it ran in the new session instead of in your shell: put the command in double quotes. To choose an agent CLI, set AMPLIHACK_AGENT_BINARY.
```

- `env -u` removes every `agent_binary::SESSION_MARKERS` variable from the
  far side, so no marker of the server's starter can contradict the caller's
  answer, set off the override notice, or answer for a step further down. The
  far side then sees the caller's view: the answer, its source, and no
  marker. GNU, BSD (macOS) and BusyBox `env` all take `-u`. Further
  `NAME=value` words after the hand-off, such as the templates'
  `AMPLIHACK_HOME=...`, are read by `env` the same way.
- `-w` resolves from the directory the recipe will run in, so the
  launcher-context walk-up matches the run's own.
- The source travels with the value. A default guess is printed with
  `AMPLIHACK_AGENT_BINARY_SOURCE=default:<binary>`, so it stays a guess on the
  far side and nothing there persists it. Handing over
  `AMPLIHACK_AGENT_BINARY` alone would turn it into an instruction, and the
  nested `amplihack <cli>` would write it to `launcher_context.json`
  (#1481 again).
- Any other answer is printed with an empty tag, so a stale
  `default:<same binary>` already in the server's environment cannot veto it.
  The exception is an answer read from a marker the tmux server holds, which
  gets `tmux_server:<marker variable>` (above). That tag is not a guess either:
  the resolver honours the value beside it, `migrate.sh` treats it as a
  choice, and `recipe run` exports its steps the value untagged, as it does
  any explicit value. A tag naming a marker of a different CLI than the value
  describes nothing and is ignored.
- An answer read from a launcher context crosses with that empty tag, as an
  explicit value, so the far side's run log does not say it was inferred.
  Only `agent-binary` names the file, in its `read from <file>` line on your
  terminal. The tag cannot carry the path: `$(...)` splits its output on
  whitespace and does not remove quotes (POSIX Shell Command Language §2.6.5, §2.6.7),
  so only words without spaces, such as a binary or a marker variable, cross
  intact. This only happens without a session marker (a cron job, a plain
  shell); inside Claude Code or Copilot the marker outranks the file.
- An inferred answer is explained on stderr, which stays on your terminal while
  `$(...)` captures stdout. The explanation includes any unusable launcher
  context it skipped. For a default guess the far side's run says so again,
  from the tag; for a launcher-context answer this line is the only record.
  An exported `AMPLIHACK_AGENT_BINARY` that overrides a
  marker in the shell you run the hand-off from is handed over as your choice,
  and the same stderr line names the marker it overrode. That line is the only
  record: the hand-off removes the marker, so the far side sees an explicit
  choice and its run log says nothing about the override.
- Log lines go to stderr as well, so a `RUST_LOG` set in your shell does not
  reach the command line. Stdout is only the hand-off line, whatever the log
  filter.
- The inline form works on every tmux version. `tmux new-session -e` only
  exists from tmux 3.2, and it can set a variable but not remove one.
- The subcommand never self-installs, so it is safe inside `$(...)`.

If `amplihack agent-binary` itself fails, `$(...)` expands to nothing and the
far side resolves from the server's environment, as with no hand-off: the
default, or the starter's marker. Either way the run's own stderr says so: the
default is announced as a guess, and a marker the server holds is announced by
the tmux check. It is announced late, in the run's log, not on your terminal.

Where a detached session runs an agent CLI directly, not `amplihack`, there
is nothing to hand over. The migrate skill's remote `tmux new-session` runs
`<cli> --resume <id>` with the CLI already named.

The auto-drive-to-merge skill has nothing to hand over either, because it
starts no detached session. Its `SKILL.md` runs
`amplihack recipe run auto-drive-to-merge` in the foreground, where the run
sees the caller's own session markers. The nested runs that
`amplifier-bundle/tools/autodrive_loop.sh` starts, the round recipe and
`loop-health-evaluator`, also run in the foreground, inside a step of the
outer run, with no `tmux`, `nohup` or `setsid`. They inherit the
`AMPLIHACK_AGENT_BINARY` and `AMPLIHACK_AGENT_BINARY_SOURCE` that the outer
`recipe run` exported, so every nested step runs under the outer run's
answer. An agent that chooses to put the skill's command in tmux is acting
on the `USER_PREFERENCES.md` line on detached recipe runs, which carries the
hand-off. `tests/issue_1525_detached_launch_docs_carry_hand_off.sh` scans
both files, so a detached `recipe run` added to either without the hand-off,
or with it outside double quotes, fails CI.

### Why file-based, not env-based

Environment variables do not survive every subprocess boundary in the launcher's call graph:

- `tmux new-session -d` gives the command the tmux server's global environment, copied from whatever started the server, not the caller's (see [Handing the binary to a detached launch](#handing-the-binary-to-a-detached-launch)).
- `setsid` and `nohup` pass the caller's environment on unchanged (setsid(1), nohup(1)), so a process they start sees only what its caller had; one started from a cron job or a service manager has no session marker to inherit.
- Sub-recipes spawned by `amplihack recipe run` invoke fresh `amplihack` binaries that may be reading env from the user's shell rather than the parent recipe runner.
- Python hooks shell out to subcommands using `subprocess.run` which inherits the calling Python's env, not the Rust launcher's.

That is why the environment variable is not the only layer. It is also why a
recipe run resolves once, at the top, and hands the answer down explicitly
rather than letting each nested process re-derive it (see above).

The resolver reads `<repo>/.claude/runtime/launcher_context.json` (walking up
from the working directory, stopping at a `.git` boundary or an untrusted
directory). It has no `$AMPLIHACK_RUNTIME_ROOT` layer.

## Allowlist & Validation

The allowlist is **fixed** and identical in Rust and the shell helper:

```text
{ "claude", "copilot", "codex", "amplifier" }
```

Validation rules applied to every candidate value, from `AMPLIHACK_AGENT_BINARY` or a launcher context's `launcher` field, before it can win precedence. They are `agent_binary::validate_binary_name`, in this order:

- Reject the value if any byte of it, as given, is `/`, `\`, `.`, `;`, NUL or any other ASCII control character (tab and newline included)
- Trim surrounding whitespace, then reject an empty value, one over 32 bytes, or one with a space or tab still inside it
- Lowercase, then exact match against the allowlist
- No prefix matching, no substring matching, no shell expansion

A value that fails is treated as if its source were unset. It is never coerced into another name, and the resolver never writes it into a log line, under any filter. A rejected `AMPLIHACK_AGENT_BINARY` gets no log line of its own, and a rejected `launcher` field is named only by its file's path and a fixed reason. The stderr notice names the variable, never its value. What each source gets instead is under [Resolution Precedence](#resolution-precedence).

## Default Change: claude → copilot

Prior to this refactor, the implicit default was `"claude"`. The default is now **`"copilot"`** to match the project's preferred runtime. To preserve the old behavior for an isolated invocation, set the env var explicitly:

```sh
AMPLIHACK_AGENT_BINARY=claude amplihack recipe run smart-orchestrator -c task_description="..."
```

To force `"claude"` for a single command, set `AMPLIHACK_AGENT_BINARY=claude`.

**Existing `claude` users:** a fresh `.claude/runtime/launcher_context.json` with `"launcher": "claude"` from a prior `amplihack claude` session resolves to `claude` when no env override or session marker answers first.

## File Format: `launcher_context.json`

Path: `<repo>/.claude/runtime/launcher_context.json`
Permissions: `0o600` (owner read/write only)
Written: to a temporary file in the same directory, then renamed over the old
one, so a write cut short (a full disk, a killed process) leaves the previous
file, never an empty or partial one
Read cap: 64 KiB (oversized files are rejected with a warning)
Staleness window: 24 hours (older files fall through as if unset)
Timestamp: required, RFC 3339 as `chrono::DateTime::parse_from_rfc3339` reads it.
A file without one, or with one in another form (`2026-10-04 13:13:46` has no
offset), is unusable and named in the notice. `migrate.sh`'s `detect_cli`
gets the same rule by asking `amplihack agent-binary`.

```json
{
  "launcher": "copilot",
  "command": "amplihack copilot",
  "timestamp": "2026-10-04T13:13:46.123456789+00:00",
  "environment": {
    "AMPLIHACK_AGENT_BINARY": "copilot",
    "AMPLIHACK_LAUNCHER": "copilot"
  }
}
```

This is what `launcher_context::write_launcher_context` writes. The resolver
reads `launcher` and `timestamp`; the other fields are owned by
`LauncherContext`.

## Hook Registration

Hooks are native `amplihack-hooks <subcommand>` commands registered in settings.
They do not resolve per-binary script files.

### Hook Event Variants

`HookEvent` is an enum with eight variants. Each variant maps to a fixed on-disk filename:

| `HookEvent` variant | Native command            | Fires when                                                                  |
| ------------------- | ------------------------- | --------------------------------------------------------------------------- |
| `SessionStart`      | `amplihack-hooks session-start` | A new agent session is initialized                                   |
| `SessionEnd`        | `amplihack-hooks session-stop`  | A session terminates (normal exit, crash, or user interrupt)          |
| `UserPromptSubmit`  | `amplihack-hooks user-prompt-submit` | The user submits a prompt to the agent                              |
| `PreToolUse`        | `amplihack-hooks pre-tool-use` | Before any tool call is executed                                      |
| `PostToolUse`       | `amplihack-hooks post-tool-use` | After any tool call completes (success or failure)                   |
| `Stop`              | `amplihack-hooks stop` | The top-level agent stops emitting work                                      |
| `SubagentStop`      | `subagent_stop.py`        | A subagent (`task` tool / explore / general-purpose) finishes               |
| `PreCompact`        | `pre_compact.py`          | Before context compaction runs                                              |

The mapping is encoded in `HookEvent::filename()` and is the single source of truth for hook discovery.

### Missing-Hook Error

If the file does not exist, the resolver returns:

```rust
HookError::MissingHookForBinary {
    binary: String,
    event: HookEvent,
    expected_path: PathBuf,
    remediation: &'static str,
}
```

Display format:

```text
No SessionEnd hook registered for active agent binary 'copilot'.
Expected at: /home/alice/.amplihack/.claude/hooks/copilot/session_end.py
To fix: install the hook at the expected path, switch binaries by re-launching
with one of: 'amplihack claude' / 'amplihack copilot' / 'amplihack codex' /
'amplihack amplifier', or set AMPLIHACK_AGENT_BINARY explicitly for a single
invocation.
```

**There is no fallback to `claude`'s hooks.** A missing `copilot` hook is reported as a hard error so the user can either install the hook or switch binary explicitly. Stub files that exist solely to swallow `MissingHookForBinary` are explicitly disallowed.

The path is **always** validated:

1. The binary name is checked against the allowlist before being substituted into the path.
2. The constructed path is `canonicalize`d.
3. The result must `starts_with(amplihack_home.canonicalize())` — any escape via symlink or `..` is rejected.

## Examples

### From a recipe step (bash)

Do not parse `<repo>/.claude/runtime/launcher_context.json` from shell.
Prefer invoking nested work through `amplihack` so the shared resolver handles
`AMPLIHACK_AGENT_BINARY` (and its default-guess tag), session markers, the
launcher context, and the default consistently:

```sh
amplihack recipe run investigation-workflow \
  -c task_description="Inspect the failing workflow" \
  -c repo_path=.
```

For Rust callers inside `amplihack-rs`, always prefer
`agent_binary::resolve(&cwd)` over re-implementing the precedence.

### From Rust code

```rust
use amplihack_utils::agent_binary;
use std::process::Command;

let cwd = std::env::current_dir()?;
let binary = agent_binary::resolve(&cwd);

Command::new(binary)
    .arg("--noninteractive")
    .arg("--prompt")
    .arg("Run the next workstream")
    .status()?;
```

### Explicit override for a single command

```sh
AMPLIHACK_AGENT_BINARY=codex amplihack recipe run smart-orchestrator \
  -c task_description="..." -c repo_path=.
```

## Related

- [Agent Binary Routing](../concepts/agent-binary-routing.md) — Architectural overview and rationale
- [Environment Variables](./environment-variables.md#amplihack_agent_binary) — Full env var reference
- [Agent Configuration](./agent-configuration.md#agent-binary-resolution) — Where the default fits into config precedence
- [Hook Specifications](./hook-specifications.md) — Per-binary hook layout and supported events
