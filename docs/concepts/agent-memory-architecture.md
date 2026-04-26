# Agent Memory Architecture

> **Status: Mostly shipped.** The storage layer (SQLite + LadybugDB
> backends, transfer format, manager/facade) and the `amplihack memory`
> CLI subcommands `tree`, `export`, `import`, and `clean` are implemented.
> Higher-level *recall* APIs (`Memory::remember` / `Memory::recall`)
> exist as a library facade in `amplihack-memory`. There is **no**
> `amplihack memory recall|list|delete` CLI; do not assume one.

This page is an overlay on
[Memory Backend Architecture](./memory-backend-architecture.md). Read that
first — it covers the backend trait seams, storage layout, and migration
story. This page focuses on the **agent-facing** view: how an agent stores
and retrieves its own memory, and what scoping rules apply.

## Layered view

```
┌────────────────────────────────────────────────┐
│ Agent code                                      │
│   Memory::remember / Memory::recall (facade)    │
├────────────────────────────────────────────────┤
│ MemoryManager (session-aware)                   │
├────────────────────────────────────────────────┤
│ MemoryRuntimeBackend / MemorySessionBackend /   │
│ MemoryTreeBackend  (the three trait seams)      │
├────────────────────────────────────────────────┤
│ SqliteBackend          │ GraphDbBackend         │
│ (default for new installs) │ (LadybugDB)         │
└────────────────────────────────────────────────┘
```

The CLI surface (`amplihack memory tree | export | import | clean`)
attaches to the trait layer. The agent-facing facade attaches to
`MemoryManager`.

## Scoping

What is shared, and where, depends on the layer:

| Scope               | Shared across              | Mechanism                                       |
|---------------------|----------------------------|-------------------------------------------------|
| Per-agent memory    | One named agent's process(es) | `--agent <name>` for export/import; storage path keyed off agent name |
| Per-session memory  | One CLI session            | `MemoryRecord.session_id`                       |
| Per-repo runtime    | All worktrees of a repo    | `worktree::get_shared_runtime_dir`              |
| Per-user            | All repos for one user     | `~/.amplihack/` (default `memory_home_paths`)   |

Note: there is **no** automatic cross-repo memory sharing. If you need it,
use `amplihack memory export` then `amplihack memory import`.

## Today vs. Planned

| Capability                                | Today        | Planned                                    |
|-------------------------------------------|--------------|--------------------------------------------|
| `amplihack memory tree/export/import/clean` | ✅ shipped | unchanged                                   |
| `Memory::remember` / `Memory::recall` library facade | ✅ shipped | unchanged                          |
| Hook-based prompt-context injection       | ✅ shipped   | unchanged                                   |
| `amplihack memory recall <q>` CLI          | ❌ not present | proposed but not yet specified            |
| `amplihack memory list` / `delete` CLI     | ❌ not present | use `memory tree` / `memory clean` instead |
| `recall_memory` tool exposed to agents     | ❌ not present | being scoped                                |

For the actual command flags, run `amplihack memory --help` or read
`crates/amplihack-cli/src/cli_subcommands.rs`.

## See also

- [Memory Backend Architecture](./memory-backend-architecture.md)
- [Memory Backend Migration](./memory-backend-migration.md)
- [Integrate Agent Memory](../howto/integrate-agent-memory.md)
- [Memory Extended API reference](../reference/memory-extended-api.md)
- [Memory Backend reference](../reference/memory-backend.md)
