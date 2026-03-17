# Memory Backend Reference

Complete reference for the amplihack-rs memory backend subsystem: backend
names, environment variables, storage paths, data schema, transfer format,
and security properties.

## Contents

- [BackendChoice values](#backendchoice-values)
- [Environment variables](#environment-variables)
  - [AMPLIHACK_MEMORY_BACKEND](#amplihack_memory_backend)
- [CLI flags](#cli-flags)
- [Storage paths](#storage-paths)
  - [SQLite backend](#sqlite-backend)
  - [Graph-db backend](#graph-db-backend)
- [Flat memory schema](#flat-memory-schema)
- [Hierarchical memory schema](#hierarchical-memory-schema)
- [Transfer formats](#transfer-formats)
  - [JSON format (HierarchicalExportData)](#json-format-hierarchicalexportdata)
  - [Raw-db format](#raw-db-format)
- [Import and export behaviour](#import-and-export-behaviour)
- [Security properties](#security-properties)
- [Related](#related)

---

## BackendChoice values

`BackendChoice` is the internal enum that selects which storage engine to
use. All CLI flags, environment variables, and internal APIs accept the same
string tokens.

| Token | Backend | Notes |
|-------|---------|-------|
| `sqlite` | SQLite (`~/.amplihack/memory.db`) | Default for new installs |
| `graph-db` | Kuzu graph database | Default when an existing Kuzu DB is detected |
| `kuzu` | Kuzu graph database | Legacy alias for `graph-db`; deprecated for new automation |

Any value outside this allowlist is rejected with a structured error. Casing
is exact: `KUZU` and `Graph-DB` are invalid.

---

## Environment variables

### AMPLIHACK_MEMORY_BACKEND

**Type:** string
**Values:** `sqlite` | `graph-db` | `kuzu`
**Read by:** `resolve_memory_backend_preference()`, `resolve_transfer_backend_choice()`, `resolve_backend_with_autodetect()`
**Propagated to child processes:** No — read only at the point of backend selection

Selects the memory backend for all memory commands in the current process.
When unset, `resolve_backend_with_autodetect()` selects the backend by
probing the filesystem (see [Auto-detection order](../concepts/memory-backend-architecture.md#auto-detection-order)).

```sh
# Use SQLite for all memory operations
export AMPLIHACK_MEMORY_BACKEND=sqlite

# Force graph-db for a single invocation
AMPLIHACK_MEMORY_BACKEND=graph-db amplihack memory tree

# Legacy alias still accepted (maps to graph-db)
AMPLIHACK_MEMORY_BACKEND=kuzu amplihack memory tree
```

For the transfer commands (`memory export` / `memory import`), the backend
governs which hierarchical store is written to or read from. An unrecognised
value produces a visible warning to stderr before the command falls back to
graph-db — it does **not** silently accept the string.

**Interaction with `--backend` flag:** The CLI `--backend` flag takes
priority over this environment variable for `memory tree` and `memory clean`.
For `memory export` / `memory import` there is no `--backend` flag; the env
var is the only override mechanism.

---

## CLI flags

### `--backend <value>`

Accepted by: `amplihack memory tree`, `amplihack memory clean`

Bypasses auto-detection and opens the named backend directly. Valid values:
`sqlite`, `graph-db`, `kuzu`.

```sh
# View SQLite flat memory tree
amplihack memory tree --backend sqlite

# View graph-db flat memory tree
amplihack memory tree --backend graph-db

# Clean sessions from the SQLite store matching a glob
amplihack memory clean "test_*" --backend sqlite --dry-run
```

### `--storage-path <path>`

Accepted by: `amplihack memory export`, `amplihack memory import`

Overrides the default hierarchical memory root directory for a single
invocation. Useful for operating on a non-standard or temporary store.

```sh
# Export from a custom storage location
amplihack memory export my-agent export.json \
    --format json \
    --storage-path /tmp/test-memory/my-agent
```

---

## Storage paths

### SQLite backend

| File | Purpose | Default path |
|------|---------|--------------|
| `memory.db` | Sessions, learnings, context memories | `~/.amplihack/memory.db` |
| `sqlite_hierarchical.db` | Hierarchical semantic/episodic graph per agent | `~/.amplihack/hierarchical_memory/<agent>/sqlite_hierarchical.db` |

Both files are created automatically on first use. Parent directories are
created with `fs::create_dir_all`.

### Graph-db backend

| Directory / File | Purpose | Default path |
|-----------------|---------|--------------|
| `memory_graph.db` | Sessions, learnings, context memories | `~/.amplihack/memory_graph.db` |
| `graph_db/` | Hierarchical semantic/episodic graph per agent | `~/.amplihack/hierarchical_memory/<agent>/graph_db/` |
| `kuzu_db/` | Legacy hierarchical path (read-only fallback) | `~/.amplihack/hierarchical_memory/<agent>/kuzu_db/` |

The `AMPLIHACK_GRAPH_DB_PATH` environment variable overrides the
`memory_graph.db` path. `AMPLIHACK_KUZU_DB_PATH` is accepted as a legacy
alias (see [Environment Variables](./environment-variables.md)).

`resolve_hierarchical_db_path()` prefers `graph_db/` over `kuzu_db/` when
both exist, enabling incremental migration without deleting the legacy
directory.

---

## Flat memory schema

The SQLite flat memory store uses three tables. The graph-db backend uses an
equivalent Kuzu node schema.

```sql
CREATE TABLE IF NOT EXISTS memory_entries (
    id           TEXT PRIMARY KEY,
    session_id   TEXT NOT NULL,
    agent_id     TEXT NOT NULL,
    memory_type  TEXT NOT NULL,         -- 'learning', 'conversation', 'context', …
    title        TEXT NOT NULL,
    content      TEXT NOT NULL,
    content_hash TEXT,
    metadata     TEXT NOT NULL DEFAULT '{}',  -- JSON object
    tags         TEXT DEFAULT NULL,           -- JSON array
    importance   INTEGER DEFAULT NULL,        -- 0–10
    created_at   TEXT NOT NULL,
    accessed_at  TEXT NOT NULL,
    expires_at   TEXT DEFAULT NULL,           -- NULL = never expires
    parent_id    TEXT DEFAULT NULL
);

CREATE TABLE IF NOT EXISTS sessions (
    session_id    TEXT PRIMARY KEY,
    created_at    TEXT NOT NULL,
    last_accessed TEXT NOT NULL,
    metadata      TEXT NOT NULL DEFAULT '{}'
);

CREATE TABLE IF NOT EXISTS session_agents (
    session_id TEXT NOT NULL,
    agent_id   TEXT NOT NULL,
    first_used TEXT NOT NULL,
    last_used  TEXT NOT NULL,
    PRIMARY KEY (session_id, agent_id)
);
```

**Expiration:** Records with `expires_at` in the past are excluded from all
query results by both backends. `collect_agent_counts()` also excludes
expired records from the per-agent totals.

**Deduplication:** `store_session_learning()` returns `None` (without
error) when an identical `(session_id, agent_id, content)` triple already
exists. This is intentional duplicate-suppression, not a failure.

---

## Hierarchical memory schema

The hierarchical store contains 2 node tables and 4 directed edge tables.
All tables are created with `IF NOT EXISTS`, so `init_hierarchical_schema()`
is safe to call on every connection open.

### Node tables

**SemanticMemory** — long-term conceptual knowledge:

| Column | Type | Notes |
|--------|------|-------|
| `memory_id` | TEXT PK | Stable identifier |
| `concept` | TEXT | Short concept label |
| `content` | TEXT | Full knowledge content |
| `confidence` | REAL | 0.0–1.0 |
| `source_id` | TEXT | Originating episodic `memory_id` |
| `agent_id` | TEXT | Owning agent — partition key |
| `tags` | TEXT | JSON array |
| `metadata` | TEXT | JSON object |
| `created_at` | TEXT | ISO-8601 or Unix seconds |
| `entity_name` | TEXT | Named entity this concept describes |

**EpisodicMemory** — time-stamped events:

| Column | Type | Notes |
|--------|------|-------|
| `memory_id` | TEXT PK | Stable identifier |
| `content` | TEXT | Event description |
| `source_label` | TEXT | Origin label (`session`, `tool_call`, …) |
| `agent_id` | TEXT | Owning agent — partition key |
| `tags` | TEXT | JSON array |
| `metadata` | TEXT | JSON object |
| `created_at` | TEXT | ISO-8601 or Unix seconds |

### Edge tables

| Table | Direction | Key columns |
|-------|-----------|-------------|
| `SIMILAR_TO` | SemanticMemory → SemanticMemory | `weight` (0.0–1.0), `metadata` |
| `DERIVES_FROM` | SemanticMemory → EpisodicMemory | `extraction_method`, `confidence` |
| `SUPERSEDES` | SemanticMemory → SemanticMemory | `reason`, `temporal_delta` |
| `TRANSITIONED_TO` | SemanticMemory → SemanticMemory | `from_value`, `to_value`, `turn`, `transition_type` |

All four edge tables include 14 covering indexes to support agent-scoped
queries without full-table scans.

---

## Transfer formats

### JSON format (HierarchicalExportData)

`--format json` serialises the entire per-agent graph to a single JSON file.
Format version: `1.1`. The JSON schema mirrors the internal Rust structs:

```json
{
  "agent_name": "my-agent",
  "exported_at": "1741872000",
  "format_version": "1.1",
  "semantic_nodes": [
    {
      "memory_id": "sem-001",
      "concept": "backend-seam",
      "content": "Use trait objects to decouple storage from query logic.",
      "confidence": 0.95,
      "source_id": "ep-001",
      "tags": ["architecture", "memory"],
      "metadata": {"origin": "code-review"},
      "created_at": "1741872000",
      "entity_name": "amplihack-rs"
    }
  ],
  "episodic_nodes": [
    {
      "memory_id": "ep-001",
      "content": "Reviewed memory backend PR #91.",
      "source_label": "session",
      "tags": ["session"],
      "metadata": {"turn": 5},
      "created_at": "1741872000"
    }
  ],
  "similar_to_edges": [],
  "derives_from_edges": [
    {
      "source_id": "sem-001",
      "target_id": "ep-001",
      "extraction_method": "llm-extraction",
      "confidence": 0.87
    }
  ],
  "supersedes_edges": [],
  "transitioned_to_edges": [],
  "statistics": {
    "semantic_node_count": 1,
    "episodic_node_count": 1,
    "similar_to_edge_count": 0,
    "derives_from_edge_count": 1,
    "supersedes_edge_count": 0,
    "transitioned_to_edge_count": 0
  }
}
```

JSON exports are **portable between backends**: a file exported from
graph-db can be imported into SQLite and vice versa.

### Raw-db format

`--format raw-db` copies the raw database files (a single SQLite file for
the SQLite backend; a Kuzu directory tree for the graph-db backend) to the
destination path.

**Constraints:**

- `--merge` is not supported for raw-db imports. The command fails with a
  clear error if `--merge` is combined with `--format raw-db`. Use JSON
  format for merge imports.
- The existing database is renamed to a `.bak` sibling before the copy.
  If the backup already exists, it is removed first.
- Symlinks in the source tree are skipped with a warning. Symlinked source
  paths are rejected before the copy begins.

The `kuzu` format alias (`--format kuzu`) maps to `raw-db` for backward
compatibility with existing scripts.

---

## Import and export behaviour

### Export

1. Resolves the hierarchical DB path for the agent.
2. For JSON: queries all 6 tables with `agent_id = ?` binding; writes to a
   `.tmp` file in the same directory; renames atomically to the target path.
3. For raw-db: uses `symlink_metadata()` to reject symlinked sources; copies
   the file or directory tree; skips any symlinks in the tree.
4. Returns `ExportResult` with agent name, format, output path, file size,
   and per-table node/edge counts.

### Import

1. For JSON: validates file size against the 500 MB cap before
   deserialisation to prevent OOM from adversarially crafted payloads.
2. Deserialises into `HierarchicalExportData`; uses the **caller-supplied
   `agent_name`** as the partition key for all writes — the `agent_name`
   field inside the JSON is informational only and is never used as a WHERE
   clause value.
3. Opens (or creates) the agent's hierarchical DB.
4. If `merge = false` (default): runs `BEGIN IMMEDIATE` transaction, deletes
   all existing data for the agent, then inserts. This is atomic — a crash
   mid-import leaves the store empty, not partially overwritten.
5. If `merge = true`: collects existing `memory_id` values and skips
   duplicates. No deletions occur.
6. Returns `ImportResult` with per-table import counts, skipped count, and
   error count.

---

## Security properties

| Property | Implementation |
|----------|----------------|
| Path traversal prevention | `validate_agent_name()` rejects names containing `/`, `\`, or `..` before any `PathBuf` construction |
| File permissions (Unix) | `sqlite_hierarchical.db` is created with `0o600`; parent directory with `0o700`; enforced on every connection open |
| Transaction safety | `merge = false` imports use `BEGIN IMMEDIATE` to prevent partial-delete / no-insert state on process crash |
| Agent isolation | `agent_name` from CLI is the sole partition key; JSON payload's `agent_name` field is ignored during import writes |
| SQL injection | All queries use `params!` macro exclusively; zero string interpolation; JSON fields serialised with `serde_json::to_string()` before binding |
| Import payload size | JSON files larger than 500 MB are rejected before `serde_json::from_str()` |
| Env var allowlist | `AMPLIHACK_MEMORY_BACKEND` values outside `['sqlite', 'kuzu', 'graph-db']` produce a visible warning to stderr before fallback |
| Symlink protection | Raw-db export uses `symlink_metadata()` to detect and reject symlinked source paths; symlinks inside copied trees are skipped |
| Atomic writes | JSON export writes to a `.tmp` file in the destination directory, then renames — prevents partially-written exports from appearing valid |
| Probe safety | `resolve_backend_with_autodetect()` uses `symlink_metadata().is_ok()` for filesystem probes; returns `Err` when `HOME` is unavailable |

---

## Related

- [Memory Backend Architecture](../concepts/memory-backend-architecture.md) — Design rationale, trait seams, auto-detection
- [How to Migrate Memory to SQLite](../howto/migrate-memory-backend.md) — Step-by-step migration guide
- [Environment Variables](./environment-variables.md) — `AMPLIHACK_MEMORY_BACKEND`, `AMPLIHACK_GRAPH_DB_PATH`, and related vars
- [Kuzu Code Graph](../concepts/kuzu-code-graph.md) — Graph-db code graph architecture
