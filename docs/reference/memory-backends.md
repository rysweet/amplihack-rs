# Memory Backends Reference

amplihack supports two storage backends for session memory. Both are accessed through
the same `amplihack memory` subcommands; the only difference is the `--backend` flag
and the build features required.

## Quick Comparison

| Property | SQLite (default) | Kuzu |
|----------|-----------------|------|
| Build dependency | None | cmake + C++ compiler |
| Install flag | *(none)* | `--features kuzu-backend` |
| Storage file | `~/.amplihack/memory.db` | `~/.amplihack/memory_kuzu.db` |
| Schema | Flat relational tables | Typed property graph |
| Memory types | Unified `memory_entries` table | 6 node types + 6 relationship types |
| Graph queries | No | Yes |
| Concurrent writers | Single-writer (SQLite WAL) | Single-writer (Kùzu lock) |
| Export formats | `json` | `json`, `kuzu` |

---

## Backend Values

Pass to `--backend` on any `memory` subcommand.

### `sqlite`

Default. Requires no additional build flags or system libraries.

```bash
amplihack memory tree --backend sqlite
amplihack memory tree            # sqlite is the default
```

### `kuzu`

Requires the binary to be built with `--features kuzu-backend`.
If the feature is absent, the command exits with an actionable error:

```
Error: The kuzu backend requires the kuzu-backend feature.
Reinstall with:
  cargo install --git https://github.com/rysweet/amplihack-rs amplihack \
    --locked --features kuzu-backend
```

---

## File Locations

| Backend | Path |
|---------|------|
| SQLite database | `$HOME/.amplihack/memory.db` |
| Kuzu database directory | `$HOME/.amplihack/memory_kuzu.db/` |

Both directories are created automatically on first use.

---

## SQLite Schema

Three tables form the SQLite memory store:

### `sessions`

| Column | Type | Description |
|--------|------|-------------|
| `session_id` | TEXT PK | Unique session identifier |
| `created_at` | TEXT | ISO-8601 timestamp |
| `last_accessed` | TEXT | ISO-8601 timestamp |
| `metadata` | TEXT | JSON blob |

### `session_agents`

| Column | Type | Description |
|--------|------|-------------|
| `session_id` | TEXT | FK → sessions |
| `agent_id` | TEXT | Agent identifier |
| `first_used` | TEXT | ISO-8601 timestamp |
| `last_used` | TEXT | ISO-8601 timestamp |

### `memory_entries`

| Column | Type | Description |
|--------|------|-------------|
| `id` | TEXT PK | Unique memory identifier |
| `session_id` | TEXT | FK → sessions |
| `agent_id` | TEXT | Originating agent |
| `memory_type` | TEXT | e.g. `episodic`, `semantic`, `working` |
| `title` | TEXT | Short summary |
| `content` | TEXT | Full memory content |
| `content_hash` | TEXT | Optional deduplication hash |
| `metadata` | TEXT | JSON blob |
| `tags` | TEXT | Comma-separated tags |
| `importance` | INTEGER | 1–10 priority signal |
| `created_at` | TEXT | ISO-8601 timestamp |
| `accessed_at` | TEXT | ISO-8601 timestamp |
| `expires_at` | TEXT | ISO-8601 or NULL (no expiry) |
| `parent_id` | TEXT | Parent memory id or NULL |

Expired entries (`expires_at < now()`) are excluded from all queries automatically.

---

## Kuzu Schema

*(Available only when built with `--features kuzu-backend`.)*

### Node Tables

#### `Session`

| Property | Type | Description |
|----------|------|-------------|
| `session_id` | STRING PK | Unique session identifier |
| `start_time` | TIMESTAMP | Session start |
| `end_time` | TIMESTAMP | Session end |
| `user_id` | STRING | User owning the session |
| `context` | STRING | Free-text context |
| `status` | STRING | `active` / `closed` |
| `created_at` | TIMESTAMP | |
| `last_accessed` | TIMESTAMP | |
| `metadata` | STRING | JSON blob |

#### `EpisodicMemory`

| Property | Type | Description |
|----------|------|-------------|
| `memory_id` | STRING PK | |
| `timestamp` | TIMESTAMP | When the event occurred |
| `content` | STRING | Event description |
| `event_type` | STRING | Categorisation |
| `emotional_valence` | DOUBLE | –1.0 … 1.0 |
| `importance_score` | DOUBLE | 0.0 … 1.0 |
| `title` | STRING | Short label |
| `metadata` | STRING | JSON blob |
| `tags` | STRING | Comma-separated |
| `created_at` / `accessed_at` / `expires_at` | TIMESTAMP | Lifecycle timestamps |

#### `SemanticMemory`

| Property | Type | Description |
|----------|------|-------------|
| `memory_id` | STRING PK | |
| `concept` | STRING | The extracted concept name |
| `content` | STRING | Full description |
| `category` | STRING | Domain category |
| `confidence_score` | DOUBLE | 0.0 … 1.0 |
| `last_updated` | TIMESTAMP | |
| `version` | INT64 | Incremented on update |
| `title` | STRING | Short label |
| `metadata` | STRING | JSON blob |
| `tags` | STRING | Comma-separated |
| `agent_id` | STRING | Originating agent |

#### `ProceduralMemory`

| Property | Type |
|----------|------|
| `memory_id` | STRING PK |
| `procedure_name` | STRING |
| `description` | STRING |
| `steps` | STRING (JSON array) |
| `preconditions` | STRING |
| `postconditions` | STRING |
| `success_rate` | DOUBLE |
| `usage_count` | INT64 |
| `last_used` | TIMESTAMP |

#### `ProspectiveMemory`

| Property | Type |
|----------|------|
| `memory_id` | STRING PK |
| `intention` | STRING |
| `trigger_condition` | STRING |
| `priority` | STRING |
| `due_date` | TIMESTAMP |
| `status` | STRING |
| `scope` | STRING |
| `completion_criteria` | STRING |

#### `WorkingMemory`

| Property | Type |
|----------|------|
| `memory_id` | STRING PK |
| `content` | STRING |
| `memory_type` | STRING |
| `priority` | INT64 |
| `ttl_seconds` | INT64 |

### Relationship Tables

| Relationship | From | To | Key Properties |
|-------------|------|----|----------------|
| `CONTAINS_EPISODIC` | Session | EpisodicMemory | `sequence_number INT64` |
| `CONTAINS_WORKING` | Session | WorkingMemory | `activation_level DOUBLE` |
| `CONTRIBUTES_TO_SEMANTIC` | Session | SemanticMemory | `contribution_type`, `timestamp`, `delta` |
| `USES_PROCEDURE` | Session | ProceduralMemory | `timestamp`, `success BOOL`, `notes` |
| `CREATES_INTENTION` | Session | ProspectiveMemory | `timestamp` |
| `SIMILAR_TO` | SemanticMemory | SemanticMemory | `weight DOUBLE` |
| `DERIVES_FROM` | SemanticMemory | EpisodicMemory | `extraction_method`, `confidence DOUBLE` |
| `SUPERSEDES` | SemanticMemory | SemanticMemory | `reason`, `temporal_delta` |
| `TRANSITIONED_TO` | SemanticMemory | SemanticMemory | `from_value`, `to_value`, `turn INT64`, `transition_type` |

---

## CLI Options

### `amplihack memory tree`

| Flag | Default | Description |
|------|---------|-------------|
| `--backend <sqlite\|kuzu>` | `sqlite` | Storage backend to query |
| `--session <id>` | *(all)* | Restrict output to one session |
| `--type <memory-type>` | *(all)* | Filter by memory type |

### `amplihack memory clean`

| Flag | Default | Description |
|------|---------|-------------|
| `<pattern>` | required | Glob pattern matched against session IDs |
| `--backend <sqlite\|kuzu>` | `sqlite` | Backend to clean |
| `--dry-run` | true | Preview without deleting |
| `--no-dry-run` | — | Actually delete matched sessions |
| `--yes` | — | Skip interactive confirmation |

### `amplihack memory export`

| Flag | Default | Description |
|------|---------|-------------|
| `--backend <sqlite\|kuzu>` | `sqlite` | Source backend |
| `--format <json\|kuzu>` | `json` | Output format (`kuzu` requires kuzu-backend) |
| `--session <id>` | *(all)* | Export a single session |

Output is written to stdout. Redirect to a file:

```bash
amplihack memory export --backend kuzu --format json > snapshot.json
```

### `amplihack memory import`

| Flag | Default | Description |
|------|---------|-------------|
| `<file>` | required | Path to a previously exported snapshot |
| `--backend <sqlite\|kuzu>` | `sqlite` | Destination backend |

Imports abort if the source file exceeds 512 MB or is a symbolic link.

---

## Export Formats

### `json`

Available for both backends. Produces a self-contained JSON document describing all
sessions and their memory entries. Compatible with both import backends.

### `kuzu`

Available only with `--features kuzu-backend`. Produces a richer JSON document that
preserves the full graph structure — node types, relationship edges, confidence scores,
and transition metadata. This format cannot be imported into the SQLite backend.

---

## Related

- [How to Use the Kuzu Graph Backend](../howto/kuzu-backend.md) — Step-by-step guide
  including installation, migration, and troubleshooting
- [README](../../README.md) — Installation quick-start for both build variants
