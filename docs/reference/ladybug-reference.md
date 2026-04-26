# LadybugDB Reference

> Ported from upstream `rysweet/amplihack:docs/memory/LADYBUG_GRAPH_STORE.md`.
> Concepts and Cypher patterns preserved; install/import snippets and call
> sites adapted for the Rust `lbug` crate (the LadybugDB binding, formerly
> Kuzu) used by amplihack-rs in
> `crates/amplihack-cli/src/commands/memory/backend/graph_db/`.
>
> **See also:** [LadybugDB Code Graph](../concepts/kuzu-code-graph.md) ·
> [Memory Backend](./memory-backend.md) ·
> [Memory Backend Architecture](../concepts/memory-backend-architecture.md)

This page documents the **Cypher patterns** that amplihack-rs uses against the
LadybugDB graph store, together with the small, stable Rust API exposed by the
`lbug` crate. Unlike the upstream Python wrapper (`KuzuGraphStore`), the Rust
binding has no node/edge wrapper class: every operation is expressed as
parameterised Cypher executed through `Connection::prepare` plus
`Connection::execute`, or fired-and-forgotten through `Connection::query`.

## Add the dependency

Pin the `lbug` crate to the same version used in `crates/amplihack-cli/Cargo.toml`:

```toml
# Cargo.toml
[dependencies]
lbug = "0.15.3"
```

Or, from the command line:

```sh
cargo add lbug@0.15.3
```

## Import

Import `lbug` directly. amplihack-rs does **not** re-export the `lbug` types
on its public API — `GraphDbConnection`, `GraphDbDatabase`,
`GraphDbSystemConfig`, and `GraphDbValue` in
`amplihack_cli::commands::memory::backend::graph_db` are `pub(crate)` aliases
used only inside the `amplihack-cli` crate (see
[`graph_db/mod.rs`](https://github.com/rysweet/amplihack-rs/blob/main/crates/amplihack-cli/src/commands/memory/backend/graph_db/mod.rs)).

```rust
use lbug::{Connection, Database, SystemConfig, Value};
```

The functions amplihack-rs exposes for graph-backend callers are:

| Item                            | Visibility | Purpose                                           |
| ------------------------------- | ---------- | ------------------------------------------------- |
| `init_graph_backend_schema`     | `pub`      | Create the full node/relationship schema.         |
| `list_graph_sessions_from_conn` | `pub`      | List all `Session` rows as `SessionSummary`s.     |
| `graph_rows`                    | `pub`      | Run a parameterised Cypher and collect rows.      |

All other operations are written inline as Cypher and executed through `lbug`
directly.

## Open a database

Construct a `Database` with `SystemConfig::default()`, then a `Connection`:

```rust
use lbug::{Connection, Database, SystemConfig};
use std::path::Path;

let db = Database::new(Path::new("/data/graph"), SystemConfig::default())?;
let conn = Connection::new(&db)?;
```

This matches every call site in amplihack-rs (see
[`handle.rs`](https://github.com/rysweet/amplihack-rs/blob/main/crates/amplihack-cli/src/commands/memory/backend/graph_db/handle.rs)
`open_graph_db_at_path` and `connect_graph_db`).

`SystemConfig` ships with tuning defaults from the `lbug` crate. amplihack-rs
does not customise them; if you need to tune buffer-pool size, read-only mode,
or other parameters, consult the lbug 0.15.3 release for the supported builder
methods. Treat any tuning as crate-version-specific.

## Run Cypher

There are three call patterns in amplihack-rs, all routed through `lbug`:

### 1. Fire-and-forget DDL via `query`

Use `Connection::query` for statements with no parameters whose result you
discard (typical for `CREATE NODE TABLE` / `CREATE REL TABLE`):

```rust
conn.query(
    "CREATE NODE TABLE IF NOT EXISTS Session (
         session_id    STRING PRIMARY KEY,
         start_time    TIMESTAMP,
         status        STRING,
         metadata      STRING
     )",
)?;
```

### 2. Parameterised reads via `prepare` + `execute`

`Connection::execute` takes a **mutable** prepared statement; the result is
iterable and is materialised with `.collect()`:

```rust
let mut stmt = conn.prepare(
    "MATCH (s:Session {session_id: $session_id}) RETURN s.status",
)?;
let rows: Vec<Vec<Value>> = conn
    .execute(&mut stmt, vec![
        ("session_id", Value::String(session_id.clone())),
    ])?
    .collect();
```

amplihack-rs wraps this pattern in `graph_rows` (see
[`values.rs`](https://github.com/rysweet/amplihack-rs/blob/main/crates/amplihack-cli/src/commands/memory/backend/graph_db/values.rs)):

```rust
use amplihack_cli::commands::memory::backend::graph_db::{graph_rows, GraphDbValue};

let rows = graph_rows(
    &conn,
    "MATCH (s:Session {session_id: $session_id}) RETURN s.status",
    vec![("session_id", GraphDbValue::String(session_id.clone()))],
)?;
```

### 3. Parameterised writes via `prepare` + `execute`

Identical shape; the iterator is typically discarded:

```rust
let mut stmt = conn.prepare(
    "CREATE (s:Session {
         session_id: $session_id,
         start_time: $start_time,
         status:     $status,
         metadata:   $metadata
     })",
)?;
conn.execute(&mut stmt, vec![
    ("session_id", Value::String(session_id.clone())),
    ("start_time", Value::Timestamp(now)),
    ("status",     Value::String("active".to_string())),
    ("metadata",   Value::String("{}".to_string())),
])?;
```

> **Note.** `execute` requires `&mut stmt`. Passing `&stmt` will not compile.
> See `learning.rs:51,73,115,130` and `values.rs:14-15` for verified call
> sites.

## Schema

amplihack-rs defines its full schema in
[`schema.rs`](https://github.com/rysweet/amplihack-rs/blob/main/crates/amplihack-cli/src/commands/memory/backend/graph_db/schema.rs)
and applies it through:

```rust
use amplihack_cli::commands::memory::backend::graph_db::init_graph_backend_schema;

init_graph_backend_schema(&conn)?;
```

If you want to define your own node table, use `CREATE NODE TABLE IF NOT
EXISTS` with `STRING PRIMARY KEY`:

```rust
conn.query(
    "CREATE NODE TABLE IF NOT EXISTS Session (
         session_id    STRING PRIMARY KEY,
         start_time    TIMESTAMP,
         status        STRING,
         metadata      STRING
     )",
)?;
```

For a relationship table:

```rust
conn.query(
    "CREATE REL TABLE IF NOT EXISTS CONTRIBUTES_TO_SEMANTIC (
         FROM Session TO SemanticMemory,
         contribution_type STRING,
         timestamp         TIMESTAMP,
         delta             STRING
     )",
)?;
```

## Node CRUD (Cypher)

### Create

```rust
let mut stmt = conn.prepare(
    "CREATE (s:Session {session_id: $session_id, start_time: $start_time, status: $status})",
)?;
conn.execute(&mut stmt, vec![
    ("session_id", Value::String(session_id.clone())),
    ("start_time", Value::Timestamp(now)),
    ("status",     Value::String("active".to_string())),
])?;
```

### Read

```rust
let mut stmt = conn.prepare("MATCH (s:Session {session_id: $session_id}) RETURN s")?;
let rows: Vec<Vec<Value>> = conn
    .execute(&mut stmt, vec![("session_id", Value::String(session_id))])?
    .collect();
```

### Update

```rust
let mut stmt = conn.prepare(
    "MATCH (s:Session {session_id: $session_id}) SET s.status = $status",
)?;
conn.execute(&mut stmt, vec![
    ("session_id", Value::String(session_id)),
    ("status",     Value::String("done".to_string())),
])?;
```

### Delete

```rust
let mut stmt = conn.prepare("MATCH (s:Session {session_id: $session_id}) DELETE s")?;
conn.execute(&mut stmt, vec![("session_id", Value::String(session_id))])?;
```

## Querying

### Filtered reads

```rust
let mut stmt = conn.prepare(
    "MATCH (s:Session) WHERE s.status = $status RETURN s LIMIT $limit",
)?;
let rows: Vec<Vec<Value>> = conn
    .execute(&mut stmt, vec![
        ("status", Value::String("active".to_string())),
        ("limit",  Value::Int64(10)),
    ])?
    .collect();
```

amplihack-rs exposes a higher-level helper `list_graph_sessions_from_conn` for
the `Session` table specifically — see
[`queries.rs`](https://github.com/rysweet/amplihack-rs/blob/main/crates/amplihack-cli/src/commands/memory/backend/graph_db/queries.rs).

### Keyword search

amplihack-rs builds keyword search at the application layer: it splits search
text into up to 6 meaningful tokens (length ≥ 3, stop words removed) and
matches any token against the relevant `STRING` columns using `CONTAINS()`.

```rust
let mut stmt = conn.prepare(
    "MATCH (m:SemanticMemory)
     WHERE m.content CONTAINS $term OR m.concept CONTAINS $term
     RETURN m LIMIT $limit",
)?;
let rows: Vec<Vec<Value>> = conn
    .execute(&mut stmt, vec![
        ("term",  Value::String("authentication".to_string())),
        ("limit", Value::Int64(5)),
    ])?
    .collect();
```

## Edges (Cypher)

### Create

```rust
let mut stmt = conn.prepare(
    "MATCH (s:Session {session_id: $session_id}),
           (m:SemanticMemory {memory_id: $memory_id})
     CREATE (s)-[:CONTRIBUTES_TO_SEMANTIC {
         contribution_type: $contribution_type,
         timestamp:         $timestamp,
         delta:             $delta
     }]->(m)",
)?;
conn.execute(&mut stmt, vec![
    ("session_id",        Value::String(session_id)),
    ("memory_id",         Value::String(memory_id)),
    ("contribution_type", Value::String("created".to_string())),
    ("timestamp",         Value::Timestamp(now)),
    ("delta",             Value::String("initial_creation".to_string())),
])?;
```

### Traverse

Direction is controlled by the Cypher pattern: `(n)-[r]->()` (outgoing),
`()-[r]->(n)` (incoming), or `(n)-[r]-()` (both).

```rust
let mut stmt = conn.prepare(
    "MATCH (s:Session {session_id: $session_id})-[r]->(other) RETURN s, r, other",
)?;
let rows: Vec<Vec<Value>> = conn
    .execute(&mut stmt, vec![("session_id", Value::String(session_id))])?
    .collect();
```

### Delete

```rust
let mut stmt = conn.prepare(
    "MATCH (a {session_id: $from})-[r:CONTRIBUTES_TO_SEMANTIC]->(b {memory_id: $to}) DELETE r",
)?;
conn.execute(&mut stmt, vec![
    ("from", Value::String(from_id)),
    ("to",   Value::String(to_id)),
])?;
```

## Bulk Import / Export

amplihack-rs does not ship a bulk import/export wrapper at the Rust API
surface; the upstream Python `KuzuGraphStore` `export_nodes` /
`import_nodes` / `export_edges` / `import_edges` helpers are implemented in
the application layer using the same primitives:

```rust
// Export all nodes as (label, id, properties)
let mut stmt = conn.prepare(
    "MATCH (n) RETURN labels(n)[0], n.session_id, properties(n)",
)?;
let rows: Vec<Vec<Value>> = conn.execute(&mut stmt, vec![])?.collect();
```

```rust
// Idempotent re-import via MERGE
let mut stmt = conn.prepare(
    "MERGE (s:Session {session_id: $session_id}) SET s += $props",
)?;
for (id, props) in nodes {
    conn.execute(&mut stmt, vec![
        ("session_id", Value::String(id)),
        ("props",      props),
    ])?;
}
```

`MERGE` gives idempotent upsert semantics on the primary key, so re-importing
the same export is a no-op.

## Lifecycle

`Connection` and `Database` are released by `Drop`. In Rust this happens at
scope exit; for deterministic release, scope the `Database` in a block or
call `drop(db)` explicitly:

```rust
{
    let db = Database::new(path, SystemConfig::default())?;
    let conn = Connection::new(&db)?;
    // ... do work ...
} // db and conn are dropped here
```

amplihack-rs encapsulates this lifetime in `GraphDbHandle::with_conn`, which
opens a fresh `Connection` for the duration of the closure (see
[`handle.rs`](https://github.com/rysweet/amplihack-rs/blob/main/crates/amplihack-cli/src/commands/memory/backend/graph_db/handle.rs)).

## Concurrency

`GraphDbHandle` is `Send + Sync` and synchronises in-process access by
opening a `Connection` per call. Cross-process safety is delegated to
`lbug`/LadybugDB's own locking; consult the upstream LadybugDB documentation
for the lock-mode matrix and timeouts. amplihack-rs surfaces any failure to
acquire the lock as an `anyhow::Error`.

## Security

- All Cypher executed against user input uses `$param` binding via
  `Connection::prepare` + `Connection::execute`. There is no string
  interpolation of values.
- Identifier validation (table names, relationship types, property keys
  interpolated into Cypher) is the caller's responsibility. amplihack-rs
  validates against `^[a-zA-Z_][a-zA-Z0-9_]*$` before composing any
  identifier into a Cypher string.

## Environment variables and CLI flags

amplihack-rs currently honours the **legacy `KUZU_*` names** for the database
path. The `LADYBUG_*` rename is planned but not yet implemented in this repo.

| Variable / flag           | Status                | Purpose                                                                                  |
| ------------------------- | --------------------- | ---------------------------------------------------------------------------------------- |
| `AMPLIHACK_KUZU_DB_PATH`  | Current (legacy name) | Path to the LadybugDB graph database. Read in `graph_db/resolve.rs`.                     |
| `--kuzu-path` (CLI flag)  | Current (legacy name) | Override the database path on the CLI. Hidden flag; defined in `cli_commands.rs`.        |
| `--backend kuzu`          | Current               | Selects the graph-backed memory backend (`Backend::Kuzu`).                               |

Source: `crates/amplihack-cli/src/commands/memory/backend/graph_db/resolve.rs`
and `crates/amplihack-cli/src/cli_commands.rs`. New `LADYBUG_*` aliases will
be added when the CLI surface is renamed; this page will be updated at that
time.

## Related

- [LadybugDB Code Graph](../concepts/kuzu-code-graph.md)
- [Memory Backend Architecture](../concepts/memory-backend-architecture.md)
- [Memory Backend](./memory-backend.md)
- [Memory Tree](../concepts/memory-tree.md)
- [Five-Type Memory](../concepts/five-type-memory.md)
