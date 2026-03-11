# amplihack-rs

Rust core runtime for amplihack's deterministic infrastructure layer.

## Architecture

- **amplihack-types** — Thin IPC boundary types (HookInput, HookOutput, Settings)
- **amplihack-state** — File ops, locking, env config, Python bridge
- **amplihack-hooks** — All hook implementations (pre_tool_use, stop, session_start, etc.)
- **amplihack (bin)** — CLI binary (Phase 2)
- **amplihack-hooks (bin)** — Multicall hook binary

## Installation

### Standard (no cmake required)
```bash
cargo install --git https://github.com/rysweet/amplihack-rs amplihack --locked
```

### With Kuzu graph backend (requires cmake)
```bash
cargo install --git https://github.com/rysweet/amplihack-rs amplihack --locked --features kuzu-backend
```

## Quick Start

```bash
# Build
cargo build

# Install from git
cargo install --git https://github.com/rysweet/amplihack-rs amplihack --locked

# First run: call the freshly installed Rust binary explicitly in case an older
# Python/uv amplihack is earlier on PATH
~/.cargo/bin/amplihack install

# Update to the latest stable GitHub release
~/.local/bin/amplihack update

# Run tests
cargo test

# Lint
cargo clippy -- -D warnings
cargo fmt --check

# Run a hook
echo '{"hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": {"command": "ls"}}' | cargo run --bin amplihack-hooks -- pre-tool-use

# Run a local CLI parity suite
python tests/parity/validate_cli_parity.py \
  --scenario tests/parity/scenarios/tier3-memory.yaml \
  --python-repo /path/to/amploxy \
  --rust-binary target/debug/amplihack

# Run the same suite on a remote host (for example azlin)
python tests/parity/validate_cli_parity.py \
  --ssh-target azlin \
  --scenario tests/parity/scenarios/tier3-memory.yaml \
  --python-repo /home/azureuser/src/amploxy \
  --rust-binary /home/azureuser/src/amplihack-rs/target/debug/amplihack

# Shadow-mode rollout: log divergences without failing the run
python tests/parity/validate_cli_parity.py \
  --scenario tests/parity/scenarios/tier2-install.yaml \
  --python-repo /path/to/amploxy \
  --rust-binary target/debug/amplihack \
  --shadow-mode \
  --shadow-log /tmp/amplihack-shadow.jsonl
```

The first install is intentionally one binary: `amplihack`. Run the freshly installed
Rust binary once via `~/.cargo/bin/amplihack install`; that copies the managed Rust CLI
to `~/.local/bin/amplihack`, stages framework assets into `~/.amplihack/.claude`,
rewrites `~/.claude/settings.json` to those staged hooks, stages Copilot
agents/skills/workflow/context/plugin metadata when needed, and auto-installs missing
host CLIs such as Claude, Copilot, Codex, or Amplifier.

If `amplihack` still resolves to an older Python/uv installation, use `type -a amplihack`
to confirm which binary wins on PATH and invoke `~/.local/bin/amplihack` directly.

`~/.local/bin/amplihack update` switches to the tagged `v*` GitHub release channel once
stable releases are published and installs the paired `amplihack-hooks` binary alongside
the main CLI.

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

## CLI Parity Harness

`tests/parity/validate_cli_parity.py` is the migration loop for native CLI work:

- local or remote (`--ssh-target`) execution
- side-by-side observable tmux mode (`--observable`)
- semantic JSON and filesystem comparison
- shadow-mode logging (`--shadow-mode --shadow-log ...`) for migration dry runs

## Documentation

Full documentation is in the [`docs/`](docs/index.md) directory:

- [First-time install guide](docs/howto/first-install.md)
- [Install from a local repository](docs/howto/local-install.md)
- [Uninstall guide](docs/howto/uninstall.md)
- [amplihack install / uninstall reference](docs/reference/install-command.md)
- [Hook specifications](docs/reference/hook-specifications.md)
- [Bootstrap parity explained](docs/concepts/bootstrap-parity.md)

## Design Principles

1. **NO FALLBACKS** — Rust is the only implementation
2. **Correctness over performance** — Type safety eliminates bug categories
3. **Host-agnostic** — Works with Claude Code, Amplifier, and Copilot
4. **Fail-open** — Non-security hooks output `{}` on error (don't break the user)
