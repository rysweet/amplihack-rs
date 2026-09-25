# amplihack index-code and index-scip

Full CLI reference for `amplihack index-code` and `amplihack index-scip` — the
two commands that build the native LadybugDB (formerly Kuzu) code-graph from source.

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
| `--db-path <PATH>` | no | Override the code-graph database directory. Defaults to `graph_db` in the project's [artifact cache directory](project-artifact-cache.md), recovered from the input path. `--kuzu-path` remains as a backward-compatible alias. |

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

If `<INPUT>` does not exist, `index-code` returns an error instead of producing
a success-shaped zero-count import.

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
amplihack index-code /tmp/analysis/blarify.json --db-path /var/cache/myproject/graph_db
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
   `<artifact-dir>/indexes/<language>.scip`.
5. Imports each `.scip` artifact into the code-graph store via `import_scip_file`.
6. Prints a summary to stdout and exits 0 if at least one language succeeded.

Step 4's `<artifact-dir>` is the project's
[per-project cache directory](project-artifact-cache.md), outside the indexed
checkout: `${XDG_CACHE_HOME:-$HOME/.cache}/amplihack/projects/<slug>`. Override it
with `AMPLIHACK_ARTIFACT_DIR`.

Each indexer is given that destination explicitly — `--output <file>` for most,
`--index-output-path <file>` for `scip-clang`. `rust-analyzer scip` takes no
output flag, so it runs from a staging directory beside the final artifact and
its result is moved into place. A language whose indexer accepts neither is
skipped with a named reason rather than indexed into the repository. There is no
step that writes `index.scip` into the project root, and so no backup/restore
pair around one.

Languages whose indexer binary is absent are silently skipped with a note in
the summary — partial success is valid.

### Example

```sh
# Index all detected languages in the current project
cd ~/src/myproject
amplihack index-scip

# Output (plain-text, not JSON):
# Native SCIP indexing summary
# ========================================
# Success: true
# Completed: python, rust
# Skipped: typescript
# Artifact: /home/user/.cache/amplihack/projects/myproject-3f9c1ad7b2e40561/indexes/python.scip
# Artifact: /home/user/.cache/amplihack/projects/myproject-3f9c1ad7b2e40561/indexes/rust.scip
# Imported: files=89, classes=24, functions=412, imports=156, relationships=538
```

```sh
# Index only Rust and Go in a monorepo
amplihack index-scip --project-path /repo --language rust --language go
```

---

## Output format

### index-code

`index-code` prints a JSON object to stdout on success:

```json
{
  "files": 42,
  "classes": 18,
  "functions": 157,
  "imports": 83,
  "relationships": 201
}
```

### index-scip

`index-scip` prints a **plain-text summary** to stdout — not JSON. The output
is human-readable and follows this format:

```
Native SCIP indexing summary
========================================
Success: true
Completed: python, rust
Skipped: typescript
Artifact: /home/user/.cache/amplihack/projects/myproject-3f9c1ad7b2e40561/indexes/python.scip
Artifact: /home/user/.cache/amplihack/projects/myproject-3f9c1ad7b2e40561/indexes/rust.scip
Imported: files=89, classes=24, functions=412, imports=156, relationships=538
```

Each line is a key-value pair. Failed languages and error details appear on
additional lines when applicable.

Errors go to stderr. Warnings (e.g. missing prerequisites, skipped languages)
are emitted to the structured log at `WARN` level and do not appear on stderr
by default.

---

## Database location

The default code-graph database path is `graph_db` inside the project's
[artifact cache directory](project-artifact-cache.md), outside the checkout.

For `index-code`, the project is recovered from the input path: an input at
`<artifact-dir>/blarify.json` resolves to `<artifact-dir>/graph_db`, with the
project path read from the `project` pointer file in that directory. An input
path that resolves to no project is an **error**.

There is no fallback to the current directory. Falling back to `$PWD` meant
`amplihack index-code` on an unrecognised input path created a graph database
inside whatever repository the user happened to be standing in. See
[The `project` pointer file](project-artifact-cache.md#the-project-pointer-file).

Use `--db-path` on `index-code` to override. `--kuzu-path` remains accepted as a backward-compatible alias.

---

## Security constraints

| Constraint | Detail |
|-----------|--------|
| 500 MB size guard | `index-code` reads the blarify JSON file size before parsing. Files ≥ 500 MB are rejected with a clear error. |
| Path canonicalization | Both `--project-path` and `--db-path` are canonicalized via `std::fs::canonicalize`. Symlinks are followed; a `WARN` log entry is emitted if the input or DB path is a symlink. |
| Blocked path prefixes | Paths under `/proc`, `/sys`, or `/dev` are rejected immediately. |
| DB file permissions (Unix) | After the LadybugDB database is initialized, the DB file is `chmod 0600` and its parent directory is `chmod 0700`. |
| No shell expansion | All external tool invocations (SCIP indexers) pass arguments as discrete `Vec<String>` elements — never via a shell string. |
| Parameterized Cypher | All LadybugDB queries use parameterized statements. String interpolation into query text is prohibited. |

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

- [`amplihack query-code`](./query-code-command.md) — Query the populated LadybugDB code-graph
- [`amplihack doctor`](./doctor-command.md) — Check indexer prerequisites
- [Index a project end-to-end](../howto/index-a-project.md) — Step-by-step guide
- [LadybugDB Code Graph Architecture](../concepts/kuzu-code-graph.md) — How the graph is structured
- [Per-Project Artifact Cache](./project-artifact-cache.md) — Where the indexes, `blarify.json`, and graph store are written
