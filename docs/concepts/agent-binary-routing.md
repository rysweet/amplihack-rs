# Agent Binary Routing (`AMPLIHACK_AGENT_BINARY`)

## Overview

When the amplihack Rust CLI launches a tool (Claude, Copilot, Codex, or Amplifier), it sets `AMPLIHACK_AGENT_BINARY` in the child process environment. This tells downstream consumers — recipe runner, hooks, shell completions — which agent binary initiated the session.

## Rationale

The Python launcher (PR #3100) set `AMPLIHACK_AGENT_BINARY` based on the invoked CLI entry point. This Rust port preserves that behaviour so the recipe runner and hook system can make routing decisions without inspecting `argv[0]`.

## Values

| Invoked CLI | `AMPLIHACK_AGENT_BINARY` |
|-------------|--------------------------|
| `amplihack claude` | `claude` |
| `amplihack copilot` | `copilot` |
| `amplihack codex` | `codex` |
| `amplihack amplifier` | `amplifier` |

## Implementation

`EnvBuilder::with_agent_binary(tool)` in `crates/amplihack-cli/src/env_builder.rs` sets the variable. The value flows from the `Commands` dispatch in `lib.rs` → `run_launch()` in `launch.rs`.

A `debug_assert!` validates the tool name in debug/test builds (SEC-WS1-01). Release builds trust the caller.
