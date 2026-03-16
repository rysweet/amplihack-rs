# How to Index a Project with the Native SCIP Pipeline

Index a project's source code into the Kuzu code-graph so that
`amplihack query-code` can answer structural questions about it.

## Before you start

You need:

- `amplihack` binary built and on `PATH` (`cargo build --release`)
- At least one SCIP indexer installed for the languages in your project
  (see the table in [SCIP indexer prerequisites](../reference/memory-index-command.md#scip-indexer-prerequisites))
- The project checked out locally with source files present

Run `amplihack doctor` to see which prerequisites are met.

## Steps

### 1. Verify at least one indexer is available

```sh
amplihack doctor
```

Look for lines like `✓ scip-python` or `✓ rust-analyzer`. If none appear, install
the relevant indexer before continuing.

```sh
# Example: install scip-python (Go binary, despite the name)
pip install scip-python

# Example: install scip-typescript
npm install -g @sourcegraph/scip-typescript typescript
```

### 2. Run the indexer

From the project root:

```sh
amplihack index-scip
```

The command auto-detects which languages are present and runs the available
indexers. Example output for a Python + Rust project:

```
Native SCIP indexing summary
========================================
Success: true
Completed: python, rust
Artifact: /home/user/myproject/.amplihack/indexes/python.scip
Artifact: /home/user/myproject/.amplihack/indexes/rust.scip
Imported: files=89, classes=24, functions=412, imports=156, relationships=538
```

If a language's indexer is not installed, it is skipped with a note in the
`Skipped:` line — this is not an error.

### 3. Confirm the graph has data

```sh
amplihack query-code stats
```

Expected output:

```
Code Graph Statistics:
  Files:     89
  Classes:   24
  Functions: 412
  Memory→File links:     7
  Memory→Function links: 3
```

If all counts are 0, the indexer ran but produced no output (check the
`Skipped:` or `Failed:` lines from step 2).

### 4. Query the graph

```sh
# List all functions in a specific file
amplihack query-code functions --file code_graph

# Find what calls a function
amplihack query-code callers run_tui

# Search for any symbol by name
amplihack query-code search FleetTui

# Get machine-readable output
amplihack query-code --json stats
```

See the [query-code reference](../reference/query-code-command.md) for all
available subcommands.

## Index a specific path (not the current directory)

```sh
amplihack index-scip --project-path /path/to/other/project
```

## Index only selected languages

```sh
# Index only Rust and Go, skip Python even if present
amplihack index-scip --language rust --language go
```

## Re-index after code changes

Run `amplihack index-scip` again. All upserts are idempotent — existing nodes
are updated in place and no duplicates are created.

## Import a pre-existing blarify.json

If you have a `blarify.json` from a CI artifact or another tool:

```sh
amplihack index-code /path/to/blarify.json
```

The JSON is parsed and merged into the same Kuzu graph that `index-scip`
populates. If the file is absent, the command logs a warning and exits 0 — it
does not abort.

## Troubleshoot: "no supported source files found"

`index-scip` scans for files with known extensions (`.py`, `.rs`, `.ts`, etc.)
and skips `node_modules`, `.git`, `.venv`, and similar directories. If your
project uses non-standard extensions or is entirely in a language that is not
yet supported, the scan finds nothing.

Run with `--language` to specify a language explicitly:

```sh
amplihack index-scip --language python
```

If the language scan should have found files but did not, check that the
project root contains source files at the top level or in subdirectories (the
scan is recursive).

## Troubleshoot: "native SCIP indexing did not complete successfully"

This exit-1 error means no language completed successfully (all either failed
or were skipped). Common causes:

| Cause | Fix |
|-------|-----|
| Indexer binary not on PATH | Install the binary; check with `amplihack doctor` |
| `index.scip` not written | The indexer ran but produced no output; check its stderr |
| Kuzu path not writable | Ensure `<project>/.amplihack/` is writable |

## Related

- [`amplihack index-scip` and `index-code` reference](../reference/memory-index-command.md)
- [`amplihack query-code` reference](../reference/query-code-command.md)
- [Kuzu Code Graph Architecture](../concepts/kuzu-code-graph.md)
