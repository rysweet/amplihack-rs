# Integrate Agent Memory

> **Status: Mixed.** The library facade
> (`amplihack_memory::Memory::remember` / `recall`) is shipped. The CLI
> for *operating* on memory (`amplihack memory tree | export | import |
> clean`) is shipped. There is **no** `amplihack memory recall` CLI; if
> you want to read memories from the shell today, use `memory tree`.

This how-to wires an agent into the memory subsystem.

## Before you start

- amplihack-rs built and on `PATH`
- A Rust crate that can take `amplihack-memory` as a dependency, **or**
- Acceptance that CLI inspection is currently limited to `memory tree`

## Step 1: Choose a backend

Defaults are sensible — leave them alone unless you have a reason. To
override:

```sh
export AMPLIHACK_MEMORY_BACKEND=sqlite     # or graph-db
```

The same flag is exposed per-command via `--backend` on `memory tree` and
`memory clean`. See
[Memory Backend Architecture](../concepts/memory-backend-architecture.md)
for selection rules.

## Step 2: Write to memory from your agent

Use the shipped facade:

```rust
use amplihack_memory::{Memory, MemoryConfig, Backend, Topology};

let mem = Memory::new("my-agent", MemoryConfig {
    topology: Topology::Single,
    backend: Backend::Cognitive,
    ..Default::default()
})?;

mem.remember("The user prefers Rust over Python for performance work")?;
```

`remember` writes a `MemoryRecord` keyed by the agent name and current
session.

## Step 3: Read it back from your agent

```rust
let hits = mem.recall("language preferences")?;
for h in hits {
    println!("{}: {}", h.memory_type, h.content);
}
```

The `recall` call ranks records by relevance and respects the configured
backend's retrieval semantics.

## Step 4: Inspect from the shell

What works today:

```sh
amplihack memory tree --session $SESSION_ID
amplihack memory tree --type pattern --depth 2
```

What does **not** work today (do not paste these into your terminal —
they will fail with "unrecognized subcommand"):

```text
# Planned — not implemented
amplihack memory recall "language preferences"
amplihack memory list
amplihack memory delete <id>
```

If you need destructive cleanup, use the shipped command:

```sh
amplihack memory clean --pattern 'test_*' --no-dry-run --confirm
```

## Step 5: Move memory between machines

```sh
amplihack memory export --agent my-agent --output my-agent-memory.json
# … on the other machine …
amplihack memory import --agent my-agent --input my-agent-memory.json --merge
```

The transfer format is round-trip-safe; see
[Memory Backend Architecture](../concepts/memory-backend-architecture.md#the-transfer-layer).

## See also

- [Agent Memory Architecture](../concepts/agent-memory-architecture.md)
- [Memory Backend Architecture](../concepts/memory-backend-architecture.md)
- [Memory Backend Migration](./migrate-memory-backend.md)
- [Memory Extended API reference](../reference/memory-extended-api.md)
