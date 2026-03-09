# amplihack-rs

Rust core runtime for amplihack's deterministic infrastructure layer.

## Architecture

- **amplihack-types** — Thin IPC boundary types (HookInput, HookOutput, Settings)
- **amplihack-state** — File ops, locking, env config, Python bridge
- **amplihack-hooks** — All hook implementations (pre_tool_use, stop, session_start, etc.)
- **amplihack (bin)** — CLI binary (Phase 2)
- **amplihack-hooks (bin)** — Multicall hook binary

## Quick Start

```bash
# Build
cargo build

# Run tests
cargo test

# Lint
cargo clippy -- -D warnings
cargo fmt --check

# Run a hook
echo '{"hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": {"command": "ls"}}' | cargo run --bin amplihack-hooks -- pre-tool-use
```

## Hook Binary

The `amplihack-hooks` binary dispatches to hooks via subcommands:

```bash
amplihack-hooks pre-tool-use     # Validate bash commands
amplihack-hooks post-tool-use    # Record tool metrics
amplihack-hooks stop             # Lock mode + power steering
amplihack-hooks session-start    # Initialize session context
amplihack-hooks session-stop     # Store session memory
amplihack-hooks user-prompt      # Inject preferences + memory
amplihack-hooks pre-compact      # Export transcript
```

Register with your hook host:
```json
{"command": "/path/to/amplihack-hooks pre-tool-use", "timeout": 10}
```

## Design Principles

1. **NO FALLBACKS** — Rust is the only implementation
2. **Correctness over performance** — Type safety eliminates bug categories
3. **Host-agnostic** — Works with Claude Code, Amplifier, and Copilot
4. **Fail-open** — Non-security hooks output `{}` on error (don't break the user)
