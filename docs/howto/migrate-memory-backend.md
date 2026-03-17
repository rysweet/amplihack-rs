# Migrate Memory to the SQLite Backend

This guide walks through switching amplihack-rs memory storage from the
graph-db (Kuzu) backend to SQLite, exporting existing hierarchical memory
to portable JSON, and verifying the migration succeeded.

**When to use this guide:**

- You are setting up amplihack on a new machine and want SQLite by default.
- You are moving from a machine with a Kuzu installation to one without.
- You want to back up hierarchical memory to a portable format.
- You want to verify that both backends contain the same data.

**Not this guide:** If you are moving from SQLite back to graph-db, follow
the steps in reverse — export from SQLite, set the backend to `graph-db`,
and import.

## Before you start

Confirm the current backend auto-detection result:

```sh
amplihack memory tree --backend sqlite 2>&1 | head -3
# 🧠 Memory Graph (Backend: sqlite)
# └── (empty - no memories found)
```

```sh
amplihack memory tree --backend graph-db 2>&1 | head -3
# 🧠 Memory Graph (Backend: graph-db)
# ├── 📅 Sessions (4)
```

If the graph-db backend shows sessions and the SQLite backend is empty,
migration is needed. If the SQLite backend already shows sessions, the flat
memory was already using SQLite and only hierarchical memory needs attention.

## Step 1 — Export hierarchical memory for each agent

List the agents that have hierarchical memory:

```sh
ls ~/.amplihack/hierarchical_memory/
# my-agent   analyzer   code-reviewer
```

Export each agent to a JSON file. JSON is the portable format; it works
across both backends and across machines.

```sh
amplihack memory export my-agent ~/memory-backup/my-agent.json --format json
# Exported memory for agent 'my-agent'
#   Format: json
#   Output: /home/alice/memory-backup/my-agent.json
#   Size: 142.3 KB
#   semantic_node_count: 87
#   episodic_node_count: 312
#   derives_from_edge_count: 291

amplihack memory export analyzer ~/memory-backup/analyzer.json --format json
amplihack memory export code-reviewer ~/memory-backup/code-reviewer.json --format json
```

Verify each export file is valid JSON:

```sh
jq '.statistics' ~/memory-backup/my-agent.json
# {
#   "semantic_node_count": 87,
#   "episodic_node_count": 312,
#   ...
# }
```

## Step 2 — Set the backend to SQLite

Set `AMPLIHACK_MEMORY_BACKEND=sqlite` in your shell profile so all future
sessions use SQLite:

```sh
# Add to ~/.bashrc or ~/.zshrc
echo 'export AMPLIHACK_MEMORY_BACKEND=sqlite' >> ~/.bashrc
source ~/.bashrc
```

For a temporary test without modifying the profile:

```sh
export AMPLIHACK_MEMORY_BACKEND=sqlite
```

Confirm auto-detection now selects SQLite:

```sh
amplihack memory tree 2>&1 | head -2
# 🧠 Memory Graph (Backend: sqlite)
# └── (empty - no memories found)
```

The flat memory store (`memory.db`) will be empty. Flat memory (session
learnings written during live sessions) is not migrated — it accumulates
as you use amplihack with the new backend.

## Step 3 — Import hierarchical memory into SQLite

Import each agent's JSON backup into the SQLite hierarchical store:

```sh
amplihack memory import my-agent ~/memory-backup/my-agent.json --format json
# Imported memory into agent 'my-agent'
#   Format: json
#   Source agent: my-agent
#   Merge mode: False
#   semantic_nodes_imported: 87
#   episodic_nodes_imported: 312
#   edges_imported: 291
#   skipped: 0
#   errors: 0

amplihack memory import analyzer ~/memory-backup/analyzer.json --format json
amplihack memory import code-reviewer ~/memory-backup/code-reviewer.json --format json
```

If any import shows a non-zero `errors` count, the failed nodes are skipped
but the import continues. Re-run with the same file; `merge = false`
(the default) clears and re-inserts, so the operation is idempotent.

## Step 4 — Verify the migration

Re-export from the SQLite backend and compare the counts:

```sh
amplihack memory export my-agent ~/memory-verify/my-agent-sqlite.json --format json
jq '.statistics' ~/memory-verify/my-agent-sqlite.json
# {
#   "semantic_node_count": 87,
#   "episodic_node_count": 312,
#   "derives_from_edge_count": 291,
#   ...
# }
```

The `semantic_node_count` and `episodic_node_count` should match the
original export. Edge counts should also match unless the source graph-db
contained edges with missing node references (which the JSON import skips).

## Step 5 — (Optional) Archive the Kuzu store

Once the migration is verified, archive or remove the Kuzu hierarchical
directories:

```sh
# Archive
tar czf ~/memory-backup/kuzu-hierarchical.tar.gz \
    ~/.amplihack/hierarchical_memory/*/kuzu_db \
    ~/.amplihack/hierarchical_memory/*/graph_db

# Or remove (irreversible)
rm -rf ~/.amplihack/hierarchical_memory/my-agent/graph_db
rm -rf ~/.amplihack/hierarchical_memory/my-agent/kuzu_db
```

The flat memory graph (`~/.amplihack/memory_graph.db`) can be left in place.
Auto-detection rule 3 will continue to find it and select graph-db unless
`AMPLIHACK_MEMORY_BACKEND=sqlite` is set in the environment.

## Troubleshooting

### `Error: HOME environment variable is not set`

`resolve_backend_with_autodetect()` requires `HOME` to locate the default
storage paths. Set it explicitly:

```sh
export HOME=/home/alice
amplihack memory tree
```

### `Invalid backend: postgres. Must be graph-db or sqlite`

`AMPLIHACK_MEMORY_BACKEND` contains an unrecognised value. Valid values are
`sqlite`, `graph-db`, and `kuzu`. Check for typos:

```sh
echo $AMPLIHACK_MEMORY_BACKEND
export AMPLIHACK_MEMORY_BACKEND=sqlite
```

### `Merge mode is not supported for raw-db format`

Use `--format json` for merge imports. Raw-db format replaces the entire
database and cannot merge:

```sh
# Wrong
amplihack memory import my-agent backup/ --format raw-db --merge

# Correct
amplihack memory import my-agent backup.json --format json --merge
```

### Import errors but nodes are present

The `errors` count in import output reflects individual nodes that could not
be inserted (for example, an edge whose source or target node was missing).
The remaining nodes are imported successfully. Check the `skipped` count: if
you ran with `--merge`, existing nodes are skipped — this is expected.

### graph-db backend still auto-selected after setting env var

Check that `AMPLIHACK_MEMORY_BACKEND` is exported, not just set:

```sh
# Not exported (invisible to subprocesses)
AMPLIHACK_MEMORY_BACKEND=sqlite

# Exported correctly
export AMPLIHACK_MEMORY_BACKEND=sqlite
```

---

## Related

- [Memory Backend Architecture](../concepts/memory-backend-architecture.md) — Auto-detection order and backend design
- [Memory Backend Reference](../reference/memory-backend.md) — All CLI flags, env vars, schema, and security
- [Environment Variables](../reference/environment-variables.md) — `AMPLIHACK_MEMORY_BACKEND` and related vars
