# Memory Backend Architecture

amplihack-rs stores agent memory in one of two interchangeable backends:
**SQLite** (the default for new installs) and **graph-db** (LadybugDB, formerly Kuzu; kept for
existing installs and code-graph-enriched recall). Both backends implement
the same three Rust traits, so every memory command works identically
regardless of which backend is active.

## Contents

- [Why two backends?](#why-two-backends)
- [Backend trait seams](#backend-trait-seams)
- [Backend selection](#backend-selection)
  - [Auto-detection order](#auto-detection-order)
  - [Explicit override](#explicit-override)
- [Storage layout](#storage-layout)
  - [SQLite paths](#sqlite-paths)
  - [Graph-db paths](#graph-db-paths)
- [Hierarchical memory vs. flat memory](#hierarchical-memory-vs-flat-memory)
- [The transfer layer](#the-transfer-layer)
- [Migration path forward](#migration-path-forward)
- [Related](#related)

---

## Why two backends?

The original memory subsystem used LadybugDB (formerly Kuzu) exclusively — a graph database that
delivers rich relationship queries and code-context enrichment, but requires
a native C++ shared library to link. This creates friction on
platforms where the library is unavailable and couples every memory operation
to the graph schema.

SQLite is universally available, embeds in the binary via `rusqlite`, and
stores memory as a simple relational schema. It is the right default for new
installs, CI environments, and contexts where graph-enriched recall is not
needed.

The two-backend design lets existing LadybugDB users continue without disruption
while routing new installs to SQLite automatically. A single environment
variable switches between them at any time.

---

## Backend trait seams

Three traits define the memory backend contract. All three are implemented
by both `SqliteBackend` and `GraphDbBackend` in `backend/sqlite.rs` and
`backend/graph_db.rs`.

### `MemoryTreeBackend`

Used by `amplihack memory tree`.

```
fn backend_name(&self) -> &'static str
fn load_session_rows(session_id, memory_type) -> Vec<(SessionSummary, Vec<MemoryRecord>)>
fn collect_agent_counts() -> Vec<(String, usize)>
```

### `MemorySessionBackend`

Used by `amplihack memory clean`.

```
fn list_sessions() -> Vec<SessionSummary>
fn delete_session(session_id) -> bool
```

### `MemoryRuntimeBackend`

Used by the hook dispatch path (session start/stop hooks).

```
fn load_prompt_context_memories(session_id) -> Vec<MemoryRecord>
fn store_session_learning(record) -> Option<String>
```

Factory functions in `backend/mod.rs` open the correct concrete type based
on `BackendChoice`:

```
open_tree_backend(BackendChoice) -> Box<dyn MemoryTreeBackend>
open_cleanup_backend(BackendChoice) -> Box<dyn MemorySessionBackend>
open_runtime_backend(BackendChoice) -> Box<dyn MemoryRuntimeBackend>
```

---

## Backend selection

### Auto-detection order

`resolve_backend_with_autodetect()` is called by `memory tree`, `memory
clean`, and the hook dispatch path when no explicit `--backend` flag is
passed. It applies the following rules in order, stopping at the first
match:

| Priority | Condition | Result |
|----------|-----------|--------|
| 1 | `AMPLIHACK_MEMORY_BACKEND` is set to a recognised value | That backend |
| 2 | `~/.amplihack/hierarchical_memory/<agent>/graph_db/` directory found (symlink-safe probe) | `graph-db` |
| 3 | None of the above | `sqlite` (default for new installs) |

The probe uses `symlink_metadata()`, not `Path::exists()`, so a dangling
symlink does **not** silently select the graph-db backend. If a symlink is
detected inside the probe directory, `resolve_backend_with_autodetect()`
returns `Err` for security — it refuses to follow the symlink.

If `HOME` is unavailable, `resolve_backend_with_autodetect()` returns `Err`
rather than guessing. The caller surfaces a clear error; there is no silent
fallback.

### Explicit override

Pass `--backend <value>` to any memory command to bypass auto-detection:

```sh
# Inspect the SQLite store regardless of installed LadybugDB
amplihack memory tree --backend sqlite

# Force graph-db for a one-off query
amplihack memory tree --backend graph-db

# Backward-compatible alias
amplihack memory tree --backend kuzu
```

Valid values for `--backend` and `AMPLIHACK_MEMORY_BACKEND`:

| Value | Maps to |
|-------|---------|
| `sqlite` | SQLite backend |
| `graph-db` | LadybugDB graph-db backend |
| `kuzu` | LadybugDB graph-db backend (backward-compatible alias) |

Any other value is rejected with a structured error; there is no silent
degradation.

---

## Storage layout

### SQLite paths

| Store | Path |
|-------|------|
| Flat memory (sessions, learnings) | `~/.amplihack/memory.db` |
| Hierarchical memory per agent | `~/.amplihack/hierarchical_memory/<agent_name>.db` |

`<agent>` is validated against a strict allowlist before any filesystem path
is constructed. Names containing path separators (`/`, `\`) or `..`
components are rejected to prevent directory traversal.

On Unix, `sqlite_hierarchical.db` and its parent directory are created with
`0o600` / `0o700` permissions (owner-only read/write). This is enforced on
every connection open, not just on creation.

### Graph-db paths

| Store | Path |
|-------|------|
| Memory graph | `~/.amplihack/memory_graph.db` (default) or `AMPLIHACK_GRAPH_DB_PATH` |
| Hierarchical memory per agent | `~/.amplihack/hierarchical_memory/<agent>/graph_db/` |
| Legacy hierarchical path | `~/.amplihack/hierarchical_memory/<agent>/kuzu_db/` (read-only fallback) |

`resolve_hierarchical_db_path()` prefers `graph_db/` over `kuzu_db/` when
both exist, and falls back to the legacy directory for read-only access on
existing installs.

---

## Hierarchical memory vs. flat memory

The memory subsystem has two distinct storage tiers:

**Flat memory** (`memory.db` / `memory_graph.db`)
Stores session learnings, context memories, and agent notes. Used by the
runtime hooks (`store_session_learning`, `load_prompt_context_memories`) and
visualised by `memory tree`.

**Hierarchical memory** (`hierarchical_memory/<agent>/`)
A richer graph of semantic and episodic nodes connected by typed edges
(`DERIVES_FROM`, `SIMILAR_TO`, `SUPERSEDES`, `TRANSITIONED_TO`). Used by
the `memory export` and `memory import` commands. The graph schema has 6
tables (2 node types, 4 edge types) and 14 covering indexes.

Both tiers support both backends. The SQLite hierarchical backend writes a
single file (`sqlite_hierarchical.db`); the graph-db hierarchical backend
writes a LadybugDB directory (`graph_db/`). The `memory export` / `memory import`
commands use a portable JSON format that works across both backends, enabling
migration without losing node/edge structure.

---

## The transfer layer

`memory export` and `memory import` use `HierarchicalTransferBackend`, a
trait with four operations:

```
export_hierarchical_json(agent, output, storage_path) -> ExportResult
import_hierarchical_json(agent, input, merge, storage_path) -> ImportResult
export_hierarchical_raw_db(agent, output, storage_path) -> ExportResult
import_hierarchical_raw_db(agent, input, merge, storage_path) -> ImportResult
```

`open_hierarchical_transfer_backend_for(BackendChoice)` dispatches to either
`SqliteHierarchicalTransferBackend` or `GraphDbHierarchicalTransferBackend`.
`resolve_transfer_backend_choice()` reads `AMPLIHACK_MEMORY_BACKEND` (with
allowlist validation) to select the backend; it warns on unrecognised values
and defaults to graph-db for backward compatibility.

See [Memory Export and Import Reference](../reference/memory-backend.md) for
the complete format specification and security properties.

---

## Migration path forward

The remaining blocker for full SQLite migration is **LadybugDB parity**: the
code-graph query and code-context enrichment features that the LadybugDB backend
provides via `enrich_prompt_context_memories_with_code_context`. Until
LadybugDB provides an equivalent, the graph-db backend is required for
code-context-enriched recall.

New installs and CI environments that do not use code-context enrichment
should prefer the SQLite backend. Existing installs with a populated
`memory_graph.db` continue to use graph-db automatically via
auto-detection rule 3.

---

## Related

- [Memory Backend Reference](../reference/memory-backend.md) — `BackendChoice` values, env vars, schema, security
- [How to Migrate Memory to SQLite](../howto/migrate-memory-backend.md) — Step-by-step migration guide
- [Environment Variables](../reference/environment-variables.md) — `AMPLIHACK_MEMORY_BACKEND` and related vars
- [LadybugDB Code Graph](./kuzu-code-graph.md) — Architecture of the graph-db code graph
