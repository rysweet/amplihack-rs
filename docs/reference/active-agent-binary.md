# Active Agent Binary — Resolver Reference

## Overview

The **active agent binary** is the AI tool (`claude`, `copilot`, `codex`, or `amplifier`) that the current process should treat as its runtime. It is resolved by a single shared function, used by every read site across `amplihack-cli`, `amplihack-utils`, `amplihack-workflows`, and `amplihack-hooks`, plus the Python helpers in `amplifier-bundle/`.

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

**Canonical entry point (Python):**

```python
# Defined in amplifier-bundle/skills/pm-architect/scripts/agent_query.py
from agent_query import detect_runtime

binary = detect_runtime()
```

The `detect_runtime()` function in `agent_query.py` is the single Python
implementation; `delegate_response.py` imports it instead of re-implementing
the precedence. The shell helper in `amplifier-bundle/skills/migrate/scripts/migrate.sh`
re-implements the same precedence using a `case` statement allowlist (shell
scripts cannot import Python).

All implementations follow the **same precedence**, the **same allowlist**, and produce the **same default** so behavior is consistent across language boundaries and across `tmux` / subprocess hops.

## Resolution Precedence

The resolver evaluates sources in order and returns the first valid value. A value is "valid" only if it survives normalization (trim, lowercase) and matches the allowlist.

| # | Source                                                                  | Notes                                                                                                                                  |
| - | ----------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------- |
| 1 | `AMPLIHACK_AGENT_BINARY` env var                                        | Explicit override. Used by CI, tests, and external consumers (e.g. `rysweet/amplihack-recipe-runner`) that have not migrated yet.       |
| 2 | `<repo>/.claude/runtime/launcher_context.json` `launcher` field          | Canonical persisted state. Written by `amplihack <tool>` on every launch via `LauncherContext::persist`. Survives `tmux`/subprocess hops without env passthrough. |
| 3 | Built-in default                                                        | `"copilot"`                                                                                                                            |

If a source produces a value that fails validation (allowlist, length, character class), the resolver emits `tracing::warn!` with structured fields and falls through to the next source. **No source ever silently coerces an invalid value.**

### Why file-based, not env-based

Environment variables do not survive every subprocess boundary in the launcher's call graph:

- `tmux new-session -d` strips most variables unless they are explicitly forwarded.
- Detached background processes started via `setsid` may inherit a stale or stripped env.
- Sub-recipes spawned by `amplihack recipe run` invoke fresh `amplihack` binaries that may be reading env from the user's shell rather than the parent recipe runner.
- Python hooks shell out to subcommands using `subprocess.run` which inherits the calling Python's env, not the Rust launcher's.

`launcher_context.json` is written once per launch under `<repo>/.claude/runtime/` with `0o600` permissions and an atomic rename. Any descendant process can re-derive the path by walking up from its `cwd`, so the active binary is recoverable without any env coordination.

## Allowlist & Validation

The allowlist is **fixed** and identical in Rust and Python:

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

To make `"claude"` permanent for a repo, run `amplihack claude` once — this writes `claude` into `launcher_context.json` and every subsequent process in that repo resolves to `claude`.

**Existing `claude` users:** if your repo already has `.claude/runtime/launcher_context.json` with `"launcher": "claude"` from a prior `amplihack claude` invocation, no action is required. The file-based source (precedence step 2) wins over the new `copilot` default, so existing sessions keep resolving to `claude` until you explicitly run a different `amplihack <tool>` command.

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

The resolver only reads the `launcher` field. Other fields are owned by `LauncherContext` and documented in [Launcher Context](./launcher-context.md).

## Hook Resolution

Hooks live under `<amplihack-home>/.claude/hooks/<binary>/`. When a hook event fires, `amplihack-hooks::binary_hook_resolver` resolves the **active binary** via the algorithm above, then constructs the expected hook path:

```
<amplihack-home>/.claude/hooks/<binary>/<event>.py
```

### Hook Event Variants

`HookEvent` is an enum with eight variants. Each variant maps to a fixed on-disk filename:

| `HookEvent` variant | On-disk filename          | Fires when                                                                  |
| ------------------- | ------------------------- | --------------------------------------------------------------------------- |
| `SessionStart`      | `session_start.py`        | A new agent session is initialized                                          |
| `SessionEnd`        | `session_end.py`          | A session terminates (normal exit, crash, or user interrupt)                |
| `UserPromptSubmit`  | `user_prompt_submit.py`   | The user submits a prompt to the agent                                      |
| `PreToolUse`        | `pre_tool_use.py`         | Before any tool call is executed                                            |
| `PostToolUse`       | `post_tool_use.py`        | After any tool call completes (success or failure)                          |
| `Stop`              | `stop.py`                 | The top-level agent stops emitting work                                     |
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

The active binary is read from the same `launcher_context.json` the launcher writes. From a shell step, source the file directly (no dedicated subcommand exists — and one is unnecessary because the file is the canonical source):

```sh
# Active binary: read launcher_context.json by walking up from cwd
launcher_ctx="$(git rev-parse --show-toplevel)/.claude/runtime/launcher_context.json"
binary=$(jq -r .launcher "$launcher_ctx" 2>/dev/null || echo copilot)
echo "spawning sub-task with: $binary"
```

For Rust callers inside `amplihack-rs`, always prefer `agent_binary::resolve(&cwd)` over re-implementing the walk-up.

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

### From a Python skill helper

```python
# Defined in amplifier-bundle/skills/pm-architect/scripts/agent_query.py
from agent_query import detect_runtime

binary = detect_runtime()
print(f"querying via {binary}")
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
- [Hooks Reference](./hooks.md) — Per-binary hook layout and supported events
