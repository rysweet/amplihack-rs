# amplihack query-code

Full CLI reference for `amplihack query-code` — query the native
code-graph populated by `index-code` or `index-scip`.

## Contents

- [Synopsis](#synopsis)
- [Global options](#global-options)
- [Subcommands](#subcommands)
  - [stats](#stats)
  - [context](#context)
  - [files](#files)
  - [functions](#functions)
  - [classes](#classes)
  - [search](#search)
  - [callers](#callers)
  - [callees](#callees)
- [JSON output](#json-output)
- [Exit codes](#exit-codes)
- [Related commands](#related-commands)

---

## Synopsis

```sh
amplihack query-code [--db-path <PATH>] [--json] [--limit <N>] <SUBCOMMAND>
```

---

## Global options

| Option | Default | Description |
|--------|---------|-------------|
| `--db-path <PATH>` | `<cwd>/.amplihack/kuzu_db` | Path to the code-graph database directory. `--kuzu-path` remains as a compatibility alias. |
| `--json` | false | Emit output as JSON instead of human-readable text. |
| `--limit <N>` | 50 | Maximum number of rows returned by list subcommands. |

---

## Subcommands

### stats

Print aggregate counts for the entire code-graph, including any native
memory↔code relationships already linked in the same project-local Kuzu DB.

```sh
amplihack query-code stats
```

**Human-readable output:**

```
Code Graph Statistics:
  Files:     89
  Classes:   24
  Functions: 412
  Memory→File links:     7
  Memory→Function links: 3
```

**JSON output (`--json`):**

```json
{
  "files": 89,
  "classes": 24,
  "functions": 412,
  "memory_file_links": 7,
  "memory_function_links": 3
}
```

---

### context

Show the linked code context for a memory ID. This uses the native
memory→file and memory→function relationships stored in the same code-graph DB.

```sh
amplihack query-code context <MEMORY_ID>
```

| Argument | Description |
|----------|-------------|
| `<MEMORY_ID>` | Exact memory identifier to inspect. |

**Human-readable output:**

```text
Code context for memory 'mem-query':
  Files:
    - src/example/module.py [python] (10 bytes)
  Functions:
    - helper :: 
  Classes: none
```

**JSON output (`--json`):**

```json
{
  "memory_id": "mem-query",
  "files": [
    {
      "type": "file",
      "path": "src/example/module.py",
      "language": "python",
      "size_bytes": 10
    }
  ],
  "functions": [
    {
      "type": "function",
      "name": "helper",
      "signature": "",
      "docstring": "",
      "complexity": 0
    }
  ],
  "classes": []
}
```

If the memory ID does not exist, the command still exits 0 and returns empty
`files`, `functions`, and `classes` arrays.

---

### files

List all `CodeFile` nodes in the graph, optionally filtered by a path
substring.

```sh
amplihack query-code files [--pattern <SUBSTRING>]
```

| Option | Description |
|--------|-------------|
| `--pattern <SUBSTRING>` | Case-sensitive substring filter applied to the stored file path. |

**Example — all files:**

```sh
amplihack query-code files
# src/main.rs
# src/commands/fleet.rs
# src/commands/memory/code_graph.rs
# ...
```

**Example — filter by path fragment:**

```sh
amplihack query-code files --pattern commands/memory
# src/commands/memory/code_graph.rs
# src/commands/memory/scip_indexing.rs
# src/commands/memory/mod.rs
```

**JSON output (`--json`):**

```json
[
  { "file_path": "src/commands/memory/code_graph.rs", "language": "rust", "size_bytes": 28160 },
  { "file_path": "src/commands/memory/scip_indexing.rs", "language": "rust", "size_bytes": 16384 }
]
```

---

### functions

List `CodeFunction` nodes, optionally filtered by the file that defines them.

```sh
amplihack query-code functions [--file <SUBSTRING>]
```

| Option | Description |
|--------|-------------|
| `--file <SUBSTRING>` | Case-sensitive substring filter applied to the function's source file path. |

**Example:**

```sh
amplihack query-code functions --file code_graph
# import_blarify_json (src/commands/memory/code_graph.rs:236)
# import_scip_file    (src/commands/memory/code_graph.rs:200)
# summarize_code_graph (src/commands/memory/code_graph.rs:315)
# ...
```

**JSON output (`--json`):**

```json
[
  {
    "function_name": "import_blarify_json",
    "fully_qualified_name": "amplihack_cli::commands::memory::code_graph::import_blarify_json",
    "file_path": "src/commands/memory/code_graph.rs",
    "line_number": 236,
    "signature": "(input_path: &Path, kuzu_path: Option<&Path>) -> Result<CodeGraphImportCounts>",
    "is_async": false,
    "cyclomatic_complexity": 4
  }
]
```

---

### classes

List `CodeClass` nodes, optionally filtered by source file.

```sh
amplihack query-code classes [--file <SUBSTRING>]
```

| Option | Description |
|--------|-------------|
| `--file <SUBSTRING>` | Case-sensitive substring filter applied to the class's source file path. |

**Example:**

```sh
amplihack query-code classes --file fleet
# FleetState      (src/commands/fleet.rs:2011)
# FleetAdmiral    (src/commands/fleet.rs:2189)
# FleetTuiUiState (src/commands/fleet.rs:1829)
```

---

### search

Search for any symbol (file, function, or class) whose name contains the given
substring. Searches across all three node types in one pass.

```sh
amplihack query-code search <NAME>
```

| Argument | Description |
|----------|-------------|
| `<NAME>` | Substring to search for (case-sensitive). |

**Example:**

```sh
amplihack query-code search import_scip
# [function] import_scip_file @ src/commands/memory/code_graph.rs:200
```

**JSON output (`--json`):**

```json
[
  {
    "kind": "function",
    "name": "import_scip_file",
    "file_path": "src/commands/memory/code_graph.rs",
    "line_number": 200
  }
]
```

---

### callers

Find all `CodeFunction` nodes that call a given function. Uses the `CALLS`
relationship edges in the graph.

```sh
amplihack query-code callers <NAME>
```

| Argument | Description |
|----------|-------------|
| `<NAME>` | Substring matching the callee's function name. |

**Example:**

```sh
amplihack query-code callers import_scip_file
# run_index_scip        (src/commands/memory/scip_indexing.rs:113)
# import_scip_file      (src/commands/memory/code_graph.rs:200)  <- direct self-reference if any
```

---

### callees

Find all `CodeFunction` nodes called by a given function. The inverse of
`callers`.

```sh
amplihack query-code callees <NAME>
```

| Argument | Description |
|----------|-------------|
| `<NAME>` | Substring matching the caller's function name. |

**Example:**

```sh
amplihack query-code callees run_index_scip
# run_native_scip_indexing (src/commands/memory/scip_indexing.rs:172)
# import_scip_file          (src/commands/memory/code_graph.rs:200)
```

---

## JSON output

Pass `--json` to any subcommand to receive machine-readable JSON on stdout.
Errors still go to stderr.

```sh
# Pipe stats into jq
amplihack query-code --json stats | jq '.functions'
# 412
```

---

## Exit codes

| Code | Meaning |
|------|---------|
| 0 | Query completed successfully (zero results is still exit 0) |
| 1 | Error opening the database, or internal query failure |

---

## Related commands

- [`amplihack index-scip`](./memory-index-command.md#index-scip) — Build the graph from source
- [`amplihack index-code`](./memory-index-command.md#index-code) — Import a blarify JSON
- [Kuzu Code Graph Architecture](../concepts/kuzu-code-graph.md) — Schema and data model
- [Index a project end-to-end](../howto/index-a-project.md) — Walkthrough
