<!--
Provenance: Adapted from upstream rysweet/amplihack docs/AGENT_MEMORY_QUICKSTART.md
(commit 8639ee4). Python-only sections have been omitted; CLI flags have been
verified against crates/amplihack-cli/src/cli_subcommands.rs::MemoryCommands and
the library facade against crates/amplihack-memory/src/facade.rs in this checkout.
Storage paths use the amplifier-bundle layout, not ~/.amplihack/.claude/.
-->

# Agent Memory Quickstart

> **Status: Shipped.** The four `amplihack memory` subcommands
> (`tree`, `export`, `import`, `clean`) and the library facade
> (`amplihack_memory::Memory::remember` / `recall`) are present in this
> checkout. The `--enable-memory` flag on `amplihack new` is shipped; it
> scaffolds memory wiring into the generated Rust agent crate.

This quickstart covers the agent-memory surfaces in amplihack-rs:

- the top-level `amplihack memory tree` graph view
- the agent-local `amplihack memory export` / `amplihack memory import`
  transfer commands
- generated agent packages created with `amplihack new --enable-memory`

For the full architectural picture, see
[Agent Memory Architecture](../concepts/agent-memory-architecture.md).
For a deeper integration walk-through, see
[Integrate Agent Memory](./integrate-agent-memory.md).

## Before you start

- amplihack-rs built and on `PATH`
- The default backend (`graph-db`, backed by LadybugDB) is used in the
  examples below. Override with `--backend sqlite` or
  `AMPLIHACK_MEMORY_BACKEND=sqlite` if you have a reason.

## 1. Inspect the top-level memory graph

The `tree` subcommand prints the hierarchical memory graph for the
current install.

```bash
amplihack memory tree
```

Useful filters (verified against
`crates/amplihack-cli/src/cli_subcommands.rs::MemoryCommands::Tree`):

```bash
amplihack memory tree --depth 2
amplihack memory tree --session test_session_01
amplihack memory tree --type learning
amplihack memory tree --backend sqlite
```

The `--type` flag accepts exactly these values today:

- `conversation`
- `decision`
- `pattern`
- `context`
- `learning`
- `artifact`

Any other value is rejected by `clap` before the command runs.

## 2. Generate a memory-enabled agent

`amplihack new --enable-memory` scaffolds a Rust agent crate that wires
the `amplihack-memory` facade into its main entry point.

```bash
printf '%s\n' \
  'Build an agent that investigates deployment failures, remembers repeated causes, and suggests the next debugging step.' \
  > goal.md

amplihack new \
  --file goal.md \
  --name incident-memory-agent \
  --enable-memory \
  --sdk copilot
```

The generated crate lives under `./goal_agents/incident-memory-agent/`
and includes `amplihack-memory` as a dependency, so its agent code can
call `Memory::remember` and `Memory::recall` directly.

## 3. Build and run the generated agent

```bash
cd goal_agents/incident-memory-agent
cargo build --release
cargo run --release
```

There is no Python step. The generated package is a standalone Rust
binary; it does not require a Python interpreter to build or run.

## 4. Export or import an agent's memory

Use these when moving an agent's hierarchical memory between machines or
environments.

```bash
amplihack memory export \
  --agent incident-memory-agent \
  --output ./incident-memory.json

amplihack memory import \
  --agent incident-memory-agent \
  --input ./incident-memory.json \
  --merge
```

For raw graph-store replacement instead of JSON merge, use `--format
raw-db` (the `raw-db` format mirrors the on-disk LadybugDB layout):

```bash
amplihack memory export \
  --agent incident-memory-agent \
  --output ./incident-memory-db \
  --format raw-db

amplihack memory import \
  --agent incident-memory-agent \
  --input ./incident-memory-db \
  --format raw-db
```

The accepted `--format` values are `json` and `raw-db`. Any other value
is rejected.

## 5. Clean stale sessions

`memory clean` removes sessions matching a glob pattern. It defaults to
a dry run so you can preview changes:

```bash
amplihack memory clean --pattern 'test_*'              # dry-run preview
amplihack memory clean --pattern 'test_*' --no-dry-run --confirm
```

`--confirm` skips the interactive confirmation prompt, which is useful
in CI.

## 6. Know which storage you are looking at

Three related storage surfaces live in this repo:

- the top-level CLI graph view, used by `memory tree`, backed by
  LadybugDB (`graph-db`) or SQLite under the amplihack data directory
- the agent-local hierarchical store used by `memory export` /
  `memory import`, scoped per `--agent`
- the per-crate memory wired into a `--enable-memory` agent, accessed
  through the `amplihack_memory` library facade

See [Memory Backend Architecture](../concepts/memory-backend-architecture.md)
for backend selection rules and on-disk layout.

## Next steps

- [Agent Memory Architecture](../concepts/agent-memory-architecture.md)
- [Integrate Agent Memory](./integrate-agent-memory.md)
- [Memory Backend Architecture](../concepts/memory-backend-architecture.md)
- [Memory Backend Reference](../reference/memory-backend.md)
- [amplihack-memory Extended API](../reference/memory-extended-api.md)
