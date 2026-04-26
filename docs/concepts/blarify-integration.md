# Blarify Integration

> **Status: Shipped (consumer side).** amplihack-rs **consumes** blarify's
> JSON output via the `amplihack index-code` CLI; it does **not** embed or
> re-implement blarify itself. Generation of blarify JSON still runs as a
> separate (Python) upstream tool. See
> [LadybugDB Code Graph](./kuzu-code-graph.md) for the broader code-graph
> story.

## What blarify is, in this project

Blarify is an external code-analysis tool that emits a JSON description of
a project's structural elements — files, classes, functions, and their
relationships. amplihack-rs treats that JSON as an input format and imports
it into the LadybugDB code-graph so that `amplihack query-code` can answer
structural questions.

## What we ship

| Component              | Status     | Notes                                              |
|------------------------|------------|----------------------------------------------------|
| `amplihack index-code` | ✅ shipped | Imports a blarify JSON export into the graph.      |
| `amplihack index-scip` | ✅ shipped | Native SCIP path, the **preferred** modern source. |
| Embedded blarify       | ❌ no      | Blarify itself is upstream and out of scope here.  |
| Python blarify wrapper | ❌ removed | The previous Python CLI has been deleted.          |

The CLI flag surface for the importer is defined by the `IndexCode`
variant in `crates/amplihack-cli/src/cli_commands.rs` — `input` (positional
JSON path), `--db-path`, plus the legacy `--kuzu-path` alias.

## Why we still take blarify input

SCIP indexers (`scip-python`, `scip-typescript`, `rust-analyzer --scip`,
…) are the primary ingestion path for the code-graph. Blarify JSON remains
a useful fallback when:

- A language has a blarify analyzer but no SCIP indexer.
- An existing pipeline already produces blarify JSON.
- A user wants to compare graphs from the two sources.

Both paths feed the same on-disk schema, so downstream queries don't care
which importer was used.

## Out of scope on this page

- **Generating** blarify JSON — that is upstream blarify's job.
- The graph schema itself — see [LadybugDB Code Graph](./kuzu-code-graph.md).
- SCIP indexer prerequisites — see
  [memory-index-command reference](../reference/memory-index-command.md#scip-indexer-prerequisites).

## See also

- [LadybugDB Code Graph](./kuzu-code-graph.md)
- [Blarify Quickstart](../howto/blarify-quickstart.md)
- [Index a Project with the Native SCIP Pipeline](../howto/index-a-project.md)
