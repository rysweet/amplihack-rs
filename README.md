# amplihack-rs

[![CI](https://github.com/rysweet/amplihack-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/rysweet/amplihack-rs/actions/workflows/ci.yml)
[![Docs](https://github.com/rysweet/amplihack-rs/actions/workflows/docs.yml/badge.svg)](https://rysweet.github.io/amplihack-rs/)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://opensource.org/licenses/MIT)

Rust core runtime for amplihack's deterministic infrastructure layer.
Native binary that bootstraps the complete amplihack environment — structured
workflows, persistent memory, specialized agents, and quality gates — in a
single command. No Python runtime required.

**📚 [View Full Documentation](https://rysweet.github.io/amplihack-rs/)**

---

## Table of Contents

- [Why Rust?](#why-rust)
- [Installation](#installation)
- [Quick Start](#quick-start)
- [Architecture](#architecture)
- [Hook Binary](#hook-binary)
- [Configuration](#configuration)
- [CLI Parity Harness](#cli-parity-harness)
- [Documentation](#documentation)
- [Contributing](#contributing)
- [Design Principles](#design-principles)
- [License](#license)

## Why Rust?

The Python amplihack CLI works but carries a ~200 MB runtime dependency (Python + uv + venv).
amplihack-rs compiles to a single static binary (~15 MB) with:

- **Zero external runtime** — no Python, no Node.js, no interpreter at all
- **Sub-millisecond hook latency** — hooks run in the critical path of every tool call
- **Type-safe IPC** — serde-derived types eliminate serialization bugs
- **Deterministic builds** — `cargo install --locked` reproduces the exact binary

## Architecture

### Core Types & State
- **amplihack-types** — Thin IPC boundary types (HookInput, HookOutput, Settings)
- **amplihack-state** — File ops, locking, env config, atomic JSON persistence

### Hooks & Security
- **amplihack-hooks** — All hook implementations (pre_tool_use, stop, session_start, etc.)
- **amplihack-security** — XPIA threat detection, prompt injection analysis, security scanning
- **amplihack-safety** — Conflict detection, safe file operations, guardrails

### Intelligence & Coordination
- **amplihack-workflows** — Workflow execution engine (default, cascade, consensus, investigation)
- **amplihack-recovery** — Failure recovery orchestration, retry strategies, state checkpointing
- **amplihack-context** — Runtime context detection, environment inference, session awareness

### Memory & Fleet
- **amplihack-memory** — Memory backends (SQLite, LadybugDB graph), bloom filters, transfer/export
- **amplihack-fleet** — Multi-agent fleet coordination, tmux/Azure VM orchestration

### CLI & Recipes
- **amplihack-cli** — CLI commands (install, launch, memory, fleet, update, doctor)
- **amplihack-launcher** — Agent binary resolution, launch environment setup
- **amplihack-recipe** — Recipe system, YAML parsing, step execution

### Agent System
- **amplihack-agent-core** — Agent lifecycle, session management, OODA loop engine
- **amplihack-domain-agents** — Specialized agents: teaching, code review, meeting synthesis
- **amplihack-agent-eval** — Progressive evaluation framework (L1–L12), graders, self-improvement
- **amplihack-hive** — Multi-agent orchestration, workload management, distributed swarms
- **amplihack-agent-generator** — Goal-to-agent pipeline: analyze → plan → synthesize → assemble

### Utilities
- **amplihack-utils** — Process management, project init, plugin system, slugify, defensive parsing
- **amplihack-delegation** — Meta-delegation orchestration, persona strategies, subprocess tracking

### Binaries
- **amplihack (bin)** — Main CLI binary
- **amplihack-hooks (bin)** — Multicall hook binary

## Installation

### Build Prerequisites
- **Rust** (edition 2024, install via [rustup](https://rustup.rs))
- **cmake** — required to build the LadybugDB (formerly Kuzu) graph database engine
  - Ubuntu/Debian: `sudo apt install cmake build-essential`
  - macOS: `brew install cmake`

### Install from source
```bash
cargo install --git https://github.com/rysweet/amplihack-rs amplihack --locked
```

### Install or run through npm / npx
```bash
# One-shot install via a git package spec
npx --yes --package=git+https://github.com/rysweet/amplihack-rs.git -- amplihack install

# Equivalent npm exec form
npm exec --yes --package=git+https://github.com/rysweet/amplihack-rs.git -- amplihack install
```

The npm wrapper exposes the `amplihack` bin, provisions both `amplihack` and
`amplihack-hooks`, then delegates to the Rust CLI. It tries the matching GitHub
release archive first and falls back to a local Cargo build when the package
contents include the Rust workspace (for example when installed from a git
checkout).

Published release archives currently cover Linux and macOS on `x64`/`arm64`.
On Windows, or any other platform without a published release target, the npm
wrapper only works when the packaged Rust workspace is present and a local Rust
toolchain is available for the source-build fallback. If you want the most
predictable cross-platform path, use `cargo install` or a native binary release.

### Pre-built binaries (no build tools required)
Download from https://github.com/rysweet/amplihack-rs/releases for your platform.

## Quick Start

```bash
# Build
cargo build

# Install from git
cargo install --git https://github.com/rysweet/amplihack-rs amplihack --locked

# Or bootstrap the Rust CLI through npm/npx
npx --yes --package=git+https://github.com/rysweet/amplihack-rs.git -- amplihack install

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

# On disk-constrained machines, route local Cargo artifacts into /tmp and audit
# repo/worktree growth before it becomes a cleanup incident.
scripts/dev-space.sh cargo test -p amplihack-cli memory -- --nocapture
scripts/dev-space.sh status

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

The accepted npm/npx form uses npm's `--package` flag, for example:
`npx --yes --package=git+https://github.com/rysweet/amplihack-rs.git -- amplihack install`.
That syntax installs the wrapper package into the npm cache, makes its `amplihack`
bin available on `PATH`, and then hands off to the native Rust CLI.

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

## Configuration

amplihack reads configuration from several sources (highest priority first):

| Source | Location | Purpose |
|--------|----------|---------|
| Environment variables | `AMPLIHACK_*` | Runtime overrides |
| Settings file | `~/.amplihack/settings.json` | Persistent user settings |
| Project config | `.amplihack.toml` in repo root | Per-project overrides |
| Defaults | Compiled into binary | Sensible defaults |

Key environment variables:

| Variable | Default | Description |
|----------|---------|-------------|
| `AMPLIHACK_HOME` | `~/.amplihack` | Root of amplihack installation |
| `AMPLIHACK_AGENT_BINARY` | Auto-detected | Which AI tool to use (`claude`, `copilot`, `codex`) |
| `AMPLIHACK_MAX_DEPTH` | `3` | Max recursion depth for nested agent sessions |
| `AMPLIHACK_NONINTERACTIVE` | unset | Set to `1` for CI/pipeline usage |
| `AMPLIHACK_LOG_LEVEL` | `info` | Tracing verbosity (`trace`, `debug`, `info`, `warn`, `error`) |

See [Environment Variables Reference](docs/reference/environment-variables.md) for the complete list.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for setup instructions, testing guidelines,
and pull request process.

**Quick version:**

```bash
git clone https://github.com/rysweet/amplihack-rs.git
cd amplihack-rs
cargo build
cargo test --workspace --skip fleet_probe --skip kuzu --skip fleet::fleet_local --skip memory::kuzu
```

All PRs must pass `cargo fmt`, `cargo clippy -- -D warnings`, and the test suite.

## Design Principles

1. **NO FALLBACKS** — Rust is the only implementation
2. **Correctness over performance** — Type safety eliminates bug categories
3. **Host-agnostic** — Works with Claude Code, Amplifier, and Copilot
4. **Fail-open** — Non-security hooks output `{}` on error (don't break the user)

## License

This project is licensed under the [MIT License](https://opensource.org/licenses/MIT).
