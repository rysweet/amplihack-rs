# Documentation Knowledge Graph — Quick Reference

**One-page guide for using the documentation graph system**

!!! note "Rust Port"
    In amplihack-rs, the documentation knowledge graph is accessed through
    the `amplihack memory` subcommands and the Rust `amplihack::memory` crate.
    The Python snippets below show the upstream API for reference; the Rust
    CLI exposes equivalent functionality via `amplihack memory index` and
    `amplihack memory query`.

---

## Import Documentation

```python
# Upstream Python API (reference only)
from amplihack.memory.neo4j import Neo4jConnector, DocGraphIntegration
from pathlib import Path

connector = Neo4jConnector()
connector.connect()
doc_integration = DocGraphIntegration(connector)
doc_integration.initialize_doc_schema()

# Import single file
stats = doc_integration.import_documentation(
    file_path=Path("docs/my_doc.md"),
    project_id="my-project"
)

# Import all files in directory
for doc_path in Path("docs/").glob("**/*.md"):
    doc_integration.import_documentation(doc_path)
```

**Rust CLI equivalent:**

```bash
# Index a project's documentation
amplihack memory index --project my-project docs/
```

---

## Link to Code

```python
# Upstream Python API (reference only)
link_count = doc_integration.link_docs_to_code()
print(f"Created {link_count} links")
```

---

## Query Documentation

```python
# Upstream Python API (reference only)
results = doc_integration.query_relevant_docs("authentication", limit=5)

for doc in results:
    print(f"{doc['title']}")
    print(f"  Path: {doc['path']}")
    print(f"  Concept matches: {doc['concept_matches']}")
```

**Rust CLI equivalent:**

```bash
amplihack memory query "authentication" --limit 5
```

---

## CLI Usage

```bash
# Import all markdown files (Rust CLI)
amplihack memory index docs/

# Upstream Python CLI (reference only)
# python scripts/import_docs_to_neo4j.py docs/
# python scripts/import_docs_to_neo4j.py --link-code --link-memory docs/
```

---

## Graph Schema

```
DocFile → HAS_SECTION → Section
DocFile → DEFINES → Concept
DocFile → REFERENCES → CodeFile
Concept → IMPLEMENTED_IN → Function/Class
Memory → DOCUMENTED_IN → DocFile
```

---

## Extracted Data

From each markdown file:

- **Title**: First H1 heading
- **Sections**: All headings (H1-H6)
- **Concepts**: Headings, **bold text**, code languages
- **Code refs**: `@file.rs`, `file.rs:line`, inline code references
- **Links**: `[text](url)`
- **Metadata**: Size, word count, last modified

---

## Example Queries

### Find documentation about a topic

```python
# Upstream Python API (reference only)
docs = doc_integration.query_relevant_docs("neo4j memory")
```

### Get all concepts from a document

```cypher
MATCH (df:DocFile {path: $path})-[:DEFINES]->(c:Concept)
RETURN c.name, c.category
```

### Find code implementing a concept

```cypher
MATCH (c:Concept {name: $concept})-[:IMPLEMENTED_IN]->(f:Function)
RETURN f.name, f.file_path
```

### Find documentation referencing specific code

```cypher
MATCH (df:DocFile)-[r:REFERENCES]->(cf:CodeFile {path: $code_path})
RETURN df.title, df.path
```

---

## Statistics

```python
# Upstream Python API (reference only)
stats = doc_integration.get_doc_stats()
print(f"Documents: {stats['doc_count']}")
print(f"Concepts: {stats['concept_count']}")
print(f"Sections: {stats['section_count']}")
print(f"Code refs: {stats['code_ref_count']}")
```

---

## Code Reference Patterns

The system detects:

```markdown
@src/main.rs   → src/main.rs
file.rs:42     → file.rs, line 42
`config.toml`  → config.toml
```

---

## Integration

### With Code Graph (blarify)

See [Blarify Integration](../concepts/blarify-integration.md) for how the code
graph connects to the documentation knowledge graph.

```python
# Upstream Python API (reference only)
# 1. Import code graph
blarify = BlarifyIntegration(connector)
blarify.import_blarify_output(Path("code_graph.json"))

# 2. Import documentation
doc_integration.import_documentation(Path("docs/"))

# 3. Link them
doc_integration.link_docs_to_code()
```

### With Memory System

See [Agent Memory Architecture](../concepts/agent-memory-architecture.md) for
the full memory integration story.

```python
# Upstream Python API (reference only)
doc_integration.import_documentation(Path("docs/"))
doc_integration.link_docs_to_memories()
```

---

## Testing

```bash
# Full test (requires Neo4j running)
cargo test --package amplihack-memory -- doc_graph

# Upstream Python (reference only)
# python scripts/test_doc_graph.py
# python scripts/test_doc_parsing_standalone.py
```

**Upstream test results** (5 real files):

- 187 sections extracted
- 362 concepts identified
- 5 code references found
- 0 errors ✓

---

## Files

**Upstream implementation**:

- `src/amplihack/memory/neo4j/doc_graph.py`

**Rust implementation**:

- `crates/amplihack-memory/src/doc_graph.rs` <!-- TODO: link when module exists -->

**Documentation**:

- [Documentation Knowledge Graph](../concepts/documentation-knowledge-graph.md) (full guide)
- This file (quick reference)

---

## Common Patterns

### Import all project docs

```python
# Upstream Python API (reference only)
from pathlib import Path

project_root = Path(".")
doc_files = list(project_root.glob("**/*.md"))

for doc_file in doc_files:
    try:
        stats = doc_integration.import_documentation(doc_file)
        print(f"✓ {doc_file.name}: {stats}")
    except Exception as e:
        print(f"✗ {doc_file.name}: {e}")
```

### Find documentation for current task

```python
# Upstream Python API (reference only)
def get_relevant_docs(task_description: str):
    """Find documentation relevant to a task."""
    key_terms = extract_keywords(task_description)
    all_docs = []
    for term in key_terms:
        docs = doc_integration.query_relevant_docs(term, limit=3)
        all_docs.extend(docs)
    unique_docs = {doc['path']: doc for doc in all_docs}
    return list(unique_docs.values())
```

---

## Performance Tips

1. **Batch imports**: Import multiple files in one transaction
2. **Link after import**: Import all docs first, then link
3. **Use project_id**: Scope queries to specific projects
4. **Cache results**: Query results are stable until docs change

---

## Troubleshooting

**Neo4j connection fails**:

```bash
export NEO4J_PASSWORD='your_password'
docker-compose -f docker/docker-compose.neo4j.yml up -d
```

**No concepts extracted**:

- Check markdown has headings
- Check for bold text (**important**)
- Check for code blocks with language

**No code links created**:

- Ensure code graph imported first (blarify)
- Check code references in docs (`@file.rs`)
- Verify CodeFile nodes exist in Neo4j

---

## Next Steps

After importing documentation:

1. **Test queries**: Verify you can find relevant docs
2. **Link to code**: Create doc-code relationships
3. **Link to memories**: Connect learnings to docs
4. **Use in agents**: Provide doc context to agents

---

**Status**: Implementation complete ✓ (upstream); Rust port in progress
