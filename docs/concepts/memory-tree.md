# Memory Tree

> Ported from upstream `rysweet/amplihack:docs/memory/MEMORY_TREE_VISUALIZATION.md`.
> Concepts preserved verbatim; CLI invocations are unchanged because
> `amplihack memory tree` is a CLI surface shared between the Python and Rust
> implementations.
>
> **See also:** [Memory Backend Architecture](./memory-backend-architecture.md) ·
> [Agent Memory Quickstart](../howto/agent-memory-quickstart.md) ·
> [Memory Backend Reference](../reference/memory-backend.md)

This page documents the current `amplihack memory tree` command.

## Overview

`amplihack memory tree` renders the repo's top-level session memory graph
using the amplihack memory database — a SQLite store at
`~/.amplihack/memory.db` by default, or a LadybugDB graph store when the
graph backend is selected (see
[Memory Backend Architecture](./memory-backend-architecture.md)).

It is a graph view over the top-level CLI/session memory store. It is **not**
a live view into a generated goal-agent's local `./memory/` directory.

## Mental Model

The memory tree is a hierarchy with three levels:

1. **Root** — the memory database itself (one per `~/.amplihack/` install,
   plus per-bundle databases when `amplihack new --enable-memory` is used).
2. **Sessions** — each top-level session (CLI run, recipe execution, or
   long-lived agent loop) is a node under the root.
3. **Memory items** — individual entries (learnings, decisions, patterns,
   conversations, contexts, artifacts) attached to a session.

Edges between memory items express provenance and relationships (for example,
a `learning` derived from a `pattern`, or an `artifact` produced by a
`decision`). The tree view collapses these edges into a parent/child view
suitable for human inspection.

## Usage

### Basic command

```sh
amplihack memory tree
```

### Filter by session

```sh
amplihack memory tree --session Session-2026-01-12
```

### Filter by memory-item type

```sh
amplihack memory tree --type learning
amplihack memory tree --type pattern
```

The current parser accepts these legacy type names:

- `conversation`
- `decision`
- `pattern`
- `context`
- `learning`
- `artifact`

### Limit depth

```sh
amplihack memory tree --depth 3
```

## Supported Options

```sh
amplihack memory tree [--session SESSION] [--type TYPE] [--depth N]
```

There is no supported top-level `--backend` flag on `memory tree` in the
current parser. The backend is selected by the same environment variables and
config that drive every other `amplihack memory` subcommand — see
[Memory Backend Reference](../reference/memory-backend.md).

## Output Shape

The command renders a tree of the current session graph, grouped by session
and memory item metadata. The exact styling may change, but the current
command is intended for human inspection rather than machine parsing. For
machine-readable output, use `amplihack memory export`.

## Related Commands

- `amplihack memory export` / `amplihack memory import` — agent-local
  hierarchical memory transfer.
- `amplihack new --enable-memory` — generated agent scaffolds with a local
  `./memory/` directory.

## Related Docs

- [Agent Memory Quickstart](../howto/agent-memory-quickstart.md)
- [Memory Backend Reference](../reference/memory-backend.md)
- [Memory Backend Architecture](./memory-backend-architecture.md)
- [LadybugDB Reference](../reference/ladybug-reference.md)
- [Five-Type Memory](./five-type-memory.md)
