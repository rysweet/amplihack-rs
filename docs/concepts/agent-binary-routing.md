# Agent Binary Routing

amplihack supports multiple AI agent CLIs — `claude`, `copilot`, `codex`, and
`amplifier`. The recipe runner that orchestrates prompts and hooks is agent-agnostic
and must know which binary to invoke. This document explains how that information flows
from the CLI invocation to every consumer downstream.

## Contents

- [The problem](#the-problem)
- [The solution: AMPLIHACK_AGENT_BINARY](#the-solution-amplihack_agent_binary)
- [How it propagates](#how-it-propagates)
- [Supported values](#supported-values)
- [Consumers](#consumers)
- [Security note](#security-note)

---

## The problem

The recipe runner is deliberately agent-agnostic. It issues prompts, applies hooks,
and manages session state the same way regardless of which AI model backs the session.
But to actually launch a conversation, it must know:

1. Which binary to invoke (`claude`, `copilot`, etc.)
2. What argument syntax that binary expects

Hardcoding a specific binary name into the recipe runner would break multi-agent
support. Passing it as a command-line argument to every recipe runner invocation would
create tight coupling and verbose argument lists.

The solution is to encode the binary name once — at the top-level `amplihack` launch
command — and propagate it through the environment so every subprocess can read it
without additional configuration.

---

## The solution: AMPLIHACK_AGENT_BINARY

When the user runs:

```sh
amplihack launch claude
```

The launcher sets `AMPLIHACK_AGENT_BINARY=claude` in the environment of every child
process it spawns. The value is the exact binary name as it would appear on `PATH`.

This happens inside `EnvBuilder::with_agent_binary(tool)` in
`crates/amplihack-cli/src/env_builder.rs`:

```rust
pub fn with_agent_binary(self, tool: impl Into<String>) -> Self {
    let tool = tool.into();
    debug_assert!(
        ["claude", "copilot", "codex", "amplifier"].contains(&tool.as_str()),
        "unsupported agent binary: {tool}"
    );
    self.set("AMPLIHACK_AGENT_BINARY", tool)
}
```

The `debug_assert!` validates the value against the allowlist in debug and test
builds. In release builds the check is omitted for performance; the allowlist is
enforced at the CLI argument-parsing layer.

---

## How it propagates

The variable is set as part of the canonical `EnvBuilder` chain in
`crates/amplihack-cli/src/commands/launch.rs`:

```rust
EnvBuilder::new()
    .with_amplihack_session_id()
    .with_amplihack_vars()
    .with_agent_binary(tool)        // ← sets AMPLIHACK_AGENT_BINARY
    .with_amplihack_home()
    .set_if(is_noninteractive(), "AMPLIHACK_NONINTERACTIVE", "1")
    .build()
```

The environment is then passed to `Command::new()` which uses `execve()` (not a shell)
to launch the child process. This means the value is passed as a raw byte string with
no shell interpolation.

Every subsequent process spawned by the recipe runner inherits the variable through
normal POSIX environment inheritance, so sub-agents and hook scripts can read it
without being explicitly told the binary name.

---

## Supported values

| Value | Binary | Install method |
|-------|--------|----------------|
| `claude` | Claude Code CLI | `npm install -g @anthropic-ai/claude-code` |
| `copilot` | GitHub Copilot CLI | `gh extension install github/gh-copilot` |
| `codex` | OpenAI Codex CLI | `npm install -g @openai/codex` |
| `amplifier` | Amplifier CLI | amplihack internal tooling |

The list of supported values is maintained in:

- The CLI argument parser (user-facing validation)
- The `debug_assert!` in `with_agent_binary()` (test-time validation)

---

## Consumers

### Recipe runner

The recipe runner reads `AMPLIHACK_AGENT_BINARY` to decide which binary to invoke and
which argument format to use. Example (Python pseudocode):

```python
agent_binary = os.environ.get("AMPLIHACK_AGENT_BINARY", "claude")
subprocess.run([agent_binary, "--print", prompt])
```

### Hooks

Pre- and post-conversation hooks may read `AMPLIHACK_AGENT_BINARY` to log which agent
is active or to apply agent-specific configuration:

```python
import os
agent = os.environ["AMPLIHACK_AGENT_BINARY"]
print(f"Hook running for agent: {agent}", file=sys.stderr)
```

### Sub-agents

When amplihack spawns a nested `amplihack` process (e.g. for a sub-task), the child
process inherits `AMPLIHACK_AGENT_BINARY` via environment inheritance. The child does
not need to re-specify the tool.

---

## Security note

`AMPLIHACK_AGENT_BINARY` is used as a process name passed to `Command::new()`, not as
a shell command string. This means it is never shell-interpolated. However:

- Consumers must **not** pass the value directly to a shell (`sh -c`), as an attacker
  with control over the environment could inject arbitrary commands.
- The value should always be validated against the known allowlist before use in
  security-sensitive contexts.
- The debug-only `debug_assert!` in `with_agent_binary()` is **not** a production
  security control; it is a developer-time correctness aid.
