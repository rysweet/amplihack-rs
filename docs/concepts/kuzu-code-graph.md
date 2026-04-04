# LadybugDB Code Graph

The LadybugDB (formerly Kuzu) code-graph is amplihack-rs's native, zero-Python store for structural
information about a project's source code — files, classes, functions, and the
relationships between them.

## Contents

- [What it is](#what-it-is)
- [Why LadybugDB](#why-ladybugdb)
- [Schema](#schema)
- [Data ingestion pipeline](#data-ingestion-pipeline)
- [blarify: consumption vs. generation](#blarify-consumption-vs-generation)
- [SCIP: the primary ingestion path](#scip-the-primary-ingestion-path)
- [Why scip-python is not Python delegation](#why-scip-python-is-not-python-delegation)
- [Known gap: native blarify generation](#known-gap-native-blarify-generation)
- [On-disk layout](#on-disk-layout)
- [Security model](#security-model)
- [Related](#related)

---

## What it is

The LadybugDB code-graph answers structural questions about a codebase without
reading source files at query time:

- "How many functions are in this project?"
- "Which functions call `run_tui`?"
- "What classes are defined in `fleet.rs`?"
- "What imports does `code_graph.rs` have?"

It is a persistent property graph, updated by running `amplihack index-scip`
(or `amplihack index-code` for blarify JSON imports) and queried via
`amplihack query-code`.

> **Historical naming:** The graph database engine was previously known as "Kuzu"
> and has been rebranded to "LadybugDB". The `lbug` crate (formerly `kuzu`)
> provides the Rust FFI bindings. CLI flags like `--kuzu-path` and env vars
> like `AMPLIHACK_KUZU_DB_PATH` remain as backward-compatible aliases.

---

## Why LadybugDB

[LadybugDB](https://kuzudb.com/) (formerly Kuzu) is an embeddable property graph database with a
C++ core. The `lbug` Rust crate exposes it through a native C++ FFI binding
(`cxx-build`, pinned to `=1.0.138` per [the version contract](./cxx-version-contract.md)).

LadybugDB has no runtime dependency on Python or any interpreter. The FFI boundary
is compile-time — `cargo build` links the LadybugDB C++ library into the
`amplihack` binary. There is no subprocess launched to query the graph.

---

## Schema

The graph stores 3 node types and 7 relationship types:

### Node tables

| Table | Primary key | Key fields |
|-------|-------------|------------|
| `CodeFile` | `file_id` (SHA-256 of path) | `file_path`, `language`, `size_bytes`, `last_modified` |
| `CodeClass` | `class_id` | `class_name`, `fully_qualified_name`, `file_path`, `line_number`, `is_abstract` |
| `CodeFunction` | `function_id` | `function_name`, `fully_qualified_name`, `signature`, `file_path`, `line_number`, `is_async`, `cyclomatic_complexity` |

All node tables carry a `metadata` JSON string column for extension fields and
a `created_at` timestamp.

### Relationship tables

| Relationship | From → To | Key fields |
|-------------|-----------|------------|
| `DEFINED_IN` | `CodeFunction → CodeFile` | `line_number`, `end_line` |
| `CLASS_DEFINED_IN` | `CodeClass → CodeFile` | `line_number` |
| `METHOD_OF` | `CodeFunction → CodeClass` | `method_type`, `visibility` |
| `CALLS` | `CodeFunction → CodeFunction` | `call_count`, `context` |
| `INHERITS` | `CodeClass → CodeClass` | `inheritance_order`, `inheritance_type` |
| `REFERENCES_CLASS` | `CodeFunction → CodeClass` | `reference_type`, `context` |
| `IMPORTS` | `CodeFile → CodeFile` | `import_type`, `alias` |

Schema creation is idempotent — all `CREATE` statements use `IF NOT EXISTS`.
Running `index-scip` on an already-indexed project upserts records in place
without duplicating nodes.

---

## Data ingestion pipeline

Two ingestion paths populate the graph:

```
Source code
    │
    ├── path A: SCIP pipeline (primary)
    │       │
    │       ▼
    │   SCIP indexer binary (scip-python, rust-analyzer, scip-go, …)
    │       │  subprocess via std::process::Command — no interpreter
    │       ▼
    │   index.scip  (protobuf binary)
    │       │
    │       ▼
    │   import_scip_file()  — prost decode + SCIP-to-BlarifyOutput conversion
    │       │
    │       ▼
    │   LadybugDB graph  ◄──────────────────────────────────┐
    │                                                   │
    └── path B: blarify JSON import                     │
            │                                           │
            ▼                                           │
        blarify.json  (produced externally)             │
            │                                           │
            ▼                                           │
        import_blarify_json()  — serde_json parse       │
            │                                           │
            └───────────────────────────────────────────┘
```

Path A (`index-scip`) is the recommended path for new projects.
Path B (`index-code`) is for environments where `blarify.json` is already
available (e.g. produced by a CI job or another tool).

---

## blarify: consumption vs. generation

amplihack-rs **consumes** blarify JSON but does not **generate** it.

`blarify` is a Python tree-sitter-based tool with parsers for 20+ languages
that produces a `blarify.json` call-graph export. A native Rust port of the
blarify *generator* is out of scope for this project.

What amplihack-rs does:

- Defines the `BlarifyOutput` deserialization schema in Rust (`serde`)
- Imports any conforming `blarify.json` into LadybugDB via `import_blarify_json()`
- Never invokes `python blarify` or `python -m blarify` as a subprocess

If `blarify.json` is absent, `index-code` logs a `WARN` and exits cleanly with
zero counts. It does not abort the process or fall back to a Python subprocess.

The live path for code-graph indexing in amplihack-rs uses SCIP (path A
above), not blarify generation.

---

## SCIP: the primary ingestion path

[SCIP](https://sourcegraph.com/blog/announcing-scip) (SCIP Code Intelligence
Protocol) is a protobuf-based format for precise code intelligence — symbols,
occurrences, and relationships across a codebase.

amplihack-rs uses `prost` to decode SCIP protobuf files and then converts them
to the internal `BlarifyOutput` structure for import into LadybugDB.

The SCIP indexer binaries are external native tools:

| Indexer | Language | Type |
|---------|----------|------|
| `scip-python` | Python source | Go binary |
| `scip-typescript` | TypeScript/JavaScript | Node binary |
| `scip-go` | Go | Go binary |
| `rust-analyzer` | Rust | Rust binary |
| `scip-dotnet` | C# | .NET binary |
| `scip-clang` | C/C++ | Clang-based binary |

These are invoked via `std::process::Command` with arguments as discrete
`Vec<String>` elements — no shell string interpolation, no interpreter.

---

## Why scip-python is not Python delegation

`scip-python` is distributed by Sourcegraph as a compiled Go binary. The name
is misleading: it indexes *Python source code* but is itself a Go executable.
Installing it via `pip install scip-python` places a Go binary on `PATH`.

Invoking `scip-python index` from Rust is functionally identical to invoking
`scip-go` or `rust-analyzer scip`. It does not launch a Python interpreter.
This satisfies the no-Python-subprocess constraint.

The constraint that is enforced: no `python3 -c ...`, no `python script.py`,
no PyO3 embedding. Language-specific SCIP indexer binaries are valid external
tool use.

---

## Known gap: native blarify generation

**Scope boundary for issue #77:**

Generating `blarify.json` natively in Rust (replacing the Python tree-sitter
blarify tool) is *not* part of this project's scope. Doing so would require
porting 20+ tree-sitter language parsers — a multi-month effort tracked
separately as issue #78.

The current position:

| Capability | Status |
|-----------|--------|
| Consume `blarify.json` from any source | ✅ Implemented |
| Index via SCIP (no blarify needed) | ✅ Implemented |
| No `python blarify` subprocess on the live path | ✅ Verified by probe |
| Invoke `blarify` binary (if installed) as external tool | Acceptable, not yet needed |
| Generate blarify JSON natively in Rust | ⏳ Issue #78 |

---

## On-disk layout

After running `amplihack index-scip` in a project:

```
<project>/
└── .amplihack/
    ├── graph_db/         ← Graph database directory (0700)
    │   ├── data.kz       ← graph data (0600)
    │   └── ...
    └── indexes/
        ├── python.scip   ← SCIP artifact for Python
        ├── rust.scip     ← SCIP artifact for Rust
        └── ...
```

The `graph_db` directory and its contents are created with restrictive
permissions (`0700` / `0600`) to prevent other users on a shared system from
reading graph data that may include sensitive symbol names or docstrings.

---

## Security model

| Property | Implementation |
|----------|---------------|
| No interpreter subprocess | All SCIP indexers are binary executables; `python3` is never launched |
| Parameterized queries | All LadybugDB Cypher statements use parameter binding; no string interpolation |
| Path canonicalization | `--project-path` and `--db-path` are canonicalized; symlinks emit `WARN` |
| Blocked prefixes | `/proc`, `/sys`, `/dev` are rejected immediately |
| DB file permissions | `0600` file / `0700` directory enforced after first open (Unix only) |
| JSON size guard | `blarify.json` ≥ 500 MB is rejected before parsing |
| Argument injection prevention | External tool commands use `Vec<String>` argument lists |

---

## Related

- [`amplihack index-scip` and `index-code` reference](../reference/memory-index-command.md)
- [`amplihack query-code` reference](../reference/query-code-command.md)
- [Index a project end-to-end](../howto/index-a-project.md)
- [The cxx/cxx-build Version Contract](./cxx-version-contract.md) — why LadybugDB requires a pinned `cxx` version
