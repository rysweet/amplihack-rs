# How to Use the Kuzu Graph Backend

amplihack stores session memory in SQLite by default. If you need hierarchical,
graph-structured memory — semantic nodes, episodic events, and typed relationships
between them — you can rebuild the binary with the `kuzu-backend` feature.

## Prerequisites

| Requirement | Version |
|-------------|---------|
| Rust toolchain | 1.77+ |
| cmake | 3.20+ |
| C++ compiler (gcc / clang / MSVC) | Any modern version |

Install cmake on common platforms:

```bash
# Debian / Ubuntu
sudo apt install cmake build-essential

# macOS (Homebrew)
brew install cmake

# Fedora / RHEL
sudo dnf install cmake gcc-c++
```

## Install with Kuzu Support

```bash
# Install directly from GitHub
cargo install \
  --git https://github.com/rysweet/amplihack-rs \
  amplihack \
  --locked \
  --features kuzu-backend

# Or build locally from a checkout
cargo build --release --features kuzu-backend
```

The default install (without `--features kuzu-backend`) builds against SQLite only and
requires **no system dependencies**.

## Use the Kuzu Backend

Pass `--backend kuzu` to any `memory` subcommand:

```bash
# View all sessions stored in the Kuzu graph
amplihack memory tree --backend kuzu

# View a specific session
amplihack memory tree --backend kuzu --session my-session-id

# Clean sessions matching a pattern
amplihack memory clean "session-*" --backend kuzu

# Export a graph snapshot to JSON
amplihack memory export --backend kuzu --format json > snapshot.json

# Import a previously exported snapshot
amplihack memory import --backend kuzu snapshot.json
```

The Kuzu database lives at `~/.amplihack/memory_kuzu.db`. It is separate from the
SQLite database at `~/.amplihack/memory.db`.

## What Changes with Kuzu

The Kuzu backend stores memory across six typed node tables:

| Node type | What it captures |
|-----------|-----------------|
| `EpisodicMemory` | Timestamped events from a session |
| `SemanticMemory` | Extracted concepts with confidence scores |
| `ProceduralMemory` | Step-by-step procedures and their success rates |
| `ProspectiveMemory` | Intentions and future triggers |
| `WorkingMemory` | Transient, short-lived context |
| `Session` | Container linking all memory types for one session |

Relationships between nodes (`SIMILAR_TO`, `DERIVES_FROM`, `SUPERSEDES`,
`TRANSITIONED_TO`) let you query *how* memories are connected, not just *what* they
contain. SQLite stores all memory in a single flat table without these connections.

## Migrate from SQLite

Export all SQLite data to JSON, then import into Kuzu:

```bash
# 1. Export every SQLite session to a JSON file
amplihack memory export --backend sqlite --format json > all-sessions.json

# 2. Inspect the snapshot (optional)
wc -l all-sessions.json

# 3. Import into Kuzu
amplihack memory import --backend kuzu all-sessions.json
```

Each session is imported as a `Session` node. Episodic and semantic memories are
inferred from the flat SQLite records during import.

## Troubleshoot

### Error: "The kuzu backend requires the kuzu-backend feature"

The binary was installed without Kuzu support. Reinstall with the feature flag:

```bash
cargo install \
  --git https://github.com/rysweet/amplihack-rs \
  amplihack \
  --locked \
  --features kuzu-backend
```

### cmake not found during build

Install cmake for your platform (see [Prerequisites](#prerequisites) above) and retry
the build.

### Database locked

Only one process can hold the Kuzu write lock at a time. If a previous command
crashed, delete the lock file:

```bash
rm ~/.amplihack/memory_kuzu.db/.lock
```

## Related

- [Memory Backends Reference](../reference/memory-backends.md) — Complete option table,
  file locations, and schema details
- [README](../../README.md) — Installation overview and both install variants
