# Documentation Knowledge Graph

**Implementation Complete** — Documentation parsing, Neo4j integration, and code/memory linking

!!! note "Rust Port"
    In amplihack-rs, the documentation knowledge graph is accessed through
    the `amplihack memory` subcommands. The Python API examples below show the
    upstream interface for reference; the Rust crate exposes equivalent
    functionality.

---

## Overview

The Documentation Knowledge Graph integrates markdown documentation into the
Neo4j memory system, creating a unified knowledge graph that links:

- **Documentation** ↔ **Code** (functions, classes, files)
- **Documentation** ↔ **Memory** (agent experiences)
- **Documentation** ↔ **Concepts** (extracted from docs)

This enables agents to:

- Find relevant documentation for coding tasks
- Link learnings to official documentation
- Understand relationships between docs, code, and experiences
- Query documentation context when solving problems

---

## Architecture

### Graph Schema

```
(:DocFile {path, title, content, line_count, word_count})
  ├─[:HAS_SECTION]→(:Section {heading, level, content, order})
  ├─[:DEFINES]→(:Concept {name, category})
  └─[:REFERENCES]→(:CodeFile)

(:Concept)
  └─[:IMPLEMENTED_IN]→(:Function | :Class)

(:Memory)
  └─[:DOCUMENTED_IN]→(:DocFile)
```

### Node Types

**DocFile**: Markdown documentation files

- Properties: path, title, content, line_count, word_count, last_modified
- Relationships: HAS_SECTION, DEFINES, REFERENCES

**Section**: Markdown sections (H1-H6 headings)

- Properties: heading, level, content, order
- Relationships: Part of DocFile

**Concept**: Key concepts extracted from documentation

- Properties: name, category (section, emphasized, language)
- Relationships: DEFINES (from DocFile), IMPLEMENTED_IN (to Code)

### Relationships

| Relationship | Source | Target | Description |
|---|---|---|---|
| `HAS_SECTION` | DocFile | Section | Document structure |
| `DEFINES` | DocFile | Concept | Concepts defined in documentation |
| `REFERENCES` | DocFile | CodeFile | Code mentioned in docs |
| `IMPLEMENTED_IN` | Concept | Function/Class | Concept-code links |
| `DOCUMENTED_IN` | Memory | DocFile | Memory-documentation links |

---

## Features

### 1. Markdown Parsing

Extracts structured data from markdown files:

- **Title**: First H1 heading
- **Sections**: All headings with content
- **Concepts**: Section headings, bold text, code languages
- **Code References**: `@file.rs`, `file.rs:line`, inline code
- **Links**: `[text](url)` markdown links
- **Metadata**: File size, word count, last modified

### 2. Neo4j Integration

Imports documentation into Neo4j graph database:

- Creates DocFile, Section, and Concept nodes
- Establishes relationships between nodes
- Links to existing CodeFile nodes (from blarify)
- Idempotent operations (safe to re-import)

### 3. Code Linking

Automatically links documentation to code:

- Matches concepts to function/class names
- Links explicit code references (`@file.rs`)
- Connects documentation to related code files

### 4. Memory Linking

Connects documentation to agent memories:

- Links memories to relevant documentation
- Uses shared concepts/tags
- Enables doc-aware learning

### 5. Documentation Queries

Find relevant documentation:

- Keyword-based search
- Concept matching
- Code reference lookup
- Statistics and analytics

---

## Usage

### Import Documentation

```python
# Upstream Python API (reference only)
from amplihack.memory.neo4j import Neo4jConnector, DocGraphIntegration

connector = Neo4jConnector()
connector.connect()

doc_integration = DocGraphIntegration(connector)
doc_integration.initialize_doc_schema()

from pathlib import Path
doc_path = Path("docs/my_documentation.md")

stats = doc_integration.import_documentation(
    file_path=doc_path,
    project_id="my-project"
)

print(f"Imported: {stats}")
# {'doc_files': 1, 'sections': 12, 'concepts': 25, 'code_refs': 3}
```

**Rust CLI equivalent:**

```bash
amplihack memory index --project my-project docs/my_documentation.md
```

### Link to Code

```python
# Upstream Python API (reference only)
link_count = doc_integration.link_docs_to_code(project_id="my-project")
print(f"Created {link_count} doc-code links")
```

### Query Documentation

```python
# Upstream Python API (reference only)
results = doc_integration.query_relevant_docs(
    query_text="neo4j memory",
    limit=5
)

for doc in results:
    print(f"- {doc['title']} ({doc['concept_matches']} concepts)")
```

**Rust CLI equivalent:**

```bash
amplihack memory query "neo4j memory" --limit 5
```

### Get Statistics

```python
# Upstream Python API (reference only)
stats = doc_integration.get_doc_stats()
print(f"Total documents: {stats['doc_count']}")
print(f"Total concepts: {stats['concept_count']}")
print(f"Total sections: {stats['section_count']}")
```

---

## CLI Tools

### Import Documentation (Rust CLI)

```bash
# Index all docs from docs/ directory
amplihack memory index docs/

# Index specific directories
amplihack memory index docs/ .claude/context/

# Index with code linking
amplihack memory index --link-code docs/

# Index with memory linking
amplihack memory index --link-memory docs/
```

### Upstream Python CLI (reference only)

```bash
# python scripts/import_docs_to_neo4j.py docs/
# python scripts/import_docs_to_neo4j.py --link-code docs/
# python scripts/import_docs_to_neo4j.py --dry-run docs/
# python scripts/import_docs_to_neo4j.py --project my-project docs/
```

---

## Testing

### Run Tests

```bash
# Rust tests
cargo test --package amplihack-memory -- doc_graph

# Upstream Python tests (reference only)
# python scripts/test_doc_graph.py           # Full test (requires Neo4j)
# python scripts/test_doc_parsing_standalone.py  # Parsing only (no Neo4j)
```

### Upstream Test Results (5 real files)

| Metric | Count |
|---|---|
| Sections extracted | 187 |
| Concepts identified | 362 |
| Code references found | 5 |
| Errors | 0 ✓ |

---

## Related Documentation

- [Doc Graph Quick Reference](../reference/doc-graph-quick-reference.md) — one-page cheat sheet
- [External Knowledge Integration](external-knowledge-integration.md) — fetching external docs
- [Agent Memory Architecture](agent-memory-architecture.md) — overall memory system
- [Blarify Integration](blarify-integration.md) — code graph integration

---

**Status**: Upstream implementation complete ✓ | Rust port in progress
