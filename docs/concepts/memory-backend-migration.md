# Memory Backend Migration

Describes how `amplihack-rs` supports migrating agent memory between
SQLite and Graph-DB (Kùzu) backends, and the env-var override contract
that controls which backend is active.

## Backend Selection

```mermaid
flowchart TD
    A([Process start]) --> B{AMPLIHACK_GRAPH_DB_PATH\nor AMPLIHACK_KUZU_DB_PATH set?}
    B -- primary env var\nAMPLIHACK_GRAPH_DB_PATH --> C[Validate path\n• absolute\n• no ..\n• not /proc /sys /dev]
    B -- legacy env var\nAMPLIHACK_KUZU_DB_PATH --> D[Emit deprecation\nwarning → validate path]
    B -- neither set --> E[Default: ~/.amplihack/\nmemory_kuzu.db]
    C --> F{Valid?}
    D --> F
    E --> G[Open Graph-DB]
    F -- yes --> G
    F -- no --> ERR([Error: invalid override path])
    G --> H{Backend preference\nresolved}
    H -- graph-db --> I[GraphDbBackend\nKùzu embedded]
    H -- sqlite --> J[SQLiteBackend\nfor transfer operations]
```

## Migration Flow (SQLite → Graph-DB)

```mermaid
sequenceDiagram
    participant CLI as amplihack memory transfer
    participant SQLite as SQLiteBackend
    participant GraphDb as GraphDbBackend
    participant FS as Filesystem

    CLI->>SQLite: open source DB (read-only)
    CLI->>GraphDb: open target DB
    CLI->>SQLite: list_sessions()
    loop For each session
        CLI->>SQLite: load_session_rows(session_id)
        SQLite-->>CLI: Vec<MemoryRecord>
        CLI->>GraphDb: store_session_learning(records)
        GraphDb-->>FS: persist to graph nodes
        CLI->>CLI: report progress
    end
    CLI->>CLI: emit migration summary
```

## Environment Variable Contract

| Variable | Status | Purpose |
|---|---|---|
| `AMPLIHACK_GRAPH_DB_PATH` | **Primary** (preferred) | Backend-neutral override; use this |
| `AMPLIHACK_KUZU_DB_PATH` | **Deprecated** | Legacy Kùzu-specific name; emits warning |

Both variables are validated with identical security rules before use.

## Related Concepts

- [Recipe Execution Flow](recipe-execution-flow.md)
- [Kùzu Code Graph](kuzu-code-graph.md)
- [Memory Backend Architecture](memory-backend-architecture.md)
