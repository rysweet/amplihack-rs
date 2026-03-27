# amplihack-rs Documentation

amplihack-rs is the Rust implementation of the amplihack CLI. It replaces the Python-based installer with a native binary that bootstraps the complete amplihack environment in a single command.

## Contents

### How-To Guides

- [Install amplihack for the First Time](./howto/first-install.md) — Bootstrap from scratch, including optional legacy-Python checks, binary deployment, and hook registration
- [Install from a Local Repository](./howto/local-install.md) — Install without network access using a local checkout
- [Uninstall amplihack](./howto/uninstall.md) — Cleanly remove all installed files, binaries, and hook registrations
- [Resolve kuzu Linker Errors](./howto/resolve-kuzu-linker-errors.md) — Diagnose and fix `undefined reference` errors caused by `cxx`/`cxx-build` version mismatch
- [Fix the cxx-build Pin CI Failure](./howto/fix-cxx-build-ci-failure.md) — Restore the `Cargo.lock` pin when the `Verify cxx-build pin` CI step fails
- [Enable Shell Completions](./howto/enable-shell-completions.md) — Install tab-completion for bash, zsh, fish, and PowerShell
- [Run amplihack in Non-interactive Mode](./howto/run-in-noninteractive-mode.md) — Use amplihack in CI pipelines, Docker containers, and piped scripts without interactive prompts
- [Manage Tool Update Notifications](./howto/manage-tool-update-checks.md) — Control or disable the pre-launch npm update check for `claude`, `copilot`, and `codex`
- [Run a Recipe End-to-End](./howto/run-a-recipe.md) — Find, inspect, dry-run, and execute YAML recipes through the Rust CLI
- [Index a Project with the Native SCIP Pipeline](./howto/index-a-project.md) — Build the Kuzu code-graph from source using native SCIP indexers
- [Validate No-Python Compliance](./howto/validate-no-python.md) — Run the AC9 probe to confirm the binary operates without a Python interpreter
- [Use the Fleet Dashboard](./howto/use-fleet-dashboard.md) — Open the cockpit, start and adopt sessions, search sessions, run the reasoner from the TUI, and exit cleanly
- [Run Fleet Scout and Advance on Azure VMs](./howto/run-fleet-scout-and-advance.md) — Discover sessions across VMs, reason about them with the LLM backend, and execute recommended actions
- [Migrate Memory to the SQLite Backend](./howto/migrate-memory-backend.md) — Export hierarchical memory to portable JSON, switch to SQLite, and verify the migration

### Reference

- [amplihack install](./reference/install-command.md) — Full CLI reference for the `install` and `uninstall` commands
- [Install Manifest](./reference/install-manifest.md) — Schema and semantics of the uninstall manifest written at install time
- [Hook Specifications](./reference/hook-specifications.md) — Canonical table of all 7 Claude Code hooks registered by amplihack
- [Binary Resolution](./reference/binary-resolution.md) — How `amplihack` locates the `amplihack-hooks` binary at install time
- [amplihack completions](./reference/completions-command.md) — Full CLI reference for the `completions` subcommand
- [Environment Variables](./reference/environment-variables.md) — All environment variables read or injected by `amplihack` during a launch
- [Launch Flag Injection](./reference/launch-flag-injection.md) — How `amplihack` builds the subprocess command line: `--dangerously-skip-permissions`, `--model`, and extra args passthrough
- [Signal Handling and Exit Codes](./reference/signal-handling.md) — SIGINT, SIGTERM, SIGHUP behavior and exit code contract (Python parity)
- [amplihack recipe](./reference/recipe-command.md) — Full CLI reference for `recipe list`, `recipe show`, `recipe validate`, and `recipe run`
- [Parity Test Scenarios](./reference/parity-test-scenarios.md) — Every parity tier file, its test cases, and expected Python↔Rust divergence
- [amplihack index-code and index-scip](./reference/memory-index-command.md) — Full CLI reference for code-graph ingestion commands
- [amplihack query-code](./reference/query-code-command.md) — Full CLI reference for querying the native Kuzu code-graph
- [amplihack fleet](./reference/fleet-command.md) — Full CLI reference for the fleet dashboard: key bindings, refresh architecture, persistent state schema, and security properties
- [Memory Backend](./reference/memory-backend.md) — `BackendChoice` values, env vars, flat and hierarchical schema, transfer formats, and security properties

### Concepts

- [Bootstrap Parity](./concepts/bootstrap-parity.md) — Why the Rust CLI replicates the Python installer's first-install flow and what that means for users
- [Idempotent Installation](./concepts/idempotent-installation.md) — How repeated installs are safe and how existing hook registrations are updated in place
- [The cxx/cxx-build Version Contract](./concepts/cxx-version-contract.md) — Why `cxx` and `cxx-build` must share the same minor version and how a mismatch produces linker errors
- [Agent Binary Routing](./concepts/agent-binary-routing.md) — How `AMPLIHACK_AGENT_BINARY` lets the recipe runner and hooks call back into the correct AI tool
- [Kuzu Code Graph](./concepts/kuzu-code-graph.md) — Architecture of the native code-graph store: schema, SCIP pipeline, blarify consumption, and security model
- [Memory Backend Architecture](./concepts/memory-backend-architecture.md) — Backend-neutral trait seams, auto-detection order, SQLite vs. graph-db storage layout, and the transfer layer
- [Fleet Dashboard Architecture](./concepts/fleet-dashboard-architecture.md) — Thread model, state design, persistence layer, terminal safety, and security rationale for `amplihack fleet`
- [Fleet Admiral Reasoning Engine](./concepts/fleet-admiral-reasoning.md) — How the LLM-backed reasoner works: what it sees, the five actions, confidence scoring, failure modes, and design rationale

## Quick Start

```sh
# Build from source
cargo build --release

# Install amplihack (first time)
~/.cargo/bin/amplihack install

# Install from a local clone (no network)
amplihack install --local /path/to/amplihack-clone

# Remove everything amplihack installed
amplihack uninstall
```

## Related

- [README](../README.md) — Architecture overview and design principles
- [CONTRIBUTING_RUST.md](../CONTRIBUTING_RUST.md) — Developer setup, build targets, test harness
