# Blarify Quickstart

> **Status: Shipped.** All commands on this page exist in the current
> binary. Cross-check with `amplihack index-code --help` and
> `amplihack query-code --help`.

This how-to imports a blarify JSON export into the code graph and runs a
sanity query.

## Before you start

You need:

- amplihack-rs built (`cargo build --release`) and on `PATH`
- A blarify JSON export for your project (produced by upstream blarify)
- Optional: SCIP indexers if you also want to use `index-scip`
  (see [Index a Project](./index-a-project.md))

## Steps

### 1. Locate or generate the blarify JSON

Blarify itself is out of scope for this guide; produce the JSON with your
existing blarify pipeline. The importer expects the path on the command
line.

### 2. Import the JSON into the code graph

```sh
amplihack index-code path/to/blarify-export.json
```

To put the resulting database somewhere other than the default location:

```sh
amplihack index-code path/to/blarify-export.json --db-path ./custom.lbug
```

The legacy `--kuzu-path` alias is still accepted for backward
compatibility but is hidden in `--help`.

### 3. Verify the import

```sh
amplihack query-code stats
```

You should see non-zero counts for `Files`, `Classes`, and `Functions`.
If everything is zero, the JSON was empty or its schema did not match what
the importer expects — re-run blarify and confirm its output is valid
before re-importing.

### 4. Run a structural query

```sh
amplihack query-code search --name MyClass
amplihack query-code callers --name run_tui
```

For the full subcommand list:

```sh
amplihack query-code --help
```

## When to prefer `index-scip` instead

If your language has a SCIP indexer available
(`scip-python`, `scip-typescript`, `rust-analyzer --scip`, …), prefer
`amplihack index-scip` — it produces the same downstream graph but with
richer cross-reference information. See
[Index a Project](./index-a-project.md).

## See also

- [Blarify Integration](../concepts/blarify-integration.md)
- [LadybugDB Code Graph](../concepts/kuzu-code-graph.md)
- [Index a Project with the Native SCIP Pipeline](./index-a-project.md)
- [memory-index-command reference](../reference/memory-index-command.md)
