# Agent Binary Routing

`amplihack` supports four AI backends — `claude`, `copilot`, `codex`, and `amplifier` — each launched via the same `amplihack <tool>` pattern. This document explains how downstream components (recipe runner, hooks, sub-agents) know which backend is active, and why this matters.

## Contents

- [The problem](#the-problem)
- [The solution: AMPLIHACK_AGENT_BINARY](#the-solution-amplihack_agent_binary)
- [How it propagates](#how-it-propagates)
- [Consumers](#consumers)
  - [Recipe runner](#recipe-runner)
  - [Hooks](#hooks)
  - [Sub-agents](#sub-agents)
- [Supported values](#supported-values)
- [Related](#related)

## The problem

The recipe runner is a shell-level orchestrator that executes multi-step workflows by spawning new AI sessions. It does not know — and should not hardcode — which AI tool the user started with. If a user invokes `amplihack copilot` and triggers a recipe that spawns a follow-up session, that session must also use `copilot`, not `claude`.

Before `AMPLIHACK_AGENT_BINARY` was propagated, components were forced to:

- Hardcode `claude` (breaking Copilot and Codex users)
- Require an explicit configuration setting (error-prone and redundant)
- Inspect `$0` or try to detect the active binary at runtime (fragile)

## The solution: AMPLIHACK_AGENT_BINARY

When `amplihack` launches any tool, it writes the tool name into the child process environment as `AMPLIHACK_AGENT_BINARY`. Every subprocess — the AI tool itself, hooks, recipe steps — inherits this variable and can use it to spawn the correct binary without any additional configuration.

```sh
# User invokes:
amplihack claude

# Child process environment contains:
AMPLIHACK_AGENT_BINARY=claude

# User invokes:
amplihack copilot

# Child process environment contains:
AMPLIHACK_AGENT_BINARY=copilot
```

The value is always one of the four known tool names. It is set unconditionally on every launch — there is no fallback or default value that could mask a misconfiguration.

## How it propagates

The Rust CLI sets `AMPLIHACK_AGENT_BINARY` inside `EnvBuilder::with_agent_binary()`, called as part of the env build chain in `launch.rs`:

```rust
let env = EnvBuilder::new()
    .with_amplihack_session_id()
    .with_amplihack_vars()
    .with_agent_binary(tool)         // sets AMPLIHACK_AGENT_BINARY
    .with_amplihack_home()
    .set_if(is_noninteractive(), "AMPLIHACK_NONINTERACTIVE", "1")
    .build();
```

`tool` is the string passed to `run_launch()` by the CLI dispatcher — it is always the exact name of the subcommand the user invoked.

## Consumers

### Recipe runner

The recipe runner reads `AMPLIHACK_AGENT_BINARY` to decide which binary to call when launching a new AI session as part of a workflow step.

```sh
# Inside a recipe step script:
${AMPLIHACK_AGENT_BINARY} --print "${PROMPT}" --model "${MODEL}"
# Equivalent to: claude --print "..." or copilot --print "..." depending on which tool was used
```

### Hooks

Claude Code hooks run as subprocesses of the AI tool. They inherit the full environment, including `AMPLIHACK_AGENT_BINARY`. A hook that needs to spawn a continuation session uses this variable:

```sh
# In a PostToolUse hook
if [ "$AMPLIHACK_AGENT_BINARY" = "claude" ]; then
  # Claude-specific post-processing
  claude --print "Review the output of the tool call"
fi
```

### Sub-agents

Agents spawned by the recipe runner or by hooks inherit `AMPLIHACK_AGENT_BINARY` automatically because it is part of the process environment. No explicit passing is required.

## Supported values

| Value | Tool | Installed by |
|-------|------|-------------|
| `claude` | Anthropic Claude Code | npm: `@anthropic-ai/claude-code` |
| `copilot` | GitHub Copilot CLI | npm: `@github/copilot` |
| `codex` | OpenAI Codex CLI | npm: `@openai/codex-cli` |
| `amplifier` | Microsoft Amplifier | uv: `git+https://github.com/microsoft/amplifier` |

No other values are valid. The Rust implementation uses a `debug_assert!` in `with_agent_binary()` that panics on unexpected values in debug and test builds, making misuse visible early in development.

## Related

- [Environment Variables](../reference/environment-variables.md#amplihack_agent_binary) — Full reference for `AMPLIHACK_AGENT_BINARY`
- [Bootstrap Parity](./bootstrap-parity.md) — Full Python/Rust parity contract for the launcher environment
