# amplihack index-code and index-scip

Full CLI reference for `amplihack index-code` and `amplihack index-scip` — the
two commands that build the native Kuzu code-graph from source.

## Contents

- [index-code](#index-code)
- [index-scip](#index-scip)
- [Output format](#output-format)
- [Database location](#database-location)
- [Security constraints](#security-constraints)
- [Supported languages (index-scip)](#supported-languages-index-scip)
- [SCIP indexer prerequisites](#scip-indexer-prerequisites)
- [Exit codes](#exit-codes)
- [Related commands](#related-commands)

---

## index-code

Import a pre-generated blarify JSON file into the native code-graph store.
Use this when you already have a `blarify.json` on disk (for example, from a CI
artifact) and want to ingest it without running the SCIP pipeline.

### Synopsis

```sh
amplihack index-code <INPUT> [--db-path <PATH>]
```

### Arguments

| Argument | Required | Description |
|----------|----------|-------------|
| `<INPUT>` | yes | Path to a blarify JSON export (`blarify.json`) |
| `--db-path <PATH>` | no | Override the code-graph database directory. Defaults to `<project>/.amplihack/kuzu_db` inferred from the input path location. `--kuzu-path` remains as a compatibility alias. |

### Behavior

1. Validates that `<INPUT>` exists and is below 500 MB.
2. Canonicalizes the input path and rejects paths under `/proc`, `/sys`, or
   `/dev`.
3. Opens (or creates) the code-graph database at the resolved `--db-path`.
4. On Unix, enforces `0600` permissions on the database file and `0700` on its
   parent directory after first open.
5. Applies the schema (idempotent — uses `CREATE IF NOT EXISTS`).
6. Upserts `CodeFile`, `CodeClass`, and `CodeFunction` nodes and all
   relationship edges from the JSON.
7. Prints an import-counts summary to stdout and exits 0.

If `<INPUT>` does not exist, `index-code` logs a warning at `WARN` level and
exits 0 with zero counts — it does **not** abort the process. This allows
pipelines that conditionally produce `blarify.json` to invoke `index-code`
unconditionally.

### Example

```sh
# Index the blarify JSON produced by a CI step
amplihack index-code /workspace/myproject/.amplihack/blarify.json

# Output:
# {
#   "files": 42,
#   "classes": 18,
#   "functions": 157,
#   "imports": 83,
#   "relationships": 201
# }
```

```sh
# Point at a custom database location
amplihack index-code /tmp/analysis/blarify.json --db-path /var/cache/myproject/kuzu
```

---

## index-scip

Auto-detect project languages, run the appropriate native SCIP indexers, and
import the resulting SCIP protobuf artifacts into the native code-graph.

No Python interpreter is invoked. Each language uses its own compiled SCIP
indexer binary (e.g. `scip-python` is a Go binary, not a Python script).

### Synopsis

```sh
amplihack index-scip [--project-path <PATH>] [--language <LANG>]...
```

### Arguments

| Argument | Required | Description |
|----------|----------|-------------|
| `--project-path <PATH>` | no | Root directory of the project to index. Defaults to the current working directory. |
| `--language <LANG>` | no | Restrict indexing to one or more languages. May be repeated. When omitted, all detected languages are indexed. |

Accepted language values: `python`, `typescript`, `javascript`, `go`, `rust`,
`csharp`, `cpp` (also accepts aliases: `ts`, `js`, `c++`, `cxx`, `c#`).

### Behavior

1. Resolves `--project-path` (default: `$PWD`).
2. Detects present source languages by scanning file extensions (skips
   `.git`, `.venv`, `node_modules`, `__pycache__`, and other standard ignored
   directories).
3. Checks prerequisites: for each detected language, verifies the required
   indexer binary is on `PATH`. Adds `~/.local/bin`, `~/.dotnet/tools`, and
   `~/go/bin` to the search path automatically.
4. Runs available indexers in sequence, saving each output to
   `<project>/.amplihack/indexes/<language>.scip`.
5. Backs up any existing `index.scip` at the project root and restores it when
   each indexer finishes (indexers write to `index.scip` by convention; the
   backup/restore prevents cross-contamination).
6. Imports each `.scip` artifact into the code-graph store via `import_scip_file`.
7. Prints a summary to stdout and exits 0 if at least one language succeeded.

Languages whose indexer binary is absent are silently skipped with a note in
the summary — partial success is valid.

### Example

```sh
# Index all detected languages in the current project
cd ~/src/myproject
amplihack index-scip

# Output:
# Native SCIP indexing summary
# ========================================
# Success: true
# Completed: python, rust
# Skipped: typescript
# Artifact: /home/user/src/myproject/.amplihack/indexes/python.scip
# Artifact: /home/user/src/myproject/.amplihack/indexes/rust.scip
# Imported: files=89, classes=24, functions=412, imports=156, relationships=538
```

```sh
# Index only Rust and Go in a monorepo
amplihack index-scip --project-path /repo --language rust --language go
```

---

## Output format

Both commands print a JSON object to stdout on success:

```json
{
  "files": 89,
  "classes": 24,
  "functions": 412,
  "imports": 156,
  "relationships": 538
}
```

`index-scip` additionally prints a human-readable preamble before the JSON
import counts.

Errors go to stderr. Warnings (e.g. missing blarify.json, skipped languages)
are emitted to the structured log at `WARN` level and do not appear on stderr
by default.

---

## Database location

The default code-graph database path is `<project-root>/.amplihack/kuzu_db`.

For `index-code`, the project root is inferred from the input path: if the
input is `<project>/.amplihack/blarify.json`, the database will be
`<project>/.amplihack/kuzu_db`. Otherwise the current directory is used.

Use `--db-path` on `index-code` to override. `--kuzu-path` remains accepted as a compatibility alias.

---

## Security constraints

| Constraint | Detail |
|-----------|--------|
| 500 MB size guard | `index-code` reads the blarify JSON file size before parsing. Files ≥ 500 MB are rejected with a clear error. |
| Path canonicalization | Both `--project-path` and `--db-path` are canonicalized via `std::fs::canonicalize`. Symlinks are followed; a `WARN` log entry is emitted if the input or DB path is a symlink. |
| Blocked path prefixes | Paths under `/proc`, `/sys`, or `/dev` are rejected immediately. |
| DB file permissions (Unix) | After the Kuzu database is initialized, the DB file is `chmod 0600` and its parent directory is `chmod 0700`. |
| No shell expansion | All external tool invocations (SCIP indexers) pass arguments as discrete `Vec<String>` elements — never via a shell string. |
| Parameterized Cypher | All Kuzu queries use parameterized statements. String interpolation into query text is prohibited. |

---

## Supported languages (index-scip)

| Language | Indexer binary | Detection extension(s) |
|----------|---------------|------------------------|
| Python | `scip-python` | `.py` |
| TypeScript | `scip-typescript` + `node` | `.ts`, `.tsx` |
| JavaScript | `scip-typescript` + `node` | `.js`, `.jsx` |
| Go | `scip-go` + `go` | `.go` |
| Rust | `rust-analyzer` + `cargo` | `.rs` |
| C# | `scip-dotnet` + `dotnet` | `.cs` |
| C/C++ | `scip-clang` | `.c`, `.cpp`, `.cc`, `.cxx`, `.h`, `.hpp` |

> **Note on `scip-python`**: Despite the name, `scip-python` is a compiled Go
> binary distributed by Sourcegraph. It indexes Python source files but is
> itself not a Python interpreter. Invoking it does not violate the
> no-Python-subprocess constraint.

---

## SCIP indexer prerequisites

Install the indexers you need before running `index-scip`.

```sh
# Python (Go binary from Sourcegraph)
pip install scip-python         # installs to ~/.local/bin

# TypeScript / JavaScript
npm install -g @sourcegraph/scip-typescript typescript

# Go
go install github.com/sourcegraph/scip-go@latest

# Rust (via rustup)
rustup component add rust-analyzer

# C# (.NET)
# Install the .NET SDK, then:
dotnet tool install -g scip-dotnet

# C/C++
# Install scip-clang from: https://github.com/sourcegraph/scip-clang/releases
# Ensure compile_commands.json is present at the project root.
```

Run `amplihack doctor` to check which prerequisites are currently satisfied.

---

## Exit codes

| Code | Meaning |
|------|---------|
| 0 | Success (all requested work completed, or graceful partial success) |
| 1 | Fatal error (no indexer succeeded, file not readable, path rejected) |

---

## Related commands

- [`amplihack query-code`](./query-code-command.md) — Query the populated Kuzu code-graph
- [`amplihack doctor`](./doctor-command.md) — Check indexer prerequisites
- [Index a project end-to-end](../howto/index-a-project.md) — Step-by-step guide
- [Kuzu Code Graph Architecture](../concepts/kuzu-code-graph.md) — How the graph is structured
