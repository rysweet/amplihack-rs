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
approximates the precedence with a regex allowlist: it honours
`AMPLIHACK_AGENT_BINARY` (and its default-guess tag), then the walked-up
`launcher_context.json`, then the parent process chain, then the default. It
has no session-marker layer. There is no Python implementation in this
repository. The Rust resolver is authoritative wherever another
implementation differs.

## Resolution Precedence

The resolver evaluates sources in order and returns the first valid value. A value is "valid" only if it survives normalization (trim, lowercase) and matches the allowlist.

| # | Source | Notes |
| - | --- | --- |
| 1 | `AMPLIHACK_AGENT_BINARY` env var | Explicit override. Used by CI, tests, and external consumers that have not migrated yet. Ignored while tagged `AMPLIHACK_AGENT_BINARY_SOURCE=default:<same binary>` (see below). |
| 2 | Live session marker | An environment variable the hosting CLI exports, such as `CLAUDECODE`, `CLAUDE_CODE_SESSION_ID`, `CLAUDE_CODE_ENTRYPOINT` or `COPILOT_CLI`. The full list is `agent_binary::SESSION_MARKERS`. |
| 3 | `<repo>/.claude/runtime/launcher_context.json` `launcher` field | Persisted state, possibly written by a different session. Consulted only while fresh, and never above a world-writable or foreign-owned directory. |
| 4 | Built-in default | `"copilot"` |

If a source produces a value that fails validation (allowlist, length, character class), the resolver emits `tracing::warn!` with structured fields and falls through to the next source. **No source ever silently coerces an invalid value.**

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
string `launcher` field, a launcher outside the allowlist, larger than 64 KiB,
unreadable, or a symlink out of its directory. A reason never quotes the file.
A stale file (older than 24h) is not listed, because sessions end and an old
file is expected. The walk-up still continues past an unusable file, so a
parent directory's file can still answer, but the notice now says so. Rust
callers get the same evidence from `agent_binary::resolve_detailed`
(`Resolution::context_file` and `Resolution::unusable_contexts`).

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
detached launch cannot. Once a tmux server is running, `tmux new-session`
gives the new command the server's environment, not the caller's, so the
session markers do not arrive and the run falls back to the default (#1335).
The first launch, which starts the server, does see them, so this can work
once and then stop.

`amplihack agent-binary` resolves in the caller's shell and prints the answer
(issue #1525):

```console
$ amplihack agent-binary
claude (session_marker)
$ amplihack agent-binary --shell
AMPLIHACK_AGENT_BINARY=claude AMPLIHACK_AGENT_BINARY_SOURCE=
```

Use the `--shell` form inline, inside the double-quoted command, so that it
expands before tmux runs anything:

```bash
tmux new-session -d -s recipe-runner \
  "cd /path/to/repo && $(amplihack agent-binary --shell -w /path/to/repo) amplihack recipe run ..."
```

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
- An inferred answer is explained on stderr, which stays on your terminal while
  `$(...)` captures stdout. The explanation includes any unusable launcher
  context it skipped.
- The inline `VAR=value command` form works on every tmux version.
  `tmux new-session -e` only exists from tmux 3.2.
- The subcommand never self-installs, so it is safe inside `$(...)`.

Where a detached session runs an agent CLI directly, not `amplihack`, there
is nothing to hand over. The migrate skill's remote `tmux new-session` runs
`<cli> --resume <id>` with the CLI already named.

### Why file-based, not env-based

Environment variables do not survive every subprocess boundary in the launcher's call graph:

- `tmux new-session -d` gives the command the tmux server's environment, not the caller's, once a server is running (see [Handing the binary to a detached launch](#handing-the-binary-to-a-detached-launch)).
- Detached background processes started via `setsid` may inherit a stale or stripped env.
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

Validation rules applied to every candidate value before it can win precedence:

- Length ≤ 32 bytes
- No `/`, `\`, `..`, null bytes, whitespace, or ASCII control characters
- Trim then lowercase, then exact match against the allowlist
- No prefix matching, no substring matching, no shell expansion

Values that fail validation are logged at `warn` level (with the rejected value redacted into a structured field, never inlined into a format string) and treated as if the source was unset.

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
Read cap: 64 KiB (oversized files are rejected with a warning)
Staleness window: 24 hours (older files fall through as if unset)

```json
{
  "launcher": "copilot",
  "session_id": "01J9ZK7E5W6X9N3Q4VBHTC8MR2",
  "cwd": "/home/alice/src/example-repo",
  "started_at": "2026-04-29T04:12:55Z",
  "amplihack_version": "0.7.4"
}
```

The resolver only reads the `launcher` field. Other fields are owned by `LauncherContext`.

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
